use crate::cli::{self, Args, MediaType};
use crate::model::RenamePlan;
use crate::operations::apply_rename_group;
use crate::parser::Guess;
use crate::quality::{count_destinations, duplicate_destination_key, select_preferred_sources};
use crate::report::RunLog;
use crate::scanning::{collect_files, find_subtitle_plans};
use crate::tmdb::{MetadataProvider, MovieMatch, SeasonEpisode, SeriesMatch, TmdbClient};
use crate::{config, parser, rename};
use anyhow::{Context, Result};
use console::style;
use dialoguer::{theme::ColorfulTheme, Confirm, Select};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

static TEMPLATE_FIELD: Lazy<Regex> = Lazy::new(|| Regex::new(r"\{([^{}]+)\}").unwrap());

pub(crate) fn run(args: Args) -> Result<()> {
    run_with_client(args, TmdbClient::new)
}

pub(crate) fn run_with_client<C: MetadataProvider>(
    args: Args,
    create_client: impl FnOnce(String) -> Result<C>,
) -> Result<()> {
    let cfg = config::load(args.config.as_deref())?;

    let recursive = resolve_bool(args.recursive, args.no_recursive, cfg.recursive);
    let batch = resolve_bool(args.batch, args.no_batch, cfg.batch);
    let lower = resolve_bool(args.lower, args.no_lower, cfg.lower);
    let scene = resolve_bool(args.scene, args.no_scene, cfg.scene);
    let output_dir = args.output_dir.clone().or_else(|| cfg.output_dir.clone());
    let ffprobe = resolve_ffprobe_path(args.ffprobe.as_deref(), cfg.ffprobe_path.as_deref())?;
    let extensions = args
        .extensions
        .clone()
        .or_else(|| cfg.extensions.clone())
        .unwrap_or_else(|| {
            cli::DEFAULT_EXTENSIONS
                .iter()
                .map(|s| s.to_string())
                .collect()
        });
    let format_movie = args
        .format_movie
        .clone()
        .or_else(|| cfg.format_movie.clone())
        .unwrap_or_else(|| cli::DEFAULT_FORMAT_MOVIE.to_string());
    let format_episode = args
        .format_episode
        .clone()
        .or_else(|| cfg.format_episode.clone())
        .unwrap_or_else(|| cli::DEFAULT_FORMAT_EPISODE.to_string());
    validate_template(&format_movie, "movie", &["title", "year", "ext"])?;
    validate_template(
        &format_episode,
        "episode",
        &[
            "series",
            "year",
            "season",
            "episode",
            "episode_end",
            "episode_range",
            "episode_title",
            "ext",
        ],
    )?;

    let files = collect_files(&args, recursive, &extensions)?;
    if files.is_empty() {
        println!("No matching media files found.");
        return Ok(());
    }

    if args.parse_only {
        let mut failed = 0;
        for f in &files {
            let stem = f.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
            match parser::parse_filename(stem) {
                Ok(guess) => print_guess(f, &guess),
                Err(error) => {
                    eprintln!("{}: {error}", f.display());
                    failed += 1;
                }
            }
        }
        return ensure_no_failures(failed);
    }

    let api_key = resolve_api_key(&args, &cfg)?;
    let client = create_client(api_key)?;
    let mut log = RunLog::new(args.log.as_deref())?;

    let mut renamed = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;
    let mut plans = Vec::new();

    for f in &files {
        println!("\n{} {}", style("File:").cyan().bold(), f.display());
        let stem = f.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
        let ext = f.extension().and_then(|s| s.to_str()).unwrap_or_default();
        let mut guess = match parser::parse_filename(stem) {
            Ok(guess) => guess,
            Err(error) => {
                eprintln!("{} {error}", style("  failed:").red().bold());
                log.record(f, None, "failed", &error.to_string())?;
                failed += 1;
                continue;
            }
        };

        // Respect a forced --media override
        guess = match args.media {
            MediaType::Movie => force_movie(guess),
            MediaType::Episode => force_episode(guess),
            MediaType::Auto => guess,
        };

        let target_name = match &guess {
            Guess::Movie { title, year } => match resolve_movie(&client, title, *year, batch) {
                Err(error) => {
                    eprintln!("{} {error:#}", style("  failed:").red().bold());
                    log.record(f, None, "failed", &format!("{error:#}"))?;
                    failed += 1;
                    continue;
                }
                Ok(Some(m)) => rename::render_movie(
                    &format_movie,
                    &rename::MovieVars {
                        title: &m.title,
                        year: m.year,
                        ext,
                    },
                    lower,
                    scene,
                ),
                Ok(None) => {
                    println!("{}", style("  skipped (no match chosen)").yellow());
                    log.record(f, None, "skipped", "no movie match chosen")?;
                    skipped += 1;
                    continue;
                }
            },
            Guess::Episode {
                series,
                season,
                episode,
                episode_end,
                ..
            } => match resolve_episode(&client, series, *season, *episode, *episode_end, batch) {
                Err(error) => {
                    eprintln!("{} {error:#}", style("  failed:").red().bold());
                    log.record(f, None, "failed", &format!("{error:#}"))?;
                    failed += 1;
                    continue;
                }
                Ok(Some((s, ep_title))) => rename::render_episode(
                    &format_episode,
                    &rename::EpisodeVars {
                        series: &s.name,
                        year: s.first_air_year,
                        season: *season,
                        episode: *episode,
                        episode_end: *episode_end,
                        episode_title: &ep_title,
                        ext,
                    },
                    lower,
                    scene,
                ),
                Ok(None) => {
                    println!("{}", style("  skipped (no match chosen)").yellow());
                    log.record(f, None, "skipped", "no series match chosen")?;
                    skipped += 1;
                    continue;
                }
            },
        };

        let dest_dir = output_dir.clone().unwrap_or_else(|| {
            f.parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."))
        });
        if target_name.is_empty() {
            let reason = "filename template produced an empty destination name";
            eprintln!("{} {reason}", style("  failed:").red().bold());
            log.record(f, None, "failed", reason)?;
            failed += 1;
            continue;
        }
        let dest = dest_dir.join(&target_name);

        println!("  {} {}", style("->").green().bold(), dest.display());

        if dest == *f {
            println!("{}", style("  already correctly named, skipping").dim());
            log.record(f, Some(&dest), "skipped", "already correctly named")?;
            skipped += 1;
            continue;
        }

        let subtitles = if args.subtitles {
            match find_subtitle_plans(f, &dest) {
                Ok(subtitles) => subtitles,
                Err(error) => {
                    eprintln!("{} {error:#}", style("  failed:").red().bold());
                    log.record(f, Some(&dest), "failed", &format!("{error:#}"))?;
                    failed += 1;
                    continue;
                }
            }
        } else {
            Vec::new()
        };

        plans.push(RenamePlan {
            source: f.clone(),
            destination: dest,
            subtitles,
        });
    }

    let destination_counts = count_destinations(&plans);
    let preferred_sources = select_preferred_sources(&plans, &ffprobe);

    for plan in plans {
        println!(
            "\n{} {} {} {}",
            style("Apply:").cyan().bold(),
            plan.source.display(),
            style("->").green().bold(),
            plan.destination.display()
        );
        for subtitle in &plan.subtitles {
            println!(
                "       {} {} {} {}",
                style("subtitle:").cyan(),
                subtitle.source.display(),
                style("->").green().bold(),
                subtitle.destination.display()
            );
        }

        let duplicate_key = duplicate_destination_key(&plan.destination);
        if destination_counts[&duplicate_key] > 1 {
            match &preferred_sources[&duplicate_key] {
                Some(preferred) if preferred == &plan.source => {
                    println!(
                        "{} selected as the highest-quality duplicate",
                        style("  preferred:").green().bold()
                    );
                }
                Some(_) => {
                    println!(
                        "{} a higher-quality duplicate was selected; original left unchanged",
                        style("  skipped:").yellow().bold()
                    );
                    log.record_group(&plan, "skipped", "a higher-quality duplicate was selected")?;
                    skipped += 1 + plan.subtitles.len();
                    continue;
                }
                None => {
                    println!(
                        "{} duplicate quality is tied, unknown, or not comparable; original left unchanged",
                        style("  skipped:").yellow().bold()
                    );
                    log.record_group(
                        &plan,
                        "skipped",
                        "duplicate quality is tied, unknown, or not comparable",
                    )?;
                    skipped += 1 + plan.subtitles.len();
                    continue;
                }
            }
        }

        if plan.destination.exists() {
            println!(
                "{} destination already exists; original left unchanged",
                style("  skipped:").yellow().bold()
            );
            log.record_group(&plan, "skipped", "destination already exists")?;
            skipped += 1 + plan.subtitles.len();
            continue;
        }

        if let Some(subtitle) = plan
            .subtitles
            .iter()
            .find(|subtitle| std::fs::symlink_metadata(&subtitle.destination).is_ok())
        {
            println!(
                "{} subtitle destination already exists; entire group left unchanged: {}",
                style("  skipped:").yellow().bold(),
                subtitle.destination.display()
            );
            log.record_group(&plan, "skipped", "a subtitle destination already exists")?;
            skipped += 1 + plan.subtitles.len();
            continue;
        }

        if !batch && !args.dry_run {
            let proceed = Confirm::with_theme(&ColorfulTheme::default())
                .with_prompt("  Apply this rename?")
                .default(true)
                .interact()
                .unwrap_or(false);
            if !proceed {
                log.record_group(&plan, "skipped", "rename declined by user")?;
                skipped += 1 + plan.subtitles.len();
                continue;
            }
        }

        if args.dry_run {
            println!("{}", style("  (dry run, not applied)").dim());
            log.record(
                &plan.source,
                Some(&plan.destination),
                "dry_run",
                "rename was not applied",
            )?;
            for subtitle in &plan.subtitles {
                log.record(
                    &subtitle.source,
                    Some(&subtitle.destination),
                    "dry_run",
                    "subtitle rename was not applied",
                )?;
            }
            continue;
        }

        match apply_rename_group(&plan, args.force_copy) {
            Ok(()) => {
                println!("{}", style("  renamed successfully").green());
                log.record(
                    &plan.source,
                    Some(&plan.destination),
                    "renamed",
                    "rename completed",
                )?;
                renamed += 1;
                for subtitle in &plan.subtitles {
                    log.record(
                        &subtitle.source,
                        Some(&subtitle.destination),
                        "renamed",
                        "subtitle rename completed",
                    )?;
                    renamed += 1;
                }
            }
            Err(error) => {
                eprintln!("{} {error:#}", style("  failed:").red().bold());
                log.record_group(&plan, "failed", &format!("{error:#}"))?;
                failed += 1 + plan.subtitles.len();
            }
        }
    }

    println!(
        "\n{} {renamed} renamed, {skipped} skipped, {failed} failed.",
        style("Done:").bold()
    );
    log.finish()?;
    ensure_no_failures(failed)
}

pub(crate) fn ensure_no_failures(failed: usize) -> Result<()> {
    if failed > 0 {
        anyhow::bail!(
            "{failed} file{} failed; see the messages above",
            if failed == 1 { "" } else { "s" }
        );
    }
    Ok(())
}

pub(crate) fn resolve_bool(enabled: bool, disabled: bool, configured: Option<bool>) -> bool {
    if enabled {
        true
    } else if disabled {
        false
    } else {
        configured.unwrap_or(false)
    }
}

pub(crate) fn validate_template(template: &str, kind: &str, allowed: &[&str]) -> Result<()> {
    if template.trim().is_empty() {
        anyhow::bail!("{kind} filename template cannot be empty");
    }
    let mut text_without_fields = template.to_string();
    for capture in TEMPLATE_FIELD.captures_iter(template) {
        let field = &capture[1];
        if !allowed.contains(&field) {
            anyhow::bail!("unknown {{{field}}} placeholder in {kind} filename template");
        }
        text_without_fields = text_without_fields.replace(&capture[0], "");
    }
    if text_without_fields.contains('{') || text_without_fields.contains('}') {
        anyhow::bail!("unmatched or nested braces in {kind} filename template");
    }
    Ok(())
}

pub(crate) fn resolve_ffprobe_path(
    cli: Option<&Path>,
    configured: Option<&Path>,
) -> Result<PathBuf> {
    let path = cli.or(configured).unwrap_or_else(|| Path::new("ffprobe"));
    if path.as_os_str().is_empty() {
        anyhow::bail!("ffprobe path cannot be empty; set --ffprobe or config ffprobe_path to an executable path");
    }
    Ok(path.to_path_buf())
}

pub(crate) fn title_match_is_confident(query: &str, result: &str) -> bool {
    let query_tokens = title_tokens(query);
    let result_tokens = title_tokens(result);
    if query_tokens.is_empty() || result_tokens.is_empty() {
        return false;
    }
    if query_tokens == result_tokens {
        return true;
    }
    let intersection = query_tokens.intersection(&result_tokens).count();
    let union = query_tokens.union(&result_tokens).count();
    intersection * 100 >= union * 80
}

pub(crate) fn title_tokens(title: &str) -> HashSet<String> {
    title
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_lowercase)
        .collect()
}

pub(crate) fn movie_match_is_confident(
    title: &str,
    year: Option<u32>,
    result: &MovieMatch,
) -> bool {
    if !title_match_is_confident(title, &result.title) {
        return false;
    }
    !matches!((year, result.year), (Some(expected), Some(actual)) if expected != actual)
}

pub(crate) fn force_movie(g: Guess) -> Guess {
    match g {
        Guess::Movie { .. } => g,
        Guess::Episode { series, year, .. } => Guess::Movie {
            title: series,
            year,
        },
    }
}

pub(crate) fn force_episode(g: Guess) -> Guess {
    match g {
        Guess::Episode { .. } => g,
        Guess::Movie { title, year } => Guess::Episode {
            series: title,
            season: 1,
            episode: 1,
            episode_end: None,
            year,
        },
    }
}

pub(crate) fn print_guess(path: &Path, guess: &Guess) {
    match guess {
        Guess::Movie { title, year } => {
            println!(
                "{}  movie   title={:?} year={}",
                path.display(),
                title,
                year.map(|y| y.to_string()).unwrap_or_else(|| "?".into())
            );
        }
        Guess::Episode {
            series,
            season,
            episode,
            episode_end,
            year,
        } => {
            let episode_label = episode_end
                .map(|end| format!("E{episode:02}-E{end:02}"))
                .unwrap_or_else(|| format!("E{episode:02}"));
            println!(
                "{}  episode series={:?} S{:02}{} year={}",
                path.display(),
                series,
                season,
                episode_label,
                year.map(|y| y.to_string()).unwrap_or_else(|| "?".into())
            );
        }
    }
}

pub(crate) fn resolve_movie(
    client: &impl MetadataProvider,
    title: &str,
    year: Option<u32>,
    batch: bool,
) -> Result<Option<MovieMatch>> {
    let matches = client
        .search_movie(title, year)
        .context("movie search failed")?;
    if matches.is_empty() {
        println!("{}", style("  no TMDb matches found").yellow());
        return Ok(None);
    }
    if batch {
        let candidate = &matches[0];
        if !movie_match_is_confident(title, year, candidate) {
            println!(
                "{} batch mode rejected uncertain match: {} ({})",
                style("  skipped:").yellow().bold(),
                candidate.title,
                candidate
                    .year
                    .map(|year| year.to_string())
                    .unwrap_or_else(|| "?".to_string())
            );
            return Ok(None);
        }
        return Ok(Some(matches.into_iter().next().unwrap()));
    }
    let labels: Vec<String> = matches
        .iter()
        .map(|m| {
            format!(
                "{} ({})",
                m.title,
                m.year.map(|y| y.to_string()).unwrap_or_else(|| "?".into())
            )
        })
        .chain(std::iter::once("Skip this file".to_string()))
        .collect();
    let idx = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("  Select a match")
        .items(&labels)
        .default(0)
        .interact()
        .unwrap_or(labels.len() - 1);
    if idx >= matches.len() {
        return Ok(None);
    }
    Ok(Some(matches.into_iter().nth(idx).unwrap()))
}

pub(crate) fn resolve_episode(
    client: &impl MetadataProvider,
    series: &str,
    season: u32,
    episode: u32,
    episode_end: Option<u32>,
    batch: bool,
) -> Result<Option<(SeriesMatch, String)>> {
    let matches = client
        .search_series(series)
        .context("series search failed")?;
    if matches.is_empty() {
        println!("{}", style("  no TMDb series matches found").yellow());
        return Ok(None);
    }
    let chosen = if batch {
        let candidate = &matches[0];
        if !title_match_is_confident(series, &candidate.name) {
            println!(
                "{} batch mode rejected uncertain match: {} ({})",
                style("  skipped:").yellow().bold(),
                candidate.name,
                candidate
                    .first_air_year
                    .map(|year| year.to_string())
                    .unwrap_or_else(|| "?".to_string())
            );
            return Ok(None);
        }
        matches.into_iter().next().unwrap()
    } else {
        let labels: Vec<String> = matches
            .iter()
            .map(|m| {
                format!(
                    "{} ({})",
                    m.name,
                    m.first_air_year
                        .map(|y| y.to_string())
                        .unwrap_or_else(|| "?".into())
                )
            })
            .chain(std::iter::once("Skip this file".to_string()))
            .collect();
        let idx = Select::with_theme(&ColorfulTheme::default())
            .with_prompt("  Select a series match")
            .items(&labels)
            .default(0)
            .interact()
            .unwrap_or(labels.len() - 1);
        if idx >= matches.len() {
            return Ok(None);
        }
        matches.into_iter().nth(idx).unwrap()
    };

    let final_episode = episode_end.unwrap_or(episode);
    if final_episode.saturating_sub(episode) >= 10 {
        anyhow::bail!(
            "multi-episode range E{episode:02}-E{final_episode:02} exceeds the 10-episode safety limit"
        );
    }
    let season_episodes = client
        .season_episodes(chosen.id, season)
        .context("season lookup failed")?;
    let today = current_utc_date();
    if let Some(reason) =
        requested_episode_metadata_issue(&season_episodes, episode, final_episode, &today)
    {
        anyhow::bail!(
            "TMDb episode metadata is not ready ({reason}); file left unchanged. Try again after TMDb updates the episode"
        );
    }
    if let Some(reason) = season_metadata_issue(&season_episodes, &today) {
        if batch {
            anyhow::bail!(
                "TMDb season metadata is not ready ({reason}); batch mode left the file unchanged. Try an interactive run to review the requested episode"
            );
        }
        println!(
            "{} other episodes in this season have incomplete metadata ({reason}); carefully review the requested title",
            style("  warning:").yellow().bold()
        );
    }

    let mut titles = Vec::new();
    for episode_number in episode..=final_episode {
        let title = season_episodes
            .iter()
            .find(|candidate| candidate.episode_number == episode_number)
            .map(|candidate| candidate.name.clone())
            .with_context(|| {
                format!(
                    "TMDb season does not contain episode {episode_number}; file left unchanged"
                )
            })?;
        titles.push(title);
    }
    Ok(Some((chosen, titles.join(" + "))))
}

fn requested_episode_metadata_issue(
    episodes: &[SeasonEpisode],
    first_episode: u32,
    final_episode: u32,
    today: &str,
) -> Option<String> {
    for episode_number in first_episode..=final_episode {
        let Some(episode) = episodes
            .iter()
            .find(|candidate| candidate.episode_number == episode_number)
        else {
            return Some(format!("episode {episode_number} is missing"));
        };
        if let Some(reason) = episode_metadata_issue(episode) {
            return Some(reason);
        }
        if episode.air_date.as_str() > today {
            return Some(format!(
                "episode {} does not air until {}",
                episode.episode_number, episode.air_date
            ));
        }
    }
    None
}

fn episode_metadata_issue(episode: &SeasonEpisode) -> Option<String> {
    let name = episode.name.trim();
    let placeholder = format!("episode {}", episode.episode_number);
    if name.is_empty() || name.eq_ignore_ascii_case(&placeholder) {
        return Some(format!(
            "episode {} still has a placeholder title",
            episode.episode_number
        ));
    }
    if episode.air_date.is_empty() {
        return Some(format!(
            "episode {} has no air date",
            episode.episode_number
        ));
    }
    None
}

fn season_metadata_issue(episodes: &[SeasonEpisode], today: &str) -> Option<String> {
    if episodes.is_empty() {
        return Some("the season has no episodes".into());
    }
    let mut first_air_date: Option<&str> = None;
    for episode in episodes {
        if let Some(reason) = episode_metadata_issue(episode) {
            return Some(reason);
        }
        first_air_date = Some(
            first_air_date
                .map(|date| date.min(episode.air_date.as_str()))
                .unwrap_or(&episode.air_date),
        );
    }
    if let Some(first_air_date) = first_air_date.filter(|date| *date > today) {
        return Some(format!("the season does not air until {first_air_date}"));
    }
    None
}

fn current_utc_date() -> String {
    let days = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        / 86_400;
    let (year, month, day) = civil_date_from_unix_days(days as i64);
    format!("{year:04}-{month:02}-{day:02}")
}

fn civil_date_from_unix_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}

pub(crate) fn resolve_api_key(args: &Args, cfg: &config::FileConfig) -> Result<String> {
    if let Some(k) = &args.api_key {
        return Ok(k.clone());
    }
    if let Ok(k) = std::env::var("TMDB_API_KEY") {
        if !k.is_empty() {
            return Ok(k);
        }
    }
    if let Some(k) = &cfg.api_key {
        if !k.is_empty() {
            return Ok(k.clone());
        }
    }
    anyhow::bail!(
        "No TMDb API key found. Pass --api-key, set $TMDB_API_KEY, or add \
         `api_key = \"...\"` to your config file ({}). Get a free key at \
         https://www.themoviedb.org/settings/api",
        config::default_config_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "config.toml".to_string())
    );
}

#[cfg(test)]
mod metadata_tests {
    use super::{
        civil_date_from_unix_days, requested_episode_metadata_issue, season_metadata_issue,
    };
    use crate::tmdb::SeasonEpisode;

    fn episode(number: u32, name: &str, air_date: &str) -> SeasonEpisode {
        SeasonEpisode {
            episode_number: number,
            name: name.into(),
            air_date: air_date.into(),
        }
    }

    #[test]
    fn incomplete_season_reports_placeholder_title() {
        let episodes = [
            episode(1, "Real Title", "2026-09-09"),
            episode(2, "Episode 2", "2026-09-09"),
        ];
        assert_eq!(
            season_metadata_issue(&episodes, "2026-09-09").as_deref(),
            Some("episode 2 still has a placeholder title")
        );
    }

    #[test]
    fn unrelated_placeholder_does_not_invalidate_requested_episode() {
        let episodes = [
            episode(8, "The Requested Title", "2026-09-09"),
            episode(9, "Episode 9", "2026-09-16"),
        ];
        assert_eq!(
            requested_episode_metadata_issue(&episodes, 8, 8, "2026-09-09"),
            None
        );
        assert_eq!(
            season_metadata_issue(&episodes, "2026-09-09").as_deref(),
            Some("episode 9 still has a placeholder title")
        );
    }

    #[test]
    fn requested_placeholder_or_future_episode_is_rejected() {
        let placeholder = [episode(2, "Episode 2", "2026-09-09")];
        assert_eq!(
            requested_episode_metadata_issue(&placeholder, 2, 2, "2026-09-09").as_deref(),
            Some("episode 2 still has a placeholder title")
        );

        let future = [episode(2, "A Real Title", "2026-09-10")];
        assert_eq!(
            requested_episode_metadata_issue(&future, 2, 2, "2026-09-09").as_deref(),
            Some("episode 2 does not air until 2026-09-10")
        );
    }

    #[test]
    fn season_premiere_must_not_be_in_the_future() {
        let episodes = [episode(1, "Real Title", "2026-09-10")];
        assert_eq!(
            season_metadata_issue(&episodes, "2026-09-09").as_deref(),
            Some("the season does not air until 2026-09-10")
        );
    }

    #[test]
    fn unix_day_conversion_matches_known_dates() {
        assert_eq!(civil_date_from_unix_days(0), (1970, 1, 1));
        assert_eq!(civil_date_from_unix_days(20_705), (2026, 9, 9));
    }
}
