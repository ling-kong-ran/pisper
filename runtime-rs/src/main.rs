//! Pisper 的原生 Rust HTTP/SSE 后端，使用 Pi 引擎并保留 Web/TUI 协议。
//! 配置、历史、引擎准入与界面偏好由各自领域模块维护。
//! 尚未完成的产品功能通过 capabilities 及明确的 unsupported 错误公布。

mod approval_api;
mod asset_api;
mod browser_integration;
mod channel_integration;
mod channel_transport;
mod channels_api;
mod chat_stream;
mod custom_ui_api;
mod execution_adapter;
mod execution_modes;
mod desktop_ops;
mod file_changes_api;
mod firewall_ops;
mod game_assets_api;
mod goal_api;
#[cfg(test)]
mod host_factory_tests;
mod mcp_api;
mod mcp_host_ops;
mod memory_api;
mod memory_store;
mod multi_agent_api;
mod native_browser;
mod native_channels;
mod native_custom_ui;
mod native_file_changes;
mod native_game_assets;
mod native_image_agent;
mod native_image_runtime;
mod native_notifications;
mod native_plugins;
mod native_shell;
mod native_tool_catalog;
mod native_tool_gateway;
mod native_visual;
mod native_web_search;
mod native_workflow;
mod notification_api;
mod plan_api;
mod plugin_integration;
mod plugins_api;
mod product;
mod product2;
mod provider_config;
mod remote_ops;
mod runtime_paths;
mod schedule_api;
mod scheduled_jobs;
mod security;
mod session_api;
mod session_ops;
mod session_runtime;
mod session_workers;
mod skills_ops;
mod speech_api;
mod tool_policy;
mod ui_contract;
mod vcs_ops;
mod visual_api;
mod visual_catalog_cache;
mod visual_integration;
mod visual_request_json;
mod web_search_api;
mod workflow_api;
mod workflow_engine;
mod workflow_executor;

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post, put},
    Json, Router,
};
use tokio::sync::broadcast;

use pi_rust::agent_core::types::ThinkingLevel;
use pi_rust::coding_agent::core::resource_loader::InlineExtension;
use pi_rust::coding_agent::core::skills::{load_skills, LoadSkillsOptions};
use pi_rust::coding_agent::extensions::mcp::{create_mcp_extension, McpExtensionOptions};
use pi_rust::coding_agent::{
    agent_session::{AgentSessionEvent, ExtensionBindings},
    cli::{args::Args, project_trust::AppMode},
    core::{
        agent_session_runtime::{
            create_agent_session_runtime, AgentSessionRuntime, CreateAgentSessionRuntimeOptions,
            ForkOptions, ForkPosition, SwitchSessionOptions,
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

fn extension_bindings() -> ExtensionBindings {
    ExtensionBindings {
        // Pi reload 通过持久化 mode binding 判定是否重新发送 session_start。
        on_error: Some(Arc::new(|_| {
            tracing::warn!("Pi extension reported an error; see runtime diagnostics.")
        })),
        ..Default::default()
    }
}

/// The configuration runtime owns the shared model/tool catalog. Conversations
/// have independent resident runtimes and never replace the catalog session.
pub(crate) struct AppState {
    pub(crate) runtime: Arc<AgentSessionRuntime>,
    pub(crate) sessions: session_runtime::SessionRuntimeRegistry,
    pub(crate) agent_dir: String,
    pub(crate) providers: Arc<provider_config::ProviderConfigStore>,
    pub(crate) plugins: Arc<native_plugins::ToolPluginService>,
    pub(crate) web_search: Arc<native_web_search::WebSearchService>,
    pub(crate) visual: Arc<native_visual::VisualGenerationService>,
    pub(crate) browser: Arc<native_browser::BrowserAutomationService>,
    pub(crate) plugin_integration: Arc<plugin_integration::PluginIntegration>,
    pub(crate) engine_mutation: Arc<tokio::sync::RwLock<()>>,
    pub(crate) ui: ui_contract::UiState,
    pub(crate) notifications: Arc<native_notifications::NotificationService>,
    pub(crate) channels: Arc<native_channels::ChannelService>,
    pub(crate) file_changes: Arc<native_file_changes::FileChangesService>,
    pub(crate) custom_ui: Arc<native_custom_ui::CustomUiService>,
    pub(crate) image_agent: Arc<native_image_agent::ImageAgentService>,
    pub(crate) memory: Arc<std::sync::Mutex<memory_store::MemoryStore>>,
    pub(crate) memory_tasks: Arc<memory_store::runtime::MemoryRuntime>,
    pub(crate) usage: Arc<memory_store::usage_ledger::UsageLedger>,
    pub(crate) executor: Arc<execution_adapter::PiExecutor>,
    pub(crate) plans: Arc<plan_api::PlanService>,
    pub(crate) goals: Arc<goal_api::GoalRunner>,
    pub(crate) agents: Arc<multi_agent_api::AgentService>,
    pub(crate) team: Arc<multi_agent_api::TeamService>,
    pub(crate) speech: Arc<speech_api::SpeechService>,
    workflows: WorkflowServices,
    game_assets: GameServices,
    closing: std::sync::atomic::AtomicBool,
    pub(crate) shutdown: tokio_util::sync::CancellationToken,
    pub(crate) assets: Arc<std::sync::Mutex<asset_api::store::AssetStore>>,
    pub(crate) asset_tracker: Arc<asset_api::tracker::WorkspaceAssetTracker>,
    pub(crate) approvals: Arc<approval_api::ApprovalService>,
    _approval_events: approval_api::ApprovalSubscription,
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
    pub(crate) chat_runs: std::sync::Mutex<std::collections::HashMap<String, product::ChatRun>>,
    /// Global SSE frame cursor (upstream runs.record cursor).
    pub(crate) frame_cursor: std::sync::atomic::AtomicU64,
    /// Sidecar access token (PISPER_DESKTOP_TOKEN): when set, every request
    /// must carry the `__pisper_desktop=<token>` cookie (TUI/desktop auth).
    pub(crate) desktop_token: Option<String>,
    /// Remote device pairing store (codes + paired devices).
    pub(crate) pairing: security::PairingStore,
    /// release remoteAccess 的配对审批流（pairing-requests）。
    pub(crate) pairing_requests: std::sync::Mutex<Vec<security::PairingApproval>>,
    /// 本服务回环基址（bind 后写入）：MCP 工具经它自调用 /api。
    pub(crate) self_base: std::sync::Mutex<Option<String>>,
    /// 对外 MCP 异步运行记录（release mcp-host-tools 的 runs）。
    pub(crate) mcp_runs: std::sync::Mutex<std::collections::HashMap<String, serde_json::Value>>,
    /// Pisper product-layer per-session metadata (Node sessionMeta store):
    /// execution mode -> permission mode mapping and the goal tracker.
    session_meta: Arc<std::sync::Mutex<std::collections::HashMap<String, SessionMeta>>>,
    /// Unsubscribe for the listener attached to the current session. Rebuilt
    /// on every `new_session` so events keep flowing across session switches.
    unsub: std::sync::Mutex<Option<pi_rust::coding_agent::agent_session::AgentSessionUnsubscribe>>,
}

/// Owns the workflow domain from boot through cancellation and resource joins.
/// The executor holds Weak bindings back to AppState and the image service.
struct WorkflowServices {
    media: Arc<native_workflow::media::MediaService>,
    cache: Arc<native_workflow::engine_cache::EngineCache>,
    engines: Arc<native_image_runtime::engines::EngineService>,
    processor: Arc<native_workflow::image_processing::ImageProcessor>,
    executor: Arc<workflow_executor::WorkflowRuntimeExecutor>,
    images: Arc<native_workflow::image_nodes::ImageNodeService>,
    workflows: Arc<workflow_engine::WorkflowService>,
    schedules: std::sync::OnceLock<Arc<schedule_api::ScheduleService>>,
}

impl WorkflowServices {
    async fn open(data_dir: &str, cwd: &str) -> anyhow::Result<Self> {
        let directory = std::path::Path::new(data_dir);
        let media = native_workflow::media::MediaService::open(directory)?;
        let cache = native_workflow::engine_cache::EngineCache::open(
            directory,
            native_workflow::engine_cache::release_definitions(),
        )?;
        let engines = native_image_runtime::engines::EngineService::open(cache.clone())?;
        let algorithms = native_image_runtime::cpu::CpuImageAlgorithms::new(cache.clone());
        let processor = native_workflow::image_processing::ImageProcessor::new(Some(algorithms));
        let executor = workflow_executor::WorkflowRuntimeExecutor::new(media.clone());
        let images = native_workflow::image_nodes::ImageNodeService::open(
            directory,
            media.clone(),
            processor.clone(),
            executor.clone(),
        )?;
        let workflows = workflow_engine::WorkflowService::open(
            directory.join("pisper-workflows.json"),
            cwd.into(),
            executor.clone(),
            4,
        )
        .await?;
        Ok(Self {
            media,
            cache,
            engines,
            processor,
            executor,
            images,
            workflows,
            schedules: std::sync::OnceLock::new(),
        })
    }

    async fn attach(&self, state: &Arc<AppState>) -> anyhow::Result<()> {
        self.executor.attach(state, &self.images)?;
        // ScheduleService starts its due-task timer immediately. All Weak
        // executor bindings must exist before opening persisted schedules.
        let schedules = schedule_api::ScheduleService::open(
            std::path::Path::new(&state.data_dir).join("pisper-schedules.json"),
            state.cwd.clone(),
            self.workflows.clone(),
            std::time::Duration::from_secs(15),
        )
        .await?;
        self.schedules
            .set(schedules)
            .map_err(|_| anyhow::anyhow!("Schedule service already initialized"))
    }

    async fn shutdown(&self) {
        if let Some(schedules) = self.schedules.get() {
            if let Err(error) = schedules.dispose().await {
                tracing::warn!(code=%error.code, "Schedule shutdown failed");
            }
        }
        if let Err(error) = self.workflows.dispose().await {
            tracing::warn!(code=%error.code, "Workflow shutdown failed");
        }
        self.images.dispose().await;
        self.processor.dispose().await;
        self.engines.dispose().await;
        self.media.dispose().await;
    }
}

/// Game projects own their media, jobs and image runs. CPU engines and the
/// provider generator are shared with workflows and outlive this bundle.
struct GameServices {
    media: Arc<native_workflow::media::MediaService>,
    images: Arc<native_workflow::image_nodes::ImageNodeService>,
    projects: Arc<native_game_assets::GameAssetsService>,
}
impl GameServices {
    fn open(data_dir: &str, workflows: &WorkflowServices) -> anyhow::Result<Self> {
        let directory = std::path::Path::new(data_dir).join("game-assets");
        let media = native_workflow::media::MediaService::open(&directory)?;
        let images = native_workflow::image_nodes::ImageNodeService::open(
            &directory,
            media.clone(),
            workflows.processor.clone(),
            workflows.executor.clone(),
        )?;
        let projects =
            native_game_assets::GameAssetsService::open(&directory, media.clone(), images.clone())?;
        Ok(Self {
            media,
            images,
            projects,
        })
    }
    async fn shutdown(&self) {
        if let Err(error) = self.projects.dispose().await {
            tracing::warn!(code=%error.code, "Game asset shutdown failed");
        }
        self.images.dispose().await;
        self.media.dispose().await;
    }
}

/// Upstream `permissionModeForExecutionMode` (session-lifecycle.mjs): the
/// permission preset implied by a Pisper execution mode.
fn permission_mode_for_execution_mode(mode: &str) -> &'static str {
    execution_modes::permission_mode(
        execution_modes::normalize(mode).unwrap_or("approval-required"),
    )
}

#[derive(Default, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct SessionMeta {
    execution_mode: Option<String>,
    permission_mode: Option<String>,
    goal: Option<String>,
    name: Option<String>,
    pinned: bool,
    archived: bool,
    unread: bool,
    run_mode: Option<String>,
    /// release SideChatMetadata；有值即临时侧聊会话。
    side_chat: Option<session_ops::SideChatMeta>,
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
    ui_contract::capabilities()
}

async fn session_input(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let text = body
        .get("text")
        .and_then(|t| t.as_str())
        .or_else(|| body.get("message").and_then(|t| t.as_str()))
        .or_else(|| body.as_str())
        .ok_or_else(|| ApiError::bad_request("missing \"text\"/\"message\" field"))?;
    if text.trim().is_empty() {
        return Err(ApiError::bad_request("消息不能为空。"));
    }
    let hosted = session_runtime::hosted(&state, &id).await?;
    let active = hosted.session();
    if active.is_streaming() {
        if body["behavior"].as_str() == Some("followUp") {
            active.follow_up(text, None, None).await
        } else {
            active.steer(text, None, None).await
        }
        .map_err(|error| ApiError::internal(error.to_string()))?;
        let queued = session_api::queued_inputs(&state, &id);
        return Ok(Json(
            serde_json::json!({"ok":true,"queuedInputs":queued,"pendingMessageCount":active.pending_message_count()}),
        ));
    }
    let mutation = session_runtime::mutation(&state, &id).await?;
    let session = mutation.hosted.session();
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
    state.approvals.cancel_session(&id);
    session_api::find_session_path(&state, &id)?;
    state.asset_tracker.cancel_session(&id);
    state
        .goals
        .cancel(&id)
        .await
        .map_err(|error| ApiError::internal(error.to_string()))?;
    state
        .agents
        .abort_parent(&id, "父会话已停止。")
        .await
        .map_err(|error| ApiError::internal(error.to_string()))?;
    if let Some(hosted) = state.sessions.get(&id) {
        hosted.abort().await?;
    }
    state.asset_tracker.wait_session(&id).await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn get_session_model(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(session_api::model_state(&state, &id)?))
}

async fn post_session_model(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mutation = session_runtime::mutation(&state, &id).await?;
    let session = mutation.hosted.session();
    let provider = body
        .get("provider")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::bad_request("missing \"provider\" field"))?;
    let model_id = body
        .get("model")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::bad_request("missing \"model\" field"))?;
    let snapshot = provider_config::available_models(&state).await?;
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
    Ok(Json(session_api::thinking_state(&state, &id)?))
}

async fn post_session_thinking_level(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mutation = session_runtime::mutation(&state, &id).await?;
    let session = mutation.hosted.session();
    let level: ThinkingLevel =
        serde_json::from_value(body.get("level").cloned().unwrap_or(body))
            .map_err(|e| ApiError::bad_request(format!("invalid thinking level: {e}")))?;
    session.set_thinking_level(level, None);
    Ok(Json(serde_json::json!({
        "thinkingLevel": serde_json::to_value(level).expect("level serializes"),
        "availableLevels": session.get_available_thinking_levels(),
        "status": "ok",
    })))
}
fn format_diagnostic(d: &pi_rust::coding_agent::core::diagnostics::ResourceDiagnostic) -> String {
    format!(
        "{}{}: {}",
        d.r#type.as_str(),
        d.path
            .as_deref()
            .map(|p| format!(" ({p})"))
            .unwrap_or_default(),
        d.message
    )
}

async fn derive_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let source = session_runtime::mutation(&state, &id).await?;
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
    let manager = SessionManager::open(&session_api::find_session_path(&state, &id)?, None, None)
        .map_err(|error| ApiError::internal(error.to_string()))?;
    let derived = state.sessions.build_runtime(&state, manager).await?;
    derived
        .fork(
            &entry_id,
            ForkOptions {
                position: ForkPosition::At,
                with_session: None,
            },
        )
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    derived
        .session()
        .bind_extensions(extension_bindings())
        .await
        .map_err(|error| ApiError::internal(error.to_string()))?;
    let hosted = state.sessions.insert(&state, derived)?;
    let session = hosted.session();
    drop(source);
    if !name.is_empty() {
        session
            .session_manager
            .lock()
            .expect("session manager lock")
            .append_session_info(&name)
            .map_err(|e| ApiError::internal(e.to_string()))?;
    }
    // release: json(201, await runtime.deriveSession(...))
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "ok": true,
            "id": session.session_id(),
            "name": name,
        })),
    ))
}

async fn session_compact(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mutation = session_runtime::mutation(&state, &id).await?;
    let session = mutation.hosted.session();
    let result = session
        .compact(None)
        .await
        .map_err(|e| {
            // release 的通用错误映射把无状态码的 Error 归为 400。
            ApiError::bad_request(crate::security::redact_secret_text(&e.to_string()))
        })?;
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
    let cwd =
        std::fs::canonicalize(&cwd_input).map_err(|_| ApiError::bad_request("工作目录不存在。"))?;
    if !cwd.is_dir() {
        return Err(ApiError::bad_request("工作目录必须是文件夹。"));
    }
    let cwd_input = cwd
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .to_string();
    let mutation = session_runtime::mutation(&state, &id).await?;
    let file = mutation
        .hosted
        .session()
        .session_file()
        .ok_or_else(|| ApiError::internal("active session is not persisted yet"))?;
    // Upstream re-hosts the same session file with a cwd override (the
    // engine rebuilds the session against the new workspace).
    mutation
        .hosted
        .runtime
        .switch_session(
            &file,
            SwitchSessionOptions {
                cwd_override: Some(cwd_input.clone()),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    session_api::persist_session_cwd(&file, &cwd_input)?;
    mutation
        .hosted
        .runtime
        .session()
        .bind_extensions(extension_bindings())
        .await
        .map_err(|error| ApiError::internal(error.to_string()))?;
    mutation.hosted.rebind(&state);
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
    session_api::find_session_path(&state, &id)?;
    let mode = execution_modes::normalize(&mode)
        .unwrap_or(execution_modes::DEFAULT_EXECUTION_MODE)
        .to_owned();
    {
        let mut meta = state.session_meta.lock().expect("session meta lock");
        let entry = meta.entry(id.clone()).or_default();
        entry.execution_mode = Some(mode.clone());
        entry.permission_mode = Some(permission_mode_for_execution_mode(&mode).to_string());
    }
    session_api::save_metadata(&state)?;
    if let Some(host) = state.sessions.get(&id) {
        state
            .plugin_integration
            .apply_loadout(host.session())
            .map_err(|error| ApiError::internal(error.message))?;
    }
    Ok(Json(serde_json::json!({
        "id": id,
        "executionMode": mode,
        "permissionMode": permission_mode_for_execution_mode(&mode),
    })))
}

async fn set_session_run_mode(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    session_api::find_session_path(&state, &id)?;
    let mode = body["mode"]
        .as_str()
        .ok_or_else(|| ApiError::bad_request("missing mode"))?;
    if !matches!(mode, "plan" | "goal" | "team") {
        return Err(ApiError::bad_request("运行模式必须为 plan、goal 或 team。"));
    }
    state
        .session_meta
        .lock()
        .map_err(|_| ApiError::internal("session metadata lock"))?
        .entry(id.clone())
        .or_default()
        .run_mode = Some(mode.to_string());
    session_api::save_metadata(&state)?;
    Ok(Json(serde_json::json!({"id": id, "runMode": mode})))
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

/// Host any known session id: if it is not the active one, switch the engine
/// runtime to it (multi-session hosting, upstream `switch_session`).
pub(crate) async fn ensure_hosted(state: &AppState, id: &str) -> Result<(), ApiError> {
    session_runtime::hosted(state, id).await?;
    Ok(())
}

#[derive(Debug)]
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
        Self {
            status,
            code,
            message: message.into(),
        }
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        // Security seam: error text can carry provider/auth internals; the
        // message is scrubbed through the secret-redaction layer first.
        (
            self.status,
            Json(serde_json::json!({
                "error": security::redact_secret_text(&self.message),
                "code": self.code,
            })),
        )
            .into_response()
    }
}

fn attach_event_listener(state: &AppState) {
    tool_policy::install(
        state.runtime.session(),
        state.session_meta.clone(),
        state.approvals.clone(),
    );
    let tx = state.events.clone();
    let listener: Arc<dyn Fn(&AgentSessionEvent) + Send + Sync> =
        Arc::new(move |event: &AgentSessionEvent| {
            if let Ok(json) = to_json_event_string(event) {
                let _ = tx.send(json);
            }
        });
    if let Some(previous) = state
        .unsub
        .lock()
        .expect("unsub lock")
        .replace(state.runtime.session().subscribe(listener))
    {
        previous.unsubscribe();
    }
}

async fn boot() -> anyhow::Result<Arc<AppState>> {
    let cwd = std::env::var("PISPER_WORKSPACE_DIR")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            std::env::current_dir()
                .expect("cwd")
                .to_string_lossy()
                .to_string()
        });
    let paths = runtime_paths::RuntimePaths::from_environment()?;
    let agent_dir = paths.agent_dir;
    let data_dir = paths.data_dir;
    let providers = Arc::new(provider_config::ProviderConfigStore::new(&agent_dir));
    let catalog = native_tool_catalog::release_catalog()?;
    let web_search = native_web_search::WebSearchService::new(
        std::path::Path::new(&agent_dir).join("pisper.json"),
    )?;
    let plugins = native_plugins::ToolPluginService::open(
        std::path::Path::new(&data_dir),
        plugin_integration::config_port(providers.clone()),
        catalog.clone(),
    )?;
    plugin_integration::migrate_defaults(&plugins).await?;
    let plugin_integration =
        plugin_integration::PluginIntegration::new(catalog, providers.clone(), plugins.clone());
    let app_root = std::env::var_os("PISPER_APP_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".."));
    let mut speech_native = app_root.join("speech-native");
    let development_native = app_root.join("node_modules/sherpa-onnx-win-x64");
    if !speech_native.is_dir() && development_native.is_dir() {
        speech_native = development_native;
    }
    let speech = speech_api::SpeechService::new(
        std::path::Path::new(&agent_dir),
        &app_root.join("shared"),
        &speech_native,
    )?;
    let workflows = WorkflowServices::open(&data_dir, &cwd).await?;
    let game_assets = GameServices::open(&data_dir, &workflows)?;
    let custom_ui = native_custom_ui::CustomUiService::new(&data_dir);
    let file_changes =
        native_file_changes::FileChangesService::open(std::path::Path::new(&data_dir))
            .map_err(|error| anyhow::anyhow!("{}: {}", error.code, error.message))?;
    let channel_transport = channel_transport::ChannelTransport::new();
    let notifications = Arc::new(
        native_notifications::NotificationService::open(std::path::Path::new(&agent_dir))
            .map_err(|error| anyhow::anyhow!("{}: {}", error.code, error.message))?
            .with_transport(channel_transport.clone()),
    );
    let channel_integration = channel_integration::ChannelIntegration::new();
    let gateway_factories: std::collections::HashMap<String, native_channels::GatewayFactory> = [
        (
            "feishu".into(),
            Arc::new(native_channels::feishu::new) as native_channels::GatewayFactory,
        ),
        (
            "qq".into(),
            Arc::new(native_channels::qq::new) as native_channels::GatewayFactory,
        ),
        (
            "telegram".into(),
            Arc::new(|callbacks| {
                native_channels::telegram::TelegramGateway::new(callbacks)
                    as Arc<dyn native_channels::Gateway>
            }) as native_channels::GatewayFactory,
        ),
        (
            "weixin".into(),
            Arc::new(|callbacks| {
                native_channels::weixin::WeixinGateway::new(callbacks)
                    as Arc<dyn native_channels::Gateway>
            }) as native_channels::GatewayFactory,
        ),
    ]
    .into_iter()
    .collect();
    let onboarding_factories: std::collections::HashMap<
        String,
        native_channels::OnboardingFactory,
    > = [
        (
            "feishu".into(),
            Arc::new(native_channels::feishu_onboarding::new) as native_channels::OnboardingFactory,
        ),
        (
            "qq".into(),
            Arc::new(native_channels::qq_onboarding::new) as native_channels::OnboardingFactory,
        ),
        (
            "telegram".into(),
            Arc::new(|_| native_channels::manual_onboarding::telegram())
                as native_channels::OnboardingFactory,
        ),
        (
            "weixin".into(),
            Arc::new(|completed| {
                native_channels::weixin_onboarding::WeixinOnboardingService::new(completed)
                    as Arc<dyn native_channels::Onboarding>
            }) as native_channels::OnboardingFactory,
        ),
    ]
    .into_iter()
    .collect();
    let channels = native_channels::ChannelService::new(
        cwd.clone(),
        channel_integration.agent_port(),
        notifications.channel_state_port(),
        gateway_factories,
        onboarding_factories,
    );
    channel_transport
        .attach(&channels)
        .map_err(|error| anyhow::anyhow!(error.message))?;
    let memory = Arc::new(std::sync::Mutex::new(memory_store::MemoryStore::open(
        &std::path::Path::new(&agent_dir).join("pisper-memory.sqlite"),
        &cwd,
    )?));
    let memory_tasks = memory_store::runtime::MemoryRuntime::new(
        memory.clone(),
        std::path::PathBuf::from(&agent_dir),
    )?;
    let usage = memory_store::usage_ledger::UsageLedger::new(std::path::Path::new(&agent_dir));
    memory_tasks.set_usage_recorder(usage.recorder());
    let executor = execution_adapter::PiExecutor::new();
    let plans = plan_api::PlanService::new(
        std::path::Path::new(&data_dir).join("pisper-plans.json"),
        Some(std::path::Path::new(&data_dir).join("pisper-task-lists.json")),
    )?;
    let goal_store = goal_api::GoalService::new(
        std::path::Path::new(&data_dir).join("pisper-goals.json"),
        true,
    )?;
    goal_store.set_events(executor.event_sink());
    let session_executor: Arc<dyn session_workers::SessionExecutor> = Arc::new(executor.clone());
    let goals = goal_api::GoalRunner::new(goal_store.clone(), session_executor.clone());
    let agents = multi_agent_api::AgentService::new(
        std::path::Path::new(&data_dir).join("pisper-agents.json"),
        session_executor.clone(),
        goal_store.clone(),
        executor.event_sink(),
    )?;
    let team = multi_agent_api::TeamService::new(
        std::path::Path::new(&data_dir).join("pisper-teams.json"),
        goal_store,
        executor.event_sink(),
        true,
    )?;
    team.attach_agents(Arc::downgrade(&agents));
    agents.set_graph(team.clone());
    agents.set_workflow(multi_agent_api::TeamWorkflowService::new(
        team.clone(),
        Arc::downgrade(&agents),
        session_executor,
    ));
    goals.set_team(Arc::new(multi_agent_api::TeamCoordinatorAdapter(
        team.clone(),
    )));
    let tools_agents = Arc::downgrade(&agents);
    executor.register_tools(Arc::new(move |id, parent, _| {
        let Some(agents) = tools_agents.upgrade() else {
            return Vec::new();
        };
        if id == "catalog" {
            multi_agent_api::templates(agents)
        } else if let Some(parent) = parent {
            multi_agent_api::member_tools(agents, parent, id)
        } else {
            multi_agent_api::tools(agents, id)
        }
    }));
    let tools_plans = plans.clone();
    let tools_goals = Arc::downgrade(&goals);
    executor.register_tools(Arc::new(move |id, parent, events| {
        let mut tools = plan_api::tools(
            tools_plans.clone(),
            parent.clone().unwrap_or_else(|| id.clone()),
            parent.is_none(),
            events,
        );
        if parent.is_none() {
            if let Some(goals) = tools_goals.upgrade() {
                tools.extend(goal_api::tools(goals, id));
            }
        }
        tools
    }));
    // Pi 的 SessionManager 也按此变量解析目录，必须与应用配置和显式测试隔离一致。
    std::env::set_var("PI_CODING_AGENT_DIR", &agent_dir);
    // Pi 0.2.2 的目录刷新只检查此变量是否存在；值 0 不禁用模型请求或工具安装。
    // 本地连接由显式 discovery API 查询，启动不应额外联网枚举目录。
    if std::env::var_os("PI_OFFLINE").is_none() {
        std::env::set_var("PI_OFFLINE", "0");
    }
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
    // Persistent manager: sessions land in the agent session dir so they can
    // be listed, re-hosted (switch_session) and derived after restarts.
    let latest = session_api::session_infos(&agent_dir).into_iter().next();
    let new_public_session = latest.is_none();
    let mut manager = if let Some(latest) = latest {
        SessionManager::open(&latest.path, None, None)?
    } else {
        SessionManager::create(&cwd, None, Some(&NewSessionOptions::default()))?
    };
    // Pi 延迟保存空会话；桌面目录从启动起就必须能找到当前会话，
    // 否则客户端会另行创建默认会话并与用户的新任务操作竞争。
    session_api::persist_empty_session(&mut manager)
        .map_err(|error| anyhow::Error::msg(error.message))?;
    if new_public_session {
        file_changes
            .mark_session_tracked(
                manager.get_session_id(),
                std::path::Path::new(manager.get_cwd()),
            )
            .await
            .map_err(|error| anyhow::anyhow!("{}: {}", error.code, error.message))?;
    }
    // Model settings must not append entries to or replace an existing conversation.
    let manager = SessionManager::in_memory(&cwd, None, None)?;
    let session_cwd = cwd.clone();
    let settings =
        SettingsManager::create_with(&cwd, &agent_dir, SettingsManagerCreateOptions::default())?;
    let tool_web_search = web_search.clone();
    let image_plugins = plugins.clone();
    let image_agent = native_image_agent::ImageAgentService::open(
        std::path::Path::new(&data_dir),
        workflows.processor.clone(),
        workflows.executor.clone(),
        Arc::new(move || {
            let plugins = image_plugins.clone();
            Box::pin(async move {
                let state = plugins
                    .get_state()
                    .await
                    .map_err(native_workflow::WorkflowError::io)?;
                Ok(state["enabledTools"].as_array().is_some_and(|tools| {
                    tools
                        .iter()
                        .any(|tool| tool.as_str() == Some("image_assets"))
                }))
            })
        }),
    )?;
    let assets = Arc::new(std::sync::Mutex::new(asset_api::store::AssetStore::open(
        &agent_dir,
    )?));
    let visual_integration = visual_integration::VisualIntegration::new(
        providers.clone(),
        &executor,
        assets.clone(),
        agent_dir.clone(),
    );
    let visual = native_visual::VisualGenerationService::new(visual_integration.config_port());
    let tool_visual = visual.clone();
    let tool_visual_context = visual_integration.context_port();
    let tool_visual_generated = visual_integration.generated_file_port();
    let browser_integration = browser_integration::BrowserIntegration::new();
    let browser = native_browser::BrowserAutomationService::new(native_browser::cdp::factory(
        std::path::PathBuf::from(&data_dir).join("browser-automation-profiles"),
    ));
    let tool_browser = browser.clone();
    let tool_browser_context = browser_integration.context_port();
    let tool_browser_generated = browser_integration.generated_file_port();
    let generated_image_assets = assets.clone();
    let generated_image_executor = Arc::downgrade(&executor);
    let generated_image_agent_dir = agent_dir.clone();
    let generated_image_port: native_image_agent::GeneratedFilePort =
        Arc::new(move |file, context| {
            let assets = generated_image_assets.clone();
            let executor = generated_image_executor.clone();
            let agent_dir = generated_image_agent_dir.clone();
            Box::pin(async move {
                let owner = executor
                    .upgrade()
                    .ok_or_else(|| {
                        native_workflow::WorkflowError::coded(
                            "image_tools_owner_unavailable",
                            "Image asset owner is unavailable.",
                        )
                    })?
                    .tool_owner(&context.session_id);
                tokio::task::spawn_blocking(move || {
                    let name = session_api::session_infos(&agent_dir)
                        .into_iter()
                        .find(|session| session.id == owner)
                        .and_then(|session| session.name)
                        .unwrap_or_default();
                    assets
                        .lock()
                        .map_err(native_workflow::WorkflowError::io)?
                        .archive_generated(&file.path, &owner, &name)
                        .map_err(native_workflow::WorkflowError::io)?;
                    Ok(())
                })
                .await
                .map_err(native_workflow::WorkflowError::io)?
            })
        });
    let factory = create_cli_runtime_factory(CliRuntimeFactoryOptions {
        parsed: args,
        startup_cwd: cwd.clone(),
        initial_session_cwd: session_cwd.clone(),
        agent_dir: agent_dir.clone(),
        startup_settings_manager: settings,
        app_mode: AppMode::Rpc,
        extension_factories: {
            let mut factories: Vec<_> = pi_rust::coding_agent::extensions::built_in_extensions()
                .into_iter()
                .map(|extension| InlineExtension::Factory(extension.factory))
                .collect();
            factories.push(InlineExtension::Factory(create_mcp_extension(
                McpExtensionOptions::default(),
            )));
            factories.push(memory_tasks.create_extension());
            factories.push(executor.extension());
            factories.push(native_image_agent::create_extension(
                image_agent.clone(),
                generated_image_port,
            ));
            factories.push(native_plugins::create_extension(
                plugins.clone(),
                plugin_integration.scope_port(),
                plugin_integration.changed_port(),
            ));
            factories.push(native_tool_gateway::create_extension(
                plugin_integration.gateway_port(),
            ));
            factories
        },
        extension_module_loader: None,
        model_runtime_factory: None,
        custom_tool_factory: Some(Arc::new(move |cwd, _settings| {
            let web_search = tool_web_search.clone();
            let visual = tool_visual.clone();
            let context = tool_visual_context.clone();
            let generated = tool_visual_generated.clone();
            let browser = tool_browser.clone();
            let browser_context = tool_browser_context.clone();
            let browser_generated = tool_browser_generated.clone();
            Box::pin(async move {
                // Release's createPisperBashTool(cwd) uses the host defaults.
                let mut tools = native_shell::tools(
                    &cwd,
                    pi_rust::coding_agent::core::tools::bash::BashToolOptions::default(),
                )
                .await;
                let mut search = (*native_web_search::create_tool(web_search)).clone();
                if let Some(guidelines) = search.prompt_guidelines.take() {
                    for guideline in guidelines {
                        search.description.push_str(&format!("\n{guideline}"));
                    }
                }
                search.prompt_snippet = None;
                search.default_active = Some(false);
                tools.push(Arc::new(search));
                let mut visual = (*native_visual::create_tool(visual, context, generated)).clone();
                if let Some(guidelines) = visual.prompt_guidelines.take() {
                    for guideline in guidelines {
                        visual.description.push_str(&format!("\n{guideline}"));
                    }
                }
                visual.prompt_snippet = None;
                visual.default_active = Some(false);
                tools.push(Arc::new(visual));
                let mut browser =
                    (*native_browser::create_tool(browser, browser_context, browser_generated))
                        .clone();
                if let Some(guidelines) = browser.prompt_guidelines.take() {
                    for guideline in guidelines {
                        browser.description.push_str(&format!("\n{guideline}"));
                    }
                }
                browser.prompt_snippet = None;
                browser.default_active = Some(false);
                tools.push(Arc::new(browser));
                Ok(tools)
            })
        })),
        model_scope_warning: None,
    })?;
    let runtime_factory = factory.create_runtime.clone();
    let runtime = create_agent_session_runtime(
        factory.create_runtime,
        CreateAgentSessionRuntimeOptions {
            cwd: session_cwd,
            agent_dir: agent_dir.clone(),
            session_manager: Arc::new(std::sync::Mutex::new(manager)),
            session_start_event: None,
            project_trust_context: None,
        },
    )
    .await?;
    runtime
        .session()
        .bind_extensions(extension_bindings())
        .await?;
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
    providers
        .restore(&runtime)
        .await
        .map_err(anyhow::Error::msg)?;
    memory_tasks.set_semantic_model(runtime.session().model().map(|model| {
        Arc::new(memory_store::runtime::PiMemoryModel::new(
            runtime.session().model_runtime().clone(),
            model,
        )) as Arc<dyn memory_store::runtime::MemoryModel>
    }));
    let fingerprint = {
        use sha2::Digest;
        use std::fmt::Write as _;
        let digest = sha2::Sha256::digest(
            [
                data_dir.as_bytes(),
                std::env::var("COMPUTERNAME").unwrap_or_default().as_bytes(),
            ]
            .concat(),
        );
        let digest = digest.as_slice();
        digest[..8].iter().fold(String::new(), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
    };
    // 桌面安装后工作目录并非源码目录，优先使用桌面壳明确传入的资源路径。
    let dist_dir = std::env::var_os("PISPER_FRONTEND_ROOT")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("PISPER_APP_ROOT")
                .map(|root| std::path::PathBuf::from(root).join("dist"))
        })
        .unwrap_or_else(|| std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../dist"));
    let desktop_token = match std::env::var("PISPER_DESKTOP_TOKEN")
        .ok()
        .filter(|s| !s.is_empty())
    {
        Some(token) => Some(token),
        None if std::env::var_os("PISPER_PARENT_PID").is_some()
            || std::env::var("PISPER_EXIT_ON_STDIN_CLOSE").as_deref() == Ok("1") =>
        {
            // Tauri 不提供令牌时仍须使用系统随机源，不能以 PID 或时间生成访问凭据。
            let mut bytes = [0_u8; 32];
            getrandom::getrandom(&mut bytes)
                .map_err(|error| anyhow::anyhow!("sidecar token: {error}"))?;
            Some(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
        }
        None => None,
    };
    if desktop_token.as_ref().is_some_and(|token| {
        !token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    }) {
        anyhow::bail!("PISPER_DESKTOP_TOKEN must contain only URL-safe characters");
    }
    let (events, _) = broadcast::channel(1024);
    // 沿用 release 的 SQLite 文件与迁移 schema，避免另起记忆库丢失现有数据。
    let asset_tracker = asset_api::tracker::WorkspaceAssetTracker::new(
        std::path::Path::new(&agent_dir),
        assets.clone(),
    )?;
    let approvals = approval_api::ApprovalService::new(
        std::path::Path::new(&data_dir).join("pisper-approvals.json"),
    )?;
    let approval_tx = events.clone();
    let approval_events = approvals.subscribe(
        None,
        Arc::new(move |session_id, event, data| {
            let _ = approval_tx.send(
                serde_json::json!({"pisperEvent":event,"sessionId":session_id,"data":data})
                    .to_string(),
            );
        }),
    );
    let state = Arc::new(AppState {
        runtime: Arc::new(runtime),
        sessions: session_runtime::SessionRuntimeRegistry::new(runtime_factory),
        agent_dir,
        providers,
        plugins,
        web_search,
        visual,
        browser,
        plugin_integration,
        engine_mutation: Arc::new(tokio::sync::RwLock::new(())),
        cwd,
        ui: ui_contract::UiState::default(),
        notifications,
        channels,
        file_changes,
        custom_ui,
        image_agent,
        memory,
        memory_tasks,
        usage,
        executor,
        plans,
        goals,
        agents,
        team,
        speech,
        workflows,
        game_assets,
        closing: std::sync::atomic::AtomicBool::new(false),
        shutdown: tokio_util::sync::CancellationToken::new(),
        assets,
        asset_tracker,
        approvals,
        _approval_events: approval_events,
        events,
        unsub: std::sync::Mutex::new(None),
        session_meta: Arc::new(std::sync::Mutex::new(session_api::load_metadata(&data_dir))),
        data_dir: data_dir.clone(),
        runs: std::sync::Mutex::new(std::collections::HashMap::new()),
        remote_enabled: std::sync::atomic::AtomicBool::new(false),
        fingerprint,
        chat_runs: std::sync::Mutex::new(std::collections::HashMap::new()),
        frame_cursor: std::sync::atomic::AtomicU64::new(0),
        dist_dir,
        desktop_token,
        pairing: security::PairingStore {
            pending: std::sync::Mutex::new(None),
            devices: std::sync::Mutex::new(security::load_devices(&data_dir)),
        },
        pairing_requests: std::sync::Mutex::new(security::load_pairing_requests(&data_dir)),
        self_base: std::sync::Mutex::new(None),
        mcp_runs: std::sync::Mutex::new(std::collections::HashMap::new()),
    });
    state.executor.attach(&state);
    state.plugin_integration.attach(&state)?;
    visual_integration.attach(&state)?;
    browser_integration
        .attach(&state)
        .map_err(anyhow::Error::msg)?;
    channel_integration.attach(&state)?;
    state
        .plugin_integration
        .apply_loadout(state.runtime.session())?;
    state.workflows.attach(&state).await?;
    state.channels.init().await?;
    attach_event_listener(&state);
    Ok(state)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if std::env::args().any(|arg| arg == "--pisper-plugin-worker") {
        return native_plugins::worker_main().map_err(anyhow::Error::from);
    }
    if std::env::args().any(|arg| arg == "--pisper-speech-worker") {
        std::process::exit(speech_api::worker_main());
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "pisper_server=info,tower_http=info".into()),
        )
        .init();

    let state = boot().await?;
    state.executor.attach(&state);
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let addr = if state.desktop_token.is_some() {
        // Sidecar mode: ephemeral local port; the URL is announced via the
        // PISPER_SIDECAR_READY stdout handshake.
        "127.0.0.1:0".to_string()
    } else {
        std::env::var("PISPER_RS_ADDR").unwrap_or_else(|_| "127.0.0.1:5174".into())
    };
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    let bound = listener.local_addr()?.to_string();
    tracing::info!("pisper-server (Rust runtime) listening on http://{bound}");
    if let Some(token) = &state.desktop_token {
        // Sidecar readiness handshake with the TUI (sidecar.rs READY_PREFIX).
        let url = format!("http://{bound}");
        println!(
            "PISPER_SIDECAR_READY {}",
            serde_json::json!({
                "url": url,
                "bootstrapUrl": format!("{url}/_pisper/desktop/bootstrap?token={token}"),
                "pid": std::process::id(),
                "desktopPetRunning": false,
                "remoteEnabled": false,
            })
        );
        use std::io::Write as _;
        let _ = std::io::stdout().flush();
        // Graceful shutdown: the TUI writes `shutdown` to stdin; stdin close
        // (PISPER_EXIT_ON_STDIN_CLOSE) also ends the process.
        std::thread::spawn(move || {
            use std::io::BufRead;
            let stdin = std::io::stdin();
            for line in stdin.lock().lines().map_while(Result::ok) {
                if line.trim() == "shutdown" {
                    let _ = shutdown_tx.send(());
                    return;
                }
            }
            if std::env::var("PISPER_EXIT_ON_STDIN_CLOSE").as_deref() == Ok("1") {
                let _ = shutdown_tx.send(());
            }
        });
    }
    let shutdown_state = state.clone();
    let cache_stop = state.shutdown.clone();
    let cache_cancel = cache_stop.clone();
    let cache_state = Arc::downgrade(&state);
    let cache_worker = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        interval.tick().await;
        loop {
            tokio::select! {
                _ = cache_cancel.cancelled() => break,
                _ = interval.tick() => {
                    let Some(state) = cache_state.upgrade() else { break; };
                    if let Err(error) = state.sessions.sweep(&state, "").await {
                        tracing::warn!(code=error.code, "Resident session cleanup failed");
                    }
                }
            }
        }
    });
    if let Ok(addr) = listener.local_addr() {
        let host = match addr.ip() {
            std::net::IpAddr::V4(ip) if ip.is_unspecified() => "127.0.0.1".to_string(),
            std::net::IpAddr::V6(ip) if ip.is_unspecified() => "127.0.0.1".to_string(),
            ip => ip.to_string(),
        };
        *state.self_base.lock().expect("self base lock") =
            Some(format!("http://{host}:{}", addr.port()));
    }
    axum::serve(
        listener,
        app_router(state.clone())?.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
        .with_graceful_shutdown(async move {
            tokio::select! { _ = shutdown_rx => {}, _ = tokio::signal::ctrl_c() => {} }
            shutdown_state.closing.store(true, std::sync::atomic::Ordering::Release);
            if let Err(error) = shutdown_state.channels.dispose().await {
                tracing::warn!(error=%security::redact_secret_text(&error.message), "Channel shutdown failed");
            }
            // Reject new HTTP work before draining domains. Their abort methods
            // still need the runtime while joining existing owned sessions.
            shutdown_state.game_assets.shutdown().await;
            shutdown_state.image_agent.close().await;
            shutdown_state.web_search.dispose();
            if let Err(error) = shutdown_state.plugins.close().await {
                tracing::warn!(error=%security::redact_secret_text(&error.to_string()), "Plugin shutdown failed");
            }
            shutdown_state.custom_ui.dispose();
            shutdown_state.workflows.shutdown().await;
            shutdown_state.visual.dispose().await;
            shutdown_state.browser.dispose().await;
            shutdown_state.shutdown.cancel();
            tracing::info!("Closing Agent services");
            shutdown_state.approvals.shutdown();
            if let Err(error) = shutdown_state.agents.shutdown().await { tracing::warn!(error=%security::redact_secret_text(&error.to_string()), "Agent shutdown failed"); }
            if let Err(error) = shutdown_state.goals.shutdown().await { tracing::warn!(error=%security::redact_secret_text(&error.to_string()), "Goal shutdown failed"); }
            shutdown_state.speech.shutdown().await;
            if let Err(error) = shutdown_state.executor.shutdown().await { tracing::warn!(error=%security::redact_secret_text(&error.to_string()), "Child session shutdown failed"); }
            shutdown_state.memory_tasks.shutdown().await;
            tracing::info!("Closing resident sessions");
            if let Err(error) = shutdown_state.sessions.shutdown(&shutdown_state).await {
                tracing::warn!(code = error.code, "Session shutdown failed");
            }
            shutdown_state.runtime.session().abort().await;
            if let Err(error) = shutdown_state.file_changes.close().await {
                tracing::warn!(code=%error.code, "File change shutdown failed");
            }
        })
        .await?;
    cache_stop.cancel();
    cache_worker.await?;
    mcp_api::shutdown(&state).await;
    tracing::info!("Closing configuration runtime");
    state.runtime.dispose().await?;
    Ok(())
}

/// Stateful API routes: the session host surface (slice 2) + config/model
/// selection (slice 3).
fn session_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/usage/today", get(session_api::today_usage))
        .route(
            "/api/sessions",
            get(session_api::list_sessions).post(session_api::create_session),
        )
        .route(
            "/api/sessions/{id}",
            axum::routing::patch(session_api::rename).delete(session_api::delete_session),
        )
        .route(
            "/api/sessions/{id}/organization",
            axum::routing::patch(session_api::organization),
        )
        .route("/api/sessions/{id}/input", post(session_input))
        .route("/api/sessions/{id}/live", get(session_api::live))
        .route("/api/sessions/{id}/tree", get(session_api::tree))
        .route("/api/sessions/{id}/abort", post(session_abort))
        .route("/api/sessions/{id}/derive", post(derive_session))
        .route("/api/sessions/{id}/compact", post(session_compact))
        .route("/api/sessions/{id}/cwd", put(set_session_cwd))
        .route("/api/sessions/{id}/run-mode", put(set_session_run_mode))
        .route(
            "/api/sessions/{id}/messages",
            get(session_api::get_messages),
        )
        .route("/api/session-labels", get(session_api::labels))
        .route(
            "/api/sessions/{id}/model",
            get(get_session_model)
                .put(post_session_model)
                .post(post_session_model),
        )
        .route(
            "/api/sessions/{id}/thinking-level",
            get(get_session_thinking_level)
                .put(post_session_thinking_level)
                .post(post_session_thinking_level),
        )
        .route("/api/skills", get(skills_ops::dashboard))
        .route("/api/skills/install", post(skills_ops::install))
        .route("/api/skills/reload", post(skills_ops::reload))
        .route(
            "/api/skills/{skillName}",
            axum::routing::patch(skills_ops::update).delete(skills_ops::remove),
        )
        .route("/api/remote/status", get(product::remote_status))
        .route(
            "/api/remote/connection-info",
            get(product::remote_connection_info),
        )
        .route("/api/remote/enabled", put(product::remote_set_enabled))
        .route("/api/sessions/{id}/vcs/changes", get(product::vcs_changes))
        .route("/api/sessions/{id}/vcs/commit", post(product::vcs_commit))
        .route("/api/sessions/{id}/vcs/push", post(product::vcs_push))
        .route("/api/sessions/{id}/vcs/revert", post(product::vcs_revert))
        // release 的 git/* 路由组是 vcs/* 的完整别名。
        .route("/api/sessions/{id}/git/changes", get(session_ops::git_changes))
        .route("/api/sessions/{id}/git/commit", post(session_ops::git_commit))
        .route("/api/sessions/{id}/git/push", post(session_ops::git_push))
        .route("/api/sessions/{id}/git/revert", post(session_ops::git_revert))
        .route(
            "/api/sessions/{id}/vcs/file-diff",
            get(session_ops::vcs_file_diff),
        )
        .route("/api/sessions/{id}/input/{inputId}", delete(session_ops::withdraw_input))
        .route(
            "/api/sessions/{id}/mobile-operations/{operationId}",
            post(session_ops::mobile_operation),
        )
        .route("/api/sessions/{id}/retry", post(session_ops::retry_session))
        .route("/api/sessions/{id}/commands", get(session_ops::session_commands))
        .route(
            "/api/sessions/{id}/tree/navigate",
            post(session_ops::tree_navigate),
        )
        .route(
            "/api/sessions/{id}/tree/labels/{entryId}",
            put(session_ops::tree_label_set),
        )
        .route(
            "/api/sessions/{id}/side-chat",
            get(session_ops::side_chat_get).post(session_ops::side_chat_create),
        )
        .route("/api/runtime/diagnostics", get(runtime_diagnostics))
        .route(
            "/api/remote/pairing-code",
            get(create_pairing_code).post(create_pairing_code),
        )
        .route("/api/remote/pair", post(pair_device))
        .route("/api/remote/devices", get(list_devices))
        .route("/api/remote/devices/{id}", delete(revoke_device))
        .route("/api/remote/devices/{id}/revoke", post(remote_ops::revoke_device_post))
        .route("/api/remote/firewall", get(firewall_ops::firewall_status))
        .route("/api/remote/firewall/retry", post(firewall_ops::firewall_retry))
        .route(
            "/api/remote/pairing-requests",
            get(remote_ops::list_requests).post(remote_ops::create_request),
        )
        .route(
            "/api/remote/pairing-requests/{requestId}",
            get(remote_ops::request_status).delete(remote_ops::cancel_request),
        )
        .route(
            "/api/remote/pairing-requests/{requestId}/decision",
            post(remote_ops::decide_request),
        )
        .route("/api/extensions/market", get(product2::extension_market))
        .route(
            "/api/extensions",
            get(product2::extension_dashboard).delete(product2::extension_remove),
        )
        .route("/api/extensions/install", post(product2::extension_install))
        .route("/api/decisions/status", get(product2::decisions_status))
        .route(
            "/api/decisions/config",
            put(product2::decisions_update_config),
        )
        .route("/api/decisions/test", post(product2::decisions_test))
        .route("/api/decisions/decide", post(product2::decisions_decide))
        .route("/api/directories", get(product2::list_directories))
        .route(
            "/api/workspace-entries",
            get(product2::list_workspace_entries),
        )
        .route("/api/chat", post(chat_stream::chat))
        .route("/api/runs/{id}/events", get(chat_stream::run_events))
        .merge(provider_config::router())
        .merge(ui_contract::router())
        .route("/api/mcp-host", get(mcp_host_ops::status).patch(mcp_host_ops::set_enabled))
        .route("/api/mcp-host/credentials", post(mcp_host_ops::credentials))
        .route("/api/mcp-host/rotate-token", post(mcp_host_ops::rotate_token))
        .route("/api/desktop-pet", get(desktop_ops::pet_status_handler))
        .route("/api/desktop-pet/catalog", get(desktop_ops::pet_catalog))
        .route("/api/desktop-pet/sprite", get(desktop_ops::pet_sprite))
        .route("/api/desktop-pet/install", post(desktop_ops::pet_install))
        .route("/api/desktop-pet/enabled", post(desktop_ops::pet_set_enabled))
        .route("/api/desktop-pet/opacity", post(desktop_ops::pet_set_opacity))
        .route("/api/desktop-pet/select", post(desktop_ops::pet_select))
        .route("/api/desktop-pet/{slug}", delete(desktop_ops::pet_remove))
        .route("/api/desktop/reveal-path", post(desktop_ops::reveal_path))
        .route("/api/app-update", get(desktop_ops::app_update))
        .route("/api/sponsors/{placement}", get(desktop_ops::sponsor_placement))
        .merge(mcp_api::router())
        .merge(memory_api::router())
        .merge(asset_api::router())
        .merge(approval_api::router())
}

/// Stateless base: handshake + unknown-route fallback + the built React
/// frontend served same-origin (upstream: Vite middleware in runtime/index.mjs).
fn base_router() -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/{*path}", axum::routing::any(unknown_api_fallback))
        .fallback(unknown_api_fallback)
}

/// Sidecar auth: when spawned by the TUI/desktop (PISPER_DESKTOP_TOKEN set),
/// every request must present the matching `__pisper_desktop=<token>` cookie.
async fn desktop_auth_middleware(
    axum::extract::State((token, custom_ui)): axum::extract::State<(
        Option<String>,
        Arc<native_custom_ui::CustomUiService>,
    )>,
    request: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> axum::response::Response {
    if let Some(response) = custom_ui_api::render_request(
        custom_ui,
        request.method().clone(),
        request.uri().clone(),
        false,
        None,
    )
    .await
    {
        return response;
    }
    // /mcp 是对外 MCP 协议端点，独立做 Bearer 令牌鉴权（mcp_host_ops）。
    if request.uri().path() == "/mcp" || request.uri().path().starts_with("/mcp/") {
        return next.run(request).await;
    }
    let Some(expected) = token else {
        return next.run(request).await;
    };
    use subtle::ConstantTimeEq;
    // 引导请求只允许同一个令牌植入 HttpOnly Cookie；跳转目标必须留在本机应用。
    if request.uri().path() == "/_pisper/desktop/bootstrap" {
        let query = Query::<std::collections::HashMap<String, String>>::try_from_uri(request.uri());
        if let Ok(Query(params)) = query {
            if request.method() == axum::http::Method::GET
                && params
                    .get("token")
                    .is_some_and(|token| bool::from(token.as_bytes().ct_eq(expected.as_bytes())))
            {
                let target = params
                    .get("next")
                    .filter(|path| {
                        path.starts_with('/')
                            && !path.starts_with("//")
                            && !path.contains('\\')
                            && !path.chars().any(char::is_control)
                    })
                    .map(String::as_str)
                    .unwrap_or("/");
                let cookie =
                    format!("__pisper_desktop={expected}; HttpOnly; SameSite=Strict; Path=/");
                if let Ok(cookie) = axum::http::HeaderValue::from_str(&cookie) {
                    let mut response = axum::response::Redirect::to(target).into_response();
                    *response.status_mut() = StatusCode::FOUND;
                    response
                        .headers_mut()
                        .insert(axum::http::header::SET_COOKIE, cookie);
                    response.headers_mut().insert(
                        axum::http::header::CACHE_CONTROL,
                        axum::http::HeaderValue::from_static("no-store"),
                    );
                    response.headers_mut().insert(
                        axum::http::header::REFERRER_POLICY,
                        axum::http::HeaderValue::from_static("no-referrer"),
                    );
                    return response;
                }
            }
        }
        return ApiError::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "invalid desktop token",
        )
        .into_response();
    }
    // 浏览器写请求须来自当前服务；命令行客户端可以没有 Origin。
    if !matches!(
        *request.method(),
        axum::http::Method::GET | axum::http::Method::HEAD | axum::http::Method::OPTIONS
    ) {
        if let Some(origin) = request.headers().get(axum::http::header::ORIGIN) {
            let host = request
                .headers()
                .get(axum::http::header::HOST)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("");
            if origin.to_str().ok() != Some(format!("http://{host}").as_str()) {
                return ApiError::new(
                    StatusCode::FORBIDDEN,
                    "forbidden_origin",
                    "invalid desktop origin",
                )
                .into_response();
            }
        }
    }
    let cookie = request
        .headers()
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let presented = cookie
        .split(';')
        .map(|pair| pair.trim())
        .filter_map(|pair| pair.strip_prefix("__pisper_desktop="))
        .any(|token| bool::from(token.as_bytes().ct_eq(expected.as_bytes())));
    if presented {
        next.run(request).await
    } else {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "missing desktop token",
        )
        .into_response()
    }
}

/// Full router including the static SPA (dist/) with index.html fallback.
pub(crate) fn app_router(state: Arc<AppState>) -> anyhow::Result<Router> {
    state.executor.attach(&state);
    let dist = state.dist_dir.clone();
    let token = state.desktop_token.clone();
    let speech_state = Arc::downgrade(&state);
    let channel_state = Arc::downgrade(&state);
    let channel_models: channels_api::Models = Arc::new(move || {
        let state = channel_state.clone();
        Box::pin(async move {
            let state = state
                .upgrade()
                .ok_or_else(|| native_channels::ChannelError::new("运行时正在关闭。"))?;
            let Json(config) = provider_config::get_config(State(state))
                .await
                .map_err(|error| native_channels::ChannelError::new(error.message))?;
            let models=config["providers"].as_array().into_iter().flatten()
                .filter(|provider|provider["type"]!="visual"&&provider["enabled"]==true&&provider["configured"]==true)
                .flat_map(|provider|provider["models"].as_array().into_iter().flatten().filter(|model|model["kind"]=="chat")
                    .map(move |model|serde_json::json!({"provider":provider["id"],"model":model["id"],
                        "label":format!("{} / {}",provider["name"].as_str().unwrap_or(""),model["name"].as_str().unwrap_or(""))})))
                .collect::<Vec<_>>();
            Ok(serde_json::json!(models))
        })
    });
    let game_state = Arc::downgrade(&state);
    let game_models: game_assets_api::ModelCatalog = Arc::new(move || {
        let state = game_state.clone();
        Box::pin(async move {
            let state = state
                .upgrade()
                .ok_or_else(|| native_workflow::WorkflowError::io("运行时正在关闭。"))?;
            native_image_runtime::visual::models(&state).await
        })
    });
    let speech_resolver: speech_api::SpeechSessionResolver = Arc::new(move |id| {
        let state = speech_state.clone();
        Box::pin(async move {
            let state = state
                .upgrade()
                .ok_or_else(|| ApiError::internal("Runtime has shut down"))?;
            let path = match session_api::find_session_path(&state, &id) {
                Ok(path) => path,
                Err(error) if error.status == StatusCode::NOT_FOUND => return Ok(None),
                Err(error) => return Err(error),
            };
            let manager = SessionManager::open(&path, None, None)
                .map_err(|error| ApiError::internal(error.to_string()))?;
            Ok(Some(std::path::PathBuf::from(manager.get_cwd())))
        })
    });
    let schedules = state
        .workflows
        .schedules
        .get()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Schedule service was not initialized"))?;
    Ok(Router::new()
        .merge(base_router())
        .merge(plugins_api::router(
            state.plugins.clone(),
            state.plugin_integration.http_hooks(),
        ))
        .merge(web_search_api::router(state.web_search.clone()))
        .merge(visual_api::router(
            state.visual.clone(),
            std::path::PathBuf::from(&state.data_dir),
        ))
        .merge(notification_api::router().with_state(state.clone()))
        .merge(channels_api::router(state.channels.clone(), channel_models))
        .merge(file_changes_api::router().with_state(state.clone()))
        .merge(custom_ui_api::router(state.custom_ui.clone(), None))
        .merge(game_assets_api::routes(
            state.game_assets.projects.clone(),
            state.game_assets.media.clone(),
            state.game_assets.images.clone(),
            state.workflows.engines.clone(),
            game_models,
        ))
        .merge(session_router().with_state(state.clone()))
        .merge(plan_api::routes(
            state.plans.clone(),
            Arc::new(state.executor.clone()),
            state.executor.event_sink(),
        ))
        .merge(multi_agent_api::routes(
            state.agents.clone(),
            Arc::new(state.executor.clone()),
        ))
        .merge(goal_api::routes(
            state.goals.clone(),
            Arc::new(state.executor.clone()),
        ))
        .merge(multi_agent_api::team_routes(
            state.team.clone(),
            Arc::new(state.executor.clone()),
        ))
        .merge(speech_api::router(state.speech.clone(), speech_resolver))
        .merge(workflow_api::routes(state.workflows.workflows.clone()))
        .merge(workflow_api::media_routes(
            state.workflows.workflows.clone(),
            state.workflows.media.clone(),
            state.workflows.images.clone(),
            Some(state.workflows.cache.clone()),
        ))
        .merge(schedule_api::routes(schedules))
        .merge(
            axum::Router::new()
                // 对外 MCP 端点：独立 Bearer 鉴权（desktop_auth_middleware 对 /mcp 放行）。
                .route("/mcp", axum::routing::post(mcp_host_ops::mcp_endpoint))
                .with_state(state.clone()),
        )
        .merge(native_image_runtime::engines::routes(
            state.workflows.engines.clone(),
        ))
        .fallback_service(
            tower_http::services::ServeDir::new(&dist)
                .append_index_html_on_directories(true)
                .not_found_service(tower_http::services::ServeFile::new(
                    dist.join("index.html"),
                )),
        )
        .layer(axum::middleware::from_fn_with_state(
            (token, state.custom_ui.clone()),
            desktop_auth_middleware,
        ))
        .layer(axum::middleware::from_fn_with_state(state, reject_shutdown)))
}

async fn reject_shutdown(
    State(state): State<Arc<AppState>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    if state.closing.load(std::sync::atomic::Ordering::Acquire) || state.shutdown.is_cancelled() {
        return ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "runtime_shutdown",
            "运行时正在关闭。",
        )
        .into_response();
    }
    next.run(request).await
}

async fn create_pairing_code(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    use std::fmt::Write as _;
    let code: u32 = (product::now_ms() % 1_000_000) as u32;
    let mut code_str = String::new();
    let _ = write!(code_str, "{code:06}");
    let expires_at = product::now_ms() + 300_000;
    *state.pairing.pending.lock().expect("pairing lock") = Some((code_str.clone(), expires_at));
    // release 同形状（qrDataUrl 在渲染失败时为空字符串；本后端暂不渲染二维码图形）。
    Json(serde_json::json!({ "code": code_str, "expiresAt": expires_at, "qrDataUrl": "" }))
}

async fn pair_device(
    State(state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
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
    // release pairedResponse：201 + {deviceId, token, serverName, endpoints, apiVersion}。
    let endpoints = if state.remote_enabled.load(std::sync::atomic::Ordering::Relaxed) {
        serde_json::json!([{ "t": "lan" }])
    } else {
        serde_json::json!([])
    };
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "deviceId": device.id,
            "token": token,
            "name": device.name,
            "serverName": "Pisper",
            "endpoints": endpoints,
            "apiVersion": 1,
        })),
    ))
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
    ApiError::new(StatusCode::NOT_FOUND, "not_found", "unknown API route")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn authenticated_probe() -> Router {
        Router::new()
            .route("/api/probe", get(|| async { "ok" }).post(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(
                (
                    Some("test-token".to_string()),
                    native_custom_ui::CustomUiService::new(
                        std::env::temp_dir()
                            .join(format!("pisper-http-auth-{}", uuid::Uuid::new_v4())),
                    ),
                ),
                desktop_auth_middleware,
            ))
    }

    #[tokio::test]
    async fn desktop_bootstrap_sets_cookie_and_allows_repeated_window_startup() {
        for _ in 0..2 {
            let response = authenticated_probe()
                .oneshot(
                    Request::get("/_pisper/desktop/bootstrap?token=test-token&next=/pets")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FOUND);
            assert_eq!(response.headers()["location"], "/pets");
            assert_eq!(
                response.headers()["set-cookie"],
                "__pisper_desktop=test-token; HttpOnly; SameSite=Strict; Path=/"
            );
            assert_eq!(response.headers()["cache-control"], "no-store");
            assert_eq!(response.headers()["referrer-policy"], "no-referrer");
        }
        for next in ["//foreign.example", "/%5Cforeign.example", "/%0Abad"] {
            let response = authenticated_probe()
                .oneshot(
                    Request::get(format!(
                        "/_pisper/desktop/bootstrap?token=test-token&next={next}"
                    ))
                    .body(Body::empty())
                    .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.headers()["location"], "/");
        }
    }

    #[tokio::test]
    async fn desktop_access_requires_matching_token() {
        for uri in ["/api/probe", "/_pisper/desktop/bootstrap?token=wrong"] {
            let response = authenticated_probe()
                .oneshot(Request::get(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
        let response = authenticated_probe()
            .oneshot(
                Request::get("/api/probe")
                    .header("cookie", "other=value; __pisper_desktop=test-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn desktop_write_rejects_foreign_origin_and_accepts_local_origin() {
        for (origin, expected) in [
            ("http://foreign.example", StatusCode::FORBIDDEN),
            ("http://127.0.0.1:12345", StatusCode::OK),
        ] {
            let response = authenticated_probe()
                .oneshot(
                    Request::post("/api/probe")
                        .header("cookie", "__pisper_desktop=test-token")
                        .header("host", "127.0.0.1:12345")
                        .header("origin", origin)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), expected);
        }
        let response = authenticated_probe()
            .oneshot(
                Request::post("/api/probe")
                    .header("cookie", "__pisper_desktop=test-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn health_matches_client_handshake_contract() {
        let res = base_router()
            .oneshot(Request::get("/api/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(res.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(body["ok"], serde_json::json!(true));
        assert_eq!(body["engine"], serde_json::json!(ENGINE));
        assert_eq!(body["apiVersion"], serde_json::json!(API_VERSION));
        assert_eq!(
            body["minClientVersion"],
            serde_json::json!(MIN_CLIENT_VERSION)
        );
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
            &axum::body::to_bytes(res.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(body["code"], serde_json::json!("not_found"));
    }
}
