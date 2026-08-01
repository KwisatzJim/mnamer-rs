use clap::{Parser, ValueEnum};
use std::path::PathBuf;

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum MediaType {
    Movie,
    Episode,
    Auto,
}

/// Built-in fallback defaults, used only if neither the CLI flag nor the
/// config file set a value.
pub const DEFAULT_FORMAT_MOVIE: &str = "{title} ({year}){ext}";
pub const DEFAULT_FORMAT_EPISODE: &str = "{series} - S{season}E{episode} - {episode_title}{ext}";
pub const DEFAULT_EXTENSIONS: &[&str] = &["mkv", "mp4", "avi", "mov", "wmv", "m4v", "flv", "webm", "ts"];

/// mnamer-rs: a terminal media file renamer (mnamer / RenameMyTVSeries alike).
///
/// Give it one or more files or directories. It parses each filename to
/// guess title / year / season / episode, looks the result up on TheMovieDB,
/// and renames (or moves) the file into a clean, consistent layout.
///
/// Precedence for template/extension/output-dir/lower/scene/recursive/batch
/// options is: CLI flag > config.toml > built-in default.
#[derive(Parser, Debug)]
#[command(name = "mnamer-rs", version, about, long_about = None)]
pub struct Args {
    /// Files or directories to process
    #[arg(required = true)]
    pub targets: Vec<PathBuf>,

    /// Recurse into subdirectories
    #[arg(short, long)]
    pub recursive: bool,

    /// Force media type instead of auto-detecting from the filename
    #[arg(short = 'm', long, value_enum, default_value = "auto")]
    pub media: MediaType,

    /// Show what would happen without touching any files
    #[arg(short = 'n', long)]
    pub dry_run: bool,

    /// Non-interactive: always accept the best match automatically
    #[arg(short = 'b', long)]
    pub batch: bool,

    /// Copy/move the file to this directory instead of renaming in place
    #[arg(short = 'o', long)]
    pub output_dir: Option<PathBuf>,

    /// Move the file across filesystems (copy + delete) instead of a rename.
    /// Used automatically when needed; this flag forces it.
    #[arg(long)]
    pub force_copy: bool,

    /// TMDb v3 API key. Falls back to $TMDB_API_KEY, then the config file.
    #[arg(long)]
    pub api_key: Option<String>,

    /// Movie filename template. Placeholders: {title} {year} {ext}
    /// [default: "{title} ({year}){ext}", overridable in config.toml]
    #[arg(long)]
    pub format_movie: Option<String>,

    /// Episode filename template.
    /// Placeholders: {series} {year} {season} {episode} {episode_title} {ext}
    /// [default: "{series} - S{season}E{episode} - {episode_title}{ext}", overridable in config.toml]
    #[arg(long)]
    pub format_episode: Option<String>,

    /// Lowercase the final filename
    #[arg(long)]
    pub lower: bool,

    /// Scene-style output: spaces become dots
    #[arg(long)]
    pub scene: bool,

    /// Skip the interactive confirmation and metadata lookup entirely;
    /// just report the parsed guess for each file
    #[arg(long)]
    pub parse_only: bool,

    /// Path to a TOML config file (default: ~/.config/mnamer-rs/config.toml)
    #[arg(long)]
    pub config: Option<PathBuf>,

    /// Only touch files with these extensions (comma separated, no dots).
    /// [default: mkv,mp4,avi,mov,wmv,m4v,flv,webm,ts, overridable in config.toml]
    #[arg(long, value_delimiter = ',')]
    pub extensions: Option<Vec<String>>,
}

