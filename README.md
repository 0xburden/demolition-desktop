# Demolition

Demolition is a local-first recording DAW for independent bands. The goal is to get from plugging in an interface to saving a take without an account, and to keep the whole session portable across macOS, Windows, and Linux.

**Status: early recording prototype.** There’s a recording workspace, but no arrangement editor, project reopen, or REAPER import yet. The [UI references](references/) show the design direction, not features in the current build.

## Record a first take

1. Choose **New Project**, enter a name, and pick a parent folder. **Create Project** opens the workspace and creates a named folder containing `Session.demo` and `media/`.
2. Select a four-channel input, enable meters, and check inputs 1–4. On Linux, choose a direct ALSA `hw:` input. Use your interface’s direct monitoring if you need to hear the live inputs.
3. For a click or guide, choose an output, preferably on the same interface. **Timing Settings** controls tempo, time signature, click level, downbeat accent, and changes at the start of a bar. **Play Click** previews it. An optional guide can be a mono or stereo 44.1 or 48 kHz WAV.
4. Choose **Record**, then **Stop & Save**. Each take has a four-channel WAV under `media/take-N/`. Keep the whole project folder together; the `.demo` SQLite file stores metadata, not audio.

**Settings → Appearance** follows the system by default and can be set to Light or Dark for all projects.

## Build and test

Install [Rust via rustup](https://rustup.rs/); the repository pins the toolchain. On Linux, install the native packages listed in [CI](.github/workflows/ci.yml).

```sh
cargo run --locked -p demolition
cargo test --workspace --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
```

The CI workflow targets macOS arm64, Windows x64, and Ubuntu x64 and produces smoke-test archives. Recording and metronome playback have been exercised through a virtual CoreAudio loopback on Apple Silicon; physical USB interfaces and Windows/Linux recording still need verification.

See [NORTH_STAR.md](NORTH_STAR.md) for product direction and [KICKOFF.md](KICKOFF.md) for milestones, project format, and hardware test instructions.
