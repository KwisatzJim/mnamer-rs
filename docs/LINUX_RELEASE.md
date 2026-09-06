# Linux release and verification

The supported Linux release target is x86-64, built on Ubuntu 22.04. Building
on this older supported runner reduces the chance of requiring a newer glibc
than users have. `ffprobe` is optional and is not bundled; install FFmpeg from
your distribution when duplicate quality comparison is needed.

## Automated build

Every push and pull request runs formatting, Clippy, tests, and a release build
on Ubuntu 22.04 and macOS. A version tag such as `v0.2.0` additionally produces:

- `mnamer-rs-linux-x86_64.tar.gz`
- `mnamer-rs-macos-aarch64.tar.gz`
- `SHA256SUMS`

The release job fails if the tag does not exactly match the Cargo package
version. The repository owner must review the prepared source and explicitly
create/push the tag; normal pushes never publish releases.

## Linux verification

On an Ubuntu 22.04 x86-64 machine, run:

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked
target/release/mnamer-rs --version
target/release/mnamer-rs --help
```

For a downloaded release, verify and install it for the current user:

```sh
sha256sum -c SHA256SUMS
tar -xzf mnamer-rs-linux-x86_64.tar.gz
install -Dm755 mnamer-rs-linux-x86_64/mnamer-rs "$HOME/.local/bin/mnamer-rs"
```

Then run a non-destructive test with representative filenames:

```sh
mnamer-rs --parse-only /path/to/media/*
mnamer-rs --dry-run --batch --subtitles --log rename-test.jsonl /path/to/media/*
```

Inspect every proposed change and the JSON Lines report before performing a
real rename. A successful CI build establishes Linux compilation and automated
behavior; it does not replace a hands-on test on the intended Linux desktop and
filesystem.
