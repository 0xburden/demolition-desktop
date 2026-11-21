# Demolition

A local-first recording DAW. Portability and ease of use are acceptance criteria: keep Windows, macOS, and Linux in the first slice; cut feature scope before dropping a platform. Consult [NORTH_STAR.md](NORTH_STAR.md) for product decisions and [KICKOFF.md](KICKOFF.md) for milestones and storage/interchange decisions. Open `references/` only for UI work.

- `apps/desktop`: GPUI through GPUI Kit's re-export. Don't add a second GPUI dependency.
- `crates/audio-io`: CPAL discovery and streams. Keep driver work off the UI thread and callbacks free of locks, allocation, and file I/O.
- `crates/project`: headless SQLite metadata and WAV media. Keep portable media paths project-relative. Add an engine or REAPER crate when a slice needs it; never rewrite an imported `.rpp`.

Use the pinned Rust toolchain and commit `Cargo.lock`. On Linux, use the build dependencies from [CI](.github/workflows/ci.yml).

```sh
cargo run --locked -p demolition
cargo run --locked -p demolition-audio-io --example devices # hardware probe
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

CI builds desktop smoke-test archives; it cannot verify real device or recording behavior. Report the OS, backend, and hardware actually exercised. Don't claim clean-machine install support from an archive build alone.
