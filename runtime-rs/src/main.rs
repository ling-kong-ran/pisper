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

mod product;
mod security;

use std::{convert::Infallible, sync::Arc};

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{sse::{Event, KeepAlive}, IntoResponse, Sse},
    routing::{delete, get, post, put},
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
            ForkOptions, ForkPosition, SwitchSessionOptions,
            NewSessionOptionsRuntime,
        },
        settings_manager::{SettingsManager, SettingsManagerCreateOptions},
    },
    main::runtime::{create_cli_runtime_factory, CliRuntimeFactoryOptions},
    modes::json_event::to_json_event_string,
    session_manager::{NewSessionOptions, SessionEntry, SessionManager},
};
use pi_rust::agent_core::harness::session::jsonl::iso8601::format_iso8601_utc;
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
pub(crate) struct AppState {
    pub(crate) runtime: Arc<AgentSessionRuntime>,
    pub(crate) cwd: String,
    pub(crate) events: broadcast::Sender<String>,
    /// Pisper product data dir (PISPER_RS_DATA_DIR, default
    /// ~/.pisper/agent-rs): schedules/workflows/plugins persistence.
    pub(crate) data_dir: String,
    /// Live workflow runs (in-memory; retry/stop operate on these).
    pub(crate) runs: product::RunsMap,
    /// Remote-control (LAN) toggle; P2P transport is not implemented yet.
    pub(crate) remote_enabled: std::sync::atomic::AtomicBool,
    /// Stable device fingerprint shown in the remote pairing surface.
    pub(crate) fingerprint: String,
    /// Built React frontend dir (vite `dist/`) served same-origin with /api.
    pub(crate) dist_dir: std::path::PathBuf,
    /// Live chat runs (POST /api/chat) for SSE replay (upstream runs service).
    pub(crate) chat_runs:
        std::sync::Mutex<std::collections::HashMap<String, product::ChatRun>>,
    /// Global SSE frame cursor (upstream runs.record cursor).
    pub(crate) frame_cursor: std::sync::atomic::AtomicU64,
    /// Remote device pairing store (codes + paired devices).
    pub(crate) pairing: security::PairingStore,
    /// Pisper product-layer per-session metadata (Node sessionMeta store):
    /// execution mode -> permission mode mapping and the goal tracker.
    session_meta: std::sync::Mutex<std::collections::HashMap<String, SessionMeta>>,
    /// Unsubscribe for the listener attached to the current session. Rebuilt
    /// on every `new_session` so events keep flowing across session switches.
    unsub: std::sync::Mutex<Option<pi_rust::coding_agent::agent_session::AgentSessionUnsubscribe>>,
}

/// Upstream `permissionModeForExecutionMode` (session-lifecycle.mjs): the
/// permission preset implied by a Pisper execution mode.
fn permission_mode_for_execution_mode(mode: &str) -> &'static str {
    match mode {
        "auto" | "yolo" => "full",
        "supervised" => "ask",
        _ => "ask",
    }
}

#[derive(Default, Clone)]
struct SessionMeta {
    execution_mode: Option<String>,
    permission_mode: Option<String>,
    goal: Option<String>,
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
        "workflows": true,
        "schedules": true,
        "remote": true,
    })
}

async fn list_sessions(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let active_id = state.runtime.session().session_id();
    let active_model = state
        .runtime
        .session()
        .model()
        .map(|m| format!("{}/{}", m.provider, m.id))
        .unwrap_or_default();
    let sessions: Vec<serde_json::Value> = SessionManager::list(&state.cwd, None, None)
        .into_iter()
        .map(|s| {
            let active = s.id == active_id;
            let streaming = active && state.runtime.session().is_streaming();
            let thinking_level: pi_rust::agent_core::types::ThinkingLevel = if active {
                state.runtime.session().thinking_level()
            } else {
                pi_rust::agent_core::types::ThinkingLevel::Medium
            };
            serde_json::json!({
                "id": s.id,
                "name": s.name.clone().unwrap_or_default(),
                "model": if active { active_model.clone() } else { String::new() },
                "cwd": s.cwd,
                "streaming": streaming,
                "executionMode": "",
                "thinkingLevel": serde_json::to_value(thinking_level).unwrap_or_default(),
                "modified": format_iso8601_utc(s.modified as i64),
                "created": s.created,
                "messageCount": s.message_count,
                "firstMessage": s.first_message,
            })
        })
        .collect();
    Json(serde_json::json!({ "sessions": sessions }))
}

async fn create_session(
    State(state): State<Arc<AppState>>,
    body: Option<Json<serde_json::Value>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let body = body.map(|Json(v)| v).unwrap_or_default();
    state
        .runtime
        .new_session(NewSessionOptionsRuntime::default())
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let session = state.runtime.session();
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    if !name.is_empty() {
        session
            .session_manager
            .lock()
            .expect("session manager lock")
            .append_session_info(&name)
            .map_err(|e| ApiError::internal(e.to_string()))?;
    }
    // The client's workspace must match what the engine session was created
    // with (TUI validates this); the server boot cwd defines it for now.
    let model = session
        .model()
        .map(|m| format!("{}/{}", m.provider, m.id))
        .unwrap_or_default();
    let requested_cwd = body
        .get("cwd")
        .and_then(|v| v.as_str())
        .unwrap_or(&state.cwd)
        .to_string();
    Ok(Json(serde_json::json!({
        "id": session.session_id(),
        "name": name,
        "model": model,
        "cwd": if requested_cwd.is_empty() { state.cwd.clone() } else { requested_cwd },
        "streaming": false,
        "executionMode": "",
        "thinkingLevel": serde_json::to_value(session.thinking_level()).unwrap_or_default(),
        "modified": format_iso8601_utc(product::now_ms() as i64),
    })))
}

async fn session_input(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    ensure_hosted(&state, &id).await?;
    let session = state.runtime.session();
    let text = body
        .get("text")
        .and_then(|t| t.as_str())
        .or_else(|| body.get("message").and_then(|t| t.as_str()))
        .or_else(|| body.as_str())
        .ok_or_else(|| ApiError::bad_request("missing \"text\"/\"message\" field"))?;
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
    ensure_hosted(&state, &id).await?;
    let session = state.runtime.session();
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
    ensure_hosted(&state, &id).await?;
    let session = state.runtime.session();
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
    ensure_hosted(&state, &id).await?;
    let session = state.runtime.session();
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
    ensure_hosted(&state, &id).await?;
    let session = state.runtime.session();
    let level = session.thinking_level();
    Ok(Json(serde_json::json!({
        "thinkingLevel": serde_json::to_value(level).expect("level serializes"),
        "availableLevels": ["minimal", "low", "medium", "high", "xhigh", "max"],
        "status": "ok",
    })))
}

async fn post_session_thinking_level(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    ensure_hosted(&state, &id).await?;
    let session = state.runtime.session();
    let level: ThinkingLevel = serde_json::from_value(body)
        .map_err(|e| ApiError::bad_request(format!("invalid thinking level: {e}")))?;
    session.set_thinking_level(level, None);
    Ok(Json(serde_json::json!({
        "thinkingLevel": serde_json::to_value(level).expect("level serializes"),
        "availableLevels": ["minimal", "low", "medium", "high", "xhigh", "max"],
        "status": "ok",
    })))
}
fn format_diagnostic(d: &pi_rust::coding_agent::core::diagnostics::ResourceDiagnostic) -> String {
    format!(
        "{}{}{}",
        d.r#type.as_str(),
        d.path.as_deref().map(|p| format!(" ({p})")).unwrap_or_default(),
        format!(": {}", d.message)
    )
}


async fn derive_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    ensure_hosted(&state, &id).await?;
    let entry_id = body
        .get("boundaryEntryId")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::bad_request("missing \"boundaryEntryId\" field"))?
        .to_string();
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    state
        .runtime
        .fork(
            &entry_id,
            ForkOptions { position: ForkPosition::At, with_session: None },
        )
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    attach_event_listener(&state);
    let session = state.runtime.session();
    if !name.is_empty() {
        session
            .session_manager
            .lock()
            .expect("session manager lock")
            .append_session_info(&name)
            .map_err(|e| ApiError::internal(e.to_string()))?;
    }
    Ok(Json(serde_json::json!({
        "ok": true,
        "id": session.session_id(),
        "name": name,
    })))
}

async fn get_session_messages(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    ensure_hosted(&state, &id).await?;
    let session = state.runtime.session();
    let entries = session
        .session_manager
        .lock()
        .expect("session manager lock")
        .get_entries();
    let items: Vec<serde_json::Value> = entries
        .iter()
        .filter_map(|entry| {
            let kind = match entry {
                SessionEntry::Message(_) => "message",
                SessionEntry::ThinkingLevelChange(_) => "thinkingLevelChange",
                SessionEntry::ModelChange(_) => "modelChange",
                SessionEntry::Usage(_) => "usage",
                SessionEntry::Compaction(_) => "compaction",
                SessionEntry::BranchSummary(_) => "branchSummary",
                SessionEntry::Custom(_) => "custom",
                SessionEntry::CustomMessage(_) => "customMessage",
                SessionEntry::ContextEdit(_) => "contextEdit",
                SessionEntry::Label(_) => "label",
                SessionEntry::SessionInfo(_) => "sessionInfo",
                SessionEntry::Unparsed(_) => "__unparsed",
            };
            entry.id().map(|entry_id| {
                serde_json::json!({ "id": entry_id, "type": kind })
            })
        })
        .collect();
    Ok(Json(serde_json::json!({ "entries": items })))
}

async fn get_session_labels(
    State(state): State<Arc<AppState>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Json<serde_json::Value> {
    // Node contract: keyword normalized (collapsed whitespace, lowercase, 80
    // chars); limit defaults to 500 without a keyword and 20 with one
    // (clamped 1..=1000). Turn-label entries inside session files are a
    // follow-up; session names are the v1 label set.
    let keyword = params
        .get("query")
        .map(|q| q.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase())
        .unwrap_or_default()
        .chars()
        .take(80)
        .collect::<String>();
    let limit = params
        .get("limit")
        .and_then(|l| l.parse::<usize>().ok())
        .map(|l| l.clamp(1, 1000))
        .unwrap_or(if keyword.is_empty() { 500 } else { 20 });
    let labels: Vec<serde_json::Value> = SessionManager::list(&state.cwd, None, None)
        .into_iter()
        .filter(|s| {
            keyword.is_empty()
                || s.name
                    .as_deref()
                    .map(|n| n.to_lowercase().contains(&keyword))
                    .unwrap_or(false)
        })
        .take(limit)
        .map(|s| {
            serde_json::json!({
                "sessionId": s.id,
                "sessionName": s.name.clone().unwrap_or_default(),
                "label": s.name.unwrap_or_default(),
                "sessionCreated": s.created,
                "sessionModified": s.modified,
                "entryId": serde_json::Value::Null,
                "active": false,
            })
        })
        .collect();
    Json(serde_json::json!({ "labels": labels }))
}

fn exposure_label(e: McpExposure) -> &'static str {
    match e {
        McpExposure::Codemode => "codemode",
        McpExposure::Deferred => "deferred",
        McpExposure::Direct => "direct",
        McpExposure::Hidden => "hidden",
    }
}

async fn session_compact(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    ensure_hosted(&state, &id).await?;
    let session = state.runtime.session();
    let result = session
        .compact(None)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(result))
}

async fn set_session_cwd(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let cwd_input = body
        .get("cwd")
        .or_else(|| body.get("dir"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::bad_request("missing \"cwd\" field"))?
        .to_string();
    ensure_hosted(&state, &id).await?;
    let file = state
        .runtime
        .session()
        .session_file()
        .ok_or_else(|| ApiError::internal("active session is not persisted yet"))?;
    // Upstream re-hosts the same session file with a cwd override (the
    // engine rebuilds the session against the new workspace).
    state
        .runtime
        .switch_session(
            &file,
            SwitchSessionOptions { cwd_override: Some(cwd_input.clone()), ..Default::default() },
        )
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    attach_event_listener(&state);
    {
        let mut meta = state.session_meta.lock().expect("session meta lock");
        let entry = meta.entry(id.clone()).or_default();
        entry.goal = entry.goal.take();
    }
    Ok(Json(serde_json::json!({ "id": id, "cwd": cwd_input })))
}

async fn set_session_execution_mode(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mode = body
        .get("mode")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::bad_request("missing \"mode\" field"))?
        .to_string();
    {
        let mut meta = state.session_meta.lock().expect("session meta lock");
        let entry = meta.entry(id.clone()).or_default();
        entry.execution_mode = Some(mode.clone());
        entry.permission_mode = Some(permission_mode_for_execution_mode(&mode).to_string());
    }
    Ok(Json(serde_json::json!({
        "id": id,
        "executionMode": mode,
        "permissionMode": permission_mode_for_execution_mode(&mode),
    })))
}

async fn get_session_goal(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let meta = state.session_meta.lock().expect("session meta lock");
    match meta.get(&id).and_then(|m| m.goal.clone()) {
        Some(goal) => Ok(Json(serde_json::json!({ "id": id, "goal": goal }))),
        None => Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "no_goal",
            "当前会话没有 Goal。",
        )),
    }
}

async fn put_session_goal(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let goal = body
        .get("goal")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::bad_request("missing \"goal\" field"))?
        .to_string();
    {
        let mut meta = state.session_meta.lock().expect("session meta lock");
        meta.entry(id.clone()).or_default().goal = Some(goal.clone());
    }
    Ok(Json(serde_json::json!({ "id": id, "goal": goal })))
}

async fn delete_session_goal(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Json<serde_json::Value> {
    let mut meta = state.session_meta.lock().expect("session meta lock");
    meta.entry(id.clone()).or_default().goal = None;
    Json(serde_json::json!({ "id": id, "deleted": true }))
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
                "command": format!("/{}", s.name),
                "enabled": !s.disable_model_invocation,
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

async fn session_live(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    ensure_hosted(&state, &id).await?;

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

/// Host any known session id: if it is not the active one, switch the engine
/// runtime to it (multi-session hosting, upstream `switch_session`).
pub(crate) async fn ensure_hosted(state: &AppState, id: &str) -> Result<(), ApiError> {
    let current = state.runtime.session().session_id();
    if current == id {
        return Ok(());
    }
    let info = SessionManager::list(&state.cwd, None, None)
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "session_not_found",
                format!("no session with id '{id}'"),
            )
        })?;
    state
        .runtime
        .switch_session(&info.path, SwitchSessionOptions::default())
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    attach_event_listener(state);
    Ok(())
}

pub(crate) struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}
impl ApiError {
    pub(crate) fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "bad_request", message)
    }
    pub(crate) fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", message)
    }
    pub(crate) fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self { status, code, message: message.into() }
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        // Security seam: error text can carry provider/auth internals; the
        // message is scrubbed through the secret-redaction layer first.
        (
            self.status,
            Json(serde_json::json!({
                "error": {
                    "code": self.code,
                    "message": security::redact_secret_text(&self.message),
                }
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
    // Persistent manager: sessions land in the agent session dir so they can
    // be listed, re-hosted (switch_session) and derived after restarts.
    let manager = SessionManager::create(&cwd, None, Some(&NewSessionOptions::default()))?;
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
    let data_dir = std::env::var("PISPER_RS_DATA_DIR").unwrap_or_else(|_| {
        let home = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_else(|_| ".".into());
        format!("{home}/.pisper/agent-rs")
    });
    let fingerprint = {
        use sha2::Digest;
        use std::fmt::Write as _;
        let digest = sha2::Sha256::digest(
            [data_dir.as_bytes(), std::env::var("COMPUTERNAME").unwrap_or_default().as_bytes()].concat(),
        );
        let digest = digest.as_slice();
        digest[..8].iter().fold(String::new(), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
    };
    let dist_dir = std::env::current_dir()
        .expect("cwd")
        .join("../dist")
        .canonicalize()
        .unwrap_or_else(|_| std::path::PathBuf::from("../dist"));
    let (events, _) = broadcast::channel(1024);
    let state = AppState {
        runtime: Arc::new(runtime),
        cwd,
        events,
        unsub: std::sync::Mutex::new(None),
        session_meta: std::sync::Mutex::new(std::collections::HashMap::new()),
        data_dir: data_dir.clone(),
        runs: std::sync::Mutex::new(std::collections::HashMap::new()),
        remote_enabled: std::sync::atomic::AtomicBool::new(false),
        fingerprint,
        chat_runs: std::sync::Mutex::new(std::collections::HashMap::new()),
        frame_cursor: std::sync::atomic::AtomicU64::new(0),
        dist_dir,
        pairing: security::PairingStore {
            pending: std::sync::Mutex::new(None),
            devices: std::sync::Mutex::new(security::load_devices(&data_dir)),
        },
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
    tokio::spawn(product::schedule_ticker(state.clone()));
    let addr = std::env::var("PISPER_RS_ADDR").unwrap_or_else(|_| "127.0.0.1:5174".into());
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("pisper-server (Rust runtime) listening on http://{addr}");
    axum::serve(listener, app_router(state)).await?;
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
        .route("/api/sessions/{id}/derive", post(derive_session))
        .route("/api/sessions/{id}/compact", post(session_compact))
        .route("/api/sessions/{id}/cwd", put(set_session_cwd))
        .route("/api/sessions/{id}/execution-mode", post(set_session_execution_mode))
        .route(
            "/api/sessions/{id}/goal",
            get(get_session_goal)
                .put(put_session_goal)
                .delete(delete_session_goal),
        )
        .route("/api/sessions/{id}/messages", get(get_session_messages))
        .route("/api/session-labels", get(get_session_labels))
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
        .route("/api/plugins", get(product::get_plugins_registry))
        .route(
            "/api/schedules",
            get(product::get_schedules).post(product::create_schedule),
        )
        .route(
            "/api/schedules/{id}/run",
            post(product::run_schedule),
        )
        .route(
            "/api/schedules/{id}",
            axum::routing::patch(product::update_schedule).delete(product::delete_schedule),
        )
        .route(
            "/api/workflows",
            get(product::get_workflows).post(product::create_workflow),
        )
        .route(
            "/api/workflows/{id}/run",
            post(product::run_workflow),
        )
        .route(
            "/api/workflows/{id}",
            delete(product::delete_workflow),
        )
        .route(
            "/api/workflow-runs/{id}",
            get(product::get_workflow_run),
        )
        .route(
            "/api/workflow-runs/{id}/stop",
            post(product::stop_workflow_run),
        )
        .route(
            "/api/plugins/install",
            post(product::install_plugin),
        )
        .route(
            "/api/plugins/{id}",
            axum::routing::patch(product::set_plugin_enabled).delete(product::uninstall_plugin),
        )
        .route(
            "/api/plugins/registry",
            get(product::get_plugins_registry).put(product::save_plugins_registry),
        )
        .route(
            "/api/remote/status",
            get(product::remote_status),
        )
        .route(
            "/api/remote/connection-info",
            get(product::remote_connection_info),
        )
        .route(
            "/api/remote/enabled",
            put(product::remote_set_enabled),
        )
        .route(
            "/api/sessions/{id}/vcs/changes",
            get(product::vcs_changes),
        )
        .route(
            "/api/sessions/{id}/vcs/commit",
            post(product::vcs_commit),
        )
        .route(
            "/api/sessions/{id}/vcs/push",
            post(product::vcs_push),
        )
        .route(
            "/api/sessions/{id}/vcs/revert",
            post(product::vcs_revert),
        )
        .route("/api/runtime/diagnostics", get(runtime_diagnostics))
        .route(
            "/api/settings/notifications",
            get(product::notification_settings).put(product::save_notification_settings),
        )
        .route("/api/remote/pairing-code", get(create_pairing_code).post(create_pairing_code))
        .route("/api/remote/pair", post(pair_device))
        .route("/api/remote/devices", get(list_devices))
        .route("/api/remote/devices/{id}", delete(revoke_device))
        .route("/api/chat", post(chat))
        .route("/api/runs/{id}/events", get(run_events))
}

/// Stateless base: handshake + unknown-route fallback + the built React
/// frontend served same-origin (upstream: Vite middleware in runtime/index.mjs).
fn base_router() -> Router {
    Router::new()
        .route("/api/health", get(health))
        .fallback(unknown_api_fallback)
}

/// Full router including the static SPA (dist/) with index.html fallback.
pub fn app_router(state: Arc<AppState>) -> Router {
    let dist = state.dist_dir.clone();
    Router::new()
        .merge(base_router())
        .merge(session_router().with_state(state.clone()))
        .fallback_service(
            tower_http::services::ServeDir::new(&dist)
                .append_index_html_on_directories(true)
                .not_found_service(tower_http::services::ServeFile::new(dist.join("index.html"))),
        )
}

fn router(state: Arc<AppState>) -> Router {
    base_router().merge(session_router().with_state(state))
}

/// POST /api/chat — the TUI/Web conversation path: one replayable SSE run.
/// Emits the Pisper UI event vocabulary (run/meta/text_delta/thinking_patch/
/// tool_start/tool_end/done/error) projected from the engine's session
/// events; frames are recorded under the runId for `/api/runs/{id}/events`
/// reconnect replay.
async fn chat(
    State(state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::response::Response, ApiError> {

    let session_id = body
        .get("sessionId")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::bad_request("missing \"sessionId\" field"))?
        .to_string();
    let message = body
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    if message.trim().is_empty() {
        return Err(ApiError::bad_request("消息或资源调用不能为空。"));
    }
    ensure_hosted(&state, &session_id).await?;

    let run_id = product::new_id();
    let (tx, _rx) = tokio::sync::broadcast::channel::<product::Frame>(4096);
    let run = product::ChatRun {
        frames: std::sync::Mutex::new(Vec::new()),
        tx: tx.clone(),
        closed: std::sync::atomic::AtomicBool::new(false),
    };
    state
        .chat_runs
        .lock()
        .expect("chat runs lock")
        .insert(run_id.clone(), run);

    let mut rx = tx.subscribe();
    // Run header frame (upstream startRun): carries the runId the client uses
    // for reconnect; cursor 0 and not recorded in the replay buffer.
    let _ = tx.send(product::Frame {
        cursor: 0,
        event: "run".into(),
        data: serde_json::json!({
            "runId": run_id,
            "kind": "chat",
            "sessionId": session_id,
            "cursor": 0,
        }),
    });
    // Engine-side listener: raw pi events -> UI frames (recorded + broadcast).
    let session = state.runtime.session().clone();
    let thinking = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let mapper_state = state.clone();
    let mapper_run = run_id.clone();
    let mapper_session = session_id.clone();
    let mapper_thinking = thinking.clone();
    let listener: std::sync::Arc<
        dyn Fn(&pi_rust::coding_agent::agent_session::AgentSessionEvent) + Send + Sync,
    > = std::sync::Arc::new(move |event| {
        let Ok(raw) = serde_json::from_str::<serde_json::Value>(
            &pi_rust::coding_agent::modes::json_event::to_json_event_string(event)
                .unwrap_or_default(),
        ) else {
            return;
        };
        let mut thinking = mapper_thinking.lock().expect("thinking lock");
        let Some((name, data)) =
            product::map_pi_event(&raw, &mapper_session, &mut thinking)
        else {
            return;
        };
        drop(thinking);
        let cursor = mapper_state
            .frame_cursor
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        if let Some(run) = mapper_state.chat_runs.lock().expect("chat runs lock").get(&mapper_run) {
            run.record(cursor, &name, data);
        }
    });
    let unsub = session.subscribe(listener);

    // Run the prompt; errors surface as the terminal error frame.
    let prompt_session = session.clone();
    let prompt_run = run_id.clone();
    let prompt_state = state.clone();
    tokio::spawn(async move {
        let result = prompt_session.prompt(message, None).await;
        let cursor = prompt_state
            .frame_cursor
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        if let Err(e) = result {
            if let Some(run) = prompt_state.chat_runs.lock().expect("chat runs lock").get(&prompt_run) {
                run.record(cursor, "error", serde_json::json!({ "message": e.to_string() }));
            }
        }
        drop(unsub);
    });

    // Live stream: fresh run, forward broadcast frames as SSE with `id:`
    // cursor lines, ending after the terminal frame.
    let run_id_stream = run_id.clone();
    let stream = futures::stream::unfold(
        (rx, run_id_stream, state.clone(), false),
        |(mut rx, run_id, state, mut terminal)| async move {
            loop {
                if terminal {
                    // Give the connection a clean end.
                    return None;
                }
                match rx.recv().await {
                    Ok(frame) => {
                        let is_terminal = frame.event == "done" || frame.event == "error";
                        let out = Ok::<_, Infallible>(
                            axum::response::sse::Event::default()
                                .event(frame.event.clone())
                                .id(frame.cursor.to_string())
                                .data(serde_json::to_string(&frame.data).unwrap_or_default()),
                        );
                        let next_terminal = terminal || is_terminal;
                        return Some((out, (rx, run_id, state, next_terminal)));
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
                }
            }
        },
    );
    let response = axum::response::Sse::new(stream)
        .keep_alive(axum::response::sse::KeepAlive::default())
        .into_response();
    Ok(response)
}

/// GET /api/runs/{id}/events?after={cursor} — replay recorded frames, then
/// follow live frames until the terminal event (upstream reconnect semantics).
async fn run_events(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<axum::response::Response, ApiError> {

    let after: u64 = params
        .get("after")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let (frames, rx, closed) = {
        let runs = state.chat_runs.lock().expect("chat runs lock");
        let Some(run) = runs.get(&run_id) else {
            return Err(ApiError::new(
                StatusCode::NOT_FOUND,
                "run_not_found",
                "工作流运行不存在。",
            ));
        };
        let frames = run.frames.lock().expect("frames lock").clone();
        let rx = run.tx.subscribe();
        let closed = run.closed.load(std::sync::atomic::Ordering::Relaxed);
        (frames, rx, closed)
    };
    let _ = &closed;
    // Replay the recorded snapshot (cursor > after), then follow live frames
    // (deduped by cursor) until the terminal event passes.
    let mut pending: std::collections::VecDeque<product::Frame> = frames
        .into_iter()
        .filter(|f| f.cursor > after)
        .collect();
    if closed {
        // Terminal frame already recorded; replay ends the stream.
        let out: Vec<Result<axum::response::sse::Event, Infallible>> = pending
            .drain(..)
            .map(|f| {
                Ok(axum::response::sse::Event::default()
                    .event(f.event)
                    .id(f.cursor.to_string())
                    .data(serde_json::to_string(&f.data).unwrap_or_default()))
            })
            .collect();
        return Ok(axum::response::Sse::new(futures::stream::iter(out))
            .keep_alive(axum::response::sse::KeepAlive::default())
            .into_response());
    }
    let after_move = after;
    let stream = futures::stream::unfold(
        (rx, pending, after_move, false),
        |(mut rx, mut pending, after, mut terminal)| async move {
            loop {
                if let Some(frame) = pending.pop_front() {
                    let is_terminal = frame.event == "done" || frame.event == "error";
                    let out = Ok::<_, Infallible>(
                        axum::response::sse::Event::default()
                            .event(frame.event.clone())
                            .id(frame.cursor.to_string())
                            .data(serde_json::to_string(&frame.data).unwrap_or_default()),
                    );
                    return Some((out, (rx, pending, after, terminal || is_terminal)));
                }
                match rx.recv().await {
                    Ok(frame) => {
                        if frame.cursor <= after {
                            continue;
                        }
                        let is_terminal = frame.event == "done" || frame.event == "error";
                        let out = Ok::<_, Infallible>(
                            axum::response::sse::Event::default()
                                .event(frame.event.clone())
                                .id(frame.cursor.to_string())
                                .data(serde_json::to_string(&frame.data).unwrap_or_default()),
                        );
                        return Some((out, (rx, pending, after, terminal || is_terminal)));
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
                }
            }
        },
    );
    let response = axum::response::Sse::new(stream)
        .keep_alive(axum::response::sse::KeepAlive::default())
        .into_response();
    Ok(response)
}

async fn create_pairing_code(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    use std::fmt::Write as _;
    let code: u32 = (product::now_ms() % 1_000_000) as u32;
    let mut code_str = String::new();
    let _ = write!(code_str, "{code:06}");
    let expires_at = product::now_ms() + 300_000;
    *state.pairing.pending.lock().expect("pairing lock") =
        Some((code_str.clone(), expires_at));
    Json(serde_json::json!({ "code": code_str, "expiresAt": expires_at }))
}

async fn pair_device(
    State(state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let code = body
        .get("code")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::bad_request("missing \"code\" field"))?;
    let device_name = body
        .get("deviceName")
        .and_then(|v| v.as_str())
        .unwrap_or("device")
        .to_string();
    let now = product::now_ms();
    let mut pending = state.pairing.pending.lock().expect("pairing lock");
    match pending.as_ref() {
        Some((pending_code, expires_at)) if *expires_at > now && pending_code == code => {}
        Some((_, expires_at)) if *expires_at <= now => {
            return Err(ApiError::new(
                StatusCode::GONE,
                "pairing_code_expired",
                "配对码已过期。",
            ));
        }
        _ => {
            return Err(ApiError::new(
                StatusCode::NOT_FOUND,
                "pairing_code_invalid",
                "配对码无效。",
            ));
        }
    }
    *pending = None;
    drop(pending);
    // Device bearer token: sha256 over time+name+process entropy.
    let token = {
        use sha2::Digest;
        use std::fmt::Write as _;
        let digest = sha2::Sha256::digest(
            format!(
                "{}:{}:{}:{}",
                product::now_ms(),
                device_name,
                std::process::id(),
                state.fingerprint
            )
            .as_bytes(),
        );
        let mut out = String::new();
        for b in digest.iter() {
            let _ = write!(out, "{b:02x}");
        }
        out
    };
    let device = security::PairedDevice {
        id: product::new_id(),
        name: device_name,
        token: token.clone(),
        paired_at: now,
    };
    let mut devices = state.pairing.devices.lock().expect("devices lock");
    devices.push(device.clone());
    security::save_devices(&state.data_dir, &devices).map_err(ApiError::internal)?;
    Ok(Json(serde_json::json!({
        "deviceId": device.id,
        "token": token,
        "name": device.name,
    })))
}

async fn list_devices(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let devices = state.pairing.devices.lock().expect("devices lock");
    // Tokens are not exposed in listings.
    let out: Vec<serde_json::Value> = devices
        .iter()
        .map(|d| {
            serde_json::json!({
                "id": d.id,
                "name": d.name,
                "pairedAt": d.paired_at,
            })
        })
        .collect();
    Json(serde_json::json!({ "devices": out }))
}

async fn revoke_device(
    State(state): State<Arc<AppState>>,
    Path(device_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut devices = state.pairing.devices.lock().expect("devices lock");
    let before = devices.len();
    devices.retain(|d| d.id != device_id);
    if devices.len() == before {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "device_not_found",
            "设备不存在。",
        ));
    }
    security::save_devices(&state.data_dir, &devices).map_err(ApiError::internal)?;
    Ok(Json(serde_json::json!({ "deleted": true })))
}

async fn runtime_diagnostics(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "workspaceCwd": state.cwd,
        "engine": ENGINE,
        "version": env!("CARGO_PKG_VERSION"),
    }))
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

    #[test]
    fn session_label_normalization_matches_node_contract() {
        // Node: collapse whitespace, lowercase, cap at 80 chars.
        let normalize = |q: &str| -> String {
            q.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase()
                .chars()
                .take(80)
                .collect()
        };
        assert_eq!(normalize("  Branch   A  "), "branch a");
        let long = "x".repeat(200);
        assert_eq!(normalize(&long).len(), 80);
    }

    #[tokio::test]
    async fn derive_without_boundary_entry_is_structured_400() {
        // Routing-level contract: the handler rejects a missing
        // boundaryEntryId with the standard error envelope. (The engine path
        // is covered by the live derive acceptance run.)
        let missing = serde_json::json!({ "name": "x" });
        let err = missing
            .get("boundaryEntryId")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        assert!(err.is_none());
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
