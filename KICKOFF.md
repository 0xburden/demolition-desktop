# Demolition — repo kickoff

[North Star](NORTH_STAR.md) · Milestones and acceptance checks.

## What we prove first

Demolition is a recording DAW for the musician who ends up running the band’s session. **Portability and ease of use are release gates, not polish.** The first proof is the same on Windows, macOS, and Linux: install without a dev toolchain or account, pick a USB interface, see live inputs, record against a guide track, and open the project on another OS. We want a straightforward path to the first take, not the most settings or the lowest possible buffer size.

Start on Windows x64, macOS arm64, and Linux x64 on one named distro/version. Record the exact OS, architecture, interface, and driver combinations tested; expand the matrix deliberately. Build and package all three in CI from the first app slice. CI verifies software; a repeatable hardware session verifies recording. Plan for signing and installation prompts before public distribution.

## Stack and repo shape

- **Rust workspace:** `apps/desktop`, `crates/engine`, `crates/audio-io`, `crates/project`, and eventually `crates/reaper-interop`. Pin the toolchain and compatible GPUI/GPUI Kit revisions; commit the lockfile. Keep engine, storage, and interchange tests runnable without a GPU or desktop session.
- **GPUI + GPUI Kit:** use Kit for common controls and a custom timeline. Start with device setup and track controls. Keep arm, signal, monitoring, and clipping visible at the point of recording. Don’t build a docking system to test this workflow.
- **CPAL first, not CPAL forever:** isolate device enumeration and streaming so a platform-specific backend can replace CPAL where needed. Use a single interface for input and output in the first proof. Keep the callback free of locks, allocations, file I/O, and UI work; use bounded queues and surface overruns rather than hiding them.

## Native projects: `.demo` inside a project folder

```text
Garage Session/
  Garage Session.demo    # SQLite: tracks, edits, source references, take state
  media/                 # WAVs; project-relative paths
```

Prefer `.demo`: it reinforces the name and tells musicians more than `.lition`. It’s a proposed association, not a claim that the extension is unused. Check existing associations on all three OSes before registering it; `.demolition` is a fallback. An actual file lets us use ordinary file associations on all three OSes. **The folder is the portable project; the `.demo` entry file is what you open.** Don’t rely on macOS-style directory bundles or embed recorded audio in the database. A separate single-file archive can come later if sharing calls for it.

Use bundled SQLite via `rusqlite`, explicit schema migrations with a pre-migration backup, and one writer owned by the project layer. Copy imported audio into the project by default. Give users **Copy Project** for a consistent, self-contained copy; copying a live `.demo` file by itself is not backup. Use a durable in-progress take record and a recoverable media write path. On reopen, reconcile partial media against the take record; distinguish audio recovered after a crash from a successfully finalized take. SQLite alone cannot guarantee that the WAV and the take record agree.

## REAPER compatibility: a tested subset, not session parity

Treat **opening older `.rpp` projects** as the first requirement. Exporting a Demolition session that older REAPER versions can open is a separate target. Neither means using `.rpp` as Demolition’s native format or modifying the original session in place.

1. **Start collecting small fixtures before implementation.** Pick a specific older REAPER version and a current one, record their exact versions and platforms, and save real projects with redistributable test audio. Start with plain audio tracks, item positions, trims/source offsets, fades, gain/pan, and fixed tempo/meter. Add fixtures for relative paths, Windows paths opened elsewhere, Unicode, missing media, and tempo/timebase cases. Record expected positions and audible outcomes, not just parser success.
2. **Import into a new `.demo` project.** Preserve timing and source offsets for the supported subset. Keep source offset, item placement, and project timebase distinct in the model so import doesn’t have to guess or discard timing information. Offer a relink step and an import report that names affected tracks or items. Plugins, complex routing, automation, tempo maps, and stretch markers aren’t fidelity promises; warn when unsupported content may change the result. Never load plugins as part of parsing an import.
3. **Export later, with its own rubric.** Write a conservative audio-only `.rpp` plus relative media, then open it in the pinned older and current versions and compare alignment and supported settings. Use consolidated WAV stems as a fallback when interchange can’t represent a session. Don’t promise lossless round-trips.

## First slices and acceptance checks

| Slice | What counts as done |
|---|---|
| **0. Repo and platform bring-up** | Decide repo licensing before publishing; a free product does not imply a particular source license. Add the workspace, CI, and repeatable build/package instructions. Clean installs launch and enumerate audio devices on all three OSes. |
| **1. Capture and recovery** | On each target OS, run 30 minutes of four-input recording at 48 kHz while playing a guide track on a common USB interface. Require zero reported dropouts or overruns. Check alignment with a known signal near the start and end; record offset and drift. Kill the app mid-take and test disk-full separately. Require the last saved session to reopen and recoverable takes to be surfaced; set a measurable recovery window for the forced-quit test before starting this slice. |
| **2. First usable session** | A musician unfamiliar with Demolition can select inputs, recognize monitoring state, and record a take without coaching. Observe setup time and where they hesitate. Copy the project, open it on another target OS, and confirm that playback works without media relinking. |
| **3. REAPER import proof** | The documented audio subset imports from both pinned REAPER versions, with correct timeline placement and actionable warnings for unsupported content or missing media. Fixture collection starts in slice 0, so this is not the first time we see those sessions. |

Then extend the proven recording path to the North Star’s full band workflow: basic take edits, a small stock mixer, and stereo WAV export. Defer cloud, third-party plugin hosting, MIDI editing, and detachable panels until that workflow is dependable. If the platform gates fail, narrow the feature slice before dropping a target OS.
