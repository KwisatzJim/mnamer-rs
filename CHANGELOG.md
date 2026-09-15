# Changelog

## 0.3.0 - 2026-09-15

- Add TVmaze as an explicitly selectable television metadata provider.
- Allow TVmaze-only episode runs without a TMDb API key.
- Add provider-specific `--series-id` overrides, saved series mappings, and
  lookup-only `--search-series` results with visible provider IDs.
- Accept temporary episode titles such as `Episode 4`, `TBA`, and `TBD` so
  files can be renamed now and refreshed after provider metadata improves.
- Update the minimum supported Rust version to 1.98.1 and refresh compatible
  dependencies.

## 0.2.0 - 2026-09-06

- Continue batches after per-file lookup and filesystem failures, with nonzero
  exit status when failures occurred.
- Prevent destination overwrites and roll back subtitle groups when possible.
- Detect duplicate destinations across container extensions and conservatively
  prefer a unique higher-quality video using `ffprobe` metadata.
- Add configurable `ffprobe` paths and transparent quality-comparison output.
- Add opt-in subtitle discovery, renaming, reporting, and JSON Lines audit logs.
- Add multi-episode naming and reject ambiguous or unsafe episode sequences.
- Add cross-platform unit, workflow, and command-line tests.
- Add Ubuntu and macOS CI plus Linux x86-64 and macOS ARM64 release archives.
