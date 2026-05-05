use crate::inference::Engine;
use crate::server::{OriginPolicy, RuntimeLimits, ServerConfig};
use axum::extract::{ws::WebSocket, DefaultBodyLimit, State, WebSocketUpgrade};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;

pub async fn run_with_config(
    engine: Engine,
    config: ServerConfig,
    _shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
) -> anyhow::Result<()> {
    let app = create_router(Arc::new(engine), &config);

    let addr: SocketAddr = format!("{}:{}", config.host, config.port).parse()?;
    let listener = TcpListener::bind(addr).await?;
    tracing::info!("server listening on http://{}", addr);

    axum::serve(listener, app).await?;
    Ok(())
}

fn create_router(engine: Arc<Engine>, config: &ServerConfig) -> Router {
    Router::new()
        .route("/health", get(health_handler))
        .route("/v1/transcribe", post(transcribe_handler))
        .route("/v1/ws", get(ws_handler))
        .layer(DefaultBodyLimit::max(config.limits.body_limit_bytes))
        .with_state(engine)
}

async fn health_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "model": "reazonspeech-k2-v2",
        "language": "ja"
    }))
}

async fn transcribe_handler(
    State(_engine): State<Arc<Engine>>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "text": "",
        "language": "ja"
    }))
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(_engine): State<Arc<Engine>>,
) -> Response {
    ws.on_upgrade(handle_socket)
}

async fn handle_socket(mut socket: WebSocket) {
    use crate::protocol::ServerMessage;
    use axum::extract::ws::Message;

    let ready = ServerMessage::Ready {
        protocol_version: crate::protocol::PROTOCOL_VERSION.to_string(),
        sample_rate: 16000,
        language: "ja".to_string(),
    };

    if let Ok(json) = serde_json::to_string(&ready) {
        let _ = socket.send(Message::Text(json.into())).await;
    }

    // TODO: implement VAD-based streaming loop
}
