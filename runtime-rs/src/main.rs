//! Pisper Rust backend — vertical slice 2: the pi-rs session host.
//!
//! This server replaces the Node.js `runtime/` layer: the same HTTP contract
//! (`/api/*`) backed by the pi-rs engine (crates.io `pi-rs`) instead of the
//! `@earendil-works/pi-coding-agent` npm package.
//!
//! Slice 1: process bootstrap, `/api/health` contract, JSON 404 fallback.
//! Slice 2 (this file): an in-process `AgentSessionRuntime` from the pi-rs
//! engine hosting ONE active session, exposed as
//!   GET  /api/sessions                    (list from the session store)
//!   POST /api/sessions                    (create + switch to a new session)
//!   POST /api/sessions/{id}/input         (prompt the engine)
//!   GET  /api/sessions/{id}/live          (SSE stream of JSON session events)
//!   POST /api/sessions/{id}/abort         (abort streaming)
//! Multi-session switching (hosting any {id}, not just the current one) is
//! slice 3; the engine already supports it via `switch_session`.

use std::{convert::Infallible, sync::Arc};

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{sse::{Event, KeepAlive}, IntoResponse, Sse},
    routing::{get, post},
    Json, Router,
};
use futures::stream::Stream;
use tokio::sync::broadcast;

use pi_rust::coding_agent::{
    agent_session::AgentSessionEvent,
    cli::{args::Args, project_trust::AppMode},
    core::{
        agent_session_runtime::{
            create_agent_session_runtime, AgentSessionRuntime, CreateAgentSessionRuntimeOptions,
            NewSessionOptionsRuntime,
        },
        settings_manager::{SettingsManager, SettingsManagerCreateOptions},
    },
    main::runtime::{create_cli_runtime_factory, CliRuntimeFactoryOptions},
    modes::json_event::to_json_event_string,
    session_manager::{NewSessionOptions, SessionManager},
};

/// API version handshake (see `runtime/http/routes/sessions-runtime.mjs`).
const API_VERSION: u32 = 1;
const MIN_CLIENT_VERSION: u32 = 1;
const ENGINE: &str = "pi-rs";

/// Shared server state: one pi-rs engine runtime hosting the current session,
/// plus a broadcast fan-out of JSON-serialized session events for `/live`.
struct AppState {
    runtime: Arc<AgentSessionRuntime>,
    cwd: String,
    events: broadcast::Sender<String>,
    /// Unsubscribe for the listener attached to the current session. Rebuilt
    /// on every `new_session` so events keep flowing across session switches.
    unsub: std::sync::Mutex<Option<pi_rust::coding_agent::agent_session::AgentSessionUnsubscribe>>,
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "ok": true,
        "engine": ENGINE,
        "version": env!("CARGO_PKG_VERSION"),
        "apiVersion": API_VERSION,
        "minClientVersion": MIN_CLIENT_VERSION,
        "capabilities": capabilities(),
    }))
}

fn capabilities() -> serde_json::Value {
    serde_json::json!({
        "sessions": true,
        "mcp": false,
        "skills": false,
        "workflows": false,
        "schedules": false,
        "remote": false,
    })
}

async fn list_sessions(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let sessions: Vec<serde_json::Value> = SessionManager::list(&state.cwd, None, None)
        .into_iter()
        .map(|s| {
            serde_json::json!({
                "id": s.id,
                "name": s.name,
                "cwd": s.cwd,
                "created": s.created,
                "modified": s.modified,
                "messageCount": s.message_count,
                "firstMessage": s.first_message,
            })
        })
        .collect();
    Json(serde_json::json!({ "sessions": sessions }))
}

async fn create_session(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state.runtime.new_session(NewSessionOptionsRuntime::default()).await.map_err(|e| ApiError::internal(e.to_string()))?;
    attach_event_listener(&state);
    let session = state.runtime.session();
    Ok(Json(serde_json::json!({
        "id": session.session_id(),
        "cwd": state.cwd,
    })))
}

async fn session_input(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let session = state.runtime.session();
    ensure_current_session(&session.session_id(), &id)?;
    let text = body
        .get("text")
        .and_then(|t| t.as_str())
        .or_else(|| body.as_str())
        .ok_or_else(|| ApiError::bad_request("missing \"text\" field"))?;
    session
        .prompt(text.to_string(), None)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn session_abort(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let session = state.runtime.session();
    ensure_current_session(&session.session_id(), &id)?;
    session.abort().await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn session_live(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let session = state.runtime.session();
    ensure_current_session(&session.session_id(), &id)?;
    let mut rx = state.events.subscribe();
    let stream = futures::stream::unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(json) => {
                    return Some((
                        Ok::<_, Infallible>(
                            axum::response::sse::Event::default().data(json),
                        ),
                        rx,
                    ));
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

fn ensure_current_session(current: &str, requested: &str) -> Result<(), ApiError> {
    if current == requested {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::CONFLICT,
            "session_not_active",
            format!("session '{requested}' exists but the active session is '{current}'"),
        ))
    }
}

struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}
impl ApiError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "bad_request", message)
    }
    fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", message)
    }
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self { status, code, message: message.into() }
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        (
            self.status,
            Json(serde_json::json!({
                "error": { "code": self.code, "message": self.message }
            })),
        )
            .into_response()
    }
}

fn attach_event_listener(state: &AppState) {
    let tx = state.events.clone();
    let listener: Arc<dyn Fn(&AgentSessionEvent) + Send + Sync> =
        Arc::new(move |event: &AgentSessionEvent| {
            if let Ok(json) = to_json_event_string(event) {
                let _ = tx.send(json);
            }
        });
    *state.unsub.lock().expect("unsub lock") =
        Some(state.runtime.session().subscribe(listener));
}

async fn boot() -> anyhow::Result<AppState> {
    let cwd = std::env::current_dir()?
        .to_string_lossy()
        .to_string();
    let agent_dir = pi_rust::coding_agent::core::get_agent_dir();
    let settings = SettingsManager::create_with(
        &cwd,
        &agent_dir,
        SettingsManagerCreateOptions::default(),
    )?;
    let factory = create_cli_runtime_factory(CliRuntimeFactoryOptions {
        parsed: Args::default(),
        startup_cwd: cwd.clone(),
        initial_session_cwd: cwd.clone(),
        agent_dir: agent_dir.clone(),
        startup_settings_manager: settings,
        app_mode: AppMode::Rpc,
        extension_factories: vec![],
        extension_module_loader: None,
        model_runtime_factory: None,
        model_scope_warning: None,
    })?;
    let manager = SessionManager::in_memory(&cwd, Some(&NewSessionOptions::default()), None)?;
    let runtime = create_agent_session_runtime(
        factory.create_runtime,
        CreateAgentSessionRuntimeOptions {
            cwd: cwd.clone(),
            agent_dir,
            session_manager: Arc::new(std::sync::Mutex::new(manager)),
            session_start_event: None,
            project_trust_context: None,
        },
    )
    .await?;
    runtime.new_session(NewSessionOptionsRuntime::default()).await?;
    let (events, _) = broadcast::channel(1024);
    let state = AppState {
        runtime: Arc::new(runtime),
        cwd,
        events,
        unsub: std::sync::Mutex::new(None),
    };
    attach_event_listener(&state);
    Ok(state)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "pisper_server=info,tower_http=info".into()),
        )
        .init();

    let state = Arc::new(boot().await?);
    let addr = std::env::var("PISPER_RS_ADDR").unwrap_or_else(|_| "127.0.0.1:5174".into());
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("pisper-server (Rust runtime) listening on http://{addr}");
    axum::serve(listener, router(state)).await?;
    Ok(())
}

/// Stateful API routes: the session host surface (slice 2).
fn session_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/sessions", get(list_sessions).post(create_session))
        .route("/api/sessions/{id}/input", post(session_input))
        .route("/api/sessions/{id}/live", get(session_live))
        .route("/api/sessions/{id}/abort", post(session_abort))
}

/// Stateless base: handshake + unknown-route fallback.
fn base_router() -> Router {
    Router::new()
        .route("/api/health", get(health))
        .fallback(unknown_api_fallback)
}

fn router(state: Arc<AppState>) -> Router {
    base_router().merge(session_router().with_state(state))
}

async fn unknown_api_fallback() -> impl IntoResponse {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({
            "error": { "code": "not_found", "message": "unknown API route" }
        })),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    #[tokio::test]
    async fn health_matches_client_handshake_contract() {
        let res = base_router()
            .oneshot(Request::get("/api/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap(),
        )
        .unwrap();
        assert_eq!(body["ok"], serde_json::json!(true));
        assert_eq!(body["engine"], serde_json::json!(ENGINE));
        assert_eq!(body["apiVersion"], serde_json::json!(API_VERSION));
        assert_eq!(body["minClientVersion"], serde_json::json!(MIN_CLIENT_VERSION));
        assert!(body["capabilities"].is_object());
    }

    #[tokio::test]
    async fn unknown_api_routes_return_structured_404() {
        let res = base_router()
            .oneshot(
                Request::get("/api/does-not-exist")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
        let body: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap(),
        )
        .unwrap();
        assert_eq!(body["error"]["code"], serde_json::json!("not_found"));
    }
}
