# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.1] - 2026-05-05

### Added

- **Expanded benchmark** — 534 real native-speaker clips (Tatoeba 9 + Tatoeba Extended 425 + JSUT basic5000 100). Overall CER: **7.82%** (579/7407 chars, punctuation-normalized). See `tests/benchmark.rs`.
- **Confidence scores** — `POST /v1/transcribe` now returns an optional `confidence` field (average token probability from greedy argmax). See `docs/api-versioning.md`.
- **Readiness probe** — new `GET /ready` endpoint returns 200 when the inference pool has available sessions, 503 when saturated. Exempt from rate limiting like `/health`.
- **API versioning policy** — documented in `docs/api-versioning.md` (additive-only, deprecation cycle, stability guarantees).
- **Kubernetes manifests** — `k8s/deployment.yaml` + `k8s/service.yaml` with init-container model download, liveness (`/health`) and readiness (`/ready`) probes.
- **Nightly soak tests** — `.github/workflows/soak.yml` runs `soak_test` and `load_test` every night at 03:17 UTC.
- **Benchmark tracking** — `.github/workflows/benchmark.yml` runs CER benchmark on PR and main push, uploads artifacts, and posts results to PR comments.
- **Demo GIF** — `docs/nihostt-demo.gif` embedded in README showing server startup, health check, and REST API transcription.
- **GHCR Docker badge** and `cargo install nihostt` in Quick Start.

### Changed

- **Rate limiting on by default** — `--rate-limit-per-minute` now defaults to `60` (was `0`). Burst size increased to `20`.
- **E2E tests on PRs** — removed `refs/heads/main` gate so e2e tests run on pull requests too.
- **Production-hardened error handling** — replaced `unwrap()` / `expect()` in VAD state machine and WebSocket session creation with proper `Result` propagation. `StreamingSession::new` now returns `anyhow::Result`.
- **OpenAPI spec** — updated with `/ready`, `confidence` field, and `429`/`503` response headers.

### Fixed

- **README badges** — uncommented crates.io badge, fixed WebSocket JS example (real PCM16 via AudioContext).
- **Homebrew Formula** — pinned to v0.1.1 with correct SHA256s.

## [0.1.0] - 2026-05-05

Initial release — local Japanese speech-to-text server powered by ReazonSpeech-k2-v2.

### Added

- **ONNX inference engine** — Zipformer encoder + LSTM decoder + joiner (159M params) via ONNX Runtime (`ort` 2.0.0-rc.12).
- **VAD-based streaming** — Silero VAD ONNX model drives pseudo-streaming: speech segments are extracted and transcribed, yielding final results with natural punctuation boundaries.
- **REST / SSE / WebSocket server** — axum 0.8 with single-port HTTP + WebSocket:
  - `GET /health` — health check
  - `POST /v1/transcribe` — file upload, full JSON transcript
  - `POST /v1/transcribe/stream` — SSE stream of partial/final results
  - `WS /v1/ws` — real-time streaming with `ready`, `partial`, `final`, `error` messages
- **INT8 quantization** — native Rust quantization pipeline reduces encoder from ~565 MB FP32 to ~155 MB INT8 with no measurable CER regression.
- **Speaker diarization** — optional `--features diarization` enables WeSpeaker ResNet34 embeddings with online incremental clustering (`WordInfo.speaker` per word).
- **Android FFI** — `libnihostt.so` C-ABI exports (`nihostt_engine_new`, `nihostt_transcribe_file`, `nihostt_stream_process_chunk`, etc.) for on-device mobile STT. NNAPI acceleration via `ort/nnapi` when available.
- **Auto-punctuation** — VAD segment boundaries preserve natural sentence breaks; CER improved from 10.2% (raw greedy decode) to **2.04%** on real native speech.
- **Real speech benchmark** — Tatoeba Japanese dataset (9 native-speaker clips) evaluated at **CER 2.04%**. See `tests/benchmark.rs` and `tests/fixtures/tatoeba/`.
- **Docker support** — multi-stage `Dockerfile` (CPU) and `Dockerfile.cuda` (NVIDIA CUDA 12+), plus baked-image build-arg (`NIHOSTT_BAKE_MODEL=1`).
- **Cross-platform execution providers** — `--features coreml` (macOS ARM64 Neural Engine), `--features cuda` (Linux x86_64), CPU default.
- **Privacy-first defaults** — loopback-only bind, origin allowlist, optional per-IP rate limiting, SHA-256 verified model downloads with atomic rename.

[Unreleased]: https://github.com/ekhodzitsky/nihostt/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/ekhodzitsky/nihostt/releases/tag/v0.1.1
[0.1.0]: https://github.com/ekhodzitsky/nihostt/releases/tag/v0.1.0
