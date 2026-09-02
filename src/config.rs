use anyhow::{Context, Result};
use serde::Deserialize;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Deserialize)]
pub struct FileConfig {
    pub api_key: Option<String>,
    pub format_movie: Option<String>,
    pub format_episode: Option<String>,
    pub extensions: Option<Vec<String>>,
    pub output_dir: Option<PathBuf>,
    pub lower: Option<bool>,
    pub scene: Option<bool>,
    pub recursive: Option<bool>,
    pub batch: Option<bool>,
}

/// Always ~/.config/mnamer-rs/config.toml, on every platform (including
/// macOS). We deliberately don't use directories::ProjectDirs here -- its
/// macOS convention (~/Library/Application Support/...) doesn't match what
/// this tool documents, and a single consistent path is more convenient
/// when syncing config across machines.
pub fn default_config_path() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|dirs| dirs.home_dir().join(".config/mnamer-rs/config.toml"))
}

pub fn load(path: Option<&Path>) -> Result<FileConfig> {
    let explicitly_requested = path.is_some();
    let candidate = match path {
        Some(p) => Some(p.to_path_buf()),
        None => default_config_path(),
    };
    let Some(candidate) = candidate else {
        return Ok(FileConfig::default());
    };
    match std::fs::read_to_string(&candidate) {
        Ok(text) => toml::from_str(&text)
            .with_context(|| format!("invalid config file {}", candidate.display())),
        Err(error) if error.kind() == ErrorKind::NotFound && !explicitly_requested => {
            Ok(FileConfig::default())
        }
        Err(error) => Err(error)
            .with_context(|| format!("failed to read config file {}", candidate.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_malformed_explicit_config() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(&path, "batch = definitely-not-a-boolean").unwrap();

        let error = load(Some(&path)).unwrap_err();

        assert!(error.to_string().contains("invalid config file"));
        assert!(error.to_string().contains("config.toml"));
    }

    #[test]
    fn reports_missing_explicit_config() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("missing.toml");

        let error = load(Some(&path)).unwrap_err();

        assert!(error.to_string().contains("failed to read config file"));
        assert!(error.to_string().contains("missing.toml"));
    }
}
