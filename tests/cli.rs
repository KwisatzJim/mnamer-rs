use std::process::Command;

#[test]
fn parse_only_reports_bad_episode_and_continues_without_api_key() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    std::fs::write(&config, "").unwrap();
    let bad = dir.path().join("Outlander.S01E0304.mkv");
    let good = dir.path().join("Outlander.S01E03E04.mkv");
    std::fs::write(&bad, "bad").unwrap();
    std::fs::write(&good, "good").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_mnamer-rs"))
        .args(["--parse-only", "--config"])
        .arg(config)
        .arg(&bad)
        .arg(&good)
        .env_remove("TMDB_API_KEY")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("ambiguous episode numbering"));
    assert!(String::from_utf8_lossy(&output.stdout).contains("S01E03-E04"));
    assert_eq!(std::fs::read(bad).unwrap(), b"bad");
    assert_eq!(std::fs::read(good).unwrap(), b"good");
}

#[test]
fn invalid_config_exits_cleanly_before_renaming() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    std::fs::write(&config, "batch = not-a-boolean").unwrap();
    let video = dir.path().join("Alpha.2026.mkv");
    std::fs::write(&video, "video").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_mnamer-rs"))
        .arg("--config")
        .arg(config)
        .arg(&video)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("invalid config"));
    assert_eq!(std::fs::read(video).unwrap(), b"video");
}
