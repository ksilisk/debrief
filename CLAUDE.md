# Debrief

A macOS-first desktop app that records meetings (microphone and system audio as separate tracks, optional low-fps screen video), transcribes them on the device, and optionally writes a summary through an OpenAI-compatible API. Output is plain files in a folder the user chooses.

- Stack, layout and roadmap: `docs/architecture.md`

## Repository layout

```
crates/core      meeting model, processing pipeline, export, summaries (no UI)
crates/capture   capture interface + per-platform modules (macOS: ScreenCaptureKit)
crates/transcribe  speech to text (Whisper), names and terms, model downloads
crates/summarize  summaries (OpenAI, Anthropic), settings and the API key file
crates/cli       the `debrief` command-line tool
app/src          Svelte 5 + TypeScript window
app/src-tauri    Tauri 2 shell (Rust): window, menu bar icon, commands
docs/            architecture and roadmap
```

## Commands

Rust (from the repo root):

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
cargo run -p debrief-cli -- --help
```

App (from `app/`):

```bash
npm install
npm run tauri dev        # run the app with hot reload
npm run check            # Svelte and TypeScript type check
npm run tauri build      # build Debrief.app and a DMG (ad-hoc signed)
APPLE_SIGNING_IDENTITY="<certificate name>" npm run tauri build   # signed: macOS keeps permissions between builds
```

## Working rules

- Before calling work done, run fmt, clippy (no warnings), tests and `npm run check`.
- Minimum macOS is 15.0 (ScreenCaptureKit microphone capture and recording output need it).
- Add external crates to `[workspace.dependencies]` in the root `Cargo.toml` and use `name.workspace = true` in member crates. Pin exact major versions; `screencapturekit` changes often.
- Library crates use `thiserror` for errors; binaries use `anyhow`. No `unwrap()` outside tests.
- Keep `crates/core` free of UI and platform code so it can be tested on its own.
- Never log or print transcript or summary text; it is private meeting content.

## UI rules (summary of the private design)

- Svelte 5 runes (`$state`, `$derived`, `$props`), TypeScript everywhere.
- One main window plus a menu bar icon. Screens: New recording, Recording, Processing, Meeting (Summary, Transcript, Files tabs), Settings.
- Colors and fonts only through the variables in `app/src/styles/tokens.css`; support light and dark.
- Explanations go into an (i) info icon with a tooltip, not as text under a field. Live status (sizes, token counts, model in use) stays visible.
- UI text is plain English: short sentences, active voice, labels that say what happens.
