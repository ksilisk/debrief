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
| Video after pauses | AVFoundation through `objc2-av-foundation` | Joins the video parts into one file without re-encoding |
| Windows / Linux capture | `cpal` plus WASAPI / PipeWire modules | Later (v0.3), same interface |
| Audio files | `hound` writes WAV while recording; macOS `afconvert` makes m4a afterwards | WAV survives a crash best |
| Resampling | `rubato` → 16 kHz mono for Whisper | |
| Speech to text | `whisper-rs` 0.16 (whisper.cpp 1.8.3, Metal), model large-v3-turbo | One pass per whole track. Silence detection, names hints and Auto per piece are measured in M0.3 first; `transcribe-rs` if Parakeet wins there |
| Model files | `reqwest` (blocking, native TLS) + `sha2` | Downloaded into `~/Library/Application Support/io.github.ksilisk.debrief/models`, checked by size and SHA-256 |
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
│   ├── transcribe/     Whisper and its model files
│   └── cli/            the `debrief` command
├── app/
│   ├── src-tauri/      Tauri shell: window, menu bar icon, hotkey, settings, permissions
│   └── src/            Svelte screens and components
└── docs/               this file; docs/local/ holds private notes and test results (not in git)
```

## How a meeting flows

**Recording.** One ScreenCaptureKit stream delivers the microphone and system audio on the same clock. When only one app is recorded, its sound comes from a Core Audio process tap instead: the tap follows the app and its helper processes, even ones that start during the recording, and its timestamps use the same host clock. Audio is written to `me.wav` and `them.wav` (48 kHz mono), and each chunk is placed by its timestamp, so a gap becomes silence and the tracks stay aligned. With video on, a second stream writes the video.

**Pause and marks.** Paused time is cut out of the recording, using the same clock as the audio timestamps: audio captured during a pause is dropped, and later audio moves back by the pause's length on both tracks. The video file is closed on Pause and a new one starts on Resume (`screen-1.mp4`, `screen-2.mp4`, …); on Stop they are joined into `screen.mp4`, each at its position in the audio. Marked moments and pauses go into `meeting.json` as positions in the recorded audio.

**After a crash.** A lock file (`.recording.lock`) is held while recording, and the OS releases it when the process ends. A meeting whose lock is free but whose `stopped_at` is empty was cut off. Repair sets the WAV headers to the audio actually in the files (they are rewritten only every 5 seconds while recording), fills in `meeting.json`, and joins the video parts. The video survives too: ScreenCaptureKit's MP4 stays playable when the process is killed.

**Processing** is a background job. Each step saves its status in `meeting.json`, so a crash or quit resumes from the last finished step:

1. **Transcribe** each track: converted to 16 kHz mono in memory, then one Whisper pass over the whole track in the meeting's language. Each track's result is kept in `.work/`, so a resume redoes only an unfinished track.
2. **Merge** both tracks by time into `transcript.json`, marking echo lines: a Me line that overlaps Them lines within a second and shares at least 60% of its three-letter chunks with them, so the microphone heard the speakers. Lines under 8 letters are never marked.
3. **Export** `transcript.md`, `transcript.txt`, `transcript.srt` (M1.4).
4. **Summary**, if summaries are on: `summary.md` and `summary.txt`; long transcripts are summarized in parts, then joined (M1.5).
5. **Compress** the audio to m4a (M1.4).

The language is set per meeting, Russian by default. Auto lets Whisper detect one language per track from its first 30 seconds; since Me and Them are separate tracks, a meeting where each side speaks its own language works, but a track that switches language midway does not (see M0.3).

Audio and video never leave the computer. Only the transcript text is sent, and only to the summary provider the user chose.

## Meeting folder

Each meeting gets its own folder inside the folder chosen in Settings (default `~/Documents/Debrief`), named after the date, time and title, for example `2026-11-04 14-30 Product sync`. Characters that a file system rejects are dropped from the title. A taken name gets ` (2)`, ` (3)` and so on.

```
2026-11-04 14-30 Product sync/
├── meeting.json     title, times, settings, tracks, marked moments, processing steps
├── transcript.json  every line from both tracks, with times, speaker, language and echo flag
├── me.wav           microphone; me.m4a after processing
├── them.wav         system audio; them.m4a after processing
├── screen.mp4       only when video is on; screen-1.mp4, screen-2.mp4, … until joined
├── transcript.md    Me and Them with timecodes
├── transcript.txt
├── transcript.srt
├── summary.md       only when summaries are on
└── summary.txt
```

`meeting.json` is written when recording starts and saved again after each change, through a temporary file and a rename, so a crash never leaves half a file:

- `format`: 1. It changes only when an older Debrief would read a field wrong; new optional fields keep the number. Debrief refuses a meeting with a newer format. Until v0.1 ships, format 1 may still change.
- `recording`: `started_at`, `stopped_at` (empty while recording or after a crash), `duration_seconds`, `error`.
- `settings`: `language` (`ru`, `en` or `auto`; `ru` by default).
- `tracks.me`, `tracks.them`: file name, source (the microphone, or the app or apps), recorder diagnostics.
- `video`: file, source, frame rate, size, and `parts` (file and start position) while they aren't joined yet.
- `marks`: marked moments, in seconds from the start of the audio.
- `pauses`: where each pause happened in the audio and how long it lasted, so the real clock time of any moment can be worked out.
- `steps`: the status of each processing step (`transcribe`, `merge`, `export`, `summary`, `compress`): `pending`, `running`, `done`, `skipped` or `failed`, with start and finish times, an error if it failed, and the `model` it used. Processing resumes at the first step that isn't done or skipped.

`transcript.json` (format 1) holds `segments`, sorted by time. Each has `start` and `end` (seconds in the recorded audio, the same timeline as the marks), `speaker` (`me` or `them`), `language`, `text` and `echo`. Echo lines stay in the file; the exports and the summary leave them out.

## Main risks

- ScreenCaptureKit details: choosing the microphone, audio-only mode (the stream still needs a display; it gets a tiny one that is ignored). Covered by M0.2. M0.2 also showed that the app filter misses Chrome's helper processes, so one-app audio uses a Core Audio process tap.
- Quality of mixed Russian and English transcription. Covered by M0.3, which runs once Debrief records real meetings; until then the default stays large-v3-turbo.
- Local servers often run with a small context and cut longer input. The app doesn't recommend a local model; M1.5 checks the OpenAI-compatible path against Ollama or LM Studio.
- The `screencapturekit` crate releases major versions often; pin it exactly.
