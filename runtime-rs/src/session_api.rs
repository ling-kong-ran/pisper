//! React/TUI 共用的会话目录、持久化和消息快照适配。
//!
//! Pi 的空会话默认延迟落盘；Pisper 的会话页签必须从创建时起可寻址。
//! 历史与实时快照按会话文件读取，不为查看另一个页签替换正在运行的引擎。

use std::{collections::HashMap, convert::Infallible, io::Write, sync::Arc};

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{sse::Event, IntoResponse, Response, Sse},
    Json,
};
use pi_rust::coding_agent::session_manager::{
    FileEntry, SessionEntry, SessionInfo, SessionManager,
};
use serde_json::{json, Value};

use crate::{product, ApiError, AppState, SessionMeta};

pub(crate) fn load_metadata(data_dir: &str) -> HashMap<String, SessionMeta> {
    std::fs::read(std::path::Path::new(data_dir).join("session-meta.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub(crate) fn save_metadata(state: &AppState) -> Result<(), ApiError> {
    let metadata = state
        .session_meta
        .lock()
        .map_err(|_| ApiError::internal("session metadata lock poisoned"))?;
    std::fs::create_dir_all(&state.data_dir).map_err(|e| ApiError::internal(e.to_string()))?;
    let bytes =
        serde_json::to_vec_pretty(&*metadata).map_err(|e| ApiError::internal(e.to_string()))?;
    // 同目录原子替换，避免进程中断把已有整理与执行设置截成半个 JSON。
    let directory = std::path::Path::new(&state.data_dir);
    let temporary = directory.join("session-meta.json.tmp");
    let mut file =
        std::fs::File::create(&temporary).map_err(|e| ApiError::internal(e.to_string()))?;
    file.write_all(&bytes)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    file.sync_all()
        .map_err(|e| ApiError::internal(e.to_string()))?;
    drop(file);
    std::fs::rename(temporary, directory.join("session-meta.json"))
        .map_err(|e| ApiError::internal(e.to_string()))
}

/// Node 把会话放在 sessions 根目录，Pi 放在工作目录对应子目录；兼容两者。
pub(crate) fn session_infos(agent_dir: &str) -> Vec<SessionInfo> {
    let root = std::path::Path::new(agent_dir).join("sessions");
    let mut directories = vec![root.clone()];
    if let Ok(entries) = std::fs::read_dir(&root) {
        directories.extend(entries.filter_map(Result::ok).filter_map(|entry| {
            entry
                .file_type()
                .ok()
                .filter(|kind| kind.is_dir())
                .map(|_| entry.path())
        }));
    }
    let mut sessions: Vec<SessionInfo> = directories
        .into_iter()
        .flat_map(|dir| SessionManager::list_all(Some(&dir.to_string_lossy()), None))
        .collect();
    sessions.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| a.id.cmp(&b.id)));
    let mut seen = std::collections::HashSet::new();
    sessions.retain(|session| seen.insert(session.id.clone()));
    sessions
}

pub(crate) fn find_session_path(state: &AppState, id: &str) -> Result<String, ApiError> {
    if let Some(info) = session_infos(&state.agent_dir)
        .into_iter()
        .find(|s| s.id == id)
    {
        return Ok(info.path);
    }
    Err(ApiError::new(
        StatusCode::NOT_FOUND,
        "session_not_found",
        "会话不存在。",
    ))
}

/// 不能只写文件：必须重新载入管理器，避免下一条消息再次以 create_new 写同一文件。
pub(crate) fn persist_empty_session(manager: &mut SessionManager) -> Result<(), ApiError> {
    let path = manager
        .get_session_file()
        .ok_or_else(|| ApiError::internal("session persistence disabled"))?
        .to_string();
    if std::path::Path::new(&path).exists() {
        return Ok(());
    }
    let header = manager
        .get_header()
        .ok_or_else(|| ApiError::internal("session header missing"))?;
    let mut entries = vec![FileEntry::Session(header)];
    entries.extend(manager.get_entries().into_iter().map(FileEntry::Entry));
    let mut bytes = Vec::new();
    for entry in entries {
        serde_json::to_writer(&mut bytes, &entry).map_err(|e| ApiError::internal(e.to_string()))?;
        bytes.push(b'\n');
    }
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(mut file) => {
            file.write_all(&bytes)
                .map_err(|e| ApiError::internal(e.to_string()))?;
            file.sync_all()
                .map_err(|e| ApiError::internal(e.to_string()))?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(ApiError::internal(error.to_string())),
    }
    manager
        .set_session_file(&path)
        .map_err(|e| ApiError::internal(e.to_string()))
}

/// 更换工作目录时保留原始对话字节和会话头中的未知字段。
pub(crate) fn persist_session_cwd(path: &str, cwd: &str) -> Result<(), ApiError> {
    let bytes = std::fs::read(path).map_err(|e| ApiError::internal(e.to_string()))?;
    let end = bytes
        .iter()
        .position(|byte| *byte == b'\n')
        .unwrap_or(bytes.len());
    let mut header: Value = serde_json::from_slice(&bytes[..end])
        .map_err(|_| ApiError::internal("invalid session header"))?;
    if header["type"] != "session" {
        return Err(ApiError::internal("missing session header"));
    }
    header["cwd"] = json!(cwd);
    let mut replacement =
        serde_json::to_vec(&header).map_err(|e| ApiError::internal(e.to_string()))?;
    replacement.push(b'\n');
    if end < bytes.len() {
        replacement.extend_from_slice(&bytes[end + 1..]);
    }
    let temporary = std::path::Path::new(path).with_extension("jsonl.cwd.tmp");
    let mut file =
        std::fs::File::create(&temporary).map_err(|e| ApiError::internal(e.to_string()))?;
    file.write_all(&replacement)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    file.sync_all()
        .map_err(|e| ApiError::internal(e.to_string()))?;
    drop(file);
    std::fs::rename(temporary, path).map_err(|e| ApiError::internal(e.to_string()))
}

fn content_text(content: &Value) -> String {
    if let Some(text) = content.as_str() {
        return text.to_string();
    }
    content
        .as_array()
        .map(|parts| {
            parts
                .iter()
                .filter(|part| part["type"] == "text")
                .filter_map(|part| part["text"].as_str())
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

pub(crate) const ATTACHMENT_MARKER: &str = "\n\n---\nAttachment context (injected by Pisper):\n";
fn internal_parent_message(message: &Value) -> bool {
    let text = content_text(&message["content"]);
    text.starts_with(crate::goal_api::CONTINUATION_MARKER)
        || text.starts_with(crate::multi_agent_api::COMPLETION_MARKER)
}
fn serial_message(message: &Value, index: usize, entry_id: &str) -> Value {
    let role = message["role"].as_str().unwrap_or("assistant");
    let attachments = message["content"]
        .as_array()
        .map(|parts| {
            parts
                .iter()
                .enumerate()
                .filter(|(_, part)| part["type"] == "image")
                .map(|(part_index, part)| {
                    json!({
                        "id": format!("image-{index}-{part_index}"), "kind": "image",
                        "name": format!("图片附件 {}", part_index + 1),
                        "mimeType": part["mimeType"], "data": part["data"],
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut item = json!({
        "id": if entry_id.is_empty() { format!("{role}-{}-{index}", message["timestamp"]) }
              else { format!("message-{entry_id}") },
        "role": if role == "assistant" { "agent" } else { "user" },
        "text": if role == "user" { content_text(&message["content"]).split(ATTACHMENT_MARKER).next().unwrap_or_default().to_owned() } else { content_text(&message["content"]) }, "timestamp": message["timestamp"],
        "error": message["errorMessage"], "attachments": attachments,
    });
    if !entry_id.is_empty() && completed_turn(message) {
        item["turnBoundaryEntryId"] = Value::String(entry_id.to_string());
    }
    item
}

fn completed_turn(message: &Value) -> bool {
    if message["role"] != "assistant"
        || message["errorMessage"]
            .as_str()
            .is_some_and(|error| !error.is_empty())
    {
        return false;
    }
    if let Some(reason) = message["stopReason"]
        .as_str()
        .filter(|reason| !reason.is_empty())
    {
        return !matches!(reason, "toolUse" | "error" | "aborted");
    }
    !message["content"]
        .as_array()
        .is_some_and(|parts| parts.iter().any(|part| part["type"] == "toolCall"))
}

fn finish_activity(item: &mut Value, thinking: &[String], tools: &[Value]) {
    if thinking.is_empty() && tools.is_empty() {
        return;
    }
    let tools: Vec<Value> = tools
        .iter()
        .cloned()
        .map(|mut tool| {
            if tool["status"] == "running" {
                tool["status"] = json!("done");
            }
            tool
        })
        .collect();
    item["runActivity"] = json!({
        "thinkingText": thinking.join("\n\n"), "tools": tools,
        "activityFeed": tools, "startedAt": item["timestamp"],
        "lastActivityAt": item["timestamp"], "finishedAt": item["timestamp"],
    });
}

/// 工具中间轮合并到最终助手回复，保留思考和工具结果供重启后展示。
pub(crate) fn serialize_transcript(messages: &[Value], entry_ids: &[String]) -> Vec<Value> {
    let mut result = Vec::new();
    let mut thinking = Vec::new();
    let mut tools: Vec<Value> = Vec::new();
    let mut pending: Option<Value> = None;
    for (index, message) in messages.iter().enumerate() {
        let entry_id = entry_ids.get(index).map(String::as_str).unwrap_or("");
        match message["role"].as_str() {
            Some("toolResult") => {
                if let Some(tool) = tools
                    .iter_mut()
                    .find(|tool| tool["id"] == message["toolCallId"])
                {
                    let output = content_text(&message["content"]);
                    tool["status"] = json!(if message["isError"] == true {
                        "error"
                    } else {
                        "done"
                    });
                    tool["output"] = json!(output);
                    tool["message"] = if message["isError"] == true {
                        json!(output)
                    } else {
                        json!("")
                    };
                    tool["finishedAt"] = message["timestamp"].clone();
                }
            }
            Some("assistant") => {
                if let Some(parts) = message["content"].as_array() {
                    for part in parts {
                        match part["type"].as_str() {
                            Some("thinking") => {
                                if let Some(text) = part["thinking"].as_str().filter(|s| !s.is_empty()) {
                                    thinking.push(text.to_string());
                                }
                            }
                            Some("toolCall") => tools.push(json!({
                                "type": "tool", "id": part["id"], "name": part["name"],
                                "args": part["arguments"], "status": "running",
                                "startedAt": message["timestamp"], "updatedAt": message["timestamp"],
                            })),
                            _ => {}
                        }
                    }
                }
                let item = serial_message(message, index, entry_id);
                let terminal = message["stopReason"]
                    .as_str()
                    .map(|s| s != "toolUse")
                    .unwrap_or_else(|| {
                        !message["content"].as_array().is_some_and(|parts| {
                            parts.iter().any(|part| part["type"] == "toolCall")
                        })
                    });
                if !terminal {
                    pending = Some(item);
                } else {
                    let mut item = item;
                    if item["text"] == "" {
                        if let Some(previous) = &pending {
                            item["text"] = previous["text"].clone();
                        }
                    }
                    finish_activity(&mut item, &thinking, &tools);
                    if item["text"] != ""
                        || item["error"].is_string()
                        || !tools.is_empty()
                        || !thinking.is_empty()
                    {
                        result.push(item);
                    }
                    pending = None;
                    thinking.clear();
                    tools.clear();
                }
            }
            Some("user") => {
                if internal_parent_message(message) {
                    continue;
                }
                if let Some(mut item) = pending.take() {
                    finish_activity(&mut item, &thinking, &tools);
                    result.push(item);
                    thinking.clear();
                    tools.clear();
                }
                result.push(serial_message(message, index, entry_id));
            }
            _ => {}
        }
    }
    if let Some(mut item) = pending {
        finish_activity(&mut item, &thinking, &tools);
        result.push(item);
    }
    result
}

pub(crate) fn message_page(messages: &[Value], before: Option<&str>, limit: usize) -> Value {
    let end = before
        .and_then(|cursor| cursor.parse::<usize>().ok())
        .unwrap_or(messages.len())
        .min(messages.len());
    let start = end.saturating_sub(limit.clamp(1, 200));
    json!({ "messages": messages[start..end], "pageInfo": {
        "start": start, "end": end, "total": messages.len(), "hasMore": start > 0,
        "nextCursor": if start > 0 { Some(start.to_string()) } else { None },
    } })
}

fn manager_messages(manager: &SessionManager) -> (Vec<Value>, Vec<String>) {
    manager
        .get_branch(None)
        .into_iter()
        .filter_map(|entry| match entry {
            SessionEntry::Message(entry) => serde_json::to_value(entry.message)
                .ok()
                .map(|message| (message, entry.id)),
            _ => None,
        })
        .unzip()
}

fn model_and_thinking(manager: &SessionManager) -> (String, String) {
    let mut model = String::new();
    let mut thinking = "off".to_string();
    for entry in manager.get_branch(None) {
        match entry {
            SessionEntry::ModelChange(entry) => {
                model = format!("{}/{}", entry.provider, entry.model_id)
            }
            SessionEntry::ThinkingLevelChange(entry) => thinking = entry.thinking_level,
            _ => {}
        }
    }
    (model, thinking)
}

fn bounded_text(text: &str, limit: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= limit {
        return text;
    }
    format!(
        "{}…",
        text.chars()
            .take(limit.saturating_sub(1))
            .collect::<String>()
            .trim_end()
    )
}

fn tree_entry_projection(entry: &Value) -> (String, String, String, String) {
    let kind = entry["type"].as_str().unwrap_or("metadata");
    let mut role = String::new();
    let mut status = String::new();
    let (kind, text) = match kind {
        "message" => {
            let message = &entry["message"];
            role = message["role"].as_str().unwrap_or("message").to_string();
            let text = content_text(&message["content"]);
            let calls: Vec<_> = message["content"]
                .as_array()
                .map(|parts| {
                    parts
                        .iter()
                        .filter(|part| part["type"] == "toolCall")
                        .filter_map(|part| part["name"].as_str())
                        .collect()
                })
                .unwrap_or_default();
            if role == "toolResult" {
                status = if message["isError"] == true {
                    "error"
                } else {
                    "completed"
                }
                .to_string();
                (
                    "tool",
                    message["toolName"]
                        .as_str()
                        .or_else(|| message["toolCallId"].as_str())
                        .unwrap_or("")
                        .to_string(),
                )
            } else if role == "assistant" && !calls.is_empty() {
                (
                    "tool-call",
                    if text.is_empty() {
                        calls.join(", ")
                    } else {
                        format!("{text} · {}", calls.join(", "))
                    },
                )
            } else {
                if completed_turn(message) {
                    status = "completed".to_string();
                }
                (
                    match role.as_str() {
                        "user" => "user",
                        "assistant" => "assistant",
                        _ => "message",
                    },
                    text,
                )
            }
        }
        "branch_summary" => (
            "summary",
            entry["summary"].as_str().unwrap_or("").to_string(),
        ),
        "compaction" => (
            "compaction",
            entry["summary"].as_str().unwrap_or("").to_string(),
        ),
        "model_change" => (
            "settings",
            format!(
                "{}/{}",
                entry["provider"].as_str().unwrap_or(""),
                entry["modelId"].as_str().unwrap_or("")
            ),
        ),
        "thinking_level_change" => (
            "settings",
            entry["thinkingLevel"].as_str().unwrap_or("").to_string(),
        ),
        "session_info" => ("metadata", entry["name"].as_str().unwrap_or("").to_string()),
        "label" => ("label", entry["label"].as_str().unwrap_or("").to_string()),
        "custom_message" if entry["display"] == true => {
            ("extension", content_text(&entry["content"]))
        }
        "custom" if entry["customType"] == "pisper.session-tree-position" => {
            ("position", String::new())
        }
        "custom" => (
            "extension",
            entry["customType"].as_str().unwrap_or("").to_string(),
        ),
        _ => ("metadata", kind.to_string()),
    };
    (kind.to_string(), role, bounded_text(&text, 320), status)
}

pub(crate) fn project_tree(manager: &SessionManager, streaming: bool) -> Result<Value, ApiError> {
    let active_ids: std::collections::HashSet<_> = manager
        .get_branch(None)
        .iter()
        .filter_map(|entry| entry.id().map(str::to_string))
        .collect();
    let leaf_id = manager.get_leaf_id();
    let mut stack: Vec<_> = manager
        .get_tree()
        .into_iter()
        .rev()
        .map(|node| (node, None::<String>))
        .collect();
    let mut nodes = Vec::new();
    let mut branch_count = 0;
    while let Some((node, parent_id)) = stack.pop() {
        let raw =
            serde_json::to_value(&node.entry).map_err(|e| ApiError::internal(e.to_string()))?;
        let Some(id) = node.entry.id().map(str::to_string) else {
            continue;
        };
        let (kind, role, text, status) = tree_entry_projection(&raw);
        branch_count += node.children.len().saturating_sub(1);
        nodes.push(json!({
            "id":id, "parentId":parent_id,"type":raw["type"],"kind":kind,"role":role,
            "text":text,"status":status,"label":bounded_text(node.label.as_deref().unwrap_or(""),80),
            "timestamp":node.entry.timestamp(),"active":active_ids.contains(&id),
            "leaf":leaf_id == Some(id.as_str()),"branchPoint":node.children.len() > 1,
        }));
        for child in node.children.into_iter().rev() {
            stack.push((child, Some(id.clone())));
        }
    }
    Ok(
        json!({"sessionId":manager.get_session_id(),"leafId":leaf_id,"nodeCount":nodes.len(),
        "branchCount":branch_count,"streaming":streaming,"nodes":nodes,"lineage":null}),
    )
}

pub(crate) async fn tree(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let active = state
        .sessions
        .get(&id)
        .map(|host| host.session())
        .unwrap_or_else(|| state.runtime.session());
    if active.session_id() == id {
        let manager = active
            .session_manager
            .lock()
            .map_err(|_| ApiError::internal("session manager lock poisoned"))?;
        return Ok(Json(project_tree(&manager, active.is_streaming())?));
    }
    let manager = SessionManager::open(&find_session_path(&state, &id)?, None, None)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(project_tree(&manager, false)?))
}

pub(crate) async fn labels(
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Value>, ApiError> {
    let query =
        bounded_text(params.get("query").map(String::as_str).unwrap_or(""), 80).to_lowercase();
    let limit = params
        .get("limit")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(if query.is_empty() { 500 } else { 20 })
        .clamp(1, 500);
    let mut labels = Vec::new();
    for info in session_infos(&state.agent_dir) {
        let manager = SessionManager::open(&info.path, None, None)
            .map_err(|e| ApiError::internal(e.to_string()))?;
        let tree = project_tree(&manager, false)?;
        for node in tree["nodes"].as_array().into_iter().flatten() {
            let label = node["label"].as_str().unwrap_or("");
            if label.is_empty()
                || node["kind"] != "assistant"
                || node["status"] != "completed"
                || (!query.is_empty() && !label.to_lowercase().contains(&query))
            {
                continue;
            }
            labels.push(json!({"sessionId":info.id,"sessionName":info.name.as_deref().unwrap_or(""),
                "sessionCreated":info.created.map(pi_rust::agent_core::harness::session::jsonl::iso8601::format_iso8601_utc).unwrap_or_default(),
                "sessionModified":pi_rust::agent_core::harness::session::jsonl::iso8601::format_iso8601_utc(info.modified),
                "entryId":node["id"],"label":label,"summary":node["text"],
                "nodeTimestamp":node["timestamp"],"active":node["active"]}));
            if labels.len() >= limit {
                return Ok(Json(json!({"labels":labels})));
            }
        }
    }
    Ok(Json(json!({"labels":labels})))
}

pub(crate) fn model_state(state: &AppState, id: &str) -> Result<Value, ApiError> {
    let active = state
        .sessions
        .get(id)
        .map(|host| host.session())
        .unwrap_or_else(|| state.runtime.session());
    let model = if active.session_id() == id {
        active.model()
    } else {
        let manager = SessionManager::open(&find_session_path(state, id)?, None, None)
            .map_err(|e| ApiError::internal(e.to_string()))?;
        let reference = model_and_thinking(&manager).0;
        active
            .model_runtime()
            .get_available_snapshot()
            .into_iter()
            .find(|model| format!("{}/{}", model.provider, model.id) == reference)
    };
    Ok(match model {
        Some(model) => json!({"provider":model.provider,"id":model.id,"name":model.name,
        "baseUrl":model.base_url,"reasoning":model.reasoning,"model":format!("{}/{}",model.provider,model.id)}),
        None => Value::Null,
    })
}

pub(crate) fn thinking_state(state: &AppState, id: &str) -> Result<Value, ApiError> {
    let active = state
        .sessions
        .get(id)
        .map(|host| host.session())
        .unwrap_or_else(|| state.runtime.session());
    let (level, levels) = if active.session_id() == id {
        (
            serde_json::to_value(active.thinking_level())
                .map_err(|e| ApiError::internal(e.to_string()))?,
            serde_json::to_value(active.get_available_thinking_levels())
                .map_err(|e| ApiError::internal(e.to_string()))?,
        )
    } else {
        let manager = SessionManager::open(&find_session_path(state, id)?, None, None)
            .map_err(|e| ApiError::internal(e.to_string()))?;
        let (reference, level) = model_and_thinking(&manager);
        let model = active
            .model_runtime()
            .get_available_snapshot()
            .into_iter()
            .find(|model| format!("{}/{}", model.provider, model.id) == reference);
        let levels = model
            .as_ref()
            .map(pi_rust::ai::models::get_supported_thinking_levels)
            .map(|levels| json!(levels))
            .unwrap_or_else(|| json!(["off"]));
        (json!(level), levels)
    };
    Ok(json!({"thinkingLevel":level,"availableLevels":levels,
        "supported":levels.as_array().is_some_and(|levels|levels.len()>1),"status":"ok"}))
}

/// Pi 当前仅提供队列文本，没有可用于精确撤回的稳定标识；空 id 禁用撤回按钮。
pub(crate) fn queued_inputs(state: &AppState, id: &str) -> Vec<Value> {
    let active = state
        .sessions
        .get(id)
        .map(|host| host.session())
        .unwrap_or_else(|| state.runtime.session());
    if active.session_id() != id {
        return Vec::new();
    }
    let mut values = Vec::new();
    for (behavior, texts) in [
        ("steer", active.get_steering_messages()),
        ("followUp", active.get_follow_up_messages()),
    ] {
        for text in texts {
            values.push(json!({"id":"","text":text,"behavior":behavior,"attachments":[],"withdrawable":false}));
        }
    }
    values
}

fn session_usage(manager: &SessionManager) -> Result<Value, ApiError> {
    let mut total = json!({"input":0.0,"output":0.0,"cacheRead":0.0,"cacheWrite":0.0,"reasoning":0.0,
        "totalTokens":0.0,"processedTokens":0.0,"requests":0,"promptTokens":0.0,"cacheReported":false,"cacheHitRate":null,"cost":0.0});
    for entry in manager.get_entries() {
        let entry = serde_json::to_value(entry).map_err(|e| ApiError::internal(e.to_string()))?;
        let usage = if entry["type"] == "message" {
            &entry["message"]["usage"]
        } else {
            &entry["usage"]
        };
        if !usage.is_object() {
            continue;
        }
        for key in ["input", "output", "cacheRead", "cacheWrite", "reasoning"] {
            total[key] = json!(
                total[key].as_f64().unwrap_or(0.0) + usage[key].as_f64().unwrap_or(0.0).max(0.0)
            );
        }
        let input = usage["input"].as_f64().unwrap_or(0.0).max(0.0);
        let output = usage["output"].as_f64().unwrap_or(0.0).max(0.0);
        let read = usage["cacheRead"].as_f64().unwrap_or(0.0).max(0.0);
        let write = usage["cacheWrite"].as_f64().unwrap_or(0.0).max(0.0);
        let reported = usage["totalTokens"]
            .as_f64()
            .or_else(|| usage["total"].as_f64())
            .unwrap_or(0.0);
        total["totalTokens"] = json!(
            total["totalTokens"].as_f64().unwrap_or(0.0)
                + if reported > 0.0 {
                    reported
                } else {
                    input + output + read + write
                }
        );
        total["processedTokens"] =
            json!(total["processedTokens"].as_f64().unwrap_or(0.0) + input + output + write);
        total["requests"] = json!(total["requests"].as_u64().unwrap_or(0) + 1);
        if read > 0.0 || write > 0.0 {
            total["cacheReported"] = json!(true);
        }
        total["cost"] = json!(
            total["cost"].as_f64().unwrap_or(0.0) + usage["cost"]["total"].as_f64().unwrap_or(0.0)
        );
    }
    let prompt = total["input"].as_f64().unwrap_or(0.0)
        + total["cacheRead"].as_f64().unwrap_or(0.0)
        + total["cacheWrite"].as_f64().unwrap_or(0.0);
    total["promptTokens"] = json!(prompt);
    if total["cacheReported"] == true && prompt > 0.0 {
        total["cacheHitRate"] = json!(total["cacheRead"].as_f64().unwrap_or(0.0) / prompt * 100.0);
    }
    Ok(total)
}

pub(crate) async fn today_usage(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, ApiError> {
    let day = chrono::Local::now().format("%Y-%m-%d").to_string();
    for session in session_infos(&state.agent_dir) {
        state
            .usage
            .scan_session(std::path::Path::new(&session.path), &session.id, &day)
            .map_err(|error| ApiError::internal(error.to_string()))?;
    }
    Ok(Json(
        state
            .usage
            .today_usage()
            .map_err(|error| ApiError::internal(error.to_string()))?,
    ))
}

fn context_usage(state: &AppState, id: &str, manager: &SessionManager) -> Result<Value, ApiError> {
    let active = state
        .sessions
        .get(id)
        .map(|host| host.session())
        .unwrap_or_else(|| state.runtime.session());
    if active.session_id() == id {
        return serde_json::to_value(
            active
                .get_context_usage()
                .map_err(|e| ApiError::internal(e.to_string()))?,
        )
        .map_err(|e| ApiError::internal(e.to_string()));
    }
    let reference = model_and_thinking(manager).0;
    let Some(model) = active
        .model_runtime()
        .get_available_snapshot()
        .into_iter()
        .find(|model| {
            format!("{}/{}", model.provider, model.id) == reference && model.context_window > 0
        })
    else {
        return Ok(Value::Null);
    };
    let branch = manager.get_branch(None);
    if let Some(boundary) = branch
        .iter()
        .rposition(|entry| matches!(entry, SessionEntry::Compaction(_)))
    {
        let post_usage = branch[boundary + 1..].iter().any(|entry| {
            let Ok(entry) = serde_json::to_value(entry) else {
                return false;
            };
            entry["type"] == "message"
                && entry["message"]["role"] == "assistant"
                && !matches!(
                    entry["message"]["stopReason"].as_str(),
                    Some("error" | "aborted")
                )
                && entry["message"]["usage"]["input"].as_f64().unwrap_or(0.0) > 0.0
        });
        if !post_usage {
            return Ok(json!({"tokens":null,"contextWindow":model.context_window,"percent":null}));
        }
    }
    let estimate = pi_rust::coding_agent::core::compaction::estimate_context_tokens(
        &manager.build_session_context().messages,
    );
    Ok(
        json!({"tokens":estimate.tokens,"contextWindow":model.context_window,"percent":estimate.tokens as f64/model.context_window as f64*100.0}),
    )
}

fn latest_run_frames(state: &AppState, id: &str) -> Result<Vec<product::Frame>, ApiError> {
    let runs = state
        .chat_runs
        .lock()
        .map_err(|_| ApiError::internal("chat runs lock poisoned"))?;
    let mut latest = Vec::new();
    let mut cursor = 0;
    for run in runs.values() {
        let frames = run
            .frames
            .lock()
            .map_err(|_| ApiError::internal("run frames lock poisoned"))?;
        if frames
            .first()
            .is_some_and(|frame| frame.event == "run" && frame.data["sessionId"] == id)
            && frames.last().is_some_and(|frame| frame.cursor > cursor)
        {
            cursor = frames.last().map(|frame| frame.cursor).unwrap_or(0);
            latest = frames.clone();
        }
    }
    Ok(latest)
}

fn apply_run_activity(value: &mut Value, frames: &[product::Frame], streaming: bool) {
    let mut tools: Vec<Value> = Vec::new();
    let mut thinking = String::new();
    let mut text = String::new();
    let mut latest_event = "prompt_submitted";
    for frame in frames {
        if let Some(at) = frame.data.get("eventAt").filter(|at| at.is_string()) {
            value["lastActivityAt"] = at.clone();
        }
        latest_event = frame.event.as_str();
        match frame.event.as_str() {
            "meta" => {
                value["startedAt"] = frame.data["startedAt"].clone();
            }
            "text_delta" => text.push_str(frame.data["delta"].as_str().unwrap_or("")),
            "text_end" => text = frame.data["text"].as_str().unwrap_or("").to_string(),
            "thinking_patch" => {
                if let Some(part) = frame.data["text"].as_str() {
                    thinking.push_str(part);
                }
            }
            "tool_start" => {
                let mut tool = frame.data.clone();
                tool["type"] = json!("tool");
                tool["status"] = json!("running");
                tools.push(tool);
            }
            "tool_update" | "tool_end" => {
                if let Some(tool) = tools.iter_mut().find(|tool| tool["id"] == frame.data["id"]) {
                    if let Some(fields) = frame.data.as_object() {
                        for (key, item) in fields {
                            tool[key] = item.clone();
                        }
                    }
                }
            }
            "done" | "error" => {
                value["finishedAt"] = frame.data["eventAt"].clone();
                if frame.event == "error" {
                    value["error"] = frame.data["message"].clone();
                }
            }
            _ => {}
        }
    }
    if !frames.is_empty() {
        value["tools"] = json!(tools);
        value["activityFeed"] = json!(tools);
        if streaming && !thinking.is_empty() {
            value["thinkingText"] = json!(thinking);
        }
        let phase = if streaming {
            if tools.iter().any(|tool| tool["status"] == "running") {
                "using_tool"
            } else if !text.is_empty() {
                "responding"
            } else {
                "thinking"
            }
        } else if matches!(latest_event, "run" | "meta") {
            "starting"
        } else if value["error"]
            .as_str()
            .is_some_and(|error| !error.is_empty())
        {
            "failed"
        } else {
            "completed"
        };
        value["lifecycle"] =
            json!({"phase":phase,"event":latest_event,"updatedAt":value["lastActivityAt"]});
        if streaming || phase == "starting" {
            value["currentActivity"] = tools
                .iter()
                .rev()
                .find(|tool| tool["status"] == "running")
                .cloned()
                .unwrap_or_else(
                    || json!({"type":"model","stage":phase,"updatedAt":value["lastActivityAt"]}),
                );
        } else {
            value["currentActivity"] = Value::Null;
        }
    }
}

pub(crate) fn summary(state: &AppState, manager: &SessionManager, modified: i64) -> Result<Value, ApiError> {
    let id = manager.get_session_id();
    let (model, thinking) = model_and_thinking(manager);
    let active = state
        .sessions
        .get(id)
        .map(|host| host.session())
        .unwrap_or_else(|| state.runtime.session());
    let active_id = active.session_id();
    let is_active = active_id == id;
    let model = if is_active {
        active
            .model()
            .map(|m| format!("{}/{}", m.provider, m.id))
            .unwrap_or(model)
    } else {
        model
    };
    let thinking = if is_active {
        serde_json::to_value(active.thinking_level())
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or(thinking)
    } else {
        thinking
    };
    let meta = state
        .session_meta
        .lock()
        .map_err(|_| ApiError::internal("session metadata lock poisoned"))?
        .get(id)
        .cloned()
        .unwrap_or_default();
    let (messages, _) = manager_messages(manager);
    Ok(json!({
        "id": id, "name": manager.get_session_name().unwrap_or_default(),
        "cwd": manager.get_cwd(), "model": model, "thinkingLevel": thinking,
        "modified": pi_rust::agent_core::harness::session::jsonl::iso8601::format_iso8601_utc(modified),
        "created": manager.get_header().and_then(|h| h.timestamp),
        "streaming": (is_active && active.is_streaming()) || state.goals.busy(id), "messageCount": messages.len(),
        "firstMessage": messages.iter().find(|m| m["role"] == "user")
            .map(|m| content_text(&m["content"])).unwrap_or_default(),
        "executionMode": meta.execution_mode.as_deref().unwrap_or("approval-required"),
        "permissionMode": meta.permission_mode.as_deref().unwrap_or("ask"),
        "runMode": meta.run_mode.as_deref().unwrap_or("plan"),
        "pinned": meta.pinned, "archived": meta.archived, "unread": meta.unread,
        "needsAttention": !state.approvals.pending(&id).is_empty(),
        "attentionReason": if state.approvals.pending(&id).is_empty() { Value::Null } else { json!("approval") }, "goal": null,
        "plan": null, "team": null,
    }))
}

pub(crate) fn snapshot(state: &AppState, id: &str) -> Result<Value, ApiError> {
    let active = state
        .sessions
        .get(id)
        .map(|host| host.session())
        .unwrap_or_else(|| state.runtime.session());
    let is_active = active.session_id() == id;
    let (manager, modified) = {
        let path = find_session_path(state, id)?;
        let modified = std::fs::metadata(&path)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or(0);
        (
            SessionManager::open(&path, None, None)
                .map_err(|e| ApiError::internal(e.to_string()))?,
            modified,
        )
    };
    let (mut messages, mut entry_ids) = manager_messages(&manager);
    let mut transcript = serialize_transcript(&messages, &entry_ids);
    let mut error = transcript
        .last()
        .and_then(|m| m["error"].as_str())
        .unwrap_or("")
        .to_string();
    let mut live_thinking = String::new();
    let streaming = (is_active && active.is_streaming()) || state.goals.busy(id);
    if is_active {
        let agent_state = active.state();
        error = agent_state.error_message.clone().unwrap_or_default();
        if let Some(partial) = &agent_state.streaming_message {
            if let Ok(raw) = serde_json::to_value(partial) {
                // 把工具中间轮与当前文本一起投影，避免恢复快照出现两个回复气泡。
                messages.push(raw);
                entry_ids.push(String::new());
                transcript = serialize_transcript(&messages, &entry_ids);
                if let Some(item) = transcript.last_mut().filter(|item| item["role"] == "agent") {
                    item["id"] = json!(format!("live-{id}"));
                    item["streaming"] = json!(streaming);
                    live_thinking = item["runActivity"]["thinkingText"]
                        .as_str()
                        .unwrap_or("")
                        .to_string();
                }
            }
        }
    }
    let mut value = summary(state, &manager, modified)?;
    transcript = crate::asset_api::projection::attach_generated_assets(
        &transcript,
        &state
            .assets
            .lock()
            .map_err(|_| ApiError::internal("Asset index lock failed"))?
            .generated_for_session(id),
    );
    let page = message_page(&transcript, None, 80);
    value["messages"] = page["messages"].clone();
    value["pageInfo"] = page["pageInfo"].clone();
    value["configurationBusy"] = json!(streaming || (is_active && active.is_compacting()));
    value["streaming"] = json!(streaming);
    value["error"] = json!(error);
    value["thinkingText"] = json!(live_thinking);
    for key in [
        "tools",
        "approvals",
        "agents",
        "activityFeed",
        "queuedInputs",
    ] {
        value[key] = json!([]);
    }
    value["approvals"] = json!(state.approvals.pending(id));
    value["plan"] = state.plans.get(id);
    value["goal"] = json!(state.goals.goals.get(id));
    value["agents"] = json!(state.agents.summaries(id));
    value["team"] = json!(state.team.projection(id));
    for key in [
        "currentActivity",
        "lifecycle",
        "contextUsage",
        "sessionUsage",
        "promptCache",
        "compaction",
        "startedAt",
        "lastActivityAt",
        "finishedAt",
    ] {
        value[key] = Value::Null;
    }
    value["sessionTreeRevision"] = json!(manager.get_entry_count());
    value["queuedInputs"] = json!(queued_inputs(state, id));
    value["pendingMessageCount"] = json!(if is_active {
        active.pending_message_count()
    } else {
        0
    });
    value["contextUsage"] = context_usage(state, id, &manager)?;
    value["sessionUsage"] = session_usage(&manager)?;
    let frames = latest_run_frames(state, id)?;
    apply_run_activity(&mut value, &frames, streaming);
    if !frames.is_empty() {
        value["queueRevision"] = json!(frames.last().map(|frame| frame.cursor).unwrap_or(0));
    }
    if is_active && active.is_compacting() {
        value["compaction"] = json!({"active":true,"status":"running"});
        value["currentActivity"] = json!({"type":"compaction","compaction":value["compaction"],"updatedAt":value["lastActivityAt"]});
        value["lifecycle"] = json!({"phase":"compacting","event":"compaction_start","updatedAt":value["lastActivityAt"]});
    } else if is_active && streaming && active.retry_attempt() > 0 {
        value["lifecycle"] = json!({"phase":"retrying","event":"auto_retry_start","updatedAt":value["lastActivityAt"],
            "retry":{"attempt":active.retry_attempt()}});
        value["currentActivity"] =
            json!({"type":"retry","stage":"waiting_retry","updatedAt":value["lastActivityAt"]});
    }
    if value["lifecycle"].is_null() {
        if let Some(last) = transcript.last().filter(|item| item["role"] == "agent") {
            let phase = if streaming {
                "responding"
            } else if !error.is_empty() {
                "failed"
            } else {
                "completed"
            };
            value["lifecycle"] = json!({"phase":phase,"event":if streaming{"message_update"}else{"agent_settled"},"updatedAt":last["timestamp"]});
        }
    }
    Ok(value)
}

pub(crate) async fn list_sessions(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, ApiError> {
    let mut sessions = Vec::new();
    for info in session_infos(&state.agent_dir) {
        let manager = SessionManager::open(&info.path, None, None)
            .map_err(|e| ApiError::internal(e.to_string()))?;
        sessions.push(summary(&state, &manager, info.modified)?);
    }
    Ok(Json(json!({ "sessions": sessions })))
}

pub(crate) async fn create_session(
    State(state): State<Arc<AppState>>,
    body: Option<Json<Value>>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let body = body.map(|Json(value)| value).unwrap_or_else(|| json!({}));
    let cwd = body["cwd"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(&state.cwd);
    let cwd = std::fs::canonicalize(cwd).map_err(|_| ApiError::bad_request("工作目录不存在。"))?;
    if !cwd.is_dir() {
        return Err(ApiError::bad_request("工作目录必须是文件夹。"));
    }
    let cwd = cwd
        .to_string_lossy()
        .strip_prefix("\\\\?\\")
        .unwrap_or(&cwd.to_string_lossy())
        .to_string();
    let dir = std::path::Path::new(&state.agent_dir).join("sessions");
    let mut manager = SessionManager::create(&cwd, Some(&dir.to_string_lossy()), None)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let name = body["name"].as_str().unwrap_or("New conversation");
    manager
        .append_session_info(name)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let active = state.runtime.session();
    if let Some(model) = active.model() {
        manager
            .append_model_change(&model.provider, &model.id)
            .map_err(|e| ApiError::internal(e.to_string()))?;
    }
    let thinking = serde_json::to_value(active.thinking_level())
        .map_err(|e| ApiError::internal(e.to_string()))?;
    manager
        .append_thinking_level_change(thinking.as_str().unwrap_or("off"))
        .map_err(|e| ApiError::internal(e.to_string()))?;
    persist_empty_session(&mut manager)?;
    if let Err(error) = state
        .file_changes
        .mark_session_tracked(
            manager.get_session_id(),
            std::path::Path::new(manager.get_cwd()),
        )
        .await
    {
        if let Some(path) = manager.get_session_file() {
            if let Err(cleanup) = std::fs::remove_file(path) {
                return Err(ApiError::internal(format!(
                    "{}; new session cleanup also failed: {}",
                    error.message, cleanup
                )));
            }
        }
        return Err(error);
    }
    Ok((
        StatusCode::CREATED,
        Json(summary(&state, &manager, product::now_ms() as i64)?),
    ))
}

pub(crate) async fn get_messages(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Value>, ApiError> {
    let manager = SessionManager::open(&find_session_path(&state, &id)?, None, None)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let (messages, ids) = manager_messages(&manager);
    let transcript = crate::asset_api::projection::attach_generated_assets(
        &serialize_transcript(&messages, &ids),
        &state
            .assets
            .lock()
            .map_err(|_| ApiError::internal("Asset index lock failed"))?
            .generated_for_session(&id),
    );
    let limit = params
        .get("limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(40);
    let mut page = message_page(&transcript, params.get("before").map(String::as_str), limit);
    page["model"] = json!(model_and_thinking(&manager).0);
    page["contextUsage"] = context_usage(&state, &id, &manager)?;
    page["sessionUsage"] = session_usage(&manager)?;
    Ok(Json(page))
}

pub(crate) async fn live(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let value = snapshot(&state, &id)?;
    if !headers
        .get("accept")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("text/event-stream"))
    {
        return Ok(Json(value).into_response());
    }
    let rx = state.events.subscribe();
    let initial =
        Some(serde_json::to_string(&value).map_err(|e| ApiError::internal(e.to_string()))?);
    let projection = Arc::new(std::sync::Mutex::new(
        crate::chat_stream::Projection::default(),
    ));
    let stream = futures::stream::unfold(
        (rx, initial, state, id),
        move |(mut rx, initial, state, id)| {
            let projection = projection.clone();
            async move {
                if let Some(value) = initial {
                    return Some((
                        Ok::<_, Infallible>(Event::default().event("snapshot").data(value)),
                        (rx, None, state, id),
                    ));
                }
                loop {
                    let incoming = tokio::select! {
                        _ = state.shutdown.cancelled() => return None,
                        incoming = rx.recv() => incoming,
                    };
                    match incoming {
                        Ok(value) => {
                            if let Ok(envelope) = serde_json::from_str::<Value>(&value) {
                                if let Some(event) = envelope["pisperEvent"].as_str() {
                                    if envelope["sessionId"].as_str() != Some(&id) {
                                        continue;
                                    }
                                    let (event, data) = if event == "engine" {
                                        let mapped = crate::chat_stream::project(
                                            &envelope["data"],
                                            &id,
                                            &mut projection.lock().expect("live projection"),
                                        );
                                        let Some(mapped) = mapped else {
                                            continue;
                                        };
                                        mapped
                                    } else {
                                        (event.to_owned(), envelope["data"].clone())
                                    };
                                    let response = Event::default().event(event);
                                    return Some((
                                        Ok(response.data(data.to_string())),
                                        (rx, None, state, id),
                                    ));
                                }
                            }
                            // 共用引擎换会话后，原会话的订阅不得收到另一个页签的事件。
                            if state.runtime.session().session_id() != id {
                                continue;
                            }
                            return Some((Ok(Event::default().data(value)), (rx, None, state, id)));
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
                    }
                }
            }
        },
    );
    Ok(Sse::new(stream)
        .keep_alive(axum::response::sse::KeepAlive::default())
        .into_response())
}

pub(crate) async fn rename(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let mutation = crate::session_runtime::mutation(&state, &id).await?;
    let name = body["name"]
        .as_str()
        .ok_or_else(|| ApiError::bad_request("会话名称必须是字符串。"))?;
    let path = find_session_path(&state, &id)?;
    let active = mutation.hosted.session();
    if active.session_id() == id {
        let mut manager = active
            .session_manager
            .lock()
            .map_err(|_| ApiError::internal("session manager lock poisoned"))?;
        manager
            .append_session_info(name)
            .map_err(|e| ApiError::internal(e.to_string()))?;
    } else {
        let mut manager = SessionManager::open(&path, None, None)
            .map_err(|e| ApiError::internal(e.to_string()))?;
        manager
            .append_session_info(name)
            .map_err(|e| ApiError::internal(e.to_string()))?;
    }
    let manager =
        SessionManager::open(&path, None, None).map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(summary(&state, &manager, product::now_ms() as i64)?))
}

pub(crate) async fn delete_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let path = find_session_path(&state, &id)?;
    state.sessions.delete(&state, &id, &path).await?;
    state
        .agents
        .remove_parent(&id)
        .await
        .map_err(|error| ApiError::internal(error.to_string()))?;
    state
        .plans
        .remove(&id)
        .map_err(|error| ApiError::internal(error.to_string()))?;
    state
        .goals
        .goals
        .clear(&id)
        .map_err(|error| ApiError::internal(error.to_string()))?;
    state.file_changes.wait_session(&id).await?;
    state.file_changes.clear(&id).await?;
    state
        .session_meta
        .lock()
        .map_err(|_| ApiError::internal("session metadata lock poisoned"))?
        .remove(&id);
    save_metadata(&state)?;
    Ok(Json(json!({ "id": id, "deleted": true })))
}

pub(crate) async fn organization(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let _mutation = state.engine_mutation.try_read().map_err(|_| {
        ApiError::new(
            StatusCode::CONFLICT,
            "session_busy",
            "当前会话正在运行，请等待结束或停止运行。",
        )
    })?;
    find_session_path(&state, &id)?;
    if !["pinned", "archived", "read"]
        .iter()
        .any(|key| body.get(key).is_some())
    {
        return Err(ApiError::bad_request("缺少会话整理字段。"));
    }
    for key in ["pinned", "archived", "read"] {
        if body.get(key).is_some_and(|value| !value.is_boolean()) {
            return Err(ApiError::bad_request("会话整理字段必须是布尔值。"));
        }
    }
    let response = {
        let mut metadata = state
            .session_meta
            .lock()
            .map_err(|_| ApiError::internal("session metadata lock poisoned"))?;
        let meta = metadata.entry(id.clone()).or_default();
        if let Some(value) = body["pinned"].as_bool() {
            meta.pinned = value;
        }
        if let Some(value) = body["archived"].as_bool() {
            meta.archived = value;
        }
        if let Some(value) = body["read"].as_bool() {
            meta.unread = !value;
        }
        json!({ "id": id, "pinned": meta.pinned, "archived": meta.archived,
            "unread": meta.unread, "needsAttention": false, "attentionReason": null })
    };
    save_metadata(&state)?;
    Ok(Json(response))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assistant(
        text: &str,
        usage: pi_rust::ai::types::Usage,
    ) -> pi_rust::agent_core::types::AgentMessage {
        serde_json::from_value(json!({
            "role":"assistant","content":[{"type":"text","text":text}],
            "api":"openai-completions","provider":"fixture","model":"fixture",
            "stopReason":"stop","timestamp":2,"usage":usage,
        }))
        .unwrap()
    }

    #[test]
    fn tree_preserves_branches_and_labels_without_navigation() {
        let mut manager = SessionManager::in_memory("C:/fixture", None, None).unwrap();
        let user =
            serde_json::from_value(json!({"role":"user","content":"question","timestamp":1}))
                .unwrap();
        manager.append_message(user).unwrap();
        let branch = manager.get_leaf_id().unwrap().to_string();
        manager
            .append_message(assistant("original", Default::default()))
            .unwrap();
        let original = manager.get_leaf_id().unwrap().to_string();
        manager
            .append_label_change(&original, Some("release answer"))
            .unwrap();
        manager.branch(&branch).unwrap();
        manager
            .append_message(assistant("revised", Default::default()))
            .unwrap();
        let revised = manager.get_leaf_id().unwrap().to_string();
        let tree = project_tree(&manager, false).unwrap();
        assert_eq!(tree["nodeCount"], 4);
        assert_eq!(tree["branchCount"], 1);
        assert_eq!(tree["leafId"], revised);
        let nodes = tree["nodes"].as_array().unwrap();
        let old = nodes.iter().find(|node| node["id"] == original).unwrap();
        assert_eq!(old["label"], "release answer");
        assert_eq!(old["status"], "completed");
        assert_eq!(old["active"], false);
        let new = nodes.iter().find(|node| node["id"] == revised).unwrap();
        assert_eq!(new["parentId"], branch);
        assert_eq!(new["active"], true);
        assert_eq!(nodes[0]["branchPoint"], true);
    }

    #[test]
    fn usage_includes_both_branches_without_reasoning_double_count() {
        use pi_rust::ai::types::Usage;
        let mut manager = SessionManager::in_memory("C:/fixture", None, None).unwrap();
        manager
            .append_message(assistant(
                "first",
                Usage {
                    input: 10,
                    output: 2,
                    ..Default::default()
                },
            ))
            .unwrap();
        manager.reset_leaf();
        manager
            .append_message(assistant(
                "second",
                Usage {
                    input: 20,
                    output: 3,
                    cache_read: 10,
                    cache_write: 1,
                    reasoning: Some(1),
                    ..Default::default()
                },
            ))
            .unwrap();
        let usage = session_usage(&manager).unwrap();
        assert_eq!(usage["input"], 30.0);
        assert_eq!(usage["output"], 5.0);
        assert_eq!(usage["reasoning"], 1.0);
        assert_eq!(usage["processedTokens"], 36.0);
        assert_eq!(usage["totalTokens"], 46.0);
        assert_eq!(usage["requests"], 2);
        assert_eq!(usage["cacheReported"], true);
        assert!((usage["cacheHitRate"].as_f64().unwrap() - 10.0 / 41.0 * 100.0).abs() < 1e-8);
    }

    #[test]
    fn run_replay_recovers_tool_activity_and_terminal_time() {
        let mut frames = vec![
            product::Frame {
                cursor: 1,
                event: "run".into(),
                data: json!({}),
            },
            product::Frame {
                cursor: 2,
                event: "meta".into(),
                data: json!({"eventAt":"at-0","startedAt":"at-0"}),
            },
            product::Frame {
                cursor: 3,
                event: "tool_start".into(),
                data: json!({"id":"call-1","name":"read","eventAt":"at-1"}),
            },
        ];
        let mut snapshot = json!({"error":null});
        apply_run_activity(&mut snapshot, &frames, true);
        assert_eq!(snapshot["lifecycle"]["phase"], "using_tool");
        assert_eq!(snapshot["currentActivity"]["id"], "call-1");
        frames.push(product::Frame {
            cursor: 4,
            event: "tool_end".into(),
            data: json!({"id":"call-1","status":"done","output":"actual","eventAt":"at-2"}),
        });
        frames.push(product::Frame {
            cursor: 5,
            event: "done".into(),
            data: json!({"eventAt":"at-3"}),
        });
        apply_run_activity(&mut snapshot, &frames, false);
        assert_eq!(snapshot["tools"][0]["output"], "actual");
        assert_eq!(snapshot["tools"][0]["status"], "done");
        assert_eq!(snapshot["lifecycle"]["phase"], "completed");
        assert_eq!(snapshot["finishedAt"], "at-3");
        assert_eq!(snapshot["currentActivity"], Value::Null);
    }

    #[test]
    fn incomplete_assistant_turns_do_not_expose_navigation_boundaries() {
        for reason in ["error", "aborted", "toolUse"] {
            let item = serial_message(
                &json!({"role":"assistant","content":[],"stopReason":reason}),
                0,
                "entry",
            );
            assert!(item.get("turnBoundaryEntryId").is_none());
        }
    }

    #[test]
    fn transcript_keeps_tool_activity_on_final_reply() {
        let raw = vec![
            json!({"role":"user","content":"read it","timestamp":1}),
            json!({"role":"assistant","content":[{"type":"thinking","thinking":"look"},{"type":"toolCall","id":"call-1","name":"read","arguments":{"path":"test"}}],"stopReason":"toolUse","timestamp":2}),
            json!({"role":"toolResult","toolCallId":"call-1","content":[{"type":"text","text":"contents"}],"isError":false,"timestamp":3}),
            json!({"role":"assistant","content":[{"type":"text","text":"done"}],"stopReason":"stop","timestamp":4}),
        ];
        let messages =
            serialize_transcript(&raw, &["a".into(), "b".into(), "c".into(), "d".into()]);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1]["role"], "agent");
        assert_eq!(messages[1]["text"], "done");
        assert_eq!(messages[1]["turnBoundaryEntryId"], "d");
        assert_eq!(messages[1]["runActivity"]["thinkingText"], "look");
        assert_eq!(messages[1]["runActivity"]["tools"][0]["output"], "contents");
        assert_eq!(messages[1]["runActivity"]["tools"][0]["status"], "done");
    }

    #[test]
    fn paging_uses_transcript_offsets_and_stable_ids() {
        let messages = (0..5)
            .map(|id| json!({"id":id.to_string(),"role":"user","text":"x"}))
            .collect::<Vec<_>>();
        let newest = message_page(&messages, None, 2);
        assert_eq!(newest["pageInfo"]["start"], 3);
        assert_eq!(newest["pageInfo"]["nextCursor"], "3");
        let older = message_page(&messages, Some("3"), 2);
        assert_eq!(older["messages"][0]["id"], "1");
        let earliest = message_page(&messages, Some("1"), 2);
        assert_eq!(earliest["pageInfo"]["hasMore"], false);
        assert_eq!(earliest["pageInfo"]["nextCursor"], Value::Null);
    }

    #[test]
    fn internal_parent_prompts_and_injected_attachment_context_stay_out_of_transcript() {
        let raw = vec![
            json!({"role":"user","content":format!("original user request{ATTACHMENT_MARKER}private extracted attachment text"),"timestamp":1}),
            json!({"role":"assistant","content":"first result","stopReason":"stop","timestamp":2}),
            json!({"role":"user","content":format!("{}\ncontinue", crate::goal_api::CONTINUATION_MARKER),"timestamp":3}),
            json!({"role":"assistant","content":"continuation result","stopReason":"stop","timestamp":4}),
            json!({"role":"user","content":format!("{}\nchild evidence", crate::multi_agent_api::COMPLETION_MARKER),"timestamp":5}),
            json!({"role":"assistant","content":"combined result","stopReason":"stop","timestamp":6}),
        ];
        let messages = serialize_transcript(&raw, &[]);
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0]["text"], "original user request");
        assert_eq!(
            messages
                .iter()
                .filter(|message| message["role"] == "user")
                .count(),
            1
        );
        assert_eq!(messages[3]["text"], "combined result");
    }

    #[test]
    fn changed_workspace_survives_reopen_without_rewriting_transcript() {
        let root =
            std::env::temp_dir().join(format!("pisper-workspace-test-{}", product::new_id()));
        std::fs::create_dir_all(&root).unwrap();
        let cwd = root.to_string_lossy().to_string();
        let mut manager = SessionManager::create(&cwd, Some(&cwd), None).unwrap();
        manager.append_session_info("workspace").unwrap();
        persist_empty_session(&mut manager).unwrap();
        let path = manager.get_session_file().unwrap().to_string();
        let before = std::fs::read(&path).unwrap();
        let boundary = before.iter().position(|byte| *byte == b'\n').unwrap();
        let replacement = root.join("other-workspace").to_string_lossy().to_string();
        persist_session_cwd(&path, &replacement).unwrap();
        let after = std::fs::read(&path).unwrap();
        let new_boundary = after.iter().position(|byte| *byte == b'\n').unwrap();
        assert_eq!(&after[new_boundary + 1..], &before[boundary + 1..]);
        let reopened = SessionManager::open(&path, None, None).unwrap();
        assert_eq!(reopened.get_cwd(), replacement);
        assert_eq!(reopened.get_session_id(), manager.get_session_id());
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn empty_session_is_addressable_and_first_message_appends_after_reload() {
        let root = std::env::temp_dir().join(format!("pisper-session-test-{}", product::new_id()));
        std::fs::create_dir_all(&root).unwrap();
        let cwd = root.to_string_lossy().to_string();
        let mut manager = SessionManager::create(&cwd, Some(&cwd), None).unwrap();
        manager.append_session_info("empty").unwrap();
        let id = manager.get_session_id().to_string();
        persist_empty_session(&mut manager).unwrap();
        let path = manager.get_session_file().unwrap().to_string();
        assert_eq!(SessionManager::list_all(Some(&cwd), None)[0].id, id);
        let message: pi_rust::agent_core::types::AgentMessage = serde_json::from_value(json!({
            "role":"user", "content":"first", "timestamp":1,
        }))
        .unwrap();
        manager.append_message(message).unwrap();
        let reopened = SessionManager::open(&path, None, None).unwrap();
        assert_eq!(reopened.get_session_id(), id);
        assert_eq!(manager_messages(&reopened).0[0]["content"], "first");
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
}
