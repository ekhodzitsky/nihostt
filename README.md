# nihostt

Local speech-to-text server powered by ReazonSpeech-k2-v2 — on-device Japanese
speech recognition via ONNX Runtime. No cloud APIs, no API keys, full privacy.

## Features

- **WebSocket** (`/v1/ws`) — VAD-based streaming transcription
- **REST** (`/v1/transcribe`) — file upload transcription
- **SSE** (`/v1/transcribe/stream`) — streaming Server-Sent Events
- **CLI** — `serve`, `download`, `transcribe` commands
- **ONNX Runtime** — runs on CPU, CoreML (Apple Silicon), or CUDA (NVIDIA)

## Quick Start

```bash
# Download model (~590 MB, one-time)
cargo run --release -- download

# Start server
cargo run --release -- serve

# Transcribe a file
cargo run --release -- transcribe recording.wav
```

## Model

ReazonSpeech-k2-v2 (159M params, Zipformer RNN-T) — SOTA Japanese ASR on
JSUT-BASIC5000, Common Voice, and TEDxJP-10K benchmarks.

## License

MIT
