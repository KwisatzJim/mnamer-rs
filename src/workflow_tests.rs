//! Full scan -> parse -> metadata -> plan -> apply -> log tests. Only metadata
//! is substituted; files and move operations are real, isolated temporary files.
use crate::cli::Args;
use crate::tmdb::{MetadataProvider, MovieMatch, SeriesMatch};
use crate::workflow::run_with_client;
use anyhow::Result;
use clap::Parser as _;
use std::path::PathBuf;

struct Metadata {
    remove_before_beta: Option<PathBuf>,
    fail_alpha: bool,
}

impl MetadataProvider for Metadata {
    fn search_movie(&self, title: &str, year: Option<u32>) -> Result<Vec<MovieMatch>> {
        if title == "Beta" {
            if let Some(path) = &self.remove_before_beta {
                std::fs::remove_file(path)?;
            }
        }
        if self.fail_alpha && title == "Alpha" {
            anyhow::bail!("simulated metadata failure");
        }
        Ok(vec![MovieMatch {
            title: title.into(),
            year,
        }])
    }
    fn search_series(&self, name: &str) -> Result<Vec<SeriesMatch>> {
        Ok(vec![SeriesMatch {
            id: 1,
            name: name.into(),
            first_air_year: Some(2026),
        }])
    }
    fn episode_title(&self, _: u64, _: u32, episode: u32) -> Result<Option<String>> {
        Ok(Some(format!("Episode {episode}")))
    }
}

struct Fixture {
    directory: tempfile::TempDir,
}

#[test]
fn invalid_episode_range_is_logged_before_lookup_and_valid_episode_continues() {
    let f = Fixture::new();
    f.write("Show.S01E04-E03.mkv", b"bad range");
    f.write("Show.S01E05.mkv", b"good episode");
    let error = f
        .run(&[], &["Show.S01E04-E03.mkv", "Show.S01E05.mkv"])
        .unwrap_err();
    assert!(error.to_string().contains("1 file failed"));
    assert_eq!(
        std::fs::read(f.path("Show.S01E04-E03.mkv")).unwrap(),
        b"bad range"
    );
    assert_eq!(
        std::fs::read(f.path("Show - S01E05 - Episode 5.mkv")).unwrap(),
        b"good episode"
    );
    assert_eq!(f.records()[0]["status"], "failed");
    assert_eq!(f.records()[1]["status"], "renamed");
}
impl Fixture {
    fn new() -> Self {
        let result = Self {
            directory: tempfile::tempdir().unwrap(),
        };
        result.write("config.toml", b"");
        result
    }
    fn path(&self, name: &str) -> PathBuf {
        self.directory.path().join(name)
    }
    fn write(&self, name: &str, contents: &[u8]) {
        std::fs::write(self.path(name), contents).unwrap();
    }
    fn args(&self, flags: &[&str], names: &[&str]) -> Args {
        let mut args = vec![
            "mnamer-rs".into(),
            "--config".into(),
            self.path("config.toml").into_os_string(),
            "--api-key".into(),
            "test-only-key".into(),
            "--batch".into(),
            "--subtitles".into(),
            "--log".into(),
            self.path("report.jsonl").into_os_string(),
        ];
        args.extend(flags.iter().map(std::ffi::OsString::from));
        args.extend(names.iter().map(|name| self.path(name).into_os_string()));
        Args::parse_from(args)
    }
    fn run(&self, flags: &[&str], names: &[&str]) -> Result<()> {
        run_with_client(self.args(flags, names), |_| {
            Ok(Metadata {
                remove_before_beta: None,
                fail_alpha: false,
            })
        })
    }
    fn records(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(self.path("report.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

#[test]
fn cross_container_duplicates_rename_only_the_higher_resolution_with_subtitles() {
    let f = Fixture::new();
    for (name, bytes) in [
        ("Alpha.2026.1080p.mkv", b"high".as_slice()),
        ("Alpha.2026.720p.mp4", b"low"),
        ("Alpha.2026.1080p.en.srt", b"high subs"),
        ("Alpha.2026.720p.en.srt", b"low subs"),
    ] {
        f.write(name, bytes);
    }
    f.run(&[], &["Alpha.2026.1080p.mkv", "Alpha.2026.720p.mp4"])
        .unwrap();
    assert_eq!(std::fs::read(f.path("Alpha (2026).mkv")).unwrap(), b"high");
    assert_eq!(
        std::fs::read(f.path("Alpha (2026).en.srt")).unwrap(),
        b"high subs"
    );
    assert_eq!(
        std::fs::read(f.path("Alpha.2026.720p.mp4")).unwrap(),
        b"low"
    );
    assert_eq!(
        std::fs::read(f.path("Alpha.2026.720p.en.srt")).unwrap(),
        b"low subs"
    );
    assert!(!f.path("Alpha (2026).mp4").exists());
    let records = f.records();
    assert_eq!(records.len(), 4);
    assert_eq!(
        records.iter().filter(|r| r["status"] == "renamed").count(),
        2
    );
    assert_eq!(
        records.iter().filter(|r| r["status"] == "skipped").count(),
        2
    );
}

#[test]
fn subtitle_conflict_preserves_group_and_continues_with_next_video() {
    let f = Fixture::new();
    for name in ["Alpha.2026.mkv", "Alpha.2026.srt", "Beta.2026.mkv"] {
        f.write(name, name.as_bytes());
    }
    f.write("Alpha (2026).srt", b"existing subtitle");
    f.run(&[], &["Alpha.2026.mkv", "Beta.2026.mkv"]).unwrap();
    assert!(f.path("Alpha.2026.mkv").exists());
    assert!(f.path("Alpha.2026.srt").exists());
    assert!(!f.path("Alpha (2026).mkv").exists());
    assert_eq!(
        std::fs::read(f.path("Alpha (2026).srt")).unwrap(),
        b"existing subtitle"
    );
    assert!(f.path("Beta (2026).mkv").exists());
    let records = f.records();
    assert_eq!(records.len(), 3);
    assert_eq!(records[0]["status"], "skipped");
    assert_eq!(records[1]["status"], "skipped");
    assert_eq!(records[2]["status"], "renamed");
}

#[test]
fn failed_moves_and_rollback_are_logged_and_do_not_stop_other_groups() {
    for remove in ["Alpha.2026.srt", "Alpha.2026.mkv"] {
        let f = Fixture::new();
        for name in ["Alpha.2026.mkv", "Alpha.2026.srt", "Beta.2026.mkv"] {
            f.write(name, name.as_bytes());
        }
        let error = run_with_client(f.args(&[], &["Alpha.2026.mkv", "Beta.2026.mkv"]), |_| {
            Ok(Metadata {
                remove_before_beta: Some(f.path(remove)),
                fail_alpha: false,
            })
        })
        .unwrap_err();
        assert!(error.to_string().contains("2 files failed"));
        assert!(!f.path("Alpha (2026).mkv").exists());
        if remove.ends_with("srt") {
            assert_eq!(
                std::fs::read(f.path("Alpha.2026.mkv")).unwrap(),
                b"Alpha.2026.mkv"
            );
        } else {
            assert!(f.path("Alpha.2026.srt").exists());
        }
        assert!(f.path("Beta (2026).mkv").exists());
        let records = f.records();
        assert_eq!(records.len(), 3);
        assert_eq!(records[0]["status"], "failed");
        assert_eq!(records[1]["status"], "failed");
        assert_eq!(records[2]["status"], "renamed");
    }
}

#[test]
fn metadata_failure_does_not_block_later_files() {
    let f = Fixture::new();
    f.write("Alpha.2026.mkv", b"alpha");
    f.write("Beta.2026.mkv", b"beta");
    assert!(
        run_with_client(f.args(&[], &["Alpha.2026.mkv", "Beta.2026.mkv"]), |_| {
            Ok(Metadata {
                remove_before_beta: None,
                fail_alpha: true,
            })
        })
        .is_err()
    );
    assert_eq!(std::fs::read(f.path("Alpha.2026.mkv")).unwrap(), b"alpha");
    assert!(f.path("Beta (2026).mkv").exists());
    assert_eq!(f.records()[0]["status"], "failed");
    assert_eq!(f.records()[1]["status"], "renamed");
}

#[test]
fn dry_run_and_existing_log_never_modify_media() {
    let f = Fixture::new();
    f.write("Alpha.2026.mkv", b"alpha");
    f.write("Alpha.2026.srt", b"subtitle");
    f.run(&["--dry-run"], &["Alpha.2026.mkv"]).unwrap();
    assert_eq!(f.records().len(), 2);
    assert!(f.records().iter().all(|r| r["status"] == "dry_run"));
    let log_before = std::fs::read(f.path("report.jsonl")).unwrap();
    assert!(f.run(&[], &["Alpha.2026.mkv"]).is_err());
    assert_eq!(std::fs::read(f.path("report.jsonl")).unwrap(), log_before);
    assert_eq!(std::fs::read(f.path("Alpha.2026.mkv")).unwrap(), b"alpha");
    assert_eq!(
        std::fs::read(f.path("Alpha.2026.srt")).unwrap(),
        b"subtitle"
    );
    assert!(!f.path("Alpha (2026).mkv").exists());
}

#[cfg(unix)]
#[test]
fn configured_probe_and_cli_override_select_actual_height_without_filename_tags() {
    use std::os::unix::fs::PermissionsExt;
    for override_cli in [false, true] {
        let f = Fixture::new();
        let probe = f.path("fake probe");
        std::fs::write(&probe, "#!/bin/sh\nfor last do :; done\ncase \"$last\" in\n*.mkv) height=1080 ;;\n*) height=720 ;;\nesac\nprintf '{\"streams\":[{\"height\":%s,\"codec_name\":\"h264\"}]}' \"$height\"\n").unwrap();
        std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o700)).unwrap();
        let configured = if override_cli {
            f.path("missing-probe")
        } else {
            probe.clone()
        };
        f.write(
            "config.toml",
            format!(
                "ffprobe_path = {}\n",
                serde_json::to_string(&configured).unwrap()
            )
            .as_bytes(),
        );
        f.write("Alpha.2026.mkv", b"high");
        f.write("Alpha.2026.mp4", b"low");
        let flags = if override_cli {
            vec!["--ffprobe", probe.to_str().unwrap()]
        } else {
            vec![]
        };
        f.run(&flags, &["Alpha.2026.mkv", "Alpha.2026.mp4"])
            .unwrap();
        assert_eq!(std::fs::read(f.path("Alpha (2026).mkv")).unwrap(), b"high");
        assert_eq!(std::fs::read(f.path("Alpha.2026.mp4")).unwrap(), b"low");
        assert_eq!(f.records()[0]["status"], "renamed");
        assert_eq!(f.records()[1]["status"], "skipped");
    }
}
