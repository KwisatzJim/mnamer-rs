mod cli;
mod config;
mod parser;
mod rename;
mod tmdb;

use anyhow::{Context, Result};
use clap::Parser as _;
use cli::{Args, MediaType};
use console::style;
use dialoguer::{theme::ColorfulTheme, Confirm, Select};
use once_cell::sync::Lazy;
use parser::Guess;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};
use tempfile::NamedTempFile;
use tmdb::{MovieMatch, SeriesMatch, TmdbClient};
use walkdir::WalkDir;

struct RenamePlan {
    source: PathBuf,
    destination: PathBuf,
    subtitles: Vec<SubtitlePlan>,
}

struct SubtitlePlan {
    source: PathBuf,
    destination: PathBuf,
}

struct RunLog {
    writer: Option<BufWriter<File>>,
}

#[derive(Serialize)]
struct LogRecord {
    source: String,
    destination: Option<String>,
    status: &'static str,
    reason: String,
}

#[derive(Clone)]
struct VideoQuality {
    height: u32,
    bitrate: Option<u64>,
    codec: Option<String>,
}

#[derive(Deserialize)]
struct FfprobeOutput {
    #[serde(default)]
    streams: Vec<FfprobeStream>,
}

#[derive(Deserialize)]
struct FfprobeStream {
    height: Option<u32>,
    bit_rate: Option<String>,
    codec_name: Option<String>,
}

static RESOLUTION: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\b(4320p|8k|2160p|4k|uhd|1080p|720p|576p|480p)\b").unwrap());
static TEMPLATE_FIELD: Lazy<Regex> = Lazy::new(|| Regex::new(r"\{([^{}]+)\}").unwrap());

fn main() {
    let args = Args::parse();
    if let Err(e) = run(args) {
        eprintln!("{} {e:#}", style("error:").red().bold());
        std::process::exit(1);
    }
}

fn run(args: Args) -> Result<()> {
    let cfg = config::load(args.config.as_deref())?;

    let recursive = resolve_bool(args.recursive, args.no_recursive, cfg.recursive);
    let batch = resolve_bool(args.batch, args.no_batch, cfg.batch);
    let lower = resolve_bool(args.lower, args.no_lower, cfg.lower);
    let scene = resolve_bool(args.scene, args.no_scene, cfg.scene);
    let output_dir = args.output_dir.clone().or_else(|| cfg.output_dir.clone());
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
    let client = TmdbClient::new(api_key)?;
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
    let preferred_sources = select_preferred_sources(&plans);

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
                    log.record(
                        &plan.source,
                        Some(&plan.destination),
                        "skipped",
                        "a higher-quality duplicate was selected",
                    )?;
                    skipped += 1;
                    continue;
                }
                None => {
                    println!(
                        "{} duplicate quality is tied, unknown, or not comparable; original left unchanged",
                        style("  skipped:").yellow().bold()
                    );
                    log.record(
                        &plan.source,
                        Some(&plan.destination),
                        "skipped",
                        "duplicate quality is tied, unknown, or not comparable",
                    )?;
                    skipped += 1;
                    continue;
                }
            }
        }

        if plan.destination.exists() {
            println!(
                "{} destination already exists; original left unchanged",
                style("  skipped:").yellow().bold()
            );
            log.record(
                &plan.source,
                Some(&plan.destination),
                "skipped",
                "destination already exists",
            )?;
            skipped += 1;
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
            log.record(
                &plan.source,
                Some(&plan.destination),
                "skipped",
                "a subtitle destination already exists",
            )?;
            skipped += 1;
            continue;
        }

        if !batch && !args.dry_run {
            let proceed = Confirm::with_theme(&ColorfulTheme::default())
                .with_prompt("  Apply this rename?")
                .default(true)
                .interact()
                .unwrap_or(false);
            if !proceed {
                log.record(
                    &plan.source,
                    Some(&plan.destination),
                    "skipped",
                    "rename declined by user",
                )?;
                skipped += 1;
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
                log.record(
                    &plan.source,
                    Some(&plan.destination),
                    "failed",
                    &format!("{error:#}"),
                )?;
                failed += 1;
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

fn ensure_no_failures(failed: usize) -> Result<()> {
    if failed > 0 {
        anyhow::bail!(
            "{failed} file{} failed; see the messages above",
            if failed == 1 { "" } else { "s" }
        );
    }
    Ok(())
}

fn resolve_bool(enabled: bool, disabled: bool, configured: Option<bool>) -> bool {
    if enabled {
        true
    } else if disabled {
        false
    } else {
        configured.unwrap_or(false)
    }
}

fn validate_template(template: &str, kind: &str, allowed: &[&str]) -> Result<()> {
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

impl RunLog {
    fn new(path: Option<&Path>) -> Result<Self> {
        let writer = match path {
            Some(path) => {
                if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
                    std::fs::create_dir_all(parent)
                        .context("failed to create log file directory")?;
                }
                let file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(path)
                    .with_context(|| {
                        format!(
                            "failed to create new log file {}; existing files are never overwritten—choose an unused log path",
                            path.display()
                        )
                    })?;
                Some(BufWriter::new(file))
            }
            None => None,
        };
        Ok(Self { writer })
    }

    fn record(
        &mut self,
        source: &Path,
        destination: Option<&Path>,
        status: &'static str,
        reason: &str,
    ) -> Result<()> {
        let Some(writer) = &mut self.writer else {
            return Ok(());
        };
        let record = LogRecord {
            source: source.display().to_string(),
            destination: destination.map(|path| path.display().to_string()),
            status,
            reason: reason.to_string(),
        };
        serde_json::to_writer(&mut *writer, &record).context("failed to write log record")?;
        writer
            .write_all(b"\n")
            .context("failed to write log record")?;
        writer.flush().context("failed to flush log record")?;
        Ok(())
    }

    fn finish(mut self) -> Result<()> {
        if let Some(writer) = &mut self.writer {
            writer
                .flush()
                .context("failed to finish writing log file")?;
        }
        Ok(())
    }
}

fn count_destinations(plans: &[RenamePlan]) -> HashMap<PathBuf, usize> {
    plans.iter().fold(HashMap::new(), |mut counts, plan| {
        *counts
            .entry(duplicate_destination_key(&plan.destination))
            .or_insert(0) += 1;
        counts
    })
}

// Keep the directory and complete title, but ignore the final container extension.
// This key is only for comparing candidates; actual destinations stay unchanged.
fn duplicate_destination_key(destination: &Path) -> PathBuf {
    destination.with_extension("")
}

fn find_subtitle_plans(video: &Path, destination: &Path) -> Result<Vec<SubtitlePlan>> {
    const SUBTITLE_EXTENSIONS: &[&str] = &["srt", "ass", "ssa", "sub", "vtt"];

    let source_stem = video
        .file_stem()
        .and_then(|stem| stem.to_str())
        .context("video filename is not valid Unicode")?;
    let destination_stem = destination
        .file_stem()
        .and_then(|stem| stem.to_str())
        .context("destination filename is not valid Unicode")?;
    let source_parent = video.parent().unwrap_or_else(|| Path::new("."));
    let destination_parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let mut subtitles = Vec::new();

    for entry in std::fs::read_dir(source_parent)
        .with_context(|| format!("failed to scan for subtitles beside {}", video.display()))?
    {
        let entry = entry.context("failed to inspect a possible subtitle file")?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase);
        if !extension
            .as_deref()
            .is_some_and(|extension| SUBTITLE_EXTENSIONS.contains(&extension))
        {
            continue;
        }
        let Some(filename) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(suffix) = filename.strip_prefix(source_stem) else {
            continue;
        };
        if !suffix.starts_with('.') {
            continue;
        }
        let suffix = suffix.to_string();
        subtitles.push(SubtitlePlan {
            source: path,
            destination: destination_parent.join(format!("{destination_stem}{suffix}")),
        });
    }

    subtitles.sort_by(|left, right| left.source.cmp(&right.source));
    Ok(subtitles)
}

fn select_preferred_sources(plans: &[RenamePlan]) -> HashMap<PathBuf, Option<PathBuf>> {
    let mut grouped: HashMap<PathBuf, Vec<&RenamePlan>> = HashMap::new();
    for plan in plans {
        grouped
            .entry(duplicate_destination_key(&plan.destination))
            .or_default()
            .push(plan);
    }

    grouped
        .into_iter()
        .map(|(destination, group)| {
            if group.len() < 2 {
                return (destination, None);
            }
            let qualities: Vec<_> = group
                .iter()
                .filter_map(|plan| quality_from_path(&plan.source).map(|quality| (*plan, quality)))
                .collect();
            let quality_values: Vec<_> = qualities
                .iter()
                .map(|(_, quality)| quality.clone())
                .collect();
            let preferred = preferred_quality_index(&quality_values)
                .map(|index| qualities[index].0.source.clone());
            (destination, preferred)
        })
        .collect()
}

fn preferred_quality_index(qualities: &[VideoQuality]) -> Option<usize> {
    let highest = qualities.iter().map(|quality| quality.height).max()?;
    let matches: Vec<_> = qualities
        .iter()
        .enumerate()
        .filter(|(_, quality)| quality.height == highest)
        .collect();
    if matches.len() == 1 {
        return Some(matches[0].0);
    }
    // Bitrates are not directly comparable across different codecs.
    // Missing codec metadata must not create an automatic preference.
    let codec = matches[0].1.codec.as_deref()?;
    if !matches
        .iter()
        .all(|(_, quality)| quality.codec.as_deref() == Some(codec))
    {
        return None;
    }
    if !matches.iter().all(|(_, quality)| quality.bitrate.is_some()) {
        return None;
    }
    let highest_bitrate = matches
        .iter()
        .filter_map(|(_, quality)| quality.bitrate)
        .max()?;
    let mut bitrate_matches = matches
        .iter()
        .filter(|(_, quality)| quality.bitrate == Some(highest_bitrate));
    let first = bitrate_matches.next()?;
    bitrate_matches.next().is_none().then_some(first.0)
}

fn quality_from_path(path: &Path) -> Option<VideoQuality> {
    probe_video_quality(path).or_else(|| {
        resolution_from_filename(path).map(|height| VideoQuality {
            height,
            bitrate: None,
            codec: None,
        })
    })
}

fn probe_video_quality(path: &Path) -> Option<VideoQuality> {
    if !path.is_file() {
        return None;
    }
    let mut command = Command::new("ffprobe");
    command
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=height,bit_rate,codec_name",
            "-of",
            "json",
        ])
        .arg(path);
    let output = match run_probe_with_timeout(&mut command, Duration::from_secs(10)) {
        Ok(output) => output,
        Err(error) => {
            eprintln!("warning: {}", probe_error_message(path, &error));
            return None;
        }
    };
    if !output.status.success() {
        return None;
    }
    parse_ffprobe_quality(&output.stdout)
}

fn probe_error_message(path: &Path, error: &std::io::Error) -> String {
    let detail = match error.kind() {
        std::io::ErrorKind::NotFound =>
            "ffprobe could not be found or launched; ensure the ffprobe executable is installed and its directory is on PATH (Homebrew on Apple Silicon usually uses /opt/homebrew/bin)".to_string(),
        std::io::ErrorKind::TimedOut => "video probe timed out".to_string(),
        _ => format!("ffprobe could not run: {error}"),
    };
    format!(
        "{detail} for {}; falling back to filename resolution tags; quality may be unknown",
        path.display()
    )
}

fn run_probe_with_timeout(command: &mut Command, timeout: Duration) -> std::io::Result<Output> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output(),
            Ok(None) if start.elapsed() < timeout => {
                std::thread::sleep(Duration::from_millis(20));
            }
            result => {
                // Always terminate and reap our probe before returning to the batch.
                let error = match result {
                    Err(error) => error,
                    _ => std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "video probe exceeded its time limit",
                    ),
                };
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        }
    }
}

fn parse_ffprobe_quality(output: &[u8]) -> Option<VideoQuality> {
    let parsed: FfprobeOutput = serde_json::from_slice(output).ok()?;
    let stream = parsed.streams.into_iter().next()?;
    let height = stream.height.filter(|height| *height > 0)?;
    let bitrate = stream
        .bit_rate
        .and_then(|bitrate| bitrate.parse().ok())
        .filter(|bitrate| *bitrate > 0);
    let codec = stream
        .codec_name
        .map(|codec| codec.trim().to_ascii_lowercase())
        .filter(|codec| !codec.is_empty() && codec != "unknown");
    Some(VideoQuality {
        height,
        bitrate,
        codec,
    })
}

fn resolution_from_filename(path: &Path) -> Option<u32> {
    let filename = path.file_name()?.to_str()?;
    RESOLUTION
        .captures_iter(filename)
        .filter_map(|capture| match capture[1].to_ascii_lowercase().as_str() {
            "8k" | "4320p" => Some(4320),
            "4k" | "uhd" | "2160p" => Some(2160),
            value => value.strip_suffix('p')?.parse().ok(),
        })
        .max()
}

fn title_match_is_confident(query: &str, result: &str) -> bool {
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

fn title_tokens(title: &str) -> HashSet<String> {
    title
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn movie_match_is_confident(title: &str, year: Option<u32>, result: &MovieMatch) -> bool {
    if !title_match_is_confident(title, &result.title) {
        return false;
    }
    !matches!((year, result.year), (Some(expected), Some(actual)) if expected != actual)
}

fn force_movie(g: Guess) -> Guess {
    match g {
        Guess::Movie { .. } => g,
        Guess::Episode { series, year, .. } => Guess::Movie {
            title: series,
            year,
        },
    }
}

fn force_episode(g: Guess) -> Guess {
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

fn print_guess(path: &Path, guess: &Guess) {
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

fn resolve_movie(
    client: &TmdbClient,
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

fn resolve_episode(
    client: &TmdbClient,
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
    let mut titles = Vec::new();
    for episode_number in episode..=final_episode {
        let title = client
            .episode_title(chosen.id, season, episode_number)
            .with_context(|| format!("episode {episode_number} lookup failed"))?
            .unwrap_or_else(|| format!("Episode {episode_number}"));
        titles.push(title);
    }
    Ok(Some((chosen, titles.join(" + "))))
}

fn resolve_api_key(args: &Args, cfg: &config::FileConfig) -> Result<String> {
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

fn collect_files(args: &Args, recursive: bool, extensions: &[String]) -> Result<Vec<PathBuf>> {
    let exts = normalize_extensions(extensions);
    let mut out = Vec::new();
    for target in &args.targets {
        if target.is_file() {
            if has_media_extension(target, &exts) {
                out.push(target.clone());
            } else {
                eprintln!(
                    "{} {} does not have an enabled media extension, skipping",
                    style("warning:").yellow(),
                    target.display()
                );
            }
            continue;
        }
        if !target.is_dir() {
            let reason = if target.exists() {
                "is not a readable file or directory"
            } else {
                "does not exist"
            };
            eprintln!(
                "{} {} {reason}, skipping",
                style("warning:").yellow(),
                target.display(),
            );
            continue;
        }
        let walker = if recursive {
            WalkDir::new(target)
        } else {
            WalkDir::new(target).max_depth(1)
        };
        for entry in walker {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    let path = error
                        .path()
                        .map(|path| path.display().to_string())
                        .unwrap_or_else(|| target.display().to_string());
                    eprintln!(
                        "{} could not inspect {}: {error}; continuing",
                        style("warning:").yellow(),
                        path
                    );
                    continue;
                }
            };
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            if has_media_extension(path, &exts) {
                out.push(path.to_path_buf());
            }
        }
    }
    out.sort();
    deduplicate_input_files(out)
}

fn deduplicate_input_files(files: Vec<PathBuf>) -> Result<Vec<PathBuf>> {
    let mut seen = HashSet::new();
    let mut unique = Vec::new();
    for path in files {
        // Use the resolved path only as an identity key. Keep the original
        // path for display and file operations.
        let identity = std::fs::canonicalize(&path)
            .with_context(|| format!("failed to resolve input file {}", path.display()))?;
        if seen.insert(identity) {
            unique.push(path);
        }
    }
    Ok(unique)
}

fn normalize_extensions(extensions: &[String]) -> Vec<String> {
    extensions
        .iter()
        .map(|extension| {
            extension
                .trim()
                .trim_start_matches('.')
                .to_ascii_lowercase()
        })
        .filter(|extension| !extension.is_empty())
        .collect()
}

fn has_media_extension(path: &Path, extensions: &[String]) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|extension| extensions.contains(&extension))
}

/// Apply a video and its subtitles as one group, rolling back completed moves
/// if a later subtitle fails.
fn apply_rename_group(plan: &RenamePlan, force_copy: bool) -> Result<()> {
    let mut completed: Vec<(&Path, &Path)> = Vec::new();

    apply_move(&plan.source, &plan.destination, force_copy)?;
    completed.push((&plan.source, &plan.destination));

    for subtitle in &plan.subtitles {
        if let Err(error) = apply_move(&subtitle.source, &subtitle.destination, force_copy) {
            return match rollback_moves(&completed) {
                Ok(()) => Err(error).with_context(|| {
                    format!(
                        "subtitle move failed for {}; completed group changes were rolled back",
                        subtitle.source.display()
                    )
                }),
                Err(rollback_error) => Err(error).with_context(|| {
                    format!(
                        "subtitle move failed for {}; rollback also failed: {rollback_error:#}",
                        subtitle.source.display()
                    )
                }),
            };
        }
        completed.push((&subtitle.source, &subtitle.destination));
    }

    Ok(())
}

fn rollback_moves(completed: &[(&Path, &Path)]) -> Result<()> {
    let mut errors = Vec::new();
    for (source, destination) in completed.iter().rev() {
        if let Err(error) = apply_move(destination, source, false) {
            errors.push(format!(
                "{} -> {}: {error:#}",
                destination.display(),
                source.display()
            ));
        }
    }
    if !errors.is_empty() {
        anyhow::bail!(errors.join("; "));
    }
    Ok(())
}

/// Move `from` to `to` without ever replacing an existing destination.
/// A hard link provides an atomic same-filesystem move; filesystems that do
/// not support it fall back to the no-clobber temporary-copy path.
fn apply_move(from: &Path, to: &Path, force_copy: bool) -> Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).context("failed to create destination directory")?;
    }
    if std::fs::symlink_metadata(to).is_ok() {
        anyhow::bail!("destination already exists: {}", to.display());
    }
    if !force_copy {
        match std::fs::hard_link(from, to) {
            Ok(()) => {
                if let Err(error) = std::fs::remove_file(from) {
                    return match std::fs::remove_file(to) {
                        Ok(()) => Err(error).with_context(|| {
                            format!(
                                "could not remove original {}; newly created destination was rolled back",
                                from.display()
                            )
                        }),
                        Err(cleanup_error) => Err(error).with_context(|| {
                            format!(
                                "could not remove original {}; cleanup of newly created destination {} also failed: {cleanup_error}",
                                from.display(),
                                to.display()
                            )
                        }),
                    };
                }
                return Ok(());
            }
            Err(error) if std::fs::symlink_metadata(to).is_ok() => {
                anyhow::bail!(
                    "destination appeared before the move completed and was left unchanged: {} ({error})",
                    to.display()
                );
            }
            Err(_) => {}
        }
    }
    copy_then_delete(from, to)?;
    Ok(())
}

fn copy_then_delete(from: &Path, to: &Path) -> Result<()> {
    let parent = to.parent().unwrap_or_else(|| Path::new("."));
    let mut source = File::open(from).context("failed to open source file for copying")?;
    let source_permissions = source
        .metadata()
        .context("failed to read source file metadata")?
        .permissions();
    let mut temporary =
        NamedTempFile::new_in(parent).context("failed to create temporary destination file")?;

    std::io::copy(&mut source, temporary.as_file_mut())
        .context("failed while copying file to temporary destination")?;
    temporary
        .as_file_mut()
        .set_permissions(source_permissions)
        .context("failed to preserve source file permissions")?;
    temporary
        .as_file_mut()
        .sync_all()
        .context("failed to finish writing temporary destination file")?;

    temporary
        .persist_noclobber(to)
        .map_err(|error| error.error)
        .context("failed to publish copied file at destination")?;
    if let Err(error) = std::fs::remove_file(from) {
        return match std::fs::remove_file(to) {
            Ok(()) => Err(error).context(
                "failed to remove original file after copy; published destination was rolled back",
            ),
            Err(cleanup_error) => Err(error).context(format!(
                "failed to remove original file after copy; cleanup of published destination also failed: {cleanup_error}"
            )),
        };
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_aliases_are_processed_once_without_changing_display_path() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("episode.mkv");
        std::fs::write(&source, b"video").unwrap();
        let alias = directory.path().join(".").join("episode.mkv");
        let absolute = std::fs::canonicalize(&source).unwrap();

        let files = deduplicate_input_files(vec![alias.clone(), source, absolute]).unwrap();

        assert_eq!(files, vec![alias]);
    }

    #[test]
    fn distinct_files_with_the_same_name_are_not_deduplicated() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first");
        let second = directory.path().join("second");
        std::fs::create_dir(&first).unwrap();
        std::fs::create_dir(&second).unwrap();
        let first = first.join("episode.mkv");
        let second = second.join("episode.mkv");
        std::fs::write(&first, b"first video").unwrap();
        std::fs::write(&second, b"second video").unwrap();

        let files = vec![first, second];
        assert_eq!(deduplicate_input_files(files.clone()).unwrap(), files);
    }

    #[test]
    fn counts_duplicate_planned_destinations() {
        let plans = vec![
            RenamePlan {
                source: PathBuf::from("messy-one.mkv"),
                destination: PathBuf::from("Movie (2024).mkv"),
                subtitles: Vec::new(),
            },
            RenamePlan {
                source: PathBuf::from("messy-two.mkv"),
                destination: PathBuf::from("Movie (2024).mkv"),
                subtitles: Vec::new(),
            },
        ];

        let counts = count_destinations(&plans);

        assert_eq!(counts[&PathBuf::from("Movie (2024)")], 2);
    }

    #[test]
    fn selects_unique_highest_resolution_duplicate() {
        let destination = PathBuf::from("Show - S01E01 - Pilot.mkv");
        let plans = vec![
            RenamePlan {
                source: PathBuf::from("show.s01e01.720p.mkv"),
                destination: destination.clone(),
                subtitles: Vec::new(),
            },
            RenamePlan {
                source: PathBuf::from("show.s01e01.2160p.mkv"),
                destination: destination.clone(),
                subtitles: Vec::new(),
            },
        ];

        let preferred = select_preferred_sources(&plans);

        assert_eq!(
            preferred[&duplicate_destination_key(&destination)],
            Some(PathBuf::from("show.s01e01.2160p.mkv"))
        );
    }

    #[test]
    fn cross_extension_duplicates_prefer_resolution_not_container() {
        for title in ["The Runner (2026)", "Reacher - S04E06 - Plum Out of Luck"] {
            for (higher, lower) in [("mkv", "mp4"), ("mp4", "mkv")] {
                let plans = vec![
                    RenamePlan {
                        source: PathBuf::from(format!("missing-source.720p.{lower}")),
                        destination: PathBuf::from(format!("{title}.{lower}")),
                        subtitles: Vec::new(),
                    },
                    RenamePlan {
                        source: PathBuf::from(format!("missing-source.1080p.{higher}")),
                        destination: PathBuf::from(format!("{title}.{higher}")),
                        subtitles: Vec::new(),
                    },
                ];
                let key = duplicate_destination_key(&plans[0].destination);
                assert_eq!(count_destinations(&plans)[&key], 2);
                assert_eq!(
                    select_preferred_sources(&plans)[&key],
                    Some(plans[1].source.clone())
                );
                assert_eq!(plans[1].destination.extension().unwrap(), higher);
            }
        }
    }

    #[test]
    fn duplicate_keys_preserve_directories_titles_and_episode_numbers() {
        let key = duplicate_destination_key(Path::new("one/Show.Name - S01E01.mkv"));
        assert_eq!(key, PathBuf::from("one/Show.Name - S01E01"));
        for other in [
            "two/Show.Name - S01E01.mp4",
            "one/Show.Name - S01E02.mp4",
            "one/Other.Name - S01E01.mp4",
        ] {
            assert_ne!(key, duplicate_destination_key(Path::new(other)));
        }
    }

    #[test]
    fn cross_extension_resolution_ties_remain_ambiguous() {
        let plans: Vec<_> = ["mkv", "mp4"]
            .into_iter()
            .map(|extension| RenamePlan {
                source: PathBuf::from(format!("missing-source.1080p.{extension}")),
                destination: PathBuf::from(format!("Movie (2026).{extension}")),
                subtitles: Vec::new(),
            })
            .collect();
        let key = duplicate_destination_key(&plans[0].destination);
        assert_eq!(count_destinations(&plans)[&key], 2);
        assert_eq!(select_preferred_sources(&plans)[&key], None);
    }

    #[test]
    fn parses_actual_video_quality_from_ffprobe_output() {
        let quality = parse_ffprobe_quality(
            br#"{"streams":[{"height":1080,"bit_rate":"6500000","codec_name":"H264"}]}"#,
        )
        .unwrap();

        assert_eq!(quality.height, 1080);
        assert_eq!(quality.bitrate, Some(6_500_000));
        assert_eq!(quality.codec.as_deref(), Some("h264"));
        let missing = parse_ffprobe_quality(br#"{"streams":[{"height":1080}]}"#).unwrap();
        assert!(missing.codec.is_none());
        assert!(parse_ffprobe_quality(b"not json").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn stalled_probe_is_stopped_at_deadline() {
        let mut command = Command::new("sh");
        command.args(["-c", "exec sleep 5"]);
        let started = Instant::now();
        let error = run_probe_with_timeout(&mut command, Duration::from_millis(50)).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn missing_probe_explains_path_and_fallback() {
        let directory = tempfile::tempdir().unwrap();
        let mut command = Command::new(directory.path().join("missing-ffprobe"));
        let error = run_probe_with_timeout(&mut command, Duration::from_secs(1)).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
        let message = probe_error_message(Path::new("movie.mkv"), &error);
        assert!(message.contains("ffprobe"));
        assert!(message.contains("PATH"));
        assert!(message.contains("movie.mkv"));
        assert!(message.contains("falling back to filename resolution tags"));
    }

    #[test]
    fn probe_errors_preserve_timeout_and_permission_details() {
        for (kind, expected) in [
            (std::io::ErrorKind::TimedOut, "video probe timed out"),
            (
                std::io::ErrorKind::PermissionDenied,
                "test permission failure",
            ),
        ] {
            let error = std::io::Error::new(kind, "test permission failure");
            let message = probe_error_message(Path::new("movie.mkv"), &error);
            assert!(message.contains(expected));
            assert!(message.contains("quality may be unknown"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn completed_probe_output_is_preserved() {
        let mut command = Command::new("sh");
        command.args(["-c", "printf 'probe result'"]);
        let output = run_probe_with_timeout(&mut command, Duration::from_secs(3)).unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"probe result");
    }

    #[test]
    fn bitrate_breaks_an_actual_resolution_tie() {
        let qualities = [
            VideoQuality {
                height: 1080,
                bitrate: Some(4_000_000),
                codec: Some("h264".to_string()),
            },
            VideoQuality {
                height: 1080,
                bitrate: Some(8_000_000),
                codec: Some("h264".to_string()),
            },
        ];

        assert_eq!(preferred_quality_index(&qualities), Some(1));
    }

    #[test]
    fn bitrate_does_not_rank_different_or_unknown_codecs() {
        for codec in [Some("hevc"), None] {
            let qualities = [
                VideoQuality {
                    height: 1080,
                    bitrate: Some(4_000_000),
                    codec: Some("h264".into()),
                },
                VideoQuality {
                    height: 1080,
                    bitrate: Some(8_000_000),
                    codec: codec.map(str::to_string),
                },
            ];
            assert_eq!(preferred_quality_index(&qualities), None);
        }
    }

    #[test]
    fn resolution_still_takes_priority_over_codec_and_bitrate() {
        let qualities = [
            VideoQuality {
                height: 2160,
                bitrate: Some(4_000_000),
                codec: Some("hevc".into()),
            },
            VideoQuality {
                height: 1080,
                bitrate: Some(8_000_000),
                codec: Some("h264".into()),
            },
        ];
        assert_eq!(preferred_quality_index(&qualities), Some(0));
    }

    #[test]
    fn same_codec_with_missing_or_tied_bitrate_remains_ambiguous() {
        for bitrate in [None, Some(4_000_000)] {
            let qualities = [
                VideoQuality {
                    height: 1080,
                    bitrate: Some(4_000_000),
                    codec: Some("h264".into()),
                },
                VideoQuality {
                    height: 1080,
                    bitrate,
                    codec: Some("h264".into()),
                },
            ];
            assert_eq!(preferred_quality_index(&qualities), None);
        }
    }

    #[test]
    fn batch_confidence_rejects_weak_titles_and_year_mismatches() {
        assert!(title_match_is_confident(
            "Spider-Man: Homecoming",
            "Spider Man Homecoming"
        ));
        assert!(!title_match_is_confident("The Office", "Office Space"));

        let result = MovieMatch {
            title: "Dune".to_string(),
            year: Some(2021),
        };
        assert!(movie_match_is_confident("Dune", Some(2021), &result));
        assert!(!movie_match_is_confident("Dune", Some(1984), &result));
    }

    #[test]
    fn does_not_choose_when_highest_resolution_is_tied() {
        let destination = PathBuf::from("Show - S01E01 - Pilot.mkv");
        let plans = vec![
            RenamePlan {
                source: PathBuf::from("show.s01e01.1080p.web.mkv"),
                destination: destination.clone(),
                subtitles: Vec::new(),
            },
            RenamePlan {
                source: PathBuf::from("show.s01e01.1080p.bluray.mkv"),
                destination: destination.clone(),
                subtitles: Vec::new(),
            },
        ];

        let preferred = select_preferred_sources(&plans);

        assert_eq!(preferred[&duplicate_destination_key(&destination)], None);
    }

    #[test]
    fn forced_copy_publishes_complete_file_then_removes_source() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.mkv");
        let destination = directory.path().join("destination.mkv");
        let contents = b"complete media contents";
        std::fs::write(&source, contents).unwrap();

        apply_move(&source, &destination, true).unwrap();

        assert!(!source.exists());
        assert_eq!(std::fs::read(destination).unwrap(), contents);
    }

    #[test]
    fn subtitle_failure_rolls_video_back_to_original_name() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("messy.mkv");
        let destination = directory.path().join("Clean.mkv");
        std::fs::write(&source, b"video").unwrap();
        let plan = RenamePlan {
            source: source.clone(),
            destination: destination.clone(),
            subtitles: vec![SubtitlePlan {
                source: directory.path().join("missing.srt"),
                destination: directory.path().join("Clean.srt"),
            }],
        };

        let error = apply_rename_group(&plan, false).unwrap_err();

        assert!(error.to_string().contains("were rolled back"));
        assert_eq!(std::fs::read(source).unwrap(), b"video");
        assert!(!destination.exists());
    }

    #[test]
    fn move_never_replaces_existing_destination() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.mkv");
        let destination = directory.path().join("destination.mkv");
        std::fs::write(&source, b"source contents").unwrap();
        std::fs::write(&destination, b"existing destination").unwrap();

        let error = apply_move(&source, &destination, false).unwrap_err();

        assert!(error.to_string().contains("destination already exists"));
        assert_eq!(std::fs::read(&source).unwrap(), b"source contents");
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            b"existing destination"
        );
    }

    #[test]
    fn writes_json_line_run_log() {
        let directory = tempfile::tempdir().unwrap();
        let log_path = directory.path().join("run.jsonl");
        let mut log = RunLog::new(Some(&log_path)).unwrap();

        log.record(
            Path::new("messy.mkv"),
            Some(Path::new("Movie (2024).mkv")),
            "renamed",
            "rename completed",
        )
        .unwrap();
        // Records must be readable before the log is closed or finalized.
        let line = std::fs::read_to_string(log_path).unwrap();
        let record: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(record["source"], "messy.mkv");
        assert_eq!(record["destination"], "Movie (2024).mkv");
        assert_eq!(record["status"], "renamed");
        log.finish().unwrap();
    }

    #[test]
    fn log_refuses_to_overwrite_existing_files() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("video.mkv");
        std::fs::write(&path, b"original contents").unwrap();

        let error = RunLog::new(Some(&path)).err().unwrap();

        assert!(error.to_string().contains("choose an unused log path"));
        assert_eq!(std::fs::read(&path).unwrap(), b"original contents");
    }

    #[test]
    fn plans_matching_subtitles_and_preserves_language_suffixes() {
        let directory = tempfile::tempdir().unwrap();
        let video = directory.path().join("show.s01e01.1080p.mkv");
        std::fs::write(&video, b"video").unwrap();
        std::fs::write(directory.path().join("show.s01e01.1080p.srt"), b"subs").unwrap();
        std::fs::write(
            directory.path().join("show.s01e01.1080p.en.forced.ass"),
            b"subs",
        )
        .unwrap();
        std::fs::write(directory.path().join("different.srt"), b"subs").unwrap();
        let destination = directory.path().join("Show - S01E01 - Pilot.mkv");

        let subtitles = find_subtitle_plans(&video, &destination).unwrap();
        let destinations: Vec<_> = subtitles
            .iter()
            .map(|subtitle| subtitle.destination.file_name().unwrap().to_owned())
            .collect();

        assert_eq!(
            destinations,
            vec![
                "Show - S01E01 - Pilot.en.forced.ass",
                "Show - S01E01 - Pilot.srt"
            ]
        );
    }

    #[test]
    fn returns_failure_status_only_when_files_failed() {
        assert!(ensure_no_failures(0).is_ok());
        assert_eq!(
            ensure_no_failures(2).unwrap_err().to_string(),
            "2 files failed; see the messages above"
        );
    }

    #[test]
    fn command_line_boolean_overrides_config() {
        assert!(resolve_bool(true, false, Some(false)));
        assert!(!resolve_bool(false, true, Some(true)));
        assert!(resolve_bool(false, false, Some(true)));
        assert!(!resolve_bool(false, false, None));
    }

    #[test]
    fn validates_filename_template_placeholders() {
        assert!(
            validate_template("{title} ({year}){ext}", "movie", &["title", "year", "ext"]).is_ok()
        );
        assert!(
            validate_template("{titel}{ext}", "movie", &["title", "year", "ext"])
                .unwrap_err()
                .to_string()
                .contains("unknown {titel}")
        );
        assert!(
            validate_template("{title", "movie", &["title", "year", "ext"])
                .unwrap_err()
                .to_string()
                .contains("unmatched")
        );
        assert!(validate_template("  ", "movie", &["title", "year", "ext"])
            .unwrap_err()
            .to_string()
            .contains("cannot be empty"));
    }

    #[test]
    fn media_extension_filter_excludes_wildcard_sidecars() {
        let configured = vec![" .MKV ".to_string(), "mp4".to_string()];
        let extensions = normalize_extensions(&configured);

        assert!(has_media_extension(Path::new("episode.MKV"), &extensions));
        assert!(!has_media_extension(
            Path::new("episode.en.srt"),
            &extensions
        ));
        assert!(!has_media_extension(
            Path::new("rename-report.jsonl"),
            &extensions
        ));
    }
}
