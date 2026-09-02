use crate::cli::Args;
use crate::config;
use crate::model::*;
use crate::operations::*;
use crate::quality::*;
use crate::report::RunLog;
use crate::scanning::*;
use crate::tmdb::MovieMatch;
use crate::workflow::*;
use clap::Parser as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

#[test]
fn ffprobe_path_precedence_and_validation() {
    assert_eq!(
        resolve_ffprobe_path(None, None).unwrap(),
        PathBuf::from("ffprobe")
    );
    let config = Path::new("/config/ffprobe");
    let cli = Path::new("/cli/ffprobe");
    assert_eq!(resolve_ffprobe_path(None, Some(config)).unwrap(), config);
    assert_eq!(resolve_ffprobe_path(Some(cli), Some(config)).unwrap(), cli);
    assert!(resolve_ffprobe_path(Some(Path::new("")), None).is_err());
    let args = Args::parse_from([
        "mnamer-rs",
        "--ffprobe",
        "/path with spaces/ffprobe",
        "movie.mkv",
    ]);
    assert_eq!(
        args.ffprobe.unwrap(),
        PathBuf::from("/path with spaces/ffprobe")
    );
    let config: config::FileConfig = toml::from_str("ffprobe_path = '/config/ffprobe'").unwrap();
    assert_eq!(
        config.ffprobe_path.unwrap(),
        PathBuf::from("/config/ffprobe")
    );
}

#[cfg(unix)]
#[test]
fn explicit_probe_path_with_spaces_is_executed_and_failure_falls_back() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let probe = dir.path().join("fake probe");
    std::fs::write(&probe, "#!/bin/sh\nprintf '%s' '{\"streams\":[{\"height\":1080,\"codec_name\":\"h264\",\"bit_rate\":\"4000000\"}]}'\n").unwrap();
    std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o700)).unwrap();
    let video = dir.path().join("movie.720p.mkv");
    std::fs::write(&video, b"test fixture").unwrap();
    let (quality, origin) = quality_from_path(&video, &probe);
    let quality = quality.unwrap();
    assert_eq!(quality.height, 1080);
    let description = quality_description(Some(&quality), origin);
    assert!(description.contains("1080 pixels high"));
    assert!(description.contains("h264"));
    assert!(description.contains("4000000 bit/s"));
    assert!(description.contains("source: ffprobe"));
    let (fallback, origin) = quality_from_path(&video, &dir.path().join("missing-probe"));
    assert_eq!(fallback.as_ref().unwrap().height, 720);
    assert!(quality_description(fallback.as_ref(), origin).contains("filename tag"));
    assert!(quality_description(None, origin).starts_with("unknown"));
}

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

    let preferred = select_preferred_sources(&plans, Path::new("ffprobe"));

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
                select_preferred_sources(&plans, Path::new("ffprobe"))[&key],
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
    assert_eq!(
        select_preferred_sources(&plans, Path::new("ffprobe"))[&key],
        None
    );
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

    let preferred = select_preferred_sources(&plans, Path::new("ffprobe"));

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
    let log_path = directory.path().join("failed-group.jsonl");
    let mut log = RunLog::new(Some(&log_path)).unwrap();
    log.record_group(&plan, "failed", &format!("{error:#}"))
        .unwrap();
    let records: Vec<serde_json::Value> = std::fs::read_to_string(log_path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(records.len(), 2);
    assert!(records.iter().all(|record| record["status"] == "failed"));
    assert!(records[1]["reason"]
        .as_str()
        .unwrap()
        .contains("were rolled back"));
    assert_eq!(
        records[1]["source"],
        plan.subtitles[0].source.display().to_string()
    );
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
fn skipped_groups_log_every_subtitle_with_the_group_reason() {
    let directory = tempfile::tempdir().unwrap();
    let plan = RenamePlan {
        source: PathBuf::from("messy.mkv"),
        destination: PathBuf::from("Clean.mkv"),
        subtitles: ["srt", "en.ass"]
            .into_iter()
            .map(|suffix| SubtitlePlan {
                source: PathBuf::from(format!("messy.{suffix}")),
                destination: PathBuf::from(format!("Clean.{suffix}")),
            })
            .collect(),
    };
    for (index, reason) in [
        "a higher-quality duplicate was selected",
        "duplicate quality is tied, unknown, or not comparable",
        "destination already exists",
        "a subtitle destination already exists",
        "rename declined by user",
    ]
    .into_iter()
    .enumerate()
    {
        let path = directory.path().join(format!("{index}.jsonl"));
        let mut log = RunLog::new(Some(&path)).unwrap();
        log.record_group(&plan, "skipped", reason).unwrap();
        let records: Vec<serde_json::Value> = std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(records.len(), 3);
        assert_eq!(records[0]["source"], "messy.mkv");
        assert_eq!(records[0]["reason"], reason);
        for (record, subtitle) in records[1..].iter().zip(&plan.subtitles) {
            assert_eq!(record["source"], subtitle.source.display().to_string());
            assert_eq!(
                record["destination"],
                subtitle.destination.display().to_string()
            );
            assert_eq!(record["status"], "skipped");
            assert!(record["reason"].as_str().unwrap().contains(reason));
        }
    }
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
fn subtitle_reporting_distinguishes_matches_from_unmatched_inputs() {
    let directory = tempfile::tempdir().unwrap();
    let video = directory.path().join("Outlander.s07e03.socoolyo.mkv");
    let videos = vec![video];
    let matched = directory
        .path()
        .join("Outlander.s07e03.socoolyo.en.forced.SRT");
    assert!(subtitle_input_message(&matched, &videos).contains("matches a video input"));
    for subtitle in [
        directory.path().join("Outlander.s07e03.subs.srt"),
        directory.path().join("Outlander.s07e03.socoolyoExtra.srt"),
        directory.path().join("other/Outlander.s07e03.socoolyo.srt"),
    ] {
        let message = subtitle_input_message(&subtitle, &videos);
        assert!(message.contains("unmatched subtitle"));
        assert!(message.contains("left unchanged"));
    }
    assert!(subtitle_input_message(&matched, &[]).contains("unmatched subtitle"));
}

#[test]
fn subtitle_inputs_are_not_collected_as_videos() {
    let directory = tempfile::tempdir().unwrap();
    let video = directory.path().join("Show.s01e01.mkv");
    let subtitle = directory.path().join("Show.s01e01.srt");
    std::fs::write(&video, b"").unwrap();
    std::fs::write(&subtitle, b"").unwrap();
    let args = Args::parse_from([
        "mnamer-rs",
        "--subtitles",
        video.to_str().unwrap(),
        subtitle.to_str().unwrap(),
        directory.path().to_str().unwrap(),
    ]);
    assert_eq!(
        collect_files(&args, false, &["mkv".into()]).unwrap(),
        vec![video]
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
    assert!(validate_template("{title} ({year}){ext}", "movie", &["title", "year", "ext"]).is_ok());
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
