# Changelog

## 0.2.0 - Unreleased

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
