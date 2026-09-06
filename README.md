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

<img width="1091" height="700" alt="Screenshot 2026-09-06 at 4 07 01 PM" src="https://github.com/user-attachments/assets/f4f5aa2e-4924-4763-8a34-e9520fca6358" />

<img width="1091" height="700" alt="Screenshot 2026-09-06 at 4 07 09 PM" src="https://github.com/user-attachments/assets/73651a94-ab39-490c-96a6-c8372071ab16" />

<img width="1091" height="700" alt="Screenshot 2026-09-06 at 4 07 16 PM" src="https://github.com/user-attachments/assets/9cff9f78-2b90-40e0-82d1-6e01a758d6d5" />

<img width="1091" height="700" alt="Screenshot 2026-09-06 at 4 07 23 PM" src="https://github.com/user-attachments/assets/71b78f3d-a181-4fa1-987c-6053bcc48269" />

<img width="1091" height="700" alt="Screenshot 2026-09-06 at 4 07 33 PM" src="https://github.com/user-attachments/assets/5e2ca8a0-ddd6-45cd-9f0c-efed6df73f26" />

<img width="1091" height="700" alt="Screenshot 2026-09-06 at 4 07 39 PM" src="https://github.com/user-attachments/assets/113695f3-63df-4269-95a4-e2d19aa83211" />

<img width="1091" height="700" alt="Screenshot 2026-09-06 at 4 07 48 PM" src="https://github.com/user-attachments/assets/6dcc5e2d-d8f6-4ad1-b143-18d41c9e4e04" />

## Building

```
cargo build --release
```

The binary is at `target/release/mnamer-rs`.

The release workflow is prepared to build archives for Linux x86-64 and macOS
ARM64. See
[Linux release and verification](docs/LINUX_RELEASE.md) for checksum, install,
and hands-on test instructions. Version 0.2.0 remains unreleased until its
source and CI results have been reviewed and the release tag is explicitly
approved.

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

# Optional absolute path to ffprobe; otherwise it is found on PATH
ffprobe_path = "/opt/homebrew/bin/ffprobe"

# Filename templates -- same placeholders as --format-movie / --format-episode
format_movie = "{title} ({year}){ext}"
format_episode = "{series} - S{season}E{episode_range} - {episode_title}{ext}"

# Only touch files with these extensions (no dots)
extensions = ["mkv", "mp4", "avi", "mov", "wmv", "m4v", "flv", "webm", "ts"]

# Move renamed files here instead of renaming in place
output_dir = "/home/you/Media"

# Booleans -- same as the matching CLI flags. A true config value can be
# disabled for one run with --no-lower, --no-scene, --no-recursive, or
# --no-batch.
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

# Use ffprobe directly, without changing your shell PATH
mnamer-rs --ffprobe /opt/homebrew/bin/ffprobe --dry-run --batch ~/Downloads/media

# Save a JSON Lines audit report with one outcome per processed file
mnamer-rs --log rename-report.jsonl --recursive ~/Downloads/media

# Rename matching .srt/.ass/.ssa/.sub/.vtt subtitle files with each video
mnamer-rs --subtitles --recursive ~/Downloads/media

# Non-interactive: accept the top TMDb match only when the title/year is confident
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
          --format-episode "{series}/Season {season}/{series} S{season}E{episode_range} {episode_title}{ext}" \
          --batch --recursive ~/Downloads/media
```

Run `mnamer-rs --help` for the full flag list.

For multi-episode files, `{episode_range}` renders a range such as `03-E04`.
Older templates using only `{episode}` now include the range automatically.
If a template explicitly uses `{episode_end}` or `{episode_range}`, `{episode}`
continues to mean the first episode number.

The `--log` path must not already exist. Choose a new report filename for each
run; existing reports and other files are never overwritten. Each completed
record is flushed immediately, though this is not a guarantee against power loss.

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
   filename characters, and `src/operations.rs` performs the move (falling back to
   copy+delete if `--output-dir` is on a different filesystem).

`src/main.rs` is only the command-line entry point. `src/workflow.rs` coordinates
the stages, `src/scanning.rs` collects inputs and matches subtitles,
`src/quality.rs` compares duplicate candidates, `src/model.rs` defines rename
plans, and `src/report.rs` writes the JSON Lines log. Filesystem changes are
isolated in `src/operations.rs` so their safety tests can run independently.

When inputs would produce the same destination name and directory, ignoring the
final extension (for example, `.mkv` versus `.mp4`), `mnamer-rs` treats them as
duplicates. The selected file keeps its original extension; skipped files are
left untouched. To choose between duplicates, `mnamer-rs` uses
`ffprobe` (when installed) to prefer the video with the greatest actual height.
If probing is unavailable, it falls back to filename tags such as `2160p`,
`1080p`, and `720p`. When actual heights tie, a unique higher reported video
bitrate wins only if all tied candidates report the same known video codec.
Different or missing codecs, or missing/tied bitrates, leave the choice ambiguous
and the files are skipped. Bitrate is a preference heuristic, not proof of visual quality.
Each video probe has a 10-second timeout. A timed-out probe is stopped and
filename resolution tags are used instead, with a warning identifying the file.
If `ffprobe` cannot start, a warning explains the problem and the filename fallback.
Each duplicate candidate displays its video height, codec, bitrate (if reported),
and the information source (`ffprobe` or `filename tag`). Missing information is
shown as unknown. The comparison explains when no unique preference is possible.
As before, candidates with unknown resolution do not outrank candidates with
known resolution; the output explicitly flags comparisons using only known data.

Set `--ffprobe /absolute/path/to/ffprobe`, or set `ffprobe_path` in the config file,
to avoid relying on `PATH`. Precedence is CLI > config > `ffprobe` on `PATH`.
The value is one executable path, not a shell command or alias. Quote paths with
spaces, use an absolute path for portability, and do not use `~` in TOML paths
(TOML does not expand it). Missing/unusable executables warn and fall back to
filename tags; an empty configured path is rejected. No automatic install or
changes to your shell configuration are performed.

## Subtitle reporting

With `--subtitles`, subtitle inputs from wildcards or directory scans are reported
separately from videos. Matching subtitles are considered with their video's
rename; unmatched subtitles are left untouched with an explanatory warning.
Matching requires the same directory and the video's complete original filename
stem, optionally followed by suffixes such as `.en.forced` before `.srt`.
These discovery messages do not mean a rename succeeded; the later apply results
show what actually happened. Discovery messages do not change summary counts.

For planned video/subtitle groups, `--log` records every member when a group is
skipped or fails, as well as on success or dry runs. Subtitle records include the
group's reason. A `failed` group status does not claim that each move was attempted
or that every file was restored; the reason includes any reported rollback errors.
Skipped and failed summary counts include the group's planned subtitles, just as
successful rename counts already do. This does not add log entries for unmatched
subtitles or subtitles whose video never reached the rename-planning stage.

## Ambiguous episode numbers

Ambiguous packed episode numbers such as `S01E0304` are reported as per-file
failures before metadata lookup, rather than treated as movies. Other files
continue processing. If you mean episodes 3 through 4, use `S01E03-E04` or
`S01E03E04`. Three-digit episode numbers such as `S01E123` remain supported.
The same validation is available without TMDb access using `--parse-only`.
Ranges must have two increasing endpoints in the same season and cover at most
10 episodes. Reversed/equal endpoints, three-or-more episode markers, multiple
season markers, and unsupported shorthand such as `S01E03-04` are rejected rather
than truncated to a single episode. `1x03` and `Season 1 Episode 3` remain supported
for single episodes; use `S01E03-E04` for ranges.

## Temporary TMDb failures

TMDb requests retry temporary failures up to three times and honor `Retry-After`
in seconds or HTTP-date form. Waits over 30 seconds, or exhausted rate-limit
retries, defer subsequent lookups for the rest of the run; rerun later. Already
resolved rename plans can still be applied. Deferred files are reported as failed.

## Tests

```
cargo test
```

Covers parsing, templates, probing, quality comparisons, configuration, logs,
subtitle grouping, no-overwrite moves, and rollback. Workflow tests run the actual
scan/plan/apply/log pipeline against temporary files with deterministic metadata
responses: no real API key or TMDb requests are needed. They check cross-container
duplicates, configured probes, conflicts, failed moves, continued processing,
dry runs, and existing-log protection. CLI process tests check exit codes and
continued parsing after invalid filenames. Unix-only tests cover executable probes.

Additional checks:

```sh
cargo fmt --check
cargo clippy --offline --all-targets -- -D warnings
cargo test --offline
cargo build --release --offline
```

## What's not implemented

Compared to `mnamer`/RenameMyTVSeries this is intentionally lean:
- Only TMDb is supported (no TVDb/OMDb fallback providers).
- No fuzzy "did you mean" correction beyond what TMDb's own search returns.
These would be reasonable next additions if you want them.
