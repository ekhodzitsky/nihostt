use std::path::Path;

use polyvoice::{DiarizationConfig, EcapaTdnnExtractor, OnlineDiarizer, SampleRate};

/// Holds the online diarizer and its embedding extractor.
///
/// If no supported speaker-embedding model is present on disk this will be `None`
/// and diarization is silently disabled (same behaviour as before).
pub struct DiarizationState {
    diarizer: OnlineDiarizer,
    extractor: EcapaTdnnExtractor,
}

impl DiarizationState {
    pub fn load(model_dir: &Path) -> Option<Self> {
        // 1. Try ECAPA-TDNN first (modern default, 192-d embeddings).
        let ecapa_path = model_dir.join("ecapa_tdnn.onnx");
        if ecapa_path.exists() {
            match Self::load_ecapa(&ecapa_path) {
                Ok(state) => return Some(state),
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        path = %ecapa_path.display(),
                        "failed to load ecapa_tdnn.onnx"
                    );
                }
            }
        }

        // 2. Fallback to legacy WeSpeaker ResNet34 (256-d embeddings).
        let wespeaker_path = model_dir.join("wespeaker_resnet34.onnx");
        if wespeaker_path.exists() {
            match Self::load_wespeaker(&wespeaker_path) {
                Ok(state) => return Some(state),
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        path = %wespeaker_path.display(),
                        "failed to load wespeaker_resnet34.onnx"
                    );
                }
            }
        }

        tracing::info!(
            "speaker embedding model not found (tried ecapa_tdnn.onnx, wespeaker_resnet34.onnx); \
             diarization disabled"
        );
        None
    }

    fn load_ecapa(path: &Path) -> anyhow::Result<Self> {
        // ECAPA-TDNN from SpeechBrain produces 192-d embeddings by default.
        const EMBEDDING_DIM: usize = 192;
        const POOL_SIZE: usize = 1;

        let extractor = EcapaTdnnExtractor::new(path, EMBEDDING_DIM, POOL_SIZE)?;
        let sample_rate = SampleRate::new(16000).expect("16000 is a valid sample rate");
        let config = DiarizationConfig {
            threshold: 0.25,
            max_speakers: 4,
            window_secs: 1.5,
            hop_secs: 0.75,
            min_speech_secs: 0.25,
            max_gap_secs: 0.5,
            sample_rate,
        };
        let diarizer = OnlineDiarizer::new(config);

        Ok(Self {
            diarizer,
            extractor,
        })
    }

    fn load_wespeaker(path: &Path) -> anyhow::Result<Self> {
        // WeSpeaker ResNet34: 256-d embeddings, fbank input (same as ECAPA).
        const EMBEDDING_DIM: usize = 256;
        const POOL_SIZE: usize = 1;

        let extractor = EcapaTdnnExtractor::new(path, EMBEDDING_DIM, POOL_SIZE)?;
        let sample_rate = SampleRate::new(16000).expect("16000 is a valid sample rate");
        let config = DiarizationConfig {
            threshold: 0.25,
            max_speakers: 4,
            window_secs: 1.5,
            hop_secs: 0.75,
            min_speech_secs: 0.25,
            max_gap_secs: 0.5,
            sample_rate,
        };
        let diarizer = OnlineDiarizer::new(config);

        Ok(Self {
            diarizer,
            extractor,
        })
    }

    /// Feed a chunk of 16 kHz mono f32 audio into the online diarizer.
    pub fn feed(&mut self, samples: &[f32]) {
        // `feed` returns completed segments; we are only interested in
        // `current_speaker()` which is updated after every internal window.
        if let Err(e) = self.diarizer.feed(samples, &self.extractor) {
            tracing::warn!(error = %e, "diarizer feed failed");
        }
    }

    /// Return the currently active speaker ID, if any.
    pub fn current_speaker(&self) -> Option<u32> {
        self.diarizer.current_speaker().map(|s| s.0)
    }

    /// Flush any trailing audio and return the final segment.
    pub fn flush(&mut self) -> Option<u32> {
        match self.diarizer.flush(&self.extractor) {
            Ok(Some(_seg)) => self.diarizer.current_speaker().map(|s| s.0),
            Ok(None) => self.diarizer.current_speaker().map(|s| s.0),
            Err(e) => {
                tracing::warn!(error = %e, "diarizer flush failed");
                self.diarizer.current_speaker().map(|s| s.0)
            }
        }
    }
}
