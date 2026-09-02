use crate::cli::Args;
use crate::model::SubtitlePlan;
use anyhow::{Context, Result};
use console::style;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub(crate) fn find_subtitle_plans(video: &Path, destination: &Path) -> Result<Vec<SubtitlePlan>> {
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
        let Some(suffix) = subtitle_suffix(source_stem, &path) else {
            continue;
        };
        subtitles.push(SubtitlePlan {
            source: path,
            destination: destination_parent.join(format!("{destination_stem}{suffix}")),
        });
    }

    subtitles.sort_by(|left, right| left.source.cmp(&right.source));
    Ok(subtitles)
}

pub(crate) fn is_subtitle(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            ["srt", "ass", "ssa", "sub", "vtt"]
                .iter()
                .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        })
}

pub(crate) fn subtitle_suffix(source_stem: &str, subtitle: &Path) -> Option<String> {
    if !is_subtitle(subtitle) {
        return None;
    }
    let suffix = subtitle.file_name()?.to_str()?.strip_prefix(source_stem)?;
    suffix.starts_with('.').then(|| suffix.to_string())
}

pub(crate) fn subtitle_input_message(subtitle: &Path, videos: &[PathBuf]) -> String {
    let parent_identity = |path: &Path| {
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        std::fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf())
    };
    let parent = parent_identity(subtitle);
    let matched = videos.iter().any(|video| {
        parent_identity(video) == parent
            && video
                .file_stem()
                .and_then(|stem| stem.to_str())
                .is_some_and(|stem| subtitle_suffix(stem, subtitle).is_some())
    });
    if matched {
        format!(
            "subtitle: {} matches a video input; will be considered with that video's rename",
            subtitle.display()
        )
    } else {
        format!("warning: unmatched subtitle {}; left unchanged (must match a video input's complete filename stem in the same directory)", subtitle.display())
    }
}

pub(crate) fn collect_files(
    args: &Args,
    recursive: bool,
    extensions: &[String],
) -> Result<Vec<PathBuf>> {
    let exts = normalize_extensions(extensions);
    let mut out = Vec::new();
    let mut subtitle_inputs = Vec::new();
    for target in &args.targets {
        if target.is_file() {
            if has_media_extension(target, &exts) {
                out.push(target.clone());
            } else if args.subtitles && is_subtitle(target) {
                subtitle_inputs.push(target.clone());
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
            } else if args.subtitles && is_subtitle(path) {
                subtitle_inputs.push(path.to_path_buf());
            }
        }
    }
    out.sort();
    let videos = deduplicate_input_files(out)?;
    subtitle_inputs.sort();
    for subtitle in deduplicate_input_files(subtitle_inputs)? {
        eprintln!("{}", subtitle_input_message(&subtitle, &videos));
    }
    Ok(videos)
}

pub(crate) fn deduplicate_input_files(files: Vec<PathBuf>) -> Result<Vec<PathBuf>> {
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

pub(crate) fn normalize_extensions(extensions: &[String]) -> Vec<String> {
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

pub(crate) fn has_media_extension(path: &Path, extensions: &[String]) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|extension| extensions.contains(&extension))
}
