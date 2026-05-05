use crate::error::NihosttError;
use anyhow::Context;
use ort::session::Session;
use ort::value::TensorRef;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Semaphore;

pub mod audio;
pub mod decode;
#[cfg(feature = "diarization")]
pub mod diarization;
pub mod features;
pub mod punctuation;
pub mod vad;

/// Number of mel frequency bins for ReazonSpeech-k2-v2.
pub const N_MELS: usize = 80;
/// FFT window size (20ms @ 16kHz).
pub const N_FFT: usize = 320;
/// Hop length (10ms @ 16kHz).
pub const HOP_LENGTH: usize = 160;
/// Default concurrent inference sessions.
pub const DEFAULT_POOL_SIZE: usize = 4;

// VAD constants for streaming segmentation.
const VAD_FRAME_SAMPLES: usize = 512;
const SAMPLE_RATE: u32 = 16000;
const SPEECH_THRESHOLD: f32 = 0.5;
const SILENCE_THRESHOLD: f32 = 0.35;
const MIN_SPEECH_MS: u32 = 250;
const MAX_SEGMENT_MS: u32 = 25000;
const PAD_MS: u32 = 300;
const PARTIAL_MS: u32 = 2000;
const SILENCE_END_MS: u32 = 300;

const MIN_SPEECH_SAMPLES: usize = (MIN_SPEECH_MS * SAMPLE_RATE / 1000) as usize;
const MAX_SEGMENT_SAMPLES: usize = (MAX_SEGMENT_MS * SAMPLE_RATE / 1000) as usize;
const PAD_SAMPLES: usize = (PAD_MS * SAMPLE_RATE / 1000) as usize;
const PARTIAL_SAMPLES: usize = (PARTIAL_MS * SAMPLE_RATE / 1000) as usize;
const SILENCE_END_SAMPLES: usize = (SILENCE_END_MS * SAMPLE_RATE / 1000) as usize;

/// Result of a transcription.
#[derive(Debug, Clone)]
pub struct InferenceResult {
    pub text: String,
}

/// Result returned by [`StreamingSession::process_chunk`].
#[derive(Debug, Clone, serde::Serialize)]
pub enum ProcessResult {
    Noop,
    Partial(String),
    Final(String, Option<u32>),
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
        let session = sessions.pop().ok_or_else(|| NihosttError::PoolSaturated)?;
        Ok(SessionGuard {
            pool: self,
            session: Some(session),
            _permit,
        })
    }

    /// Blocking checkout for use inside `spawn_blocking`.
    pub fn checkout_blocking(&self) -> anyhow::Result<BlockingSessionGuard> {
        let _permit = self
            .semaphore
            .clone()
            .try_acquire_owned()
            .context("pool semaphore closed")?;
        let mut sessions = self.sessions.blocking_lock();
        let session = sessions.pop().ok_or_else(|| NihosttError::PoolSaturated)?;
        Ok(BlockingSessionGuard {
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

impl<'a> std::ops::Deref for SessionGuard<'a> {
    type Target = PooledSession;
    fn deref(&self) -> &Self::Target {
        self.session
            .as_ref()
            .expect("session is always present until drop")
    }
}

impl<'a> std::ops::DerefMut for SessionGuard<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.session
            .as_mut()
            .expect("session is always present until drop")
    }
}

impl<'a> SessionGuard<'a> {
    pub fn session(&mut self) -> &mut PooledSession {
        self
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

/// Blocking variant of [`SessionGuard`] for use inside `spawn_blocking`.
pub struct BlockingSessionGuard {
    pool: *const SessionPool,
    session: Option<PooledSession>,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

// Safety: BlockingSessionGuard is only used inside a single thread (spawn_blocking)
// and the pointer is only used to push back into the pool on Drop.
unsafe impl Send for BlockingSessionGuard {}

impl std::ops::Deref for BlockingSessionGuard {
    type Target = PooledSession;
    fn deref(&self) -> &Self::Target {
        self.session
            .as_ref()
            .expect("session is always present until drop")
    }
}

impl std::ops::DerefMut for BlockingSessionGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.session
            .as_mut()
            .expect("session is always present until drop")
    }
}

impl BlockingSessionGuard {
    /// Mutable reference to the pooled session.
    pub fn session(&mut self) -> &mut PooledSession {
        self.session
            .as_mut()
            .expect("session is always present until drop")
    }

    /// Take ownership of the pooled session, consuming the guard without
    /// returning the session to the pool.
    pub fn into_inner(mut self) -> PooledSession {
        self.session
            .take()
            .expect("session is always present until drop")
    }
}

impl Drop for BlockingSessionGuard {
    fn drop(&mut self) {
        if let Some(session) = self.session.take() {
            // Safety: pool is always valid (SessionPool lives as long as the Engine).
            let pool = unsafe { &*self.pool };
            let pool_arc = pool.sessions.clone();
            tokio::spawn(async move {
                pool_arc.lock().await.push(session);
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
        session: &mut PooledSession,
    ) -> anyhow::Result<InferenceResult> {
        let (samples, sample_rate) = audio::load_audio(file)?;
        let samples_16k = if sample_rate != 16000 {
            audio::resample(&samples, sample_rate, 16000)?
        } else {
            samples
        };
        self.transcribe_samples(&samples_16k, session)
    }

    pub fn transcribe_samples(
        &self,
        samples: &[f32],
        session: &mut PooledSession,
    ) -> anyhow::Result<InferenceResult> {
        let mel = features::compute_mel(samples, 16000)?;
        let text = decode::greedy_decode(&mel, session, &self.tokens)?;
        Ok(InferenceResult {
            text: punctuation::auto_punctuate(&text),
        })
    }

    pub fn create_streaming_session(&self) -> StreamingSession {
        StreamingSession::new(self.model_dir.clone(), self.tokens.clone())
    }
}

/// VAD-based streaming session.
pub struct StreamingSession {
    tokens: Vec<String>,
    vad: vad::SileroVad,
    /// All 16 kHz samples received since the last finalization.
    audio_buffer: Vec<f32>,
    /// How many samples have already been fed to the VAD.
    vad_processed: usize,
    /// Whether we are currently inside a speech segment.
    in_speech: bool,
    /// Sample index (in `audio_buffer`) where current speech started.
    speech_start_sample: usize,
    /// Sample index (in `audio_buffer`) of the last frame that was speech.
    last_speech_sample: usize,
    /// Sample index where sustained silence began (while `in_speech`).
    silence_start_sample: Option<usize>,
    /// Sample index of the last partial emission.
    last_partial_at: usize,
    #[cfg(feature = "diarization")]
    diarization_state: Option<diarization::DiarizationState>,
}

impl StreamingSession {
    pub fn new(model_dir: PathBuf, tokens: Vec<String>) -> Self {
        let vad = vad::SileroVad::new(&model_dir.join("silero_vad.onnx"))
            .expect("failed to load VAD model");
        #[cfg(feature = "diarization")]
        let diarization_state = diarization::DiarizationState::load(&model_dir);
        Self {
            tokens,
            vad,
            audio_buffer: Vec::new(),
            vad_processed: 0,
            in_speech: false,
            speech_start_sample: 0,
            last_speech_sample: 0,
            silence_start_sample: None,
            last_partial_at: 0,
            #[cfg(feature = "diarization")]
            diarization_state,
        }
    }

    /// Process a chunk of audio (16 kHz, mono, f32).
    /// Returns a vector of [`ProcessResult`]s (partials and finals) produced while
    /// draining the newly arrived audio through the VAD.
    pub fn process_chunk(
        &mut self,
        chunk: &[f32],
        session: &mut PooledSession,
    ) -> anyhow::Result<Vec<ProcessResult>> {
        self.audio_buffer.extend_from_slice(chunk);
        #[cfg(feature = "diarization")]
        if let Some(ref mut state) = self.diarization_state {
            state.feed(chunk);
        }
        let mut results = Vec::new();

        while self.vad_processed + VAD_FRAME_SAMPLES <= self.audio_buffer.len() {
            let frame_start = self.vad_processed;
            let frame = &self.audio_buffer[frame_start..frame_start + VAD_FRAME_SAMPLES];
            let prob = self.vad.process(frame)?;
            self.vad_processed += VAD_FRAME_SAMPLES;
            let frame_end = self.vad_processed;

            if prob > SPEECH_THRESHOLD {
                if !self.in_speech {
                    self.in_speech = true;
                    self.speech_start_sample = frame_start;
                    self.last_speech_sample = frame_end;
                    self.silence_start_sample = None;
                    self.last_partial_at = frame_start;
                    tracing::debug!(sample = frame_start, "VAD: speech start detected");
                } else {
                    self.last_speech_sample = frame_end;
                    self.silence_start_sample = None;
                }
            } else if prob < SILENCE_THRESHOLD && self.in_speech {
                if self.silence_start_sample.is_none() {
                    self.silence_start_sample = Some(frame_start);
                }

                let silence_samples = frame_end - self.silence_start_sample.unwrap();
                let speech_samples = self
                    .last_speech_sample
                    .saturating_sub(self.speech_start_sample);

                if silence_samples >= SILENCE_END_SAMPLES && speech_samples >= MIN_SPEECH_SAMPLES {
                    tracing::debug!(
                        speech_samples,
                        silence_samples,
                        "VAD: speech end detected (silence)"
                    );
                    let text = self.transcribe_segment(session)?;
                    let speaker_id = self.extract_speaker_id();
                    results.push(ProcessResult::Final(
                        punctuation::auto_punctuate(&text),
                        speaker_id,
                    ));
                    self.reset_after_final();
                    continue;
                }
            }

            if self.in_speech {
                let segment_samples = frame_end.saturating_sub(self.speech_start_sample);

                if segment_samples >= MAX_SEGMENT_SAMPLES {
                    tracing::debug!(segment_samples, "VAD: forcing final (max segment duration)");
                    let text = self.transcribe_segment(session)?;
                    let speaker_id = self.extract_speaker_id();
                    results.push(ProcessResult::Final(
                        punctuation::auto_punctuate(&text),
                        speaker_id,
                    ));
                    self.reset_after_final();
                    continue;
                }

                if frame_end.saturating_sub(self.last_partial_at) >= PARTIAL_SAMPLES {
                    let text = self.transcribe_partial(session)?;
                    if !text.is_empty() {
                        results.push(ProcessResult::Partial(punctuation::auto_punctuate(&text)));
                    }
                    self.last_partial_at = frame_end;
                }
            }
        }

        Ok(results)
    }

    /// Finalise any pending speech segment (called on `Stop` or disconnect).
    pub fn finalize(&mut self, session: &mut PooledSession) -> anyhow::Result<Vec<ProcessResult>> {
        let mut results = Vec::new();

        // Drain any remaining complete VAD frames first.
        while self.vad_processed + VAD_FRAME_SAMPLES <= self.audio_buffer.len() {
            let frame_start = self.vad_processed;
            let frame = &self.audio_buffer[frame_start..frame_start + VAD_FRAME_SAMPLES];
            let prob = self.vad.process(frame)?;
            self.vad_processed += VAD_FRAME_SAMPLES;
            let frame_end = self.vad_processed;

            if prob > SPEECH_THRESHOLD {
                if !self.in_speech {
                    self.in_speech = true;
                    self.speech_start_sample = frame_start;
                    self.last_speech_sample = frame_end;
                    self.silence_start_sample = None;
                    self.last_partial_at = frame_start;
                } else {
                    self.last_speech_sample = frame_end;
                    self.silence_start_sample = None;
                }
            } else if prob < SILENCE_THRESHOLD && self.in_speech {
                if self.silence_start_sample.is_none() {
                    self.silence_start_sample = Some(frame_start);
                }
                let silence_samples = frame_end - self.silence_start_sample.unwrap();
                let speech_samples = self
                    .last_speech_sample
                    .saturating_sub(self.speech_start_sample);
                if silence_samples >= SILENCE_END_SAMPLES && speech_samples >= MIN_SPEECH_SAMPLES {
                    let text = self.transcribe_segment(session)?;
                    let speaker_id = self.extract_speaker_id();
                    results.push(ProcessResult::Final(
                        punctuation::auto_punctuate(&text),
                        speaker_id,
                    ));
                    self.reset_after_final();
                }
            }
        }

        if self.in_speech {
            let speech_samples = self
                .last_speech_sample
                .saturating_sub(self.speech_start_sample);
            if speech_samples >= MIN_SPEECH_SAMPLES {
                let text = self.transcribe_segment(session)?;
                let speaker_id = self.extract_speaker_id();
                results.push(ProcessResult::Final(text, speaker_id));
                self.reset_after_final();
            }
        }

        // Flush the diarizer so that any trailing partial window is processed.
        #[cfg(feature = "diarization")]
        if let Some(ref mut state) = self.diarization_state {
            let _ = state.flush();
        }

        Ok(results)
    }

    fn transcribe_segment(&self, session: &mut PooledSession) -> anyhow::Result<String> {
        let start = self.speech_start_sample.saturating_sub(PAD_SAMPLES);
        let end = (self.last_speech_sample + PAD_SAMPLES).min(self.audio_buffer.len());
        let segment = &self.audio_buffer[start..end];
        let mel = features::compute_mel(segment, SAMPLE_RATE)?;
        let text = decode::greedy_decode(&mel, session, &self.tokens)?;
        Ok(text)
    }

    fn transcribe_partial(&self, session: &mut PooledSession) -> anyhow::Result<String> {
        let start = self.speech_start_sample.saturating_sub(PAD_SAMPLES);
        let end = self.vad_processed;
        let segment = &self.audio_buffer[start..end];
        let mel = features::compute_mel(segment, SAMPLE_RATE)?;
        let text = decode::greedy_decode(&mel, session, &self.tokens)?;
        Ok(text)
    }

    fn reset_after_final(&mut self) {
        let keep_from = self.last_speech_sample;
        if keep_from < self.audio_buffer.len() {
            let remaining = self.audio_buffer.split_off(keep_from);
            self.audio_buffer = remaining;
        } else {
            self.audio_buffer.clear();
        }
        self.vad_processed = 0;
        self.in_speech = false;
        self.silence_start_sample = None;
        self.last_partial_at = 0;
        self.vad.reset();
    }

    /// Extract a speaker ID for the current speech segment.
    #[cfg(feature = "diarization")]
    fn extract_speaker_id(&mut self) -> Option<u32> {
        let speaker = self.diarization_state.as_ref()?.current_speaker();
        if speaker.is_none() {
            tracing::debug!("speaker_id not available yet (segment shorter than diarizer window)");
        }
        speaker
    }

    /// No-op fallback when diarization feature is disabled.
    #[cfg(not(feature = "diarization"))]
    fn extract_speaker_id(&mut self) -> Option<u32> {
        None
    }
}

fn load_session_triplet(model_dir: &Path) -> anyhow::Result<PooledSession> {
    let encoder = Session::builder()
        .and_then(|mut b| b.commit_from_file(model_dir.join("encoder-epoch-99-avg-1.onnx")))
        .with_context(|| "failed to load encoder.onnx")?;

    let decoder = Session::builder()
        .and_then(|mut b| b.commit_from_file(model_dir.join("decoder-epoch-99-avg-1.onnx")))
        .with_context(|| "failed to load decoder.onnx")?;

    let joiner = Session::builder()
        .and_then(|mut b| b.commit_from_file(model_dir.join("joiner-epoch-99-avg-1.onnx")))
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
    let tokens: Vec<String> = content
        .lines()
        .map(|s| s.split('\t').next().unwrap_or(s).to_string())
        .collect();
    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::time::timeout;

    #[tokio::test]
    #[ignore] // Requires model download
    async fn test_pool_checkout_blocks_when_saturated() {
        let model_dir = crate::model::default_model_dir();
        let pool = SessionPool::new(Path::new(&model_dir), 1).unwrap();

        let guard = pool.checkout().await.unwrap();

        // Second checkout should timeout because the only session is held.
        let result = timeout(Duration::from_secs(1), pool.checkout()).await;
        assert!(result.is_err(), "Expected timeout when pool is saturated");

        drop(guard);

        // After dropping the guard, checkout should succeed again.
        let _guard2 = pool.checkout().await.unwrap();
    }
}
