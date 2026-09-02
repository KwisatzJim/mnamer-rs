use crate::model::RenamePlan;
use once_cell::sync::Lazy;
use regex::Regex;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub(crate) struct VideoQuality {
    pub(crate) height: u32,
    pub(crate) bitrate: Option<u64>,
    pub(crate) codec: Option<String>,
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
pub(crate) fn count_destinations(plans: &[RenamePlan]) -> HashMap<PathBuf, usize> {
    plans.iter().fold(HashMap::new(), |mut counts, plan| {
        *counts
            .entry(duplicate_destination_key(&plan.destination))
            .or_insert(0) += 1;
        counts
    })
}

// Keep the directory and complete title, but ignore the final container extension.
// This key is only for comparing candidates; actual destinations stay unchanged.
pub(crate) fn duplicate_destination_key(destination: &Path) -> PathBuf {
    destination.with_extension("")
}

pub(crate) fn select_preferred_sources(
    plans: &[RenamePlan],
    ffprobe: &Path,
) -> HashMap<PathBuf, Option<PathBuf>> {
    let mut grouped: HashMap<PathBuf, Vec<&RenamePlan>> = HashMap::new();
    for plan in plans {
        grouped
            .entry(duplicate_destination_key(&plan.destination))
            .or_default()
            .push(plan);
    }

    let mut groups: Vec<_> = grouped.into_iter().collect();
    groups.sort_by(|left, right| left.0.cmp(&right.0));
    groups.into_iter()
        .map(|(destination, group)| {
            if group.len() < 2 {
                return (destination, None);
            }
            let qualities: Vec<_> = group
                .iter()
                .filter_map(|plan| {
                    let (quality, origin) = quality_from_path(&plan.source, ffprobe);
                    println!("  quality: {}: {}", plan.source.display(), quality_description(quality.as_ref(), origin));
                    quality.map(|quality| (*plan, quality))
                })
                .collect();
            let quality_values: Vec<_> = qualities
                .iter()
                .map(|(_, quality)| quality.clone())
                .collect();
            let preferred = preferred_quality_index(&quality_values)
                .map(|index| qualities[index].0.source.clone());
            if let Some(source) = &preferred {
                println!("  comparison: preferred {} (highest known resolution; same-codec bitrate breaks resolution ties)", source.display());
                if qualities.len() != group.len() {
                    println!("  comparison: some candidates have unknown quality; preference uses known metadata only");
                }
            } else {
                println!("  comparison: no unique preference; quality is unknown, tied, or codecs/bitrates cannot be compared");
            }
            (destination, preferred)
        })
        .collect()
}

pub(crate) fn preferred_quality_index(qualities: &[VideoQuality]) -> Option<usize> {
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

pub(crate) fn quality_from_path(
    path: &Path,
    ffprobe: &Path,
) -> (Option<VideoQuality>, &'static str) {
    if let Some(quality) = probe_video_quality(path, ffprobe) {
        return (Some(quality), "ffprobe");
    }
    (
        resolution_from_filename(path).map(|height| VideoQuality {
            height,
            bitrate: None,
            codec: None,
        }),
        "filename tag",
    )
}

pub(crate) fn quality_description(quality: Option<&VideoQuality>, origin: &str) -> String {
    match quality {
        None => "unknown (no usable probe result or filename resolution tag)".into(),
        Some(quality) => format!(
            "{} pixels high; codec {}; bitrate {}; source: {origin}",
            quality.height,
            quality.codec.as_deref().unwrap_or("unknown"),
            quality
                .bitrate
                .map(|value| format!("{value} bit/s"))
                .unwrap_or_else(|| "unknown".into())
        ),
    }
}

pub(crate) fn probe_video_quality(path: &Path, ffprobe: &Path) -> Option<VideoQuality> {
    if !path.is_file() {
        return None;
    }
    let mut command = Command::new(ffprobe);
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
            eprintln!(
                "warning: {} (executable: {})",
                probe_error_message(path, &error),
                ffprobe.display()
            );
            return None;
        }
    };
    if !output.status.success() {
        eprintln!(
            "warning: ffprobe exited with {} for {}; trying filename resolution tags",
            output.status,
            path.display()
        );
        return None;
    }
    let quality = parse_ffprobe_quality(&output.stdout);
    if quality.is_none() {
        eprintln!("warning: ffprobe returned no usable video height for {}; trying filename resolution tags", path.display());
    }
    quality
}

pub(crate) fn probe_error_message(path: &Path, error: &std::io::Error) -> String {
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

pub(crate) fn run_probe_with_timeout(
    command: &mut Command,
    timeout: Duration,
) -> std::io::Result<Output> {
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

pub(crate) fn parse_ffprobe_quality(output: &[u8]) -> Option<VideoQuality> {
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

pub(crate) fn resolution_from_filename(path: &Path) -> Option<u32> {
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
