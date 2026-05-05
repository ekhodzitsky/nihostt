//! HTTP handlers for REST API and WebSocket endpoints.

use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Multipart, State};
use axum::http::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use axum::{Router, extract::DefaultBodyLimit};
use futures_util::{SinkExt, StreamExt};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::time::{Duration, Instant, timeout};
use tokio_stream::wrappers::ReceiverStream;

use crate::inference::{Engine, ProcessResult};
use crate::protocol::{ClientMessage, ServerMessage};
use crate::server::ServerConfig;

/// Shared application state passed to all handlers.
#[derive(Clone)]
pub struct AppState {
    pub engine: Arc<Engine>,
    pub limits: crate::server::RuntimeLimits,
    pub rate_limiter: Option<Arc<crate::server::rate_limit::RateLimiter>>,
    pub trust_proxy: bool,
}

/// Start the server with the given configuration.
pub async fn run_with_config(
    engine: Engine,
    config: ServerConfig,
    shutdown_rx: Option<tokio::sync::oneshot::Receiver<()>>,
) -> anyhow::Result<()> {
    let rate_limiter = if config.limits.rate_limit_per_minute > 0 {
        Some(Arc::new(crate::server::rate_limit::RateLimiter::new(
            config.limits.rate_limit_per_minute,
            config.limits.rate_limit_burst,
        )))
    } else {
        None
    };

    let state = AppState {
        engine: Arc::new(engine),
        limits: config.limits.clone(),
        rate_limiter,
        trust_proxy: config.trust_proxy,
    };
    let app = create_router(state).into_make_service_with_connect_info::<SocketAddr>();
    let addr: SocketAddr = format!("{}:{}", config.host, config.port).parse()?;
    let listener = TcpListener::bind(addr).await?;
    tracing::info!("server listening on http://{}", addr);

    let server = axum::serve(listener, app).with_graceful_shutdown(async move {
        if let Some(rx) = shutdown_rx {
            let _ = rx.await;
        } else {
            std::future::pending::<()>().await;
        }
    });
    server.await?;
    Ok(())
}

/// Start the server on a random available port and return the bound port.
/// Used by E2E tests to eliminate the TOCTOU race between port discovery
/// and server bind.
pub async fn run_on_random_port(
    engine: Engine,
    limits: crate::server::RuntimeLimits,
) -> anyhow::Result<u16> {
    let (port, _) = run_on_random_port_with_shutdown(engine, limits).await?;
    Ok(port)
}

/// Start the server on a random available port with a shutdown handle.
/// Returns (port, shutdown_sender).
pub async fn run_on_random_port_with_shutdown(
    engine: Engine,
    limits: crate::server::RuntimeLimits,
) -> anyhow::Result<(u16, tokio::sync::oneshot::Sender<()>)> {
    let rate_limiter = if limits.rate_limit_per_minute > 0 {
        Some(Arc::new(crate::server::rate_limit::RateLimiter::new(
            limits.rate_limit_per_minute,
            limits.rate_limit_burst,
        )))
    } else {
        None
    };

    let state = AppState {
        engine: Arc::new(engine),
        limits: limits.clone(),
        rate_limiter,
        trust_proxy: false,
    };
    let app = create_router(state).into_make_service_with_connect_info::<SocketAddr>();
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let (tx, rx) = tokio::sync::oneshot::channel();

    let drain_secs = limits.shutdown_drain_secs;
    tokio::spawn(async move {
        let server = axum::serve(listener, app).with_graceful_shutdown(async move {
            let _ = rx.await;
        });
        match tokio::time::timeout(Duration::from_secs(drain_secs), server).await {
            Ok(result) => {
                if let Err(e) = result {
                    tracing::error!("server error: {e}");
                }
            }
            Err(_) => {
                tracing::info!("shutdown drain timeout expired");
            }
        }
    });

    Ok((port, tx))
}

fn create_router(state: AppState) -> Router {
    let state = Arc::new(state);
    Router::new()
        .route("/health", get(health_handler))
        .route("/v1/transcribe", post(transcribe_handler))
        .route("/v1/transcribe/stream", post(transcribe_stream_handler))
        .route("/v1/ws", get(ws_handler))
        .layer(DefaultBodyLimit::max(state.limits.body_limit_bytes))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            rate_limit_layer,
        ))
        .with_state(state)
}

async fn rate_limit_layer(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    if req.uri().path() == "/health" {
        return next.run(req).await;
    }

    if let Some(ref limiter) = state.rate_limiter {
        let ip = if state.trust_proxy {
            req.headers()
                .get("x-forwarded-for")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.split(',').next())
                .and_then(|s| s.trim().parse::<std::net::IpAddr>().ok())
                .unwrap_or(addr.ip())
        } else {
            addr.ip()
        };

        if !limiter.check(ip) {
            let retry_after = if state.limits.rate_limit_per_minute > 0 {
                (60.0 / state.limits.rate_limit_per_minute as f64).ceil() as u64
            } else {
                60
            };
            return (
                StatusCode::TOO_MANY_REQUESTS,
                [("retry-after", retry_after.to_string())],
                Json(serde_json::json!({"code": "rate_limited"})),
            )
                .into_response();
        }
    }

    next.run(req).await
}

// ---------------------------------------------------------------------------
// Health
// ---------------------------------------------------------------------------

async fn health_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "model": "reazonspeech-k2-v2",
        "language": "ja"
    }))
}

// ---------------------------------------------------------------------------
// REST transcribe
// ---------------------------------------------------------------------------

async fn transcribe_handler(
    State(state): State<Arc<AppState>>,
    mut multipart: Multipart,
) -> Result<Json<serde_json::Value>, Response> {
    // Extract the first file field from the multipart body.
    let mut file_bytes = None;
    while let Some(field) = multipart.next_field().await.map_err(|e| {
        tracing::error!("multipart parse error: {e}");
        StatusCode::BAD_REQUEST.into_response()
    })? {
        if field.file_name().is_some() {
            file_bytes = Some(field.bytes().await.map_err(|e| {
                tracing::error!("read upload bytes error: {e}");
                StatusCode::BAD_REQUEST.into_response()
            })?);
            break;
        }
    }

    let bytes = file_bytes.ok_or_else(|| {
        tracing::warn!("no file field in multipart upload");
        StatusCode::BAD_REQUEST.into_response()
    })?;

    if bytes.is_empty() {
        return Err(StatusCode::BAD_REQUEST.into_response());
    }

    // Write to a temporary file so symphonia can decode it.
    let temp_path = std::env::temp_dir().join(format!("nihostt_upload_{}.wav", std::process::id()));
    tokio::fs::write(&temp_path, &bytes).await.map_err(|e| {
        tracing::error!("failed to write temp file: {e}");
        StatusCode::INTERNAL_SERVER_ERROR.into_response()
    })?;

    let engine = state.engine.clone();
    let path_str = temp_path.to_string_lossy().to_string();

    let mut guard =
        match tokio::time::timeout(Duration::from_secs(30), engine.pool.checkout()).await {
            Ok(Ok(g)) => g,
            Ok(Err(e)) => {
                tracing::error!("pool checkout error: {e}");
                return Err((
                    StatusCode::SERVICE_UNAVAILABLE,
                    [("retry-after", "30")],
                    Json(serde_json::json!({"code": "pool_saturated"})),
                )
                    .into_response());
            }
            Err(_) => {
                tracing::warn!("pool checkout timed out");
                return Err((
                    StatusCode::SERVICE_UNAVAILABLE,
                    [("retry-after", "30")],
                    Json(serde_json::json!({"code": "timeout"})),
                )
                    .into_response());
            }
        };

    let result = engine.transcribe_file(&path_str, &mut guard);
    drop(guard);

    // Clean up temp file regardless of success or failure.
    let _ = tokio::fs::remove_file(&temp_path).await;

    let result = result.map_err(|e| {
        tracing::error!("transcription error: {e}");
        StatusCode::UNPROCESSABLE_ENTITY.into_response()
    })?;

    Ok(Json(serde_json::json!({
        "text": result.text,
        "language": "ja"
    })))
}

// ---------------------------------------------------------------------------
// SSE streaming transcribe
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct SseEvent {
    text: String,
    final_: bool,
}

async fn transcribe_stream_handler(
    State(state): State<Arc<AppState>>,
    mut multipart: Multipart,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>, StatusCode> {
    // Extract the first file field.
    let mut file_bytes = None;
    while let Some(field) = multipart.next_field().await.map_err(|e| {
        tracing::error!("multipart parse error: {e}");
        StatusCode::BAD_REQUEST
    })? {
        if field.file_name().is_some() {
            file_bytes = Some(field.bytes().await.map_err(|e| {
                tracing::error!("read upload bytes error: {e}");
                StatusCode::BAD_REQUEST
            })?);
            break;
        }
    }

    let bytes = file_bytes.ok_or_else(|| {
        tracing::warn!("no file field in multipart upload");
        StatusCode::BAD_REQUEST
    })?;

    if bytes.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let temp_path = std::env::temp_dir().join(format!("nihostt_stream_{}.wav", std::process::id()));
    if let Err(e) = tokio::fs::write(&temp_path, &bytes).await {
        tracing::error!("failed to write temp file: {e}");
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    // Decode audio (blocking I/O + decoding).
    let path_str = temp_path.to_string_lossy().to_string();
    let decode_result =
        tokio::task::spawn_blocking(move || crate::inference::audio::load_audio(&path_str))
            .await
            .map_err(|e| {
                tracing::error!("spawn_blocking join error: {e}");
                let _ = std::fs::remove_file(&temp_path);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;

    let (samples, sample_rate) = match decode_result {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("audio decode error: {e}");
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(StatusCode::UNPROCESSABLE_ENTITY);
        }
    };

    // Resample to 16kHz if necessary.
    let samples_16k = if sample_rate != 16000 {
        let resample_result = tokio::task::spawn_blocking(move || {
            crate::inference::audio::resample(&samples, sample_rate, 16000)
        })
        .await
        .map_err(|e| {
            tracing::error!("spawn_blocking join error: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
        match resample_result {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("resample error: {e}");
                let _ = tokio::fs::remove_file(&temp_path).await;
                return Err(StatusCode::UNPROCESSABLE_ENTITY);
            }
        }
    } else {
        samples
    };

    // Channel for streaming partial/final events from the inference task.
    let (tx, rx) = mpsc::channel::<Result<SseEvent, String>>(16);
    let engine = state.engine.clone();
    let temp_path_clone = temp_path.clone();

    tokio::spawn(async move {
        let mut guard =
            match tokio::time::timeout(Duration::from_secs(30), engine.pool.checkout()).await {
                Ok(Ok(g)) => g,
                Ok(Err(e)) => {
                    tracing::error!("pool checkout error: {e}");
                    let _ = tx.send(Err(format!("pool checkout: {e}"))).await;
                    let _ = tokio::fs::remove_file(&temp_path_clone).await;
                    return;
                }
                Err(_) => {
                    tracing::warn!("pool checkout timed out");
                    let _ = tx.send(Err("pool checkout timed out".to_string())).await;
                    let _ = tokio::fs::remove_file(&temp_path_clone).await;
                    return;
                }
            };

        // 5-second sliding windows with 1-second overlap.
        let chunk_samples = 5 * 16000;
        let hop_samples = 4 * 16000;
        let mut all_texts: Vec<String> = Vec::new();

        for start in (0..samples_16k.len()).step_by(hop_samples) {
            let end = (start + chunk_samples).min(samples_16k.len());
            let chunk = &samples_16k[start..end];
            if chunk.is_empty() {
                break;
            }

            match engine.transcribe_samples(chunk, &mut guard) {
                Ok(result) => {
                    if !result.text.is_empty() {
                        all_texts.push(result.text.clone());
                        let event = SseEvent {
                            text: result.text,
                            final_: false,
                        };
                        if tx.send(Ok(event)).await.is_err() {
                            // Receiver dropped (client disconnected).
                            break;
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("chunk transcription error: {e}");
                    let _ = tx.send(Err(format!("transcription: {e}"))).await;
                    break;
                }
            }
        }

        drop(guard);

        let final_text = all_texts.join(" ");
        let _ = tx
            .send(Ok(SseEvent {
                text: final_text,
                final_: true,
            }))
            .await;

        let _ = tokio::fs::remove_file(&temp_path_clone).await;
    });

    let stream = ReceiverStream::new(rx).map(|result| {
        let event = match result {
            Ok(evt) => {
                let data = serde_json::json!({"text": evt.text, "final": evt.final_});
                Event::default().data(data.to_string())
            }
            Err(msg) => {
                let data = serde_json::json!({"error": msg});
                Event::default().data(data.to_string())
            }
        };
        Ok::<_, Infallible>(event)
    });

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

// ---------------------------------------------------------------------------
// WebSocket
// ---------------------------------------------------------------------------

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, state: Arc<AppState>) {
    let ready = ServerMessage::Ready {
        protocol_version: crate::protocol::PROTOCOL_VERSION.to_string(),
        sample_rate: 16000,
        language: "ja".to_string(),
    };

    if let Ok(json) = serde_json::to_string(&ready)
        && socket.send(WsMessage::Text(json.into())).await.is_err()
    {
        return;
    }

    let mut streaming_session = state.engine.create_streaming_session();
    let mut client_sample_rate: u32 = 48000;
    let session_start = Instant::now();
    let max_session = Duration::from_secs(state.limits.max_session_secs);
    let idle_timeout = Duration::from_secs(state.limits.idle_timeout_secs);
    let frame_max = state.limits.ws_frame_max_bytes;

    loop {
        let recv_fut = timeout(idle_timeout, socket.recv());
        let msg = match recv_fut.await {
            Ok(Some(Ok(msg))) => msg,
            Ok(Some(Err(e))) => {
                tracing::warn!(error = %e, "websocket receive error");
                break;
            }
            Ok(None) => {
                tracing::debug!("websocket stream closed by client");
                break;
            }
            Err(_) => {
                tracing::info!("websocket idle timeout");
                break;
            }
        };

        if session_start.elapsed() > max_session {
            let err = ServerMessage::Error {
                message: "session exceeded maximum duration".to_string(),
                retry_after_ms: None,
            };
            if let Ok(json) = serde_json::to_string(&err) {
                let _ = socket.send(WsMessage::Text(json.into())).await;
            }
            break;
        }

        match msg {
            WsMessage::Text(text) => {
                match serde_json::from_str::<ClientMessage>(&text) {
                    Ok(ClientMessage::Configure {
                        sample_rate: Some(sr),
                    }) => {
                        client_sample_rate = sr;
                        tracing::debug!(sample_rate = sr, "client configured sample rate");
                    }
                    Ok(ClientMessage::Configure { sample_rate: None }) => {
                        // keep default
                    }
                    Ok(ClientMessage::Stop) => {
                        let mut sent_final = false;
                        if let Ok(Ok(mut guard)) = tokio::time::timeout(
                            Duration::from_secs(30),
                            state.engine.pool.checkout(),
                        )
                        .await
                        {
                            match streaming_session.finalize(&mut guard) {
                                Ok(results) => {
                                    for result in results {
                                        if let ProcessResult::Final(text, speaker_id) = result {
                                            let msg = ServerMessage::Final { text, speaker_id };
                                            if let Ok(json) = serde_json::to_string(&msg) {
                                                let _ =
                                                    socket.send(WsMessage::Text(json.into())).await;
                                            }
                                            sent_final = true;
                                        }
                                    }
                                }
                                Err(e) => {
                                    tracing::error!(error = %e, "finalize error");
                                    let err = ServerMessage::Error {
                                        message: "transcription failed".to_string(),
                                        retry_after_ms: None,
                                    };
                                    if let Ok(json) = serde_json::to_string(&err) {
                                        let _ = socket.send(WsMessage::Text(json.into())).await;
                                    }
                                }
                            }
                        }
                        if !sent_final {
                            let msg = ServerMessage::Final {
                                text: String::new(),
                                speaker_id: None,
                            };
                            if let Ok(json) = serde_json::to_string(&msg) {
                                let _ = socket.send(WsMessage::Text(json.into())).await;
                            }
                        }
                        break;
                    }
                    Ok(ClientMessage::Audio { .. }) => {
                        let err = ServerMessage::Error {
                            message: "audio frames must be sent as binary messages".to_string(),
                            retry_after_ms: None,
                        };
                        if let Ok(json) = serde_json::to_string(&err) {
                            let _ = socket.send(WsMessage::Text(json.into())).await;
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "invalid client message");
                        let err = ServerMessage::Error {
                            message: format!("invalid message: {e}"),
                            retry_after_ms: None,
                        };
                        if let Ok(json) = serde_json::to_string(&err) {
                            let _ = socket.send(WsMessage::Text(json.into())).await;
                        }
                    }
                }
            }
            WsMessage::Binary(data) => {
                if data.len() > frame_max {
                    let err = ServerMessage::Error {
                        message: "audio frame too large".to_string(),
                        retry_after_ms: None,
                    };
                    if let Ok(json) = serde_json::to_string(&err) {
                        let _ = socket.send(WsMessage::Text(json.into())).await;
                    }
                    continue;
                }

                let samples_f32: Vec<f32> = data
                    .chunks_exact(2)
                    .map(|c| {
                        let sample = i16::from_le_bytes([c[0], c[1]]);
                        sample as f32 / 32768.0
                    })
                    .collect();

                let samples_16k = if client_sample_rate != 16000 {
                    match crate::inference::audio::resample(&samples_f32, client_sample_rate, 16000)
                    {
                        Ok(s) => s,
                        Err(e) => {
                            tracing::error!(error = %e, "resample error");
                            let err = ServerMessage::Error {
                                message: "resampling failed".to_string(),
                                retry_after_ms: None,
                            };
                            if let Ok(json) = serde_json::to_string(&err) {
                                let _ = socket.send(WsMessage::Text(json.into())).await;
                            }
                            continue;
                        }
                    }
                } else {
                    samples_f32
                };

                let mut guard = match tokio::time::timeout(
                    Duration::from_secs(30),
                    state.engine.pool.checkout(),
                )
                .await
                {
                    Ok(Ok(g)) => g,
                    Ok(Err(e)) => {
                        tracing::warn!(error = %e, "pool saturated");
                        let err = ServerMessage::Error {
                            message: "server busy, try again later".to_string(),
                            retry_after_ms: Some(30000),
                        };
                        if let Ok(json) = serde_json::to_string(&err) {
                            let _ = socket.send(WsMessage::Text(json.into())).await;
                        }
                        continue;
                    }
                    Err(_) => {
                        tracing::warn!("pool checkout timed out");
                        let err = ServerMessage::Error {
                            message: "server busy, try again later".to_string(),
                            retry_after_ms: Some(30000),
                        };
                        if let Ok(json) = serde_json::to_string(&err) {
                            let _ = socket.send(WsMessage::Text(json.into())).await;
                        }
                        continue;
                    }
                };

                match streaming_session.process_chunk(&samples_16k, &mut guard) {
                    Ok(results) => {
                        for result in results {
                            let msg = match result {
                                ProcessResult::Partial(text) => ServerMessage::Partial { text },
                                ProcessResult::Final(text, speaker_id) => {
                                    ServerMessage::Final { text, speaker_id }
                                }
                                ProcessResult::Noop => continue,
                            };
                            if let Ok(json) = serde_json::to_string(&msg)
                                && socket.send(WsMessage::Text(json.into())).await.is_err()
                            {
                                return;
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "process_chunk error");
                        let err = ServerMessage::Error {
                            message: "audio processing failed".to_string(),
                            retry_after_ms: None,
                        };
                        if let Ok(json) = serde_json::to_string(&err) {
                            let _ = socket.send(WsMessage::Text(json.into())).await;
                        }
                    }
                }
            }
            WsMessage::Close(_) => {
                if let Ok(Ok(mut guard)) =
                    tokio::time::timeout(Duration::from_secs(30), state.engine.pool.checkout())
                        .await
                {
                    let _ = streaming_session.finalize(&mut guard);
                }
                break;
            }
            _ => {}
        }
    }

    // Best-effort finalisation on disconnect / timeout.
    if let Ok(Ok(mut guard)) =
        tokio::time::timeout(Duration::from_secs(30), state.engine.pool.checkout()).await
        && let Ok(results) = streaming_session.finalize(&mut guard)
    {
        for result in results {
            if let ProcessResult::Final(text, speaker_id) = result {
                let msg = ServerMessage::Final { text, speaker_id };
                if let Ok(json) = serde_json::to_string(&msg) {
                    let _ = socket.send(WsMessage::Text(json.into())).await;
                }
            }
        }
    }

    // Graceful close: send Close frame before dropping the socket.
    let _ = socket.close().await;
}
