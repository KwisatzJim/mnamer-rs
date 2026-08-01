use serde::Deserialize;
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

pub fn load(path: Option<&Path>) -> FileConfig {
    let candidate = match path {
        Some(p) => Some(p.to_path_buf()),
        None => default_config_path(),
    };
    let Some(candidate) = candidate else {
        return FileConfig::default();
    };
    match std::fs::read_to_string(&candidate) {
        Ok(text) => toml::from_str(&text).unwrap_or_default(),
        Err(_) => FileConfig::default(),
    }
}

