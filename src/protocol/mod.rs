use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: &str = "1.0";

/// WebSocket messages sent from server to client.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum ServerMessage {
    #[serde(rename = "ready")]
    Ready {
        protocol_version: String,
        sample_rate: u32,
        language: String,
    },
    #[serde(rename = "partial")]
    Partial {
        text: String,
    },
    #[serde(rename = "final")]
    Final {
        text: String,
    },
    #[serde(rename = "error")]
    Error {
        message: String,
    },
}

/// WebSocket messages sent from client to server.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum ClientMessage {
    #[serde(rename = "configure")]
    Configure {
        sample_rate: Option<u32>,
    },
    #[serde(rename = "audio")]
    Audio {
        data: Vec<u8>,
    },
    #[serde(rename = "stop")]
    Stop,
}
