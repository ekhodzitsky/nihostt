use std::fmt;

/// Public error type for the nihostt API.
#[derive(Debug, thiserror::Error)]
pub enum NihosttError {
    #[error("inference failed: {0}")]
    Inference(String),

    #[error("audio decode error: {0}")]
    AudioDecode(String),

    #[error("model not found: {0}")]
    ModelNotFound(String),

    #[error("invalid audio format: {0}")]
    InvalidFormat(String),

    #[error("session pool saturated")]
    PoolSaturated,

    #[error("VAD error: {0}")]
    Vad(String),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl NihosttError {
    pub fn inference(msg: impl Into<String>) -> Self {
        Self::Inference(msg.into())
    }

    pub fn audio_decode(msg: impl Into<String>) -> Self {
        Self::AudioDecode(msg.into())
    }

    pub fn model_not_found(msg: impl Into<String>) -> Self {
        Self::ModelNotFound(msg.into())
    }

    pub fn vad(msg: impl Into<String>) -> Self {
        Self::Vad(msg.into())
    }
}

/// Serialize errors for the WebSocket / REST wire format.
impl serde::Serialize for NihosttError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}
