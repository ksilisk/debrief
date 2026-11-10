# Debrief

Record a meeting, get a transcript, and optionally a short summary, as plain files you can send to anyone.

- **Two tracks:** your microphone (“Me”) and the sound of the call (“Them”) are recorded separately, so the transcript knows who spoke.
- **Optional screen video** at 5 or 10 fps, from a whole screen or one window.
- **On-device transcription** with Whisper. Audio and video never leave your computer.
- **Summaries on request**, with a prompt you can change per meeting, through any OpenAI-compatible API: a local server (Ollama, LM Studio, llama.cpp), OpenAI, Claude or OpenRouter.
- **Plain files:** `transcript.txt`, `transcript.md`, `transcript.srt`, and `summary.txt` / `summary.md` once you write a summary.

> **Status:** early development. Nothing is usable yet. macOS 15 or later first; Windows and Linux are planned.

## Build from source

Requirements: macOS 15+, Xcode, Rust (stable), Node.js.

```bash
cd app
npm install
npm run tauri dev
```

The command-line tool:

```bash
cargo run -p debrief-cli -- --help
```

See [`docs/architecture.md`](docs/architecture.md) for the stack.

## License

[MIT](LICENSE)
