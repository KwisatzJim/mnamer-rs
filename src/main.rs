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
use parser::Guess;
use std::path::{Path, PathBuf};
use tmdb::{MovieMatch, SeriesMatch, TmdbClient};
use walkdir::WalkDir;

fn main() {
    let args = Args::parse();
    if let Err(e) = run(args) {
        eprintln!("{} {e:#}", style("error:").red().bold());
        std::process::exit(1);
    }
}

fn run(args: Args) -> Result<()> {
    let cfg = config::load(args.config.as_deref());

    let recursive = args.recursive || cfg.recursive.unwrap_or(false);
    let batch = args.batch || cfg.batch.unwrap_or(false);
    let lower = args.lower || cfg.lower.unwrap_or(false);
    let scene = args.scene || cfg.scene.unwrap_or(false);
    let output_dir = args.output_dir.clone().or_else(|| cfg.output_dir.clone());
    let extensions = args
        .extensions
        .clone()
        .or_else(|| cfg.extensions.clone())
        .unwrap_or_else(|| cli::DEFAULT_EXTENSIONS.iter().map(|s| s.to_string()).collect());
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

    let files = collect_files(&args, recursive, &extensions)?;
    if files.is_empty() {
        println!("No matching media files found.");
        return Ok(());
    }

    if args.parse_only {
        for f in &files {
            let stem = f.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
            let guess = parser::parse_filename(stem);
            print_guess(f, &guess);
        }
        return Ok(());
    }

    let api_key = resolve_api_key(&args, &cfg)?;
    let client = TmdbClient::new(api_key);

    let mut renamed = 0usize;
    let mut skipped = 0usize;

    for f in &files {
        println!("\n{} {}", style("File:").cyan().bold(), f.display());
        let stem = f.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
        let ext = f.extension().and_then(|s| s.to_str()).unwrap_or_default();
        let mut guess = parser::parse_filename(stem);

        // Respect a forced --media override
        guess = match args.media {
            MediaType::Movie => force_movie(guess),
            MediaType::Episode => force_episode(guess),
            MediaType::Auto => guess,
        };

        let target_name = match &guess {
            Guess::Movie { title, year } => {
                match resolve_movie(&client, title, *year, batch)? {
                    Some(m) => rename::render_movie(
                        &format_movie,
                        &rename::MovieVars {
                            title: &m.title,
                            year: m.year,
                            ext,
                        },
                        lower,
                        scene,
                    ),
                    None => {
                        println!("{}", style("  skipped (no match chosen)").yellow());
                        skipped += 1;
                        continue;
                    }
                }
            }
            Guess::Episode {
                series,
                season,
                episode,
                ..
            } => {
                match resolve_episode(&client, series, *season, *episode, batch)? {
                    Some((s, ep_title)) => rename::render_episode(
                        &format_episode,
                        &rename::EpisodeVars {
                            series: &s.name,
                            year: s.first_air_year,
                            season: *season,
                            episode: *episode,
                            episode_title: &ep_title,
                            ext,
                        },
                        lower,
                        scene,
                    ),
                    None => {
                        println!("{}", style("  skipped (no match chosen)").yellow());
                        skipped += 1;
                        continue;
                    }
                }
            }
        };

        let dest_dir = output_dir.clone().unwrap_or_else(|| {
            f.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."))
        });
        let dest = dest_dir.join(&target_name);

        println!("  {} {}", style("->").green().bold(), dest.display());

        if dest == *f {
            println!("{}", style("  already correctly named, skipping").dim());
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
                skipped += 1;
                continue;
            }
        }

        if args.dry_run {
            println!("{}", style("  (dry run, not applied)").dim());
            continue;
        }

        apply_move(f, &dest, args.force_copy)?;
        renamed += 1;
    }

    println!(
        "\n{} {renamed} renamed, {skipped} skipped.",
        style("Done:").bold()
    );
    Ok(())
}

fn force_movie(g: Guess) -> Guess {
    match g {
        Guess::Movie { .. } => g,
        Guess::Episode { series, year, .. } => Guess::Movie { title: series, year },
    }
}

fn force_episode(g: Guess) -> Guess {
    match g {
        Guess::Episode { .. } => g,
        Guess::Movie { title, year } => Guess::Episode {
            series: title,
            season: 1,
            episode: 1,
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
            year,
        } => {
            println!(
                "{}  episode series={:?} S{:02}E{:02} year={}",
                path.display(),
                series,
                season,
                episode,
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
    let matches = client.search_movie(title, year).context("movie search failed")?;
    if matches.is_empty() {
        println!("{}", style("  no TMDb matches found").yellow());
        return Ok(None);
    }
    if batch {
        return Ok(Some(matches.into_iter().next().unwrap()));
    }
    let labels: Vec<String> = matches
        .iter()
        .map(|m| format!("{} ({})", m.title, m.year.map(|y| y.to_string()).unwrap_or_else(|| "?".into())))
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
    batch: bool,
) -> Result<Option<(SeriesMatch, String)>> {
    let matches = client.search_series(series).context("series search failed")?;
    if matches.is_empty() {
        println!("{}", style("  no TMDb series matches found").yellow());
        return Ok(None);
    }
    let chosen = if batch {
        matches.into_iter().next().unwrap()
    } else {
        let labels: Vec<String> = matches
            .iter()
            .map(|m| {
                format!(
                    "{} ({})",
                    m.name,
                    m.first_air_year.map(|y| y.to_string()).unwrap_or_else(|| "?".into())
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

    let ep_title = client
        .episode_title(chosen.id, season, episode)
        .context("episode lookup failed")?
        .unwrap_or_else(|| "Unknown".to_string());
    Ok(Some((chosen, ep_title)))
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
    let exts: Vec<String> = extensions.iter().map(|e| e.to_lowercase()).collect();
    let mut out = Vec::new();
    for target in &args.targets {
        if target.is_file() {
            out.push(target.clone());
            continue;
        }
        if !target.is_dir() {
            eprintln!("{} {} does not exist, skipping", style("warning:").yellow(), target.display());
            continue;
        }
        let walker = if recursive {
            WalkDir::new(target)
        } else {
            WalkDir::new(target).max_depth(1)
        };
        for entry in walker.into_iter().filter_map(|e| e.ok()) {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                if exts.contains(&ext.to_lowercase()) {
                    out.push(path.to_path_buf());
                }
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Rename `from` to `to`, falling back to copy+delete if they're on
/// different filesystems (std::fs::rename fails cross-device).
fn apply_move(from: &Path, to: &Path, force_copy: bool) -> Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).context("failed to create destination directory")?;
    }
    if to.exists() {
        anyhow::bail!("destination already exists: {}", to.display());
    }
    if !force_copy {
        if std::fs::rename(from, to).is_ok() {
            return Ok(());
        }
    }
    std::fs::copy(from, to).context("failed to copy file to destination")?;
    std::fs::remove_file(from).context("failed to remove original file after copy")?;
    Ok(())
}
