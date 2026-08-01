# mnamer-rs

A terminal media file renamer in Rust — the same idea as [`mnamer`](https://github.com/jkwill87/mnamer)
and RenameMyTVSeries: point it at messy movie/TV filenames, it looks them up
on [TheMovieDB](https://www.themoviedb.org/) (TMDb), and renames them into a
clean, consistent layout.

```
The.Matrix.1999.1080p.BluRay.x264-GROUP.mkv   ->  The Matrix (1999).mkv
Breaking.Bad.S05E14.Ozymandias.720p.WEB-DL.mp4 -> Breaking Bad - S05E14 - Ozymandias.mp4
the.office.3x05.business.school.avi           -> The Office - S03E05 - Business School.avi
```

## Building

```
cargo build --release
```

The binary is at `target/release/mnamer-rs`.

> Note: this repo pins several transitive dependencies (`indexmap`, `url`,
> `tempfile`, `toml_edit`, `getrandom`, `zeroize`) to slightly older versions.
> That's only needed because this was built/tested against Rust 1.75; if
> you're on a current stable toolchain you can safely remove those pins from
> `Cargo.toml` and `cargo update`.

## API key

You need a free TMDb API key: https://www.themoviedb.org/settings/api
(the "API Read Access Token" page — grab the v3 "API Key", not the v4 token).

Provide it any of three ways:

```
mnamer-rs --api-key YOUR_KEY ...
export TMDB_API_KEY=YOUR_KEY
```

or in the config file (see below).

## Config file

Default location is `~/.config/mnamer-rs/config.toml` on every platform
(Linux, macOS, etc.) — pass `--config /path/to/file.toml` to use a different
one.

Precedence for anything settable in the config file is: **CLI flag > config.toml > built-in default**.

All supported keys (all optional):

```toml
api_key = "your_tmdb_api_key_here"

# Filename templates -- same placeholders as --format-movie / --format-episode
format_movie = "{title} ({year}){ext}"
format_episode = "{series} - S{season}E{episode} - {episode_title}{ext}"

# Only touch files with these extensions (no dots)
extensions = ["mkv", "mp4", "avi", "mov", "wmv", "m4v", "flv", "webm", "ts"]

# Move renamed files here instead of renaming in place
output_dir = "/home/you/Media"

# Booleans -- same as the matching CLI flags. Note these can only be turned
# ON by the config file; the CLI flag has no "off" form, so if you set e.g.
# `batch = true` here you can't un-batch for a single run from the CLI.
lower = false
scene = false
recursive = false
batch = false
```

Anything not listed above (e.g. `--media`, `--dry-run`, `--force-copy`,
`--parse-only`) is CLI-only and has no config.toml equivalent.

## Usage

```
# Interactively rename everything in a folder, recursing into subdirectories
mnamer-rs --recursive ~/Downloads/media

# See what it would do without touching anything
mnamer-rs --dry-run --recursive ~/Downloads/media

# Non-interactive: auto-accept the top TMDb match for every file
mnamer-rs --batch --recursive ~/Downloads/media

# Just show the parsed guess (title/year/season/episode) — no network calls
mnamer-rs --parse-only --recursive ~/Downloads/media

# Move renamed files into a separate library folder instead of renaming in place
mnamer-rs --batch --output-dir ~/Media/Movies ~/Downloads/*.mkv

# Scene-style output (dots instead of spaces), all lowercase
mnamer-rs --scene --lower --batch ~/Downloads/media

# Force everything to be treated as episodes/movies (skips auto-detection)
mnamer-rs --media episode ~/Downloads/some_show/

# Custom naming templates
mnamer-rs --format-movie "{title} [{year}]{ext}" \
          --format-episode "{series}/Season {season}/{series} S{season}E{episode} {episode_title}{ext}" \
          --batch --recursive ~/Downloads/media
```

Run `mnamer-rs --help` for the full flag list.

## How it works

1. **Parse** — `src/parser.rs` strips separators (dots/underscores), detects
   season/episode markers (`S01E02`, `1x02`, `Season 1 Episode 2`), pulls out
   a year, strips known junk tags (`1080p`, `x264`, `WEB-DL`, release-group
   names, ...), and title-cases the remainder.
2. **Lookup** — `src/tmdb.rs` searches TMDb's `/search/movie` or `/search/tv`
   endpoint, and for episodes also fetches the actual episode title from
   `/tv/{id}/season/{n}/episode/{n}`.
3. **Confirm** — unless `--batch` is passed, you get an interactive picker
   (via `dialoguer`) to choose among the returned matches, or skip the file.
4. **Rename** — `src/rename.rs` renders your template, sanitizes illegal
   filename characters, and `src/main.rs` performs the move (falling back to
   copy+delete if `--output-dir` is on a different filesystem).

## Tests

```
cargo test
```

Covers filename parsing (movie/episode patterns, year extraction, junk
stripping) and template rendering/sanitization.

## What's not implemented

Compared to `mnamer`/RenameMyTVSeries this is intentionally lean:
- Only TMDb is supported (no TVDb/OMDb fallback providers).
- No subtitle-file handling (renaming `.srt` alongside video files).
- No fuzzy "did you mean" correction beyond what TMDb's own search returns.
These would be reasonable next additions if you want them.
