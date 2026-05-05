# nihostt — Agent Guide

> Local speech-to-text server powered by ReazonSpeech-k2-v2. On-device Japanese
> speech recognition via ONNX Runtime. No cloud APIs, no API keys, full privacy.
>
> Repository: https://github.com/ekhodzitsky/nihostt
> License: MIT

## Project Overview

**nihostt** is a single-binary Rust server that turns any machine into a
real-time Japanese speech-to-text endpoint. It loads the ReazonSpeech-k2-v2
model (Zipformer encoder + LSTM decoder + joiner, 159M params) via ONNX Runtime
and exposes:

- **WebSocket** (`/v1/ws`) — VAD-based streaming transcription with final results
- **REST** (`/v1/transcribe`) — file upload, full JSON response
- **SSE** (`/v1/transcribe/stream`) — file upload, streaming Server-Sent Events
- **CLI** — `serve`, `download`, `transcribe` commands

The model (~590 MB FP32, ~154 MB INT8) auto-downloads from HuggingFace on first
run.

### Key differences from gigastt

| Feature | gigastt | nihostt |
|---|---|---|
| Language | Russian | Japanese |
| Model | GigaAM v3 e2e_rnnt | ReazonSpeech-k2-v2 Zipformer RNN-T |
| Streaming | Native RNN-T streaming | VAD-based pseudo-streaming |
| Mel bins | 64 | 80 |
| Parameters | 240M | 159M |

## Technology Stack

- **Language**: Rust 2024 edition, stable toolchain (1.85+)
- **ONNX Runtime**: `ort` 2.0.0-rc.12
- **Async runtime**: tokio (full features)
- **HTTP + WebSocket server**: axum 0.8 (`ws`, `multipart`)
- **CLI**: clap 4 (derive, env)
- **Serialization**: serde + serde_json
- **Logging**: tracing + tracing-subscriber (env-filter)
- **Error handling**: anyhow (internal), `NihosttError` (public API)
- **Audio decoding**: symphonia (AAC, MP3, OGG, FLAC, WAV, PCM)
- **Audio resampling**: rubato 0.16
- **FFT**: rustfft 6
- **VAD**: Silero VAD (ONNX)
- **Rate limiting**: in-tree token-bucket (dashmap-backed)

### Execution providers (compile-time features)

| Platform | Feature | Provider |
|---|---|---|
| macOS ARM64 | `--features coreml` | CoreML + Neural Engine |
| Linux x86_64 + NVIDIA | `--features cuda` | CUDA 12+ |
| Any | _(default)_ | CPU |

Features `coreml` and `cuda` are **mutually exclusive**.

## Build Commands

```sh
# Debug build (CPU only, any platform)
cargo build

# Release build (LTO, stripped, single codegen unit)
cargo build --release

# macOS ARM64 with CoreML / Neural Engine
cargo build --release --features coreml

# Linux x86_64 with NVIDIA CUDA 12+
cargo build --release --features cuda
```

## Code Organization

```
src/
  lib.rs                  # Public module exports
  main.rs                 # CLI (clap): serve, download, transcribe
  error.rs                # Typed error types (NihosttError)
  inference/
    mod.rs                # Engine, SessionPool, StreamingSession
    features.rs           # Mel spectrogram (80 bins, FFT=320, hop=160)
    decode.rs             # RNN-T greedy decode loop
    audio.rs              # Audio loading, resampling, channel mixing
    vad.rs                # Silero VAD wrapper
  server/
    mod.rs                # axum router, origin middleware, graceful shutdown
    http.rs               # REST handlers: /health, /v1/transcribe, SSE, WS
    rate_limit.rs         # In-tree per-IP token-bucket rate limiter
  protocol/mod.rs         # WebSocket JSON message types
  model/mod.rs            # HuggingFace model download (streaming + atomic rename)
```

## Key Constants

Defined in `src/inference/mod.rs`:

| Constant | Value | Meaning |
|---|---|---|
| `N_MELS` | 80 | Mel frequency bins |
| `N_FFT` | 320 | FFT window size (20ms @ 16kHz) |
| `HOP_LENGTH` | 160 | Hop length (10ms @ 16kHz) |
| `DEFAULT_POOL_SIZE` | 4 | Concurrent inference sessions |

## Model Files

Downloaded to `~/.nihostt/models/` from `reazon-research/reazonspeech-k2-v2`:

| File | Size (FP32) | Purpose |
|---|---|---|
| `encoder.onnx` | 565 MB | Zipformer encoder |
| `decoder.onnx` | 12 MB | LSTM decoder |
| `joiner.onnx` | 11 MB | RNN-T joiner |
| `tokens.txt` | small | Character vocabulary |
| `silero_vad.onnx` | ~1 MB | Voice activity detection |

## Development Conventions

- Rust 2024 edition
- `anyhow` for internal error handling, `NihosttError` for public API
- `tracing` for logging (never `println!` in library code)
- **No `unwrap()` in production paths** — use `?`, `.context()`, or `unwrap_or_else`
- Shared constants live in `inference/mod.rs`, referenced by sub-modules

### TDD workflow

1. Write failing test first
2. Implement minimal code to pass
3. Refactor, verify tests still pass
4. `cargo test && cargo clippy` before every commit

### API versioning & backward compatibility

- WebSocket protocol version: `PROTOCOL_VERSION = "1.0"`
- Canonical WS path: `/v1/ws`
- New fields are **additive only**

## Useful Commands for Agents

```sh
# Quick iteration cycle
cargo test && cargo clippy

# Run with model (after `cargo run -- download`)
cargo run --release -- serve
cargo run --release -- transcribe recording.wav

# Check all feature combinations compile
cargo check --features coreml
cargo check --features cuda

# Run with tracing at debug level
RUST_LOG=nihostt=debug cargo run -- serve
```

## Notes for AI Agents

- **Always run `cargo test && cargo clippy` before finishing any change.**
- When modifying the WebSocket protocol, update `PROTOCOL_VERSION` in
  `protocol/mod.rs`.
- When adding new CLI flags, add the corresponding env var and document it in
  both `main.rs` and this file.
- Model download logic is in `src/model/mod.rs`. If you change HF repo or file
  names, update the cache key in CI workflows.
- The project uses English for all code comments, documentation, and commit
  messages.
