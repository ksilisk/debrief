# Debrief architecture

Status: agreed plan, November 2026.

## Tech stack

| Layer | Choice | Notes |
|---|---|---|
| App shell | Tauri 2 (stable) | Tauri 3 is still alpha; move when it is stable |
| Window UI | Svelte 5 + TypeScript + Vite; plain CSS tokens; bits-ui for accessible popovers, tooltips and lists | bits-ui is added with the first screen that needs it (M2) |
| Core | Rust (stable), tokio for background work | One engine for every OS |
| macOS capture | `screencapturekit` crate, feature `macos_15_0` | Microphone and all-apps system audio in one stream on the same clock; video in a second stream with recording output (HEVC) |
| One-app audio | Core Audio process tap through `objc2-core-audio` | Also hears the app's helper processes, which ScreenCaptureKit's app filter misses (Chrome plays sound from those); same clock as ScreenCaptureKit |
| Windows / Linux capture | `cpal` plus WASAPI / PipeWire modules | Later (v0.3), same interface |
| Audio files | `hound` writes WAV while recording; macOS `afconvert` makes m4a afterwards | WAV survives a crash best |
| Resampling | `rubato` → 16 kHz mono for Whisper | |
| Speech to text | `whisper-rs` (whisper.cpp, Metal), model large-v3-turbo | `transcribe-rs` if Parakeet wins the M0.3 test |
| Silence detection | Silero VAD inside whisper.cpp | Bindings to be confirmed in M0.3 |
| Token counter | `tokenizers` + Whisper's tokenizer file | Exact count for “Names and terms” (limit: 224 tokens) |
| Summaries | `reqwest` + serde, OpenAI chat completions with streaming | Providers: OpenAI-compatible server, OpenAI, Claude, OpenRouter |
| API keys | `keyring` | macOS Keychain |
| Tauri plugins | global-shortcut, notification, dialog, opener, single-instance, log, clipboard-manager | Added with the features that need them; tray icon is built into Tauri |
| macOS permissions | `objc2-*` crates | Microphone and Screen & System Audio Recording checks; the app's Info.plist needs `NSAudioCaptureUsageDescription` for the process tap |
| CLI | `clap` | `debrief sources / record / process / summarize` |
| Logging, errors | `tracing`, `thiserror`, `anyhow` | |

Minimum macOS: **15.0**. Identifier: `io.github.ksilisk.debrief`.

## Repository layout

```
debrief/
├── Cargo.toml          Rust workspace
├── crates/
│   ├── core/           meeting model, pipeline, merge, export, summaries
│   ├── capture/        capture interface + macOS module
│   └── cli/            the `debrief` command
├── app/
│   ├── src-tauri/      Tauri shell: window, menu bar icon, hotkey, settings, permissions
│   └── src/            Svelte screens and components
└── docs/               this file; docs/local/ holds private notes and test results (not in git)
```

## How a meeting flows

**Recording.** One ScreenCaptureKit stream delivers the microphone and system audio on the same clock. When only one app is recorded, its sound comes from a Core Audio process tap instead: the tap follows the app and its helper processes, even ones that start during the recording, and its timestamps use the same host clock. Audio is written to `me.wav` and `them.wav` (48 kHz mono), and each chunk is placed by its timestamp, so a gap becomes silence and the tracks stay aligned. With video on, a second stream writes `screen.mp4`. Marked moments go into `meeting.json`.

**Processing** is a background job. Each step saves its status in `meeting.json`, so a crash or quit resumes from the last finished step:

1. Convert both tracks to 16 kHz mono.
2. Skip silence.
3. Transcribe each track with Whisper.
4. Merge the two transcripts by time.
5. Remove “Me” lines that repeat a “Them” line at the same moment (echo from speakers).
6. Write `transcript.md`, `transcript.txt`, `transcript.srt`.
7. If summaries are on: write `summary.md` and `summary.txt` (long transcripts are summarized in parts, then joined).
8. Convert the audio to m4a.

Audio and video never leave the computer. Only the transcript text is sent, and only to the summary provider the user chose.

## Main risks

- ScreenCaptureKit details: choosing the microphone, audio-only mode (the stream still needs a display; it gets a tiny one that is ignored). Covered by M0.2. M0.2 also showed that the app filter misses Chrome's helper processes, so one-app audio uses a Core Audio process tap.
- Quality of mixed Russian and English transcription. Covered by M0.3.
- Local summary quality within 16 GB of memory. Covered by M0.4; remote providers are the fallback.
- The `screencapturekit` crate releases major versions often; pin it exactly.
