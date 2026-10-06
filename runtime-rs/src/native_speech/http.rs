use super::service::SpeechService;
use crate::ApiError;
use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Query, State},
    http::{HeaderMap, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{get, post},
    Json, Router,
};
use futures::{future::BoxFuture, Stream, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    convert::Infallible,
    path::PathBuf,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use tokio_util::sync::CancellationToken;
pub type SpeechSessionResolver = Arc<
    dyn Fn(String) -> BoxFuture<'static, std::result::Result<Option<PathBuf>, ApiError>>
        + Send
        + Sync,
>;
struct HttpState {
    speech: Arc<SpeechService>,
    resolve: SpeechSessionResolver,
}
pub fn router<S: Clone + Send + Sync + 'static>(
    speech: Arc<SpeechService>,
    resolve: SpeechSessionResolver,
) -> Router<S> {
    Router::new()
        .route("/api/settings/speech", get(settings).patch(update_settings))
        .route("/api/speech/terms", get(terms))
        .route("/api/speech/models", get(models))
        .route("/api/speech/models/download", post(download))
        .route("/api/speech/models/cancel", post(cancel_download))
        .route("/api/speech/session", post(prepare))
        .route("/api/speech/synthesize", post(synthesize))
        .route("/api/speech/cancel", post(cancel))
        .route("/api/speech/transcribe", post(transcribe))
        .route("/api/speech/stream/start", post(start))
        .route("/api/speech/stream/chunk", post(chunk))
        .route("/api/speech/stream/finish", post(finish))
        .route("/api/speech/stream/cancel", post(cancel_stream))
        .layer(DefaultBodyLimit::max(32_000_000))
        .with_state(Arc::new(HttpState { speech, resolve }))
}
fn fields(value: &Value, allowed: &[&str]) -> std::result::Result<(), ApiError> {
    let object = value
        .as_object()
        .ok_or_else(|| ApiError::bad_request("Invalid speech request."))?;
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(ApiError::bad_request("Invalid speech request."));
    }
    Ok(())
}
async fn settings(
    State(state): State<Arc<HttpState>>,
) -> std::result::Result<Json<Value>, ApiError> {
    Ok(Json(state.speech.terms.settings()?))
}
async fn update_settings(
    State(state): State<Arc<HttpState>>,
    Json(input): Json<Value>,
) -> std::result::Result<Json<Value>, ApiError> {
    Ok(Json(state.speech.terms.update(&input)?))
}
#[derive(Deserialize)]
struct TermsQuery {
    #[serde(rename = "sessionId", default)]
    session_id: String,
}
async fn workspace_terms(
    state: &HttpState,
    id: &str,
) -> std::result::Result<Vec<String>, ApiError> {
    let cwd = if id.trim().is_empty() {
        None
    } else {
        (state.resolve)(id.trim().to_owned()).await?
    };
    Ok(state.speech.terms.workspace(cwd.as_deref())?)
}
async fn terms(
    State(state): State<Arc<HttpState>>,
    Query(query): Query<TermsQuery>,
) -> std::result::Result<Json<Value>, ApiError> {
    Ok(Json(
        json!({"terms":workspace_terms(&state,&query.session_id).await?}),
    ))
}
async fn models(State(state): State<Arc<HttpState>>) -> std::result::Result<Json<Value>, ApiError> {
    Ok(Json(state.speech.downloads.list().await?))
}
async fn download(
    State(state): State<Arc<HttpState>>,
    Json(input): Json<Value>,
) -> std::result::Result<Json<Value>, ApiError> {
    fields(&input, &["modelId"])?;
    Ok(Json(
        state
            .speech
            .downloads
            .start(input["modelId"].as_str().unwrap_or(""))?,
    ))
}
async fn cancel_download(
    State(state): State<Arc<HttpState>>,
    Json(input): Json<Value>,
) -> std::result::Result<Json<Value>, ApiError> {
    fields(&input, &["modelId"])?;
    Ok(Json(
        state
            .speech
            .downloads
            .cancel(input["modelId"].as_str().unwrap_or(""))
            .await?,
    ))
}
struct Lease {
    service: Arc<SpeechService>,
    id: String,
    token: CancellationToken,
}
impl Drop for Lease {
    fn drop(&mut self) {
        self.token.cancel();
        self.service.release(&self.id);
    }
}
struct SessionEvents {
    events: Pin<Box<dyn Stream<Item = std::result::Result<Event, Infallible>> + Send>>,
    _lease: Lease,
}
impl Stream for SessionEvents {
    type Item = std::result::Result<Event, Infallible>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.events.as_mut().poll_next(cx)
    }
}
async fn prepare(
    State(state): State<Arc<HttpState>>,
    Json(input): Json<Value>,
) -> std::result::Result<Response, ApiError> {
    fields(&input, &["requestId", "kinds", "hotwords", "voiceId"])?;
    let token = CancellationToken::new();
    let lease = Lease {
        service: state.speech.clone(),
        id: input["requestId"].as_str().unwrap_or("").to_owned(),
        token: token.clone(),
    };
    state.speech.prepare(&input, token).await?;
    let events = futures::stream::once(async {
        Ok(Event::default().event("ready").data("{\"ready\":true}"))
    })
    .chain(futures::stream::pending());
    let mut response = Sse::new(SessionEvents {
        events: Box::pin(events),
        _lease: lease,
    })
    .keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("speech-session"),
    )
    .into_response();
    response.headers_mut().insert(
        "content-type",
        axum::http::HeaderValue::from_static("text/event-stream; charset=utf-8"),
    );
    response
        .headers_mut()
        .insert("cache-control", "no-store".parse().unwrap());
    response
        .headers_mut()
        .insert("x-content-type-options", "nosniff".parse().unwrap());
    Ok(response)
}
async fn synthesize(
    State(state): State<Arc<HttpState>>,
    Json(input): Json<Value>,
) -> std::result::Result<Response, ApiError> {
    fields(&input, &["text", "voiceId", "requestId"])?;
    let bytes = state
        .speech
        .synthesize(&input, CancellationToken::new())
        .await?;
    let length = bytes.len().to_string();
    let mut response = (StatusCode::OK, bytes).into_response();
    for (name, value) in [
        ("content-type", "audio/wav"),
        ("cache-control", "no-store"),
        ("x-content-type-options", "nosniff"),
        ("content-length", length.as_str()),
    ] {
        response.headers_mut().insert(
            axum::http::HeaderName::from_static(name),
            value.parse().unwrap(),
        );
    }
    Ok(response)
}
async fn cancel(
    State(state): State<Arc<HttpState>>,
    Json(input): Json<Value>,
) -> std::result::Result<Json<Value>, ApiError> {
    fields(&input, &["requestId"])?;
    Ok(Json(state.speech.cancel_speech(&input).await?))
}
fn header<'a>(headers: &'a HeaderMap, name: &str) -> &'a str {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
}
fn sample_rate(headers: &HeaderMap) -> std::result::Result<(), ApiError> {
    if header(headers, "x-pisper-sample-rate")
        .trim()
        .parse::<f64>()
        .ok()
        != Some(16000.0)
    {
        return Err(ApiError::bad_request("语音采样率必须是 16000 Hz。"));
    }
    Ok(())
}
fn pcm(bytes: &[u8]) -> std::result::Result<(), ApiError> {
    if bytes.is_empty() || bytes.len() % 4 != 0 {
        return Err(ApiError::bad_request("语音 PCM 数据无效。"));
    }
    Ok(())
}
async fn transcribe(
    State(state): State<Arc<HttpState>>,
    headers: HeaderMap,
    bytes: Bytes,
) -> std::result::Result<Json<Value>, ApiError> {
    sample_rate(&headers)?;
    pcm(&bytes)?;
    let terms = workspace_terms(&state, header(&headers, "x-pisper-chat-session")).await?;
    Ok(Json(
        state
            .speech
            .transcribe(&bytes, terms, CancellationToken::new())
            .await?,
    ))
}
async fn start(
    State(state): State<Arc<HttpState>>,
    headers: HeaderMap,
) -> std::result::Result<Json<Value>, ApiError> {
    let terms = workspace_terms(&state, header(&headers, "x-pisper-chat-session")).await?;
    Ok(Json(state.speech.start_session(terms).await?))
}
async fn chunk(
    State(state): State<Arc<HttpState>>,
    headers: HeaderMap,
    bytes: Bytes,
) -> std::result::Result<Json<Value>, ApiError> {
    sample_rate(&headers)?;
    pcm(&bytes)?;
    Ok(Json(
        state
            .speech
            .chunk(header(&headers, "x-pisper-speech-session"), &bytes)
            .await?,
    ))
}
async fn finish(
    State(state): State<Arc<HttpState>>,
    headers: HeaderMap,
) -> std::result::Result<Json<Value>, ApiError> {
    Ok(Json(
        state
            .speech
            .finish_session(header(&headers, "x-pisper-speech-session"))
            .await?,
    ))
}
async fn cancel_stream(
    State(state): State<Arc<HttpState>>,
    headers: HeaderMap,
) -> std::result::Result<Json<Value>, ApiError> {
    Ok(Json(
        state
            .speech
            .cancel_session(header(&headers, "x-pisper-speech-session"))
            .await?,
    ))
}
