use crate::error::NihosttError;
use ort::session::Session;
use ort::value::TensorRef;

/// Silero VAD wrapper.
pub struct SileroVad {
    session: Session,
    sample_rate: i64,
    state: Vec<f32>,
    context: Vec<f32>,
}

impl SileroVad {
    pub fn new(model_path: &std::path::Path) -> anyhow::Result<Self> {
        let session = Session::builder()
            .and_then(|mut b| b.commit_from_file(model_path))
            .map_err(|e| NihosttError::vad(format!("failed to load VAD: {e}")))?;

        // Silero VAD expects 512 samples for 16kHz
        let state_size = 2 * 128; // h + c for LSTM
        Ok(Self {
            session,
            sample_rate: 16000,
            state: vec![0.0f32; state_size],
            context: Vec::new(),
        })
    }

    /// Process a chunk of audio (16kHz, mono, f32).
    /// Returns speech probability [0.0, 1.0].
    pub fn process(&mut self, chunk: &[f32]) -> anyhow::Result<f32> {
        if chunk.len() < 512 {
            // Buffer small chunks
            self.context.extend_from_slice(chunk);
            if self.context.len() >= 512 {
                let frame: Vec<f32> = self.context.drain(..512).collect();
                return self.infer(&frame);
            }
            return Ok(0.0);
        }

        // Process full 512-sample frames
        let mut prob = 0.0f32;
        for frame in chunk.chunks(512) {
            if frame.len() == 512 {
                prob = self.infer(frame)?;
            } else {
                self.context.extend_from_slice(frame);
            }
        }
        Ok(prob)
    }

    fn infer(&mut self, samples: &[f32]) -> anyhow::Result<f32> {
        let input_tensor = TensorRef::from_array_view(([1_usize, 512], samples))?;

        let sr_data = [self.sample_rate];
        let sr_tensor = TensorRef::from_array_view(([1_usize], sr_data.as_slice()))?;

        let state_tensor = TensorRef::from_array_view(([2_usize, 128], self.state.as_slice()))?;

        let outputs = self
            .session
            .run(ort::inputs![input_tensor, sr_tensor, state_tensor])?;

        let (_prob_shape, prob_data) = outputs[0].try_extract_tensor::<f32>()?;
        let prob = prob_data.first().copied().unwrap_or(0.0);

        // Update state
        let (_state_shape, new_state) = outputs[1].try_extract_tensor::<f32>()?;
        self.state.clear();
        self.state.extend(new_state.iter().copied());

        Ok(prob)
    }

    pub fn reset(&mut self) {
        self.state.fill(0.0);
        self.context.clear();
    }
}
