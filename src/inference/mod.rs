use crate::error::NihosttError;
use anyhow::Context;
use ort::session::Session;
use ort::value::TensorRef;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Semaphore;

pub mod audio;
pub mod decode;
pub mod features;
pub mod vad;

/// Number of mel frequency bins for ReazonSpeech-k2-v2.
pub const N_MELS: usize = 80;
/// FFT window size (20ms @ 16kHz).
pub const N_FFT: usize = 320;
/// Hop length (10ms @ 16kHz).
pub const HOP_LENGTH: usize = 160;
/// Default concurrent inference sessions.
pub const DEFAULT_POOL_SIZE: usize = 4;

/// Result of a transcription.
#[derive(Debug, Clone)]
pub struct InferenceResult {
    pub text: String,
}

/// A pooled ONNX session triplet (encoder + decoder + joiner).
pub struct PooledSession {
    pub encoder: Session,
    pub decoder: Session,
    pub joiner: Session,
}

/// Thread-safe pool of pre-loaded ONNX sessions.
pub struct SessionPool {
    semaphore: Arc<Semaphore>,
    sessions: Arc<tokio::sync::Mutex<Vec<PooledSession>>>,
}

impl SessionPool {
    pub fn new(model_dir: &Path, size: usize) -> anyhow::Result<Self> {
        let mut sessions = Vec::with_capacity(size);
        for i in 0..size {
            tracing::info!(session = i, "loading ONNX session triplet…");
            let session = load_session_triplet(model_dir)?;
            sessions.push(session);
        }
        Ok(Self {
            semaphore: Arc::new(Semaphore::new(size)),
            sessions: Arc::new(tokio::sync::Mutex::new(sessions)),
        })
    }

    pub async fn checkout(&self) -> anyhow::Result<SessionGuard> {
        let _permit = self
            .semaphore
            .acquire()
            .await
            .context("pool semaphore closed")?;
        let mut sessions = self.sessions.lock().await;
        let session = sessions
            .pop()
            .ok_or_else(|| NihosttError::PoolSaturated)?;
        Ok(SessionGuard {
            pool: self,
            session: Some(session),
            _permit,
        })
    }
}

pub struct SessionGuard<'a> {
    pool: &'a SessionPool,
    session: Option<PooledSession>,
    _permit: tokio::sync::SemaphorePermit<'a>,
}

impl<'a> SessionGuard<'a> {
    pub fn session(&mut self) -> &mut PooledSession {
        self.session.as_mut().expect("session is always present until drop")
    }
}

impl<'a> Drop for SessionGuard<'a> {
    fn drop(&mut self) {
        if let Some(session) = self.session.take() {
            let pool = self.pool.sessions.clone();
            tokio::spawn(async move {
                pool.lock().await.push(session);
            });
        }
    }
}

/// Main inference engine.
pub struct Engine {
    pub pool: Arc<SessionPool>,
    model_dir: PathBuf,
    tokens: Vec<String>,
}

impl Engine {
    pub fn load_with_pool_size(model_dir: &str, pool_size: usize) -> anyhow::Result<Self> {
        let model_dir = PathBuf::from(model_dir);
        let tokens = load_tokens(&model_dir)?;
        let pool = Arc::new(SessionPool::new(&model_dir, pool_size)?);
        Ok(Self {
            pool,
            model_dir,
            tokens,
        })
    }

    pub fn transcribe_file(
        &self,
        file: &str,
        guard: &mut SessionGuard,
    ) -> anyhow::Result<InferenceResult> {
        let (samples, sample_rate) = audio::load_audio(file)?;
        let samples_16k = if sample_rate != 16000 {
            audio::resample(&samples, sample_rate, 16000)?
        } else {
            samples
        };
        self.transcribe_samples(&samples_16k, guard)
    }

    pub fn transcribe_samples(
        &self,
        samples: &[f32],
        guard: &mut SessionGuard,
    ) -> anyhow::Result<InferenceResult> {
        let mel = features::compute_mel(samples, 16000)?;
        let text = decode::greedy_decode(&mel, guard.session(), &self.tokens)?;
        Ok(InferenceResult { text })
    }

    pub fn create_streaming_session(&self) -> StreamingSession {
        StreamingSession::new(self.model_dir.clone(), self.tokens.clone())
    }
}

/// VAD-based streaming session.
pub struct StreamingSession {
    model_dir: PathBuf,
    tokens: Vec<String>,
    vad: vad::SileroVad,
    audio_buffer: Vec<f32>,
}

impl StreamingSession {
    pub fn new(model_dir: PathBuf, tokens: Vec<String>) -> Self {
        let vad = vad::SileroVad::new(&model_dir.join("silero_vad.onnx"))
            .expect("failed to load VAD model");
        Self {
            model_dir,
            tokens,
            vad,
            audio_buffer: Vec::new(),
        }
    }

    /// Process a chunk of audio (16kHz, mono, f32).
    /// Returns text when a speech segment is finalized by VAD.
    pub fn process_chunk(
        &mut self,
        chunk: &[f32],
        _guard: &mut SessionGuard,
    ) -> anyhow::Result<Option<String>> {
        self.audio_buffer.extend_from_slice(chunk);
        // TODO: run VAD on buffer, detect speech segments, feed to transducer
        Ok(None)
    }
}

fn load_session_triplet(model_dir: &Path) -> anyhow::Result<PooledSession> {
    let encoder = Session::builder()
        .and_then(|mut b| b.commit_from_file(model_dir.join("encoder.onnx")))
        .with_context(|| "failed to load encoder.onnx")?;

    let decoder = Session::builder()
        .and_then(|mut b| b.commit_from_file(model_dir.join("decoder.onnx")))
        .with_context(|| "failed to load decoder.onnx")?;

    let joiner = Session::builder()
        .and_then(|mut b| b.commit_from_file(model_dir.join("joiner.onnx")))
        .with_context(|| "failed to load joiner.onnx")?;

    Ok(PooledSession {
        encoder,
        decoder,
        joiner,
    })
}

fn load_tokens(model_dir: &Path) -> anyhow::Result<Vec<String>> {
    let path = model_dir.join("tokens.txt");
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let tokens: Vec<String> = content.lines().map(|s| s.to_string()).collect();
    Ok(tokens)
}
