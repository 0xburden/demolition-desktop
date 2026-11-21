# North Star — A DAW that gets out of the way

> **Working vision, not a specification.** Use this to guide product and engineering decisions. Favor demonstrable progress over comprehensive planning documents.

## The product

A **free, professional-capable, cross-platform native DAW** built for independent musicians and bands. Recording an excellent-sounding record should not demand expensive software, an expert's familiarity with traditional DAW interfaces, or a powerful new computer.

The experience should be **as inviting as modern creative software, as capable as a serious workstation, and exceptionally fast and dependable**. It must make sense to someone recording their first demo without constraining someone mixing their tenth album.

The entire local production workflow is free and works without an account. Future optional paid cloud services may offer backup, sharing, review, and collaboration—never gate recording, editing, mixing, project access, or audio quality.

## Experience philosophy: the expanding workspace

The **arrangement and the music are the center of gravity**. Transport, tracks, audio, and essential recording state are immediately visible. Complexity is not removed; it is available on demand.

Modules—inspector, mixer, plugin editor, routing, take lanes, browser—can **expand, collapse, pin, dock, and detach**. The workspace grows around the current task, then yields its space back. A beginner can discover an action visually; an expert can reach the same action quickly through shortcuts or commands.

Favor clarity and low interaction cost over minimalism for its own sake. Never hide clipping, armed inputs, monitoring state, unsaved work, or other safety-critical information just to make the interface prettier. Motion should explain state changes, not slow down work.

## Technical direction (chosen)

- **Rust** for the native application, real-time audio engine, and core data model.
- **GPUI + GPUI Kit** for the interface, modular panels, docking, and visual system. Their maturity and breaking-change tradeoffs are accepted; pin working versions where needed.
- Keep the **audio engine independent from the UI** and from any cloud service. Real-time processing must not depend on rendering, network access, allocations on the audio callback, or blocking UI work.
- Target **Windows, macOS, and Linux**. Prioritize dependable local audio hardware, efficient playback/recording, resilient projects, and responsiveness on modest machines.
- Embrace established standards where useful: audio files, MIDI, eventually **CLAP/VST3** hosting and **DAWproject/REAPER** interchange. Pro Tools interchange is an aspiration, not a release dependency. Never promise perfect cross-DAW plugin or session fidelity.

Architecture is a means to these goals, not a permanent constraint. Benchmark and adjust when reality disagrees with the plan.

## The first proof

Make one workflow unusually good: **a band opens a session, connects an audio interface, records multiple inputs, edits takes, mixes with useful stock tools, saves safely, and exports a finished song.**

Build vertical slices around that journey. Test real sessions on ordinary computers and with real musicians. Measure dropouts, latency, UI responsiveness, project recovery, and the number of decisions required to accomplish common tasks. Prefer one reliable, satisfying workflow to a broad collection of half-finished features.

## What we are not building first

Not a replica of REAPER, Logic, or Pro Tools; not a browser wrapper; not a subscription-first product; not a vast marketplace or AI music generator; not an exhaustive list of instrument plugins. Interoperability and cloud collaboration can grow after the core experience earns trust.

## Visual references

These are **directional concepts, not pixel-perfect screens or a required component inventory**:

1. [Arrangement / default workspace](references/01-arrangement.png) — music first, restrained controls, latent panels.
2. [Focused inspector](references/02-focused-inspector.png) — contextual expansion around a selected track and clip.
3. [Expanded mixer](references/03-expanded-mixer.png) — professional density without losing coherence or the arrangement.

**Decision test:** Does this make recording and producing music feel more capable, more dependable, or substantially more natural? If not, reconsider it.
