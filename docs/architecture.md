# Debrief architecture

Status: agreed plan, November 2026.

## Tech stack

| Layer | Choice | Notes |
|---|---|---|
| App shell | Tauri 2 (stable) | Tauri 3 is still alpha; move when it is stable |
| Window UI | Svelte 5 + TypeScript + Vite; plain CSS tokens; bits-ui for accessible popovers, tooltips and lists | bits-ui is added with the first screen that needs it (M2) |
| Core | Rust (stable), tokio for background work | One engine for every OS |
| macOS capture | `screencapturekit` crate, feature `macos_15_0` | Microphone and all-apps system audio in one stream on the same clock; video in a second stream with recording output (HEVC) |
| System audio | Core Audio process tap through `objc2-core-audio` | All apps, or one app with its helper processes, which ScreenCaptureKit's app filter misses (Chrome plays sound from those); ScreenCaptureKit's own system audio went silent mid-recording after volume key presses; same clock as ScreenCaptureKit |
| Video after pauses | AVFoundation through `objc2-av-foundation` | Joins the video parts into one file without re-encoding |
| Windows / Linux capture | `cpal` plus WASAPI / PipeWire modules | Later (v0.3), same interface |
| Audio files | `hound` writes WAV while recording | WAV survives a crash best. Compression (m4a or other) and sizes are decided later |
| Resampling | `rubato` → 16 kHz mono for Whisper | |
| Speech to text | `whisper-rs` 0.16 (whisper.cpp 1.8.3, Metal); large-v3-turbo by default, or its 5-bit version or large-v3; Silero VAD 5.1.2 (whisper.cpp's, built into the app) | One pass per track over its speech only (chosen in M0.3: on silence Whisper invents lines). Called through whisper.cpp's C API (`raw-api`), because whisper-rs can't set `carry_initial_prompt`, which keeps the names and terms in front of every 30-second window. Parakeet (`transcribe-rs`) is not tested yet |
| Model files | `reqwest` (blocking, native TLS) + `sha2` | Downloaded from Hugging Face with one click (New recording, Meeting or Settings) or `debrief model download`, into `~/Library/Application Support/io.github.ksilisk.debrief/models`. A stopped download continues from its `.part` file (HTTP range); the result is checked by size and SHA-256. The download first checks free space as Finder counts it (`volumeAvailableCapacityForImportantUsage`) |
| Token counter | whisper.cpp's vocabulary, read from the model file's header | Counts “Names and terms” the way whisper.cpp splits them, without loading the model (limit: 223 tokens, half the text context less one) |
| Summaries | `reqwest` + serde, streaming | OpenAI chat completions and Anthropic's Messages API (`anthropic-version: 2026-10-21`); other providers later |
| API keys | A file in the app's folder, mode 600 | `keys.json`, readable only by the owner; never in meeting files |
| Tauri plugins | global-shortcut, notification, dialog, opener, single-instance, log, clipboard-manager | Added with the features that need them; tray icon is built into Tauri |
| Signing | A self-signed code-signing certificate, hardened runtime, `Entitlements.plist` | macOS keeps permissions across builds signed with the same certificate; see “Building and signing” |
| macOS permissions | AVFoundation, CoreGraphics, and the private TCC framework loaded at run time | Microphone, Screen & System Audio Recording, System Audio Recording (the process tap, which has no public check) and the meetings folder. The Permissions screen opens at launch when one is missing; screen recording needs Debrief to quit and reopen after it is turned on. The app's Info.plist needs `NSAudioCaptureUsageDescription` for the process tap |
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
│   ├── summarize/      OpenAI and Anthropic clients, settings and the API key file
│   └── cli/            the `debrief` command
├── app/
│   ├── src-tauri/      Tauri shell: window, menu bar icon, hotkey, settings, permissions
│   └── src/            Svelte screens and components
└── docs/               this file; docs/local/ holds private notes and test results (not in git)
```

## How a meeting flows

**Recording.** A ScreenCaptureKit stream delivers the microphone. System audio comes from a Core Audio process tap whose timestamps use the same host clock: a global tap that leaves out only Debrief's own sound, or, when one app is recorded, a tap that follows the app and its helper processes, even ones that start during the recording. Audio is written to `me.wav` and `them.wav` (48 kHz mono), and each chunk is placed by its timestamp, so a gap becomes silence and the tracks stay aligned. With video on, a second stream writes the video.

**Pause and marks.** Paused time is cut out of the recording, using the same clock as the audio timestamps: audio captured during a pause is dropped, and later audio moves back by the pause's length on both tracks. The video file is closed on Pause and a new one starts on Resume (`screen-1.mp4`, `screen-2.mp4`, …); on Stop they are joined into `screen.mp4`, each at its position in the audio. Marked moments and pauses go into `meeting.json` as positions in the recorded audio.

**After a crash.** A lock file (`.recording.lock`) is held while recording, and the OS releases it when the process ends. A meeting whose lock is free but whose `stopped_at` is empty was cut off. Repair sets the WAV headers to the audio actually in the files (they are rewritten only every 5 seconds while recording), fills in `meeting.json`, and joins the video parts. The video survives too: ScreenCaptureKit's MP4 stays playable when the process is killed.

**Processing** is a background job. Each step saves its status in `meeting.json`, so a crash or quit resumes from the last finished step:

1. **Transcribe** each track: converted to 16 kHz mono in memory; Silero VAD finds the speech, and Whisper gets only that, joined with 0.1 s gaps the way whisper.cpp's own VAD joins it, in one pass in the meeting's language. Line times are mapped back to the track; a line edge Whisper put on the wrong side of a pause moves to the speech, and lines Whisper is known to invent (subtitle credits, sound tags, three identical lines in a row) are dropped. A track without speech gets no lines. Each track's result is kept in `.work/`, so a resume redoes only an unfinished track.
2. **Merge** both tracks by time into `transcript.json`, marking echo lines: a Me line that overlaps Them lines within a second and shares at least 60% of its three-letter chunks with them, so the microphone heard the speakers. Lines under 8 letters are never marked.
3. **Export** `transcript.md`, `transcript.txt`, `transcript.srt`: lines that aren't echo, in time order, with marked moments and pauses where they happened, and a count of the echo lines left out. Each subtitle holds at most two lines of 42 characters; a longer line is split and its time shared by length.
4. **Summary**, if summaries are on for the meeting (otherwise skipped): the transcript as plain text, without echo lines, goes to the chosen model with the user's prompt, and the answer is saved as `summary.md` and, without Markdown, `summary.txt`. Before sending, the size is estimated (characters ÷ 3, plus room for the answer); a transcript that doesn't fit the model's context is refused, not split. `debrief summarize` writes or rewrites a summary at any time.
5. **Compress** the audio: not done yet; the tracks stay WAV for now.

The language is set per meeting, Russian by default. Auto lets Whisper detect one language per track from the first 30 seconds of its speech; since Me and Them are separate tracks, a meeting where each side speaks its own language works, but a track that switches language midway does not (see M0.3).

Audio and video never leave the computer. Only the transcript text is sent, and only to the summary provider the user chose.

## Meeting folder

Each meeting gets its own folder inside the folder chosen in Settings (default `~/Documents/Debrief`), named after the date, time and title, for example `2026-11-04 14-30 Product sync`. Characters that a file system rejects are dropped from the title. A taken name gets ` (2)`, ` (3)` and so on.

```
2026-11-04 14-30 Product sync/
├── meeting.json     title, times, settings, tracks, marked moments, processing steps
├── transcript.json  every line from both tracks, with times, speaker, language and echo flag
├── me.wav           microphone
├── them.wav         system audio
├── screen.mp4       only when video is on; screen-1.mp4, screen-2.mp4, … until joined
├── transcript.md    Me and Them with timecodes, marks and pauses
├── transcript.txt   the same as plain text
├── transcript.srt   subtitles for screen.mp4 (or the audio)
├── summary.md       only when summaries are on
└── summary.txt
```

`meeting.json` is written when recording starts and saved again after each change, through a temporary file and a rename, so a crash never leaves half a file:

- `format`: 1. It changes only when an older Debrief would read a field wrong; new optional fields keep the number. Debrief refuses a meeting with a newer format. Until v0.1 ships, format 1 may still change.
- `recording`: `started_at`, `stopped_at` (empty while recording or after a crash), `duration_seconds`, `error`, and `files_deleted_at` once the storage cleanup has deleted the audio and video.
- `settings`: `language` (`ru`, `en` or `auto`; `ru` by default) and `summary` (whether to write one).
- `tracks.me`, `tracks.them`: file name, source (the microphone, or the app or apps), recorder diagnostics.
- `video`: file, source, frame rate, size, and `parts` (file and start position) while they aren't joined yet.
- `marks`: marked moments, in seconds from the start of the audio.
- `pauses`: where each pause happened in the audio and how long it lasted, so the real clock time of any moment can be worked out.
- `steps`: the status of each processing step (`transcribe`, `merge`, `export`, `summary`, `compress`): `pending`, `running`, `done`, `skipped` or `failed`, with start and finish times, an error if it failed, and the `model` it used. Processing resumes at the first step that isn't done or skipped.

`transcript.json` (format 1) holds `segments`, sorted by time. Each has `start` and `end` (seconds in the recorded audio, the same timeline as the marks), `speaker` (`me` or `them`), `language`, `text` and `echo`. Echo lines stay in the file; the exports and the summary leave them out.

## Storage cleanup

Settings can delete old meetings after 30 days, 90 days or a year, counted from the start of the recording: either their audio and video (the transcript and summary stay) or the whole meeting folder. It is off by default. Debrief checks a minute after launch, every 12 hours and when the setting changes; a change that would delete something right away asks first. A meeting without a transcript, one being recorded and one in the processing queue are never touched.

Everything goes to the Trash, through NSFileManager rather than Finder (which would ask for permission to control Finder), so it can be taken back; the space comes back when the Trash is emptied. A meeting can also be moved to the Trash by hand, from the Meeting screen or the sidebar's right-click menu, after a confirmation.

## App settings

The app's own files live in `~/Library/Application Support/io.github.ksilisk.debrief/`, never in Documents (which macOS may sync to iCloud):

- `settings.json`: the meetings folder (left out for `~/Documents/Debrief`), names and terms and the model for Whisper, the last choices on the New recording screen (video and frame rate among them), the summary provider, model, model context, answer limit and whether new recordings write a summary, and the storage cleanup (what to delete and after how many days; never by default).
- `summary-prompt.txt`: the instructions for summaries, plain text anyone can edit. It starts with a default that asks for a summary, decisions, action items and open questions in the meeting's language.
- `keys.json`: API keys by provider, created with mode 600 so only the owner can read it.
- `models/`: the downloaded Whisper models; `settings.json` names the one in use (left out for the default). When a download finishes and the model in use isn't downloaded, Debrief switches to the new one. Meetings that couldn't be transcribed for want of a model are processed when a download finishes (only those from the running session; older ones have Process now).

`debrief config summaries` asks for the provider and the key (hidden input), checks the key by listing the provider's models, and saves the choice. The app's Settings screen writes the same files: a key is saved only after Test connection lists the models with it, and the provider and model only once a model is chosen. Changing the meetings folder doesn't move meetings saved before; the app refuses it while a recording or processing runs. `debrief process --hints` overrides the names and terms for one run.

## Building and signing

macOS ties permissions (Microphone, Screen & System Audio Recording) to an app's designated requirement. An ad-hoc signed build's requirement is its own hash, so every rebuild looks like a new app and loses them. A build signed with a certificate gets `identifier "io.github.ksilisk.debrief" and certificate leaf = H"…"`, which every later build signed with the same certificate meets, so permissions stay (checked 2026-11-09 with two builds whose code hashes differed).

- The certificate is self-signed, made once in Keychain Access (Certificate Assistant → Create a Certificate…, Self Signed Root, Code Signing, a long validity). It does not have to be trusted for `codesign` to use it. Its private key is backed up as a password-protected `.p12` outside the repo; a new certificate means granting permissions once more.
- `APPLE_SIGNING_IDENTITY="<certificate name>" npm run tauri build` signs `Debrief.app` and the DMG. Without the variable the build is ad-hoc, which still runs, so the repo needs no certificate.
- Signing turns on the hardened runtime, which blocks the microphone unless `Entitlements.plist` grants `com.apple.security.device.audio-input`.
- Gatekeeper does not trust a self-signed certificate (`spctl` rejects it). Builds made on this Mac open normally because they are not quarantined; a downloaded copy needs “Open Anyway” once in Privacy & Security.
- Releases and updates: see “Releasing”. A paid Developer ID with notarization would remove the first-launch warning; switching to it means granting permissions once more.

## Releasing

`.github/workflows/release.yml` runs when a tag like `v0.1.1` is pushed, on a GitHub macOS 26 runner (Apple silicon only; `screencapturekit` needs the Xcode 26 SDK). It refuses a tag that doesn't match `version` in the root `Cargo.toml`, imports the certificate into a keychain of its own, builds with `app/src-tauri/tauri.release.conf.json` (which turns on the updater files, so local builds don't need the updater key), signs everything, and creates a **draft** release with the DMG, `Debrief.app.tar.gz`, its `.sig` and `latest.json`. Nothing reaches installed copies until the draft is published.

Repository secrets (Settings → Secrets and variables → Actions): `APPLE_CERTIFICATE` (the `.p12`, base64), `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY` (the certificate's name), `TAURI_SIGNING_PRIVATE_KEY` (the content of `~/.tauri/debrief.key`) and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. Every action in the workflow is pinned to a commit, since the job sees these secrets.

To release: set `version` in `[workspace.package]` of the root `Cargo.toml`, build once so `Cargo.lock` follows, commit, push a `vX.Y.Z` tag, check the draft and publish it.

In the app, `tauri-plugin-updater` reads `releases/latest/download/latest.json` ten seconds after launch and every 12 hours, and on Check for Updates… in the app menu (development builds don't check). It shows a new version in the sidebar and in Settings and installs only on a click, never during a recording or processing. It checks the download against the public key in `tauri.conf.json`, and `requireSignedVersion` rejects a release whose signed version isn't the one `latest.json` names, so an old signed release can't be passed off as new. After installing, Debrief quits and opens the new copy a second later; Tauri's own restart would start the new copy while the old one still runs, and the single-instance guard would close it.

An update downloaded by the app isn't quarantined, so only the first install from a DMG needs “Open Anyway”, and permissions stay because every release is signed with the same certificate (tested on 2026-11-09 with a throwaway key and a local server: 0.1.0 updated itself to 0.1.1 and kept all permissions). If the updater key is lost, installed copies can't update any more and have to be reinstalled from a release made with a new key.

## Main risks

- ScreenCaptureKit details: choosing the microphone, audio-only mode (the stream still needs a display; it gets a tiny one that is ignored). Covered by M0.2. M0.2 also showed that the app filter misses Chrome's helper processes, so one-app audio uses a Core Audio process tap. All-app audio moved to a tap too (M2.2), after ScreenCaptureKit's system audio turned to silence mid-recording when the volume keys were pressed.
- Quality of mixed Russian and English transcription. M0.3 chose the defaults on a real Russian meeting (large-v3-turbo, speech only, fixed language); a meeting that mixes both languages is still to be checked.
- Local servers often run with a small context and cut longer input. The app doesn't recommend a local model; M1.5 checks the OpenAI-compatible path against Ollama or LM Studio.
- The `screencapturekit` crate releases major versions often; pin it exactly.
