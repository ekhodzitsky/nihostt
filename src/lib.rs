//! # nihostt
//!
//! Local speech-to-text powered by ReazonSpeech-k2-v2 — on-device Japanese speech
//! recognition via ONNX Runtime. No cloud APIs, no API keys, full privacy.
//!
//! Temporary blanket allows for pre-existing lint noise in modules we are not
//! modifying in this pass.
#![allow(
    dead_code,
    unused_imports,
    clippy::needless_range_loop,
    mismatched_lifetime_syntaxes
)]
//!
//! ## Quick start
//!
//! ```ignore
//! use nihostt::inference::Engine;
//!
//! let engine = Engine::load("~/.nihostt/models")?;
//!
//! // File transcription
//! let text = engine.transcribe_file("audio.wav")?;
//!
//! // VAD-based streaming recognition
//! let mut session = engine.create_streaming_session()?;
//! session.process_chunk(&audio_16khz)?;
//! ```
//!
//! ## Modules
//!
//! - [`inference`] — ONNX inference engine, session pool, VAD, audio utilities
//! - [`error`] — Typed error types ([`NihosttError`](error::NihosttError))
//! - [`protocol`] — WebSocket JSON message types
//! - [`server`] — HTTP/WebSocket server entry point
//! - [`model`] — Model download and management

pub mod error;
pub mod ffi;
pub mod inference;
pub mod model;
pub mod onnx_proto;
pub mod protocol;
pub mod quantize;

#[cfg(feature = "server")]
pub mod server;
