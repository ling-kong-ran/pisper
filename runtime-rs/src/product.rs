//! 远程状态、会话 VCS 与聊天事件投影；领域存储由各自服务持有。

use axum::{
    extract::{Path, State},
    Json,
};
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
) -> Result<Json<serde_json::Value>, ApiError> {
    let enabled = body
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    crate::security::save_remote_enabled(&state.data_dir, enabled).map_err(ApiError::internal)?;
    if !enabled {
        *state.pairing.pending.lock().expect("pairing lock") = None;
        state
            .pairing_requests
            .lock()
            .expect("pairing requests lock")
            .clear();
    }
    state
        .remote_enabled
        .store(enabled, std::sync::atomic::Ordering::Relaxed);
    // release setEnabled({configureFirewall:true}) 触发防火墙 reconcile；
    // 失败不阻塞开关本身（release 同样吞掉防火墙错误继续返回状态）。
    if body.get("configureFirewall").and_then(|v| v.as_bool()) == Some(true) {
        let port = body
            .get("port")
            .and_then(|v| v.as_u64())
            .map(|value| value as u16);
        let _ = crate::firewall_ops::reconcile(&state, enabled, port).await;
    }
    Ok(Json(serde_json::json!({
        "enabled": enabled,
        "listening": enabled,
        "error": serde_json::Value::Null,
    })))
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
    let message = body.get("message").and_then(|v| v.as_str()).unwrap_or("");
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

// ------------------------------------------------------------ chat runs

/// One replayable SSE frame: global cursor + event name + JSON payload.
#[derive(Debug, Clone)]
pub struct Frame {
    pub cursor: u64,
    pub event: String,
    pub data: serde_json::Value,
}

/// 终态账本与租约释放之间的发布门；重放与下一轮准入共享同一完成事实。
#[derive(Default)]
pub struct TerminalPublication {
    ready: std::sync::atomic::AtomicBool,
    notified: tokio::sync::Notify,
}

impl TerminalPublication {
    pub fn is_ready(&self) -> bool {
        self.ready.load(std::sync::atomic::Ordering::Acquire)
    }
    pub async fn wait(&self) {
        loop {
            let notified = self.notified.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.is_ready() {
                return;
            }
            notified.await;
        }
    }
    pub fn publish(&self) {
        if !self.ready.swap(true, std::sync::atomic::Ordering::AcqRel) {
            self.notified.notify_waiters();
        }
    }
}

/// A live chat run: ring of recorded frames for `/api/runs/{id}/events`
/// replay plus a broadcast channel for attached live clients.
pub struct ChatRun {
    pub frames: Arc<Mutex<Vec<Frame>>>,
    pub tx: tokio::sync::broadcast::Sender<Frame>,
    pub closed: std::sync::atomic::AtomicBool,
    pub cancel: tokio_util::sync::CancellationToken,
    pub finished: Arc<std::sync::atomic::AtomicBool>,
    pub settled: Arc<tokio::sync::Notify>,
    pub terminal_publication: Arc<TerminalPublication>,
}

impl ChatRun {
    /// 只提交重放账本；调用方在释放注册表锁和会话租约后才唤醒读取者。
    pub fn stage(&self, cursor: u64, event: &str, data: serde_json::Value) -> Frame {
        let frame = Frame {
            cursor,
            event: event.to_string(),
            data,
        };
        self.frames.lock().expect("frames lock").push(frame.clone());
        if event == "done" || event == "error" {
            self.closed
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        frame
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
