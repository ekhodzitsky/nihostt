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
    Partial { text: String },
    #[serde(rename = "final")]
    Final {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        speaker_id: Option<u32>,
    },
    #[serde(rename = "error")]
    Error {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        retry_after_ms: Option<u64>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_final_message_serializes_speaker_id_when_present() {
        let msg = ServerMessage::Final {
            text: "hello".to_string(),
            speaker_id: Some(2),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"text\":\"hello\""));
        assert!(json.contains("\"speaker_id\":2"));
    }

    #[test]
    fn test_final_message_omits_speaker_id_when_none() {
        let msg = ServerMessage::Final {
            text: "hello".to_string(),
            speaker_id: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"text\":\"hello\""));
        assert!(!json.contains("speaker_id"));
    }
}

/// WebSocket messages sent from client to server.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum ClientMessage {
    #[serde(rename = "configure")]
    Configure { sample_rate: Option<u32> },
    #[serde(rename = "audio")]
    Audio { data: Vec<u8> },
    #[serde(rename = "stop")]
    Stop,
}
