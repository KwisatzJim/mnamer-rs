# Feature-completeness scope

The accepted version 0.2 scope is a focused TMDb terminal renamer, not a byte-for-byte clone of
mnamer or RenameMyTVSeries. “Feature complete” for version 0.2 means the full
documented workflow is safe, testable, installable, and supported on macOS and
Linux.

## Complete today

- Parse common movie, single-episode, and bounded multi-episode filenames.
- Search TMDb interactively or conservatively select a confident batch match.
- Render configurable movie and episode names, including nested templates.
- Scan files or directories recursively and filter media extensions.
- Move within/across filesystems without overwriting existing destinations.
- Pair common subtitle sidecars and roll back a partially failed group.
- Detect logical duplicates across containers and conservatively select quality.
- Preview with dry-run, parse without networking, and write JSON Lines audit logs.
- Continue after isolated failures and return an automation-friendly exit status.
- Cross-platform CI and release definitions for macOS ARM64 and Linux x86-64.

## Required before declaring 0.2 release-ready

- Exercise CI on GitHub, including the Ubuntu build and minimum Rust job.
- Download, checksum, and run the Linux archive on an Ubuntu 22.04 x86-64 host.
- Perform one hands-on dry run and one controlled real rename on Linux.
- Review the final diff and release notes, then explicitly approve the tag.

## Deliberately out of scope for 0.2

- Alternate metadata providers (TVDb, TVMaze, or OMDb).
- A desktop GUI, daemon, directory watcher, Docker image, or media transcoding.
- Automatic deletion of lower-quality duplicates; skipped files remain intact.
- Metadata caching, localization, provider-ID overrides, or arbitrary regex rewrite
  rules. These can be added later without weakening the safe core workflow.
- Windows release artifacts. The Rust code avoids obvious Unix-only assumptions,
  but Windows has not been established as a supported/tested release platform.

The upstream mnamer project includes multiple providers, localization, caching,
ID overrides, ignore/replace rules, and Docker automation. Those are useful
comparison points, not silent requirements for this intentionally narrower tool.
