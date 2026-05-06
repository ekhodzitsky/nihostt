# Contributing to nihostt

Thank you for your interest in making nihostt better! This document outlines how to contribute effectively.

## Development Setup

```bash
git clone https://github.com/ekhodzitsky/nihostt.git
cd nihostt

# Install dependencies
# macOS: brew install protobuf
# Debian/Ubuntu: sudo apt install protobuf-compiler

# Build
cargo build --release

# Download model (~155 MB, one-time)
cargo run --release -- download

# Run tests
cargo test

# Run E2E tests (require model)
cargo test --test e2e_rest --test e2e_ws --test e2e_errors --test e2e_shutdown --test e2e_rate_limit -- --ignored --test-threads=1

# Lint and supply-chain checks
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo deny check
```

## Architecture

```
src/
  inference/     ONNX engine, session pool, VAD, decode
  server/        axum REST / SSE / WebSocket handlers
  protocol/      WebSocket JSON message types
  model/         HuggingFace model download
  quantize.rs    Native Rust INT8 quantization
  punctuation.rs Rule-based Japanese punctuation restoration
  ffi.rs         C-ABI for Android integration
```

## Submitting Changes

1. **Fork & branch** — create a feature branch from `master`
2. **Write tests** — every bug fix and feature needs tests
3. **Run the full suite** — `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test && cargo deny check`
4. **Update docs** — README, AGENTS.md, and inline docs if interfaces change
5. **Open a PR** — describe what, why, and how you tested it

## Code Style

- Rust 2024 edition
- `anyhow` for internal errors, `NihosttError` for public API
- `tracing` for logs (never `println!` in library code)
- Constants live in `inference/mod.rs`
- Feature-gate heavy deps behind Cargo features (`diarization`, `ffi`)
- Keep model downloads pinned to immutable revisions and SHA-256 verified

## Areas Where Help is Welcome

- **LM rescoring** — integrate KenLM or neural LM for better kanji disambiguation
- **Streaming model** — when Reazon releases a streaming Zipformer, integrate it
- **More benchmarks** — add Common Voice JA, JSUT, or CSJ test sets
- **Android example app** — minimal Kotlin app showing FFI usage
- **Docker optimization** — multi-stage build, smaller image

## License

By contributing, you agree that your contributions will be licensed under the MIT License.
