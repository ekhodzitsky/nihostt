//! HTTP handlers for REST API and WebSocket endpoints.

use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Multipart, State};
use axum::http::{HeaderMap, HeaderValue, Method, Request, StatusCode, header};
use axum::middleware::Next;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use axum::{Router, extract::DefaultBodyLimit};
use futures_util::{SinkExt, StreamExt};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::time::{Duration, Instant, timeout};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;

use crate::inference::{Engine, ProcessResult};
use crate::protocol::{ClientMessage, ServerMessage};
use crate::server::{AuthConfig, OriginPolicy, ServerConfig};

static TEMP_UPLOAD_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Shared application state passed to all handlers.
#[derive(Clone)]
pub struct AppState {
    pub engine: Arc<Engine>,
    pub limits: crate::server::RuntimeLimits,
    pub rate_limiter: Option<Arc<crate::server::rate_limit::RateLimiter>>,
    pub trust_proxy: bool,
    pub origin_policy: OriginPolicy,
    pub auth: AuthConfig,
    pub metrics_enabled: bool,
    pub shutdown: CancellationToken,
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

    let cancel = CancellationToken::new();
    let state = AppState {
        engine: Arc::new(engine),
        limits: config.limits.clone(),
        rate_limiter,
        trust_proxy: config.trust_proxy,
        origin_policy: config.origin_policy,
        auth: config.auth,
        metrics_enabled: config.metrics_enabled,
        shutdown: cancel.clone(),
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
        cancel.cancel();
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
    let (port, tx) = run_on_random_port_with_shutdown(engine, limits).await?;
    // Prevent graceful shutdown — old behaviour for tests that don't need
    // a shutdown handle. Leak the sender so the receiver never fires.
    let _ = Box::leak(Box::new(tx));
    Ok(port)
}

/// Start the server on a random available port with a shutdown handle.
/// Returns (port, cancellation_token).
pub async fn run_on_random_port_with_shutdown(
    engine: Engine,
    limits: crate::server::RuntimeLimits,
) -> anyhow::Result<(u16, CancellationToken)> {
    let rate_limiter = if limits.rate_limit_per_minute > 0 {
        Some(Arc::new(crate::server::rate_limit::RateLimiter::new(
            limits.rate_limit_per_minute,
            limits.rate_limit_burst,
        )))
    } else {
        None
    };

    let cancel = CancellationToken::new();
    let state = AppState {
        engine: Arc::new(engine),
        limits: limits.clone(),
        rate_limiter,
        trust_proxy: false,
        origin_policy: OriginPolicy {
            allow_any: false,
            allowed_origins: Vec::new(),
        },
        auth: AuthConfig::default(),
        metrics_enabled: false,
        shutdown: cancel.clone(),
    };
    let app = create_router(state).into_make_service_with_connect_info::<SocketAddr>();
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    // Leak the sender so the receiver never fires — server runs until
    // the drain timeout or external cancellation.
    let _ = Box::leak(Box::new(tx));

    let drain_secs = limits.shutdown_drain_secs;
    let cancel_clone = cancel.clone();
    let cancel_drain = cancel.clone();
    tokio::spawn(async move {
        let server = axum::serve(listener, app).with_graceful_shutdown(async move {
            let _ = rx.await;
            cancel_clone.cancel();
        });
        match tokio::time::timeout(Duration::from_secs(drain_secs), server).await {
            Ok(result) => {
                if let Err(e) = result {
                    tracing::error!("server error: {e}");
                }
            }
            Err(_) => {
                tracing::info!("shutdown drain timeout expired");
                cancel_drain.cancel();
            }
        }
    });

    Ok((port, cancel))
}

fn create_router(state: AppState) -> Router {
    let state = Arc::new(state);
    let mut router = Router::new()
        .route("/health", get(health_handler))
        .route("/ready", get(ready_handler))
        .route(
            "/v1/transcribe",
            post(transcribe_handler).options(preflight_handler),
        )
        .route(
            "/v1/transcribe/stream",
            post(transcribe_stream_handler).options(preflight_handler),
        )
        .route("/v1/ws", get(ws_handler).options(preflight_handler));

    if state.metrics_enabled {
        router = router.route("/metrics", get(metrics_handler));
    }

    router
        .layer(DefaultBodyLimit::max(state.limits.body_limit_bytes))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            origin_layer,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth_layer,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            rate_limit_layer,
        ))
        .with_state(state)
}

async fn write_upload_to_temp_file(
    prefix: &str,
    bytes: &[u8],
) -> std::io::Result<std::path::PathBuf> {
    for _ in 0..16 {
        let counter = TEMP_UPLOAD_COUNTER.fetch_add(1, Ordering::Relaxed);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "{prefix}_{}_{}_{}.upload",
            std::process::id(),
            now,
            counter
        ));

        let mut file = match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await
        {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        };

        if let Err(e) = file.write_all(bytes).await {
            let _ = tokio::fs::remove_file(&path).await;
            return Err(e);
        }
        if let Err(e) = file.flush().await {
            let _ = tokio::fs::remove_file(&path).await;
            return Err(e);
        }
        return Ok(path);
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not allocate a unique upload temp path",
    ))
}

async fn rate_limit_layer(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    if req.method() == Method::OPTIONS
        || req.uri().path() == "/health"
        || req.uri().path() == "/ready"
        || req.uri().path() == "/metrics"
    {
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

async fn auth_layer(
    State(state): State<Arc<AppState>>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    if !auth_required(req.method(), req.uri().path(), &state.auth) {
        return next.run(req).await;
    }

    if request_is_authorized(req.headers(), &state.auth) {
        return next.run(req).await;
    }

    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"))],
        Json(serde_json::json!({"code": "unauthorized"})),
    )
        .into_response()
}

fn auth_required(method: &Method, path: &str, auth: &AuthConfig) -> bool {
    auth.is_enabled() && *method != Method::OPTIONS && path != "/health" && path != "/ready"
}

fn request_is_authorized(headers: &HeaderMap, auth: &AuthConfig) -> bool {
    extract_api_key(headers).is_some_and(|candidate| {
        auth.api_keys
            .iter()
            .any(|allowed| constant_time_eq(candidate.as_bytes(), allowed.as_bytes()))
    })
}

fn extract_api_key(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("x-api-key")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .and_then(extract_bearer_token)
        })
}

fn extract_bearer_token(value: &str) -> Option<&str> {
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    if token.is_empty() { None } else { Some(token) }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let max_len = left.len().max(right.len());
    let mut diff = left.len() ^ right.len();
    for i in 0..max_len {
        let left_byte = left.get(i).copied().unwrap_or(0);
        let right_byte = right.get(i).copied().unwrap_or(0);
        diff |= usize::from(left_byte ^ right_byte);
    }
    diff == 0
}

async fn origin_layer(
    State(state): State<Arc<AppState>>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let origin = match req.headers().get(header::ORIGIN) {
        Some(value) => match value.to_str() {
            Ok(origin) => Some(origin.to_string()),
            Err(_) => {
                return (
                    StatusCode::FORBIDDEN,
                    Json(serde_json::json!({"code": "origin_forbidden"})),
                )
                    .into_response();
            }
        },
        None => None,
    };

    let allowed_origin = match origin.as_deref() {
        Some(origin) if origin_is_allowed(origin, &state.origin_policy) => {
            allowed_origin_header(origin, &state.origin_policy)
        }
        Some(_) => {
            return (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({"code": "origin_forbidden"})),
            )
                .into_response();
        }
        None => None,
    };

    if req.method() == Method::OPTIONS {
        let mut response = StatusCode::NO_CONTENT.into_response();
        apply_cors_headers(response.headers_mut(), allowed_origin.as_ref());
        return response;
    }

    let mut response = next.run(req).await;
    apply_cors_headers(response.headers_mut(), allowed_origin.as_ref());
    response
}

fn origin_is_allowed(origin: &str, policy: &OriginPolicy) -> bool {
    policy.allow_any
        || policy
            .allowed_origins
            .iter()
            .any(|allowed| allowed == origin)
}

fn allowed_origin_header(origin: &str, policy: &OriginPolicy) -> Option<HeaderValue> {
    if policy.allow_any {
        Some(HeaderValue::from_static("*"))
    } else {
        HeaderValue::from_str(origin).ok()
    }
}

fn apply_cors_headers(headers: &mut axum::http::HeaderMap, origin: Option<&HeaderValue>) {
    let Some(origin) = origin else {
        return;
    };

    headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin.clone());
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET,POST,OPTIONS"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("authorization,content-type,x-api-key"),
    );
    headers.append(header::VARY, HeaderValue::from_static("origin"));
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

async fn ready_handler(State(state): State<Arc<AppState>>) -> Response {
    if state.engine.pool.is_ready() {
        Json(serde_json::json!({
            "status": "ready",
            "model": "reazonspeech-k2-v2",
            "language": "ja"
        }))
        .into_response()
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "status": "not_ready",
                "reason": "inference pool saturated"
            })),
        )
            .into_response()
    }
}

async fn preflight_handler() -> StatusCode {
    StatusCode::NO_CONTENT
}

async fn metrics_handler(State(state): State<Arc<AppState>>) -> Response {
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        render_metrics(state.engine.pool.is_ready()),
    )
        .into_response()
}

fn render_metrics(pool_ready: bool) -> String {
    let pool_ready = u8::from(pool_ready);
    format!(
        "# HELP nihostt_up Whether the nihostt process is serving requests.\n\
         # TYPE nihostt_up gauge\n\
         nihostt_up 1\n\
         # HELP nihostt_pool_ready Whether at least one inference session is available.\n\
         # TYPE nihostt_pool_ready gauge\n\
         nihostt_pool_ready {pool_ready}\n"
    )
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
    let temp_path = write_upload_to_temp_file("nihostt_upload", &bytes)
        .await
        .map_err(|e| {
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
                let _ = tokio::fs::remove_file(&temp_path).await;
                return Err((
                    StatusCode::SERVICE_UNAVAILABLE,
                    [("retry-after", "30")],
                    Json(serde_json::json!({"code": "pool_saturated"})),
                )
                    .into_response());
            }
            Err(_) => {
                tracing::warn!("pool checkout timed out");
                let _ = tokio::fs::remove_file(&temp_path).await;
                return Err((
                    StatusCode::SERVICE_UNAVAILABLE,
                    [("retry-after", "30")],
                    Json(serde_json::json!({"code": "timeout"})),
                )
                    .into_response());
            }
        };

    let join_result =
        tokio::task::spawn_blocking(move || engine.transcribe_file(&path_str, guard.session()))
            .await;

    // Clean up temp file regardless of success or failure.
    let _ = tokio::fs::remove_file(&temp_path).await;

    let result = join_result
        .map_err(|e| {
            tracing::error!("transcription task join error: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        })?
        .map_err(|e| {
            tracing::error!("transcription error: {e}");
            StatusCode::UNPROCESSABLE_ENTITY.into_response()
        })?;

    let mut response = serde_json::json!({
        "text": result.text,
        "language": "ja"
    });
    if let Some(conf) = result.confidence {
        response["confidence"] = serde_json::json!(conf);
    }
    Ok(Json(response))
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

    let temp_path = write_upload_to_temp_file("nihostt_stream", &bytes)
        .await
        .map_err(|e| {
            tracing::error!("failed to write temp file: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

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
        let guard =
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

        let join_result = tokio::task::spawn_blocking(move || {
            let mut guard = guard;
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
                            if tx.blocking_send(Ok(event)).is_err() {
                                // Receiver dropped (client disconnected).
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("chunk transcription error: {e}");
                        let _ = tx.blocking_send(Err(format!("transcription: {e}")));
                        break;
                    }
                }
            }

            let final_text = all_texts.join(" ");
            let _ = tx.blocking_send(Ok(SseEvent {
                text: final_text,
                final_: true,
            }));
        })
        .await;

        if let Err(e) = join_result {
            tracing::error!("SSE transcription task join error: {e}");
        }

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

    let mut streaming_session = match state.engine.create_streaming_session() {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, "failed to create streaming session");
            let err = ServerMessage::Error {
                message: "server error: failed to initialise streaming session".to_string(),
                retry_after_ms: Some(30000),
            };
            if let Ok(json) = serde_json::to_string(&err) {
                let _ = socket.send(WsMessage::Text(json.into())).await;
            }
            return;
        }
    };
    let mut client_sample_rate: u32 = 48000;
    let session_start = Instant::now();
    let max_session = Duration::from_secs(state.limits.max_session_secs);
    let idle_timeout = Duration::from_secs(state.limits.idle_timeout_secs);
    let frame_max = state.limits.ws_frame_max_bytes;

    loop {
        let recv_fut = timeout(idle_timeout, socket.recv());
        let msg = tokio::select! {
            biased;
            _ = state.shutdown.cancelled() => {
                tracing::info!("websocket shutting down due to server stop");
                break;
            }
            result = recv_fut => match result {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn write_upload_to_temp_file_creates_unique_paths() {
        let first = write_upload_to_temp_file("nihostt_test_upload", b"first")
            .await
            .expect("first temp upload write should succeed");
        let second = write_upload_to_temp_file("nihostt_test_upload", b"second")
            .await
            .expect("second temp upload write should succeed");

        assert_ne!(first, second, "concurrent uploads must not share a path");
        assert_eq!(tokio::fs::read(&first).await.unwrap(), b"first");
        assert_eq!(tokio::fs::read(&second).await.unwrap(), b"second");

        let _ = tokio::fs::remove_file(first).await;
        let _ = tokio::fs::remove_file(second).await;
    }

    #[test]
    fn origin_policy_denies_unknown_origins_by_default() {
        let policy = crate::server::OriginPolicy {
            allow_any: false,
            allowed_origins: vec!["https://app.example".to_string()],
        };

        assert!(origin_is_allowed("https://app.example", &policy));
        assert!(!origin_is_allowed("https://evil.example", &policy));
    }

    #[test]
    fn render_metrics_exports_pool_readiness() {
        let body = render_metrics(true);

        assert!(body.contains("nihostt_up 1"));
        assert!(body.contains("nihostt_pool_ready 1"));
    }

    #[test]
    fn auth_required_only_when_enabled_and_not_probe_or_preflight() {
        let auth = AuthConfig {
            api_keys: vec!["secret".to_string()],
        };

        assert!(auth_required(&Method::POST, "/v1/transcribe", &auth));
        assert!(auth_required(&Method::GET, "/metrics", &auth));
        assert!(!auth_required(&Method::GET, "/health", &auth));
        assert!(!auth_required(&Method::GET, "/ready", &auth));
        assert!(!auth_required(&Method::OPTIONS, "/v1/transcribe", &auth));
        assert!(!auth_required(
            &Method::POST,
            "/v1/transcribe",
            &AuthConfig::default()
        ));
    }

    #[test]
    fn request_is_authorized_accepts_bearer_or_x_api_key() {
        let auth = AuthConfig {
            api_keys: vec!["secret".to_string()],
        };
        let mut bearer_headers = HeaderMap::new();
        bearer_headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer secret"),
        );
        let mut api_key_headers = HeaderMap::new();
        api_key_headers.insert("x-api-key", HeaderValue::from_static("secret"));

        assert!(request_is_authorized(&bearer_headers, &auth));
        assert!(request_is_authorized(&api_key_headers, &auth));
    }

    #[test]
    fn request_is_authorized_rejects_missing_or_wrong_key() {
        let auth = AuthConfig {
            api_keys: vec!["secret".to_string()],
        };
        let mut wrong_headers = HeaderMap::new();
        wrong_headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer nope"),
        );

        assert!(!request_is_authorized(&HeaderMap::new(), &auth));
        assert!(!request_is_authorized(&wrong_headers, &auth));
    }

    #[test]
    fn constant_time_eq_handles_equal_different_and_prefix_values() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"secreu"));
        assert!(!constant_time_eq(b"secret", b"secret-longer"));
        assert!(!constant_time_eq(b"secret-longer", b"secret"));
    }
}
