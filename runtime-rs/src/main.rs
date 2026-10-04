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
use pi_rust::agent_core::types::ThinkingLevel;
use pi_rust::coding_agent::core::mcp_servers::McpExposure;
use pi_rust::coding_agent::core::skills::{load_skills, LoadSkillsOptions};
use pi_rust::coding_agent::extensions::mcp::config::{load_mcp_config, LoadedMcpConfigOptions};
use pi_rust::config;

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
        "mcp": true,
        "skills": true,
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

async fn get_config() -> Json<serde_json::Value> {
    let cfg = config::load_config().unwrap_or_default();
    Json(serde_json::json!({
        "provider": cfg.provider,
        "model": cfg.model,
        "baseUrl": cfg.base_url,
        "maxTokens": cfg.max_tokens,
        "contextWindow": cfg.context_window,
        "hasCredential": config::resolve_api_key(&cfg.provider, None).is_some(),
    }))
}

async fn put_config(
    State(state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut cfg = config::load_config().unwrap_or_default();
    if let Some(v) = body.get("provider").and_then(|v| v.as_str()) {
        cfg.provider = v.to_string();
    }
    if let Some(v) = body.get("model").and_then(|v| v.as_str()) {
        cfg.model = v.to_string();
    }
    if let Some(v) = body.get("baseUrl").and_then(|v| v.as_str()) {
        cfg.base_url = Some(v.to_string());
    }
    if let Some(v) = body.get("maxTokens").and_then(|v| v.as_u64()) {
        cfg.max_tokens = v;
    }
    if let Some(v) = body.get("contextWindow").and_then(|v| v.as_u64()) {
        cfg.context_window = v;
    }
    // Persist in the engine's config format (pi_rust::config::parse_config
    // reads this TOML shape back on the next boot).
    let mut toml = format!("provider = {:?}\nmodel = {:?}\n", cfg.provider, cfg.model);
    if let Some(base) = &cfg.base_url {
        toml.push_str(&format!("base_url = {base:?}\n"));
    }
    toml.push_str(&format!(
        "max_tokens = {}\ncontext_window = {}\n",
        cfg.max_tokens, cfg.context_window
    ));
    let path = config::config_path()
        .ok_or_else(|| ApiError::internal("no user config directory available"))?;
    std::fs::create_dir_all(path.parent().expect("config parent"))
        .map_err(|e| ApiError::internal(e.to_string()))?;
    std::fs::write(&path, &toml).map_err(|e| ApiError::internal(e.to_string()))?;
    apply_config_model(&state, &cfg).await?;
    Ok(Json(serde_json::json!({
        "ok": true,
        "provider": cfg.provider,
        "model": cfg.model,
    })))
}

/// Resolve the configured model against the engine's available snapshot and
/// apply it to the active session. Config-file models absent from the catalog
/// (hand-declared endpoints like internal vLLM deployments) are built via
/// `config::build_model`.
async fn apply_config_model(
    state: &AppState,
    cfg: &config::Config,
) -> Result<(), ApiError> {
    let session = state.runtime.session();
    let model = session
        .model_runtime()
        .get_available_snapshot()
        .into_iter()
        .find(|m| m.provider == cfg.provider && m.id == cfg.model)
        .unwrap_or_else(|| {
            config::build_model(cfg).expect("config model builds for known providers")
        });
    session
        .set_model(model, None)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))
}

async fn get_session_model(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let session = state.runtime.session();
    ensure_current_session(&session.session_id(), &id)?;
    let model = session.model();
    Ok(Json(match model {
        Some(m) => serde_json::json!({
            "provider": m.provider,
            "id": m.id,
            "name": m.name,
            "baseUrl": m.base_url,
            "reasoning": m.reasoning,
        }),
        None => serde_json::Value::Null,
    }))
}

async fn post_session_model(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let session = state.runtime.session();
    ensure_current_session(&session.session_id(), &id)?;
    let provider = body
        .get("provider")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::bad_request("missing \"provider\" field"))?;
    let model_id = body
        .get("model")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::bad_request("missing \"model\" field"))?;
    let snapshot = session.model_runtime().get_available_snapshot();
    let model = snapshot
        .into_iter()
        .find(|m| m.provider == provider && m.id == model_id)
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "model_not_found",
                format!("Model not found: {provider}/{model_id}"),
            )
        })?;
    session
        .set_model(model, None)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn get_session_thinking_level(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let session = state.runtime.session();
    ensure_current_session(&session.session_id(), &id)?;
    let level = session.thinking_level();
    Ok(Json(serde_json::to_value(level).expect("level serializes")))
}

async fn post_session_thinking_level(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let session = state.runtime.session();
    ensure_current_session(&session.session_id(), &id)?;
    let level: ThinkingLevel = serde_json::from_value(body)
        .map_err(|e| ApiError::bad_request(format!("invalid thinking level: {e}")))?;
    session.set_thinking_level(level, None);
    Ok(Json(serde_json::json!({ "ok": true, "level": level })))
}
fn format_diagnostic(d: &pi_rust::coding_agent::core::diagnostics::ResourceDiagnostic) -> String {
    format!(
        "{}{}{}",
        d.r#type.as_str(),
        d.path.as_deref().map(|p| format!(" ({p})")).unwrap_or_default(),
        format!(": {}", d.message)
    )
}


fn exposure_label(e: McpExposure) -> &'static str {
    match e {
        McpExposure::Codemode => "codemode",
        McpExposure::Deferred => "deferred",
        McpExposure::Direct => "direct",
        McpExposure::Hidden => "hidden",
    }
}

async fn get_mcp_dashboard(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    // Tool cold/hot gateway policy: exposure per server (direct = always in
    // prompt, deferred/codemode = discovered/called through the gateway,
    // hidden = excluded) comes straight from the engine's validated config.
    let loaded = load_mcp_config(LoadedMcpConfigOptions {
        agent_dir: state.runtime.services().agent_dir.clone(),
        cwd: state.cwd.clone(),
        project_trusted: false,
    });
    let servers: Vec<serde_json::Value> = loaded
        .servers
        .iter()
        .map(|entry| {
            let enabled = entry
                .config
                .raw()
                .get("enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            serde_json::json!({
                "name": entry.name,
                "source": entry.source,
                "enabled": enabled,
                "transport": entry
                    .config
                    .command()
                    .map(|_| "stdio")
                    .or_else(|| entry.config.url().map(|_| "http")),
                "exposure": exposure_label(entry.config.exposure()),
            })
        })
        .collect();
    Json(serde_json::json!({
        "servers": servers,
        "autoEnableCodemode": loaded.auto_enable_codemode,
        "errors": loaded.errors,
        "gateway": { "tiers": ["direct", "deferred", "codemode", "hidden"] },
    }))
}

async fn get_skills(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let services = state.runtime.services();
    let result = load_skills(LoadSkillsOptions {
        cwd: state.cwd.clone(),
        agent_dir: services.agent_dir.clone(),
        skill_paths: vec![],
        include_defaults: true,
    });
    let skills: Vec<serde_json::Value> = result
        .skills
        .iter()
        .map(|s| {
            serde_json::json!({
                "name": s.name,
                "description": s.description,
                "filePath": s.file_path,
                "baseDir": s.base_dir,
                "source": format!("{:?}", s.source_info.source),
                "disableModelInvocation": s.disable_model_invocation,
            })
        })
        .collect();
    Json(serde_json::json!({
        "skills": skills,
        "diagnostics": result
            .diagnostics
            .iter()
            .map(format_diagnostic)
            .collect::<Vec<_>>(),
    }))
}

async fn reload_skills(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    // Skills load fresh on every request (no cache to invalidate yet), so
    // reload re-runs the same discovery the dashboard uses.
    get_skills(State(state)).await
}

async fn get_plugins() -> Json<serde_json::Value> {
    // The plugin product layer (auto-generated local plugins with capability
    // toggles) persists in the Pisper agent dir; its storage lands with slice
    // 6. The engine's pi extensions surface through the MCP/extension
    // registries above.
    Json(serde_json::json!({ "plugins": [] }))
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
    let mut args = Args::default();
    // CLI-parity startup overrides: mirror `pirs --provider/--model/--api-key`
    // via environment so the acceptance run can select a model without
    // touching the user's auth.json.
    if let Ok(provider) = std::env::var("PISPER_RS_PROVIDER") {
        if !provider.is_empty() {
            args.provider = Some(provider);
        }
    }
    if let Ok(model) = std::env::var("PISPER_RS_MODEL") {
        if !model.is_empty() {
            args.model = Some(model);
        }
    }
    if let Ok(key) = std::env::var("PISPER_RS_API_KEY") {
        if !key.is_empty() {
            args.api_key = Some(key);
        }
    }
    let boot_provider = args.provider.clone();
    let boot_api_key = args.api_key.clone();
    let settings = SettingsManager::create_with(
        &cwd,
        &agent_dir,
        SettingsManagerCreateOptions::default(),
    )?;
    let factory = create_cli_runtime_factory(CliRuntimeFactoryOptions {
        parsed: args,
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
    // CLI-parity runtime credential: the factory only installs --api-key when
    // Args resolve to a catalog model; hand-declared compat models skip that
    // path, so install the key for the explicit provider here.
    if let (Some(provider), Some(key)) = (boot_provider, boot_api_key) {
        runtime
            .session()
            .model_runtime()
            .set_runtime_api_key(&provider, &key, None)
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    }
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

/// Stateful API routes: the session host surface (slice 2) + config/model
/// selection (slice 3).
fn session_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/sessions", get(list_sessions).post(create_session))
        .route("/api/sessions/{id}/input", post(session_input))
        .route("/api/sessions/{id}/live", get(session_live))
        .route("/api/sessions/{id}/abort", post(session_abort))
        .route("/api/config", get(get_config).post(put_config))
        .route(
            "/api/sessions/{id}/model",
            get(get_session_model).post(post_session_model),
        )
        .route(
            "/api/sessions/{id}/thinking-level",
            get(get_session_thinking_level).post(post_session_thinking_level),
        )
        .route("/api/mcp", get(get_mcp_dashboard))
        .route("/api/skills", get(get_skills).post(reload_skills))
        .route("/api/plugins", get(get_plugins))
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
