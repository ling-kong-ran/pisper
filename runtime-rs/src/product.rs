//! Pisper product-layer subsystems for the Rust backend (slice 6):
//! schedules, workflows + runs, plugin registry storage, remote status, and
//! the session VCS endpoints (git CLI against the session workspace).
//!
//! Persistence lives under the server data dir (PISPER_RS_DATA_DIR, default
//! `%HOME%/.pisper/agent-rs`): `schedules.json`, `workflows.json`,
//! `plugins.json`. Workflow runs are in-memory (retry/stop operate live).

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{ensure_hosted, ApiError, AppState};

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn new_id() -> String {
    // Windows clock resolution can repeat between concurrent runs. Match
    // release's cryptographically random IDs instead of deriving them from time.
    uuid::Uuid::new_v4().to_string()
}

// ---------------------------------------------------------------- schedules

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Schedule {
    pub id: String,
    pub name: String,
    /// Supported forms: "every N" (minutes) or "daily HH:MM".
    pub schedule: String,
    pub prompt: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub created_at: u64,
    #[serde(default)]
    pub last_run: Option<u64>,
    #[serde(default)]
    pub next_run: Option<u64>,
}

fn default_true() -> bool {
    true
}

fn next_run_for(schedule: &str, from_ms: u64) -> Option<u64> {
    let text = schedule.trim();
    if let Some(rest) = text.strip_prefix("every ") {
        let minutes: u64 = rest.trim_end_matches("m").trim().parse().ok()?;
        return Some(from_ms + minutes * 60_000);
    }
    if let Some(rest) = text.strip_prefix("daily ") {
        // daily HH:MM — next occurrence of that wall-clock time (UTC-based
        // computation; the server runs local-only by default).
        let mut parts = rest.split(':');
        let hour: u64 = parts.next()?.trim().parse().ok()?;
        let minute: u64 = parts.next()?.trim().parse().ok()?;
        let day_start = from_ms - (from_ms % 86_400_000);
        let target = day_start + hour * 3_600_000 + minute * 60_000;
        return Some(if target <= from_ms {
            target + 86_400_000
        } else {
            target
        });
    }
    None
}

pub fn load_schedules(data_dir: &str) -> Vec<Schedule> {
    let path = std::path::Path::new(data_dir).join("schedules.json");
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_schedules(data_dir: &str, schedules: &[Schedule]) -> Result<(), String> {
    std::fs::create_dir_all(data_dir).map_err(|e| e.to_string())?;
    let path = std::path::Path::new(data_dir).join("schedules.json");
    let json = serde_json::to_string_pretty(schedules).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())
}

pub async fn get_schedules(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let schedules = load_schedules(&state.data_dir);
    Json(serde_json::json!({ "schedules": schedules }))
}

pub async fn create_schedule(
    State(state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("schedule")
        .to_string();
    let schedule = body
        .get("schedule")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::bad_request("missing \"schedule\" field"))?
        .to_string();
    let prompt = body
        .get("prompt")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let mut schedules = load_schedules(&state.data_dir);
    let rec = Schedule {
        id: new_id(),
        name,
        schedule: schedule.clone(),
        prompt,
        enabled: body
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        created_at: now_ms(),
        last_run: None,
        next_run: next_run_for(&schedule, now_ms()),
    };
    schedules.push(rec.clone());
    save_schedules(&state.data_dir, &schedules).map_err(ApiError::internal)?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::to_value(rec).unwrap()),
    ))
}

pub async fn run_schedule(
    State(state): State<Arc<AppState>>,
    Path(schedule_id): Path<String>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let mut schedules = load_schedules(&state.data_dir);
    let schedule_prompt;
    {
        let Some(rec) = schedules.iter_mut().find(|s| s.id == schedule_id) else {
            return Err(ApiError::new(
                StatusCode::NOT_FOUND,
                "schedule_not_found",
                "定时任务不存在。",
            ));
        };
        rec.last_run = Some(now_ms());
        rec.next_run = next_run_for(&rec.schedule, now_ms());
        schedule_prompt = rec.prompt.clone();
    }
    save_schedules(&state.data_dir, &schedules).map_err(ApiError::internal)?;
    // Fire the prompt into the active session via the engine.
    state
        .runtime
        .session()
        .prompt(schedule_prompt, None)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "runId": new_id(), "scheduleId": schedule_id })),
    ))
}

pub async fn update_schedule(
    State(state): State<Arc<AppState>>,
    Path(schedule_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut schedules = load_schedules(&state.data_dir);
    let exists = schedules.iter().any(|s| s.id == schedule_id);
    if !exists {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "schedule_not_found",
            "定时任务不存在。",
        ));
    }
    let rec = schedules.iter_mut().find(|s| s.id == schedule_id).unwrap();
    if let Some(v) = body.get("name").and_then(|v| v.as_str()) {
        rec.name = v.to_string();
    }
    if let Some(v) = body.get("schedule").and_then(|v| v.as_str()) {
        rec.schedule = v.to_string();
        rec.next_run = next_run_for(&rec.schedule, now_ms());
    }
    if let Some(v) = body.get("prompt").and_then(|v| v.as_str()) {
        rec.prompt = v.to_string();
    }
    if let Some(v) = body.get("enabled").and_then(|v| v.as_bool()) {
        rec.enabled = v;
    }
    let out = serde_json::to_value(&*rec).unwrap();
    save_schedules(&state.data_dir, &schedules).map_err(ApiError::internal)?;
    Ok(Json(out))
}

pub async fn delete_schedule(
    State(state): State<Arc<AppState>>,
    Path(schedule_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut schedules = load_schedules(&state.data_dir);
    let before = schedules.len();
    schedules.retain(|s| s.id != schedule_id);
    if schedules.len() == before {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "schedule_not_found",
            "定时任务不存在。",
        ));
    }
    save_schedules(&state.data_dir, &schedules).map_err(ApiError::internal)?;
    Ok(Json(serde_json::json!({ "deleted": true })))
}

/// Background tick: fires enabled schedules whose next_run has passed.
pub async fn schedule_ticker(state: Arc<AppState>) {
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(30));
    loop {
        tick.tick().await;
        let now = now_ms();
        let due: Vec<Schedule> = {
            let mut schedules = load_schedules(&state.data_dir);
            let mut fired = Vec::new();
            for rec in schedules.iter_mut() {
                if rec.enabled
                    && rec.next_run.map(|n| n <= now).unwrap_or(false)
                    && rec.last_run.map(|l| now - l >= 60_000).unwrap_or(true)
                {
                    rec.last_run = Some(now);
                    rec.next_run = next_run_for(&rec.schedule, now);
                    fired.push(rec.clone());
                }
            }
            let _ = save_schedules(&state.data_dir, &schedules);
            fired
        };
        for rec in due {
            let _ = state.runtime.session().prompt(rec.prompt, None).await;
        }
    }
}

// ---------------------------------------------------------------- workflows

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowStep {
    pub id: String,
    pub prompt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workflow {
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub steps: Vec<WorkflowStep>,
    #[serde(default)]
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRunStep {
    pub step_id: String,
    pub status: String, // pending | running | done | failed
    pub result: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRun {
    pub id: String,
    pub workflow_id: String,
    pub status: String, // running | done | failed | stopped
    pub steps: Vec<WorkflowRunStep>,
}

pub fn load_workflows(data_dir: &str) -> Vec<Workflow> {
    let path = std::path::Path::new(data_dir).join("workflows.json");
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_workflows(data_dir: &str, workflows: &[Workflow]) -> Result<(), String> {
    std::fs::create_dir_all(data_dir).map_err(|e| e.to_string())?;
    let path = std::path::Path::new(data_dir).join("workflows.json");
    let json = serde_json::to_string_pretty(workflows).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())
}

pub async fn get_workflows(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "workflows": load_workflows(&state.data_dir) }))
}

pub async fn create_workflow(
    State(state): State<Arc<AppState>>,
    Json(mut wf): Json<Workflow>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    if wf.id.is_empty() {
        wf.id = new_id();
    }
    wf.created_at = now_ms();
    let mut workflows = load_workflows(&state.data_dir);
    workflows.push(wf.clone());
    save_workflows(&state.data_dir, &workflows).map_err(ApiError::internal)?;
    Ok((StatusCode::CREATED, Json(serde_json::to_value(wf).unwrap())))
}

pub async fn delete_workflow(
    State(state): State<Arc<AppState>>,
    Path(workflow_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut workflows = load_workflows(&state.data_dir);
    let before = workflows.len();
    workflows.retain(|w| w.id != workflow_id);
    if workflows.len() == before {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "workflow_not_found",
            "工作流不存在。",
        ));
    }
    save_workflows(&state.data_dir, &workflows).map_err(ApiError::internal)?;
    Ok(Json(serde_json::json!({ "deleted": true })))
}

pub async fn run_workflow(
    State(state): State<Arc<AppState>>,
    Path(workflow_id): Path<String>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let workflows = load_workflows(&state.data_dir);
    let Some(wf) = workflows.into_iter().find(|w| w.id == workflow_id) else {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "workflow_not_found",
            "工作流不存在。",
        ));
    };
    let steps: Vec<WorkflowRunStep> = wf
        .steps
        .iter()
        .map(|s| WorkflowRunStep {
            step_id: s.id.clone(),
            status: "pending".into(),
            result: None,
        })
        .collect();
    let step_count = steps.len();
    let run = WorkflowRun {
        id: new_id(),
        workflow_id: workflow_id.clone(),
        status: "running".into(),
        steps,
    };
    let run_id = run.id.clone();
    state
        .runs
        .lock()
        .expect("runs lock")
        .insert(run_id.clone(), run.clone());
    // Sequential executor: each step is one engine turn in the active session.
    tokio::spawn(async move {
        for index in 0..step_count {
            let running = {
                let mut runs = state.runs.lock().expect("runs lock");
                let Some(r) = runs.get_mut(&run_id) else {
                    return;
                };
                if r.status != "running" {
                    return;
                }
                r.steps[index].status = "running".into();
                r.steps[index].clone()
            };
            let prompt = wf
                .steps
                .get(index)
                .map(|s| s.prompt.clone())
                .unwrap_or_default();
            match state.runtime.session().prompt(prompt, None).await {
                Ok(()) => {
                    let mut runs = state.runs.lock().expect("runs lock");
                    if let Some(r) = runs.get_mut(&run_id) {
                        r.steps[index].status = "done".into();
                    }
                }
                Err(e) => {
                    let mut runs = state.runs.lock().expect("runs lock");
                    if let Some(r) = runs.get_mut(&run_id) {
                        r.steps[index].status = "failed".into();
                        r.steps[index].result = Some(e.to_string());
                        r.status = "failed".into();
                    }
                    return;
                }
            }
            let _ = running;
        }
        let mut runs = state.runs.lock().expect("runs lock");
        if let Some(r) = runs.get_mut(&run_id) {
            if r.status == "running" {
                r.status = "done".into();
            }
        }
    });
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::to_value(run).unwrap()),
    ))
}

pub async fn get_workflow_run(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state
        .runs
        .lock()
        .expect("runs lock")
        .get(&run_id)
        .map(|r| Json(serde_json::to_value(r).unwrap()))
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "run_not_found", "工作流运行不存在。"))
}

pub async fn stop_workflow_run(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let mut runs = state.runs.lock().expect("runs lock");
    match runs.get_mut(&run_id) {
        Some(r) if r.status == "running" => {
            r.status = "stopped".into();
            Ok((
                StatusCode::ACCEPTED,
                Json(serde_json::json!({ "stopped": true })),
            ))
        }
        Some(_) => Ok((
            StatusCode::ACCEPTED,
            Json(serde_json::json!({ "stopped": false })),
        )),
        None => Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "run_not_found",
            "工作流运行不存在。",
        )),
    }
}

// ------------------------------------------------------------------ plugins

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginRecord {
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub source: String,
}

pub fn load_plugins(data_dir: &str) -> Vec<PluginRecord> {
    let path = std::path::Path::new(data_dir).join("plugins.json");
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_plugins(data_dir: &str, plugins: &[PluginRecord]) -> Result<(), String> {
    std::fs::create_dir_all(data_dir).map_err(|e| e.to_string())?;
    let path = std::path::Path::new(data_dir).join("plugins.json");
    let json = serde_json::to_string_pretty(plugins).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())
}

/// Engine builtin tools (stable upstream set) surface in the Slash catalog.
pub fn builtin_tools() -> serde_json::Value {
    let tools = ["read", "bash", "edit", "write"]
        .into_iter()
        .map(|id| {
            serde_json::json!({
                "id": id,
                "name": id,
                "description": format!("builtin {id} tool"),
                "enabled": true,
            })
        })
        .collect::<Vec<_>>();
    serde_json::Value::Array(tools)
}

pub async fn get_plugins_registry(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "plugins": load_plugins(&state.data_dir),
        "tools": builtin_tools(),
    }))
}

pub async fn save_plugins_registry(
    State(state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let plugins: Vec<PluginRecord> =
        serde_json::from_value(body.get("plugins").cloned().unwrap_or(body))
            .map_err(|e| ApiError::bad_request(e.to_string()))?;
    save_plugins(&state.data_dir, &plugins).map_err(ApiError::internal)?;
    Ok(Json(serde_json::json!({ "plugins": plugins })))
}

pub async fn install_plugin(
    State(state): State<Arc<AppState>>,
    Json(mut rec): Json<PluginRecord>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    if rec.id.is_empty() {
        rec.id = new_id();
    }
    let mut plugins = load_plugins(&state.data_dir);
    plugins.push(rec.clone());
    save_plugins(&state.data_dir, &plugins).map_err(ApiError::internal)?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::to_value(rec).unwrap()),
    ))
}

pub async fn set_plugin_enabled(
    State(state): State<Arc<AppState>>,
    Path(plugin_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let enabled = body
        .get("enabled")
        .and_then(|v| v.as_bool())
        .ok_or_else(|| ApiError::bad_request("插件启用状态无效。"))?;
    let mut plugins = load_plugins(&state.data_dir);
    let Some(rec) = plugins.iter_mut().find(|p| p.id == plugin_id) else {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "plugin_not_found",
            "插件不存在。",
        ));
    };
    rec.enabled = enabled;
    let out = serde_json::to_value(&*rec).unwrap();
    save_plugins(&state.data_dir, &plugins).map_err(ApiError::internal)?;
    Ok(Json(out))
}

pub async fn uninstall_plugin(
    State(state): State<Arc<AppState>>,
    Path(plugin_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut plugins = load_plugins(&state.data_dir);
    let before = plugins.len();
    plugins.retain(|p| p.id != plugin_id);
    if plugins.len() == before {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "plugin_not_found",
            "插件不存在。",
        ));
    }
    save_plugins(&state.data_dir, &plugins).map_err(ApiError::internal)?;
    Ok(Json(serde_json::json!({ "deleted": true })))
}

// ------------------------------------------------------------------- remote

pub async fn remote_status(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let enabled = state
        .remote_enabled
        .load(std::sync::atomic::Ordering::Relaxed);
    Json(serde_json::json!({
        "enabled": enabled,
        "fingerprint": state.fingerprint,
        "endpoints": if enabled { serde_json::json!([{ "t": "lan" }]) } else { serde_json::json!([]) },
    }))
}

pub async fn remote_connection_info(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "fingerprint": state.fingerprint,
        "endpoints": if state.remote_enabled.load(std::sync::atomic::Ordering::Relaxed) {
            serde_json::json!([{ "t": "lan" }])
        } else {
            serde_json::json!([])
        },
    }))
}

pub async fn remote_set_enabled(
    State(state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let enabled = body
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    state
        .remote_enabled
        .store(enabled, std::sync::atomic::Ordering::Relaxed);
    Json(serde_json::json!({ "enabled": enabled }))
}

// ---------------------------------------------------------------------- vcs

fn git(cwd: &str, args: &[&str]) -> Result<String, ApiError> {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|e| ApiError::internal(format!("git spawn: {e}")))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if !out.status.success() && text.trim().is_empty() {
        return Err(ApiError::internal(format!("git {:?} failed", args)));
    }
    Ok(text)
}

pub async fn vcs_changes(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    ensure_hosted(&state, &id).await?;
    Ok(Json(crate::vcs_ops::get_changes(&state.cwd).await))
}

pub async fn vcs_commit(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    ensure_hosted(&state, &id).await?;
    let message = body
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    Ok(Json(crate::vcs_ops::commit(&state.cwd, message).await?))
}

pub async fn vcs_push(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    ensure_hosted(&state, &id).await?;
    Ok(Json(crate::vcs_ops::push(&state.cwd).await?))
}

pub async fn vcs_revert(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    ensure_hosted(&state, &id).await?;
    Ok(Json(crate::vcs_ops::revert(&state.cwd).await?))
}

pub type RunsMap = Mutex<HashMap<String, WorkflowRun>>;

// ------------------------------------------------------------ chat runs

/// One replayable SSE frame: global cursor + event name + JSON payload.
#[derive(Debug, Clone)]
pub struct Frame {
    pub cursor: u64,
    pub event: String,
    pub data: serde_json::Value,
}

/// A live chat run: ring of recorded frames for `/api/runs/{id}/events`
/// replay plus a broadcast channel for attached live clients.
pub struct ChatRun {
    pub frames: Mutex<Vec<Frame>>,
    pub tx: tokio::sync::broadcast::Sender<Frame>,
    pub closed: std::sync::atomic::AtomicBool,
    pub cancel: tokio_util::sync::CancellationToken,
    pub finished: Arc<std::sync::atomic::AtomicBool>,
    pub settled: Arc<tokio::sync::Notify>,
}

impl ChatRun {
    pub fn record(&self, cursor: u64, event: &str, data: serde_json::Value) {
        self.frames.lock().expect("frames lock").push(Frame {
            cursor,
            event: event.to_string(),
            data: data.clone(),
        });
        if event == "done" || event == "error" {
            self.closed
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        let _ = self.tx.send(Frame {
            cursor,
            event: event.to_string(),
            data,
        });
    }
}

/// Map a raw pi wire event (from `to_json_event_string`) into the Pisper UI
/// event vocabulary the TUI/Web clients consume. Returns `None` for events
/// with no client mapping.
pub fn map_pi_event(
    raw: &serde_json::Value,
    session_id: &str,
    thinking: &mut String,
) -> Option<(String, serde_json::Value)> {
    let kind = raw.get("type")?.as_str()?;
    match kind {
        "message_update" => {
            let ae = raw.get("assistantMessageEvent")?;
            let ae_type = ae.get("type")?.as_str()?;
            match ae_type {
                "text_delta" => {
                    let delta = ae.get("delta")?.as_str()?.to_string();
                    Some(("text_delta".into(), serde_json::json!({ "delta": delta })))
                }
                "thinking_delta" => {
                    let delta = ae.get("delta")?.as_str()?.to_string();
                    let start = thinking.encode_utf16().count();
                    thinking.push_str(&delta);
                    Some((
                        "thinking_patch".into(),
                        serde_json::json!({ "start": start, "text": delta }),
                    ))
                }
                _ => None,
            }
        }
        "tool_execution_start" => Some((
            "tool_start".into(),
            serde_json::json!({
                "id": raw.get("toolCallId").cloned().unwrap_or_default(),
                "name": raw.get("toolName").cloned().unwrap_or_default(),
                "args": raw.get("args").cloned().unwrap_or_default(),
                "startedAt": now_ms(),
            }),
        )),
        "tool_execution_update" => Some((
            "tool_update".into(),
            serde_json::json!({
                "id": raw.get("toolCallId").cloned().unwrap_or_default(),
                "partial": raw.get("partial").cloned().unwrap_or_default(),
            }),
        )),
        "tool_execution_end" => Some((
            "tool_end".into(),
            serde_json::json!({
                "id": raw.get("toolCallId").cloned().unwrap_or_default(),
                "status": "done",
                "result": raw.get("result").cloned().unwrap_or_default(),
            }),
        )),
        "auto_compaction_start" => Some(("compaction_start".into(), serde_json::json!({}))),
        "auto_compaction_end" => Some(("compaction_end".into(), serde_json::json!({}))),
        "agent_end" => {
            // Final assistant text: last assistant message's text content.
            let text = raw
                .get("messages")
                .and_then(|m| m.as_array())
                .and_then(|msgs| {
                    msgs.iter()
                        .rev()
                        .find(|m| m.get("role").and_then(|r| r.as_str()) == Some("assistant"))
                })
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|i| {
                            i.get("type")
                                .and_then(|t| t.as_str())
                                .filter(|t| *t == "text")
                                .and_then(|_| i.get("text"))
                                .and_then(|t| t.as_str())
                        })
                        .collect::<Vec<_>>()
                        .join("")
                })
                .unwrap_or_default();
            Some((
                "done".into(),
                serde_json::json!({ "sessionId": session_id, "text": text, "tools": [] }),
            ))
        }
        _ => None,
    }
}

// ------------------------------------------------- notification settings

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct NotificationSettings {
    #[serde(default)]
    pub chat_completed: Option<serde_json::Value>,
    #[serde(default)]
    pub chat_waiting: Option<serde_json::Value>,
}

pub fn load_notification_settings(data_dir: &str) -> NotificationSettings {
    let path = std::path::Path::new(data_dir).join("notification-settings.json");
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_notification_settings_file(
    data_dir: &str,
    settings: &NotificationSettings,
) -> Result<(), String> {
    std::fs::create_dir_all(data_dir).map_err(|e| e.to_string())?;
    let path = std::path::Path::new(data_dir).join("notification-settings.json");
    let json = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())
}

pub async fn notification_settings(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let settings = load_notification_settings(&state.data_dir);
    Json(serde_json::to_value(settings).unwrap())
}

pub async fn save_notification_settings(
    State(state): State<Arc<AppState>>,
    Json(settings): Json<NotificationSettings>,
) -> Result<Json<serde_json::Value>, ApiError> {
    save_notification_settings_file(&state.data_dir, &settings).map_err(ApiError::internal)?;
    Ok(Json(serde_json::to_value(settings).unwrap()))
}
