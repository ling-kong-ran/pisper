//! Release 对齐的会话操作层：临时侧聊（side chat）、树导航/标签、
//! 会话命令投影、最后一轮重试、git/* 别名、单文件 VCS diff、
//! 待发送输入撤回与移动端操作回传。
//! 契约逐字段对齐 release（runtime/http/routes/sessions-runtime.mjs +
//! services/side-chat-service.mjs + runtime/session-commands.mjs）。

use axum::extract::{Path, State};
use axum::Json;
use pi_rust::coding_agent::agent_session::NavigateTreeOptions;
use pi_rust::coding_agent::core::resource_loader::prompt_templates::{
    load_prompt_templates, LoadPromptTemplatesOptions,
};
use pi_rust::coding_agent::core::skills::{load_skills, LoadSkillsOptions};
use pi_rust::coding_agent::session_manager::{NewSessionOptions, SessionManager};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

use crate::vcs_ops;
use crate::{product, session_api, session_runtime, ApiError, AppState};

const SIDE_CHAT_TTL_MS: u64 = 24 * 60 * 60 * 1000;
const SIDE_CHAT_PREFIX: &str = "side-";
const MAX_COMMAND_NAME_CHARS: usize = 128;
const MAX_COMMAND_DESCRIPTION_CHARS: usize = 280;
const MAX_ARGUMENT_HINT_CHARS: usize = 120;

/// session-meta.json 里的临时侧聊标记（SessionMeta.sideChat）。
/// 内容就是 release SideChatMetadata：version/parentSessionId/lastActivityAt/expiresAt。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct SideChatMeta(pub Value);

type ArcAppState = Arc<AppState>;

// ---------------------------------------------------------------- side chat

pub(crate) fn iso_timestamp(ms: u64) -> String {
    // RFC3339 with millisecond precision (release Date.toISOString). chrono is
    // not a dependency, so compute the civil date from the Unix epoch with
    // Howard Hinnant's civil-from-days algorithm.
    let seconds = (ms / 1000) as i64;
    let millis = (ms % 1000) as i64;
    let days = seconds.div_euclid(86_400);
    let time = seconds.rem_euclid(86_400);
    let (hour, minute, second) = (time / 3600, (time % 3600) / 60, time % 60);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

pub(crate) fn parse_timestamp_ms(value: &str) -> Option<u64> {
    // Parse the subset of RFC3339 this module itself produces.
    if value.len() < 20 || !value.ends_with('Z') {
        return None;
    }
    let year: i64 = value.get(0..4)?.parse().ok()?;
    let month: i64 = value.get(5..7)?.parse().ok()?;
    let day: i64 = value.get(8..10)?.parse().ok()?;
    let hour: i64 = value.get(11..13)?.parse().ok()?;
    let minute: i64 = value.get(14..16)?.parse().ok()?;
    let second: i64 = value.get(17..19)?.parse().ok()?;
    let millis: u64 = value.get(20..23).and_then(|v| v.parse().ok()).unwrap_or(0);
    let leap = |y: i64| y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let days_before_year = |y: i64| -> i64 {
        let y = y - 1;
        365 * y + y / 4 - y / 100 + y / 400
    };
    const CUMULATIVE: [i64; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let month_index = (month - 1).clamp(0, 11) as usize;
    let mut days = days_before_year(year) + CUMULATIVE[month_index];
    if month > 2 && leap(year) {
        days += 1;
    }
    days += day - 1;
    let seconds = days * 86_400 + hour * 3600 + minute * 60 + second;
    u64::try_from(seconds).ok().map(|s| s * 1000 + millis)
}

fn side_chat_meta(state: &AppState, id: &str) -> Option<Value> {
    state
        .session_meta
        .lock()
        .expect("session meta lock")
        .get(id)
        .and_then(|meta| meta.side_chat.clone())
        .map(|meta| meta.0)
}

/// release SideChatService.children(parentId)。
fn side_chat_children(state: &AppState, parent_id: &str) -> Vec<String> {
    state
        .session_meta
        .lock()
        .expect("session meta lock")
        .iter()
        .filter(|(id, meta)| {
            id.as_str() != parent_id
                && meta
                    .side_chat
                    .as_ref()
                    .is_some_and(|side| side.0["parentSessionId"] == parent_id)
        })
        .map(|(id, _)| id.clone())
        .collect()
}

fn is_side_chat_id(id: &str) -> bool {
    id.starts_with(SIDE_CHAT_PREFIX)
}

fn session_row(state: &AppState, id: &str) -> Result<(Value, String), ApiError> {
    let path = session_api::find_session_path(state, id)?;
    let modified = std::fs::metadata(&path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0);
    let manager = SessionManager::open(&path, None, None)
        .map_err(|error| ApiError::internal(error.to_string()))?;
    let summary = session_api::summary(state, &manager, modified)?;
    let expires_at = side_chat_meta(state, id)
        .and_then(|meta| meta["expiresAt"].as_str().map(str::to_string))
        .unwrap_or_default();
    Ok((summary, expires_at))
}

/// release SideChatService.get：命中未过期的侧聊返回其会话摘要；
/// `create` 时为父会话新建一个 24 小时 TTL 的临时会话。
async fn respond_side_chat(state: Arc<AppState>, id: String, create: bool) -> Result<Json<Value>, ApiError> {
    // 父会话必须存在（release getSummary(parent) 失败 → 404）。
    session_api::find_session_path(&state, &id)?;
    if is_side_chat_id(&id) || side_chat_meta(&state, &id).is_some() {
        return Err(ApiError::bad_request("临时侧聊不能再创建侧聊。"));
    }
    let now = product::now_ms();
    // 先清扫属于当前父会话的过期侧聊（release get 的内联 sweep）。
    for child in side_chat_children(&state, &id) {
        let expires_at = side_chat_meta(&state, &child)
            .and_then(|meta| meta["expiresAt"].as_str().and_then(parse_timestamp_ms));
        if expires_at.is_none_or(|expires| expires <= now) {
            let _ = state.sessions.remove(&state, &child).await;
            state
                .session_meta
                .lock()
                .expect("session meta lock")
                .remove(&child);
        }
    }
    if let Some(child) = side_chat_children(&state, &id).into_iter().next() {
        if let Ok((mut summary, expires_at)) = session_row(&state, &child) {
            summary["metadata"] = json!({
                "manual": true,
                "sideChat": side_chat_meta(&state, &child).unwrap_or(Value::Null),
            });
            return Ok(Json(json!({
                "session": summary,
                "expiresAt": expires_at,
                "created": false,
            })));
        }
        // 元数据在但文件缺失（宿中未落盘或已丢失）：按 release deleteSession 清理。
        let _ = state.sessions.remove(&state, &child).await;
        state
            .session_meta
            .lock()
            .expect("session meta lock")
            .remove(&child);
    }
    if !create {
        return Ok(Json(
            json!({"session": Value::Null, "expiresAt": Value::Null, "created": false}),
        ));
    }
    // 新建侧聊：独立会话文件，继承父会话工作区；id 使用 side- 前缀。
    // 与 create_session 相同：只持久化 + 投影，宿中留给首条消息（避免
    // 在持有 session_manager 锁时调用 summary 造成重入死锁）。
    let parent_path = session_api::find_session_path(&state, &id)?;
    let mut parent_manager = SessionManager::open(&parent_path, None, None)
        .map_err(|error| ApiError::internal(error.to_string()))?;
    let parent_cwd = parent_manager.get_cwd().to_string();
    // 继承父会话的模型 / 思考档位 / 执行与权限设置（release InheritedSessionSettings）。
    let parent_model = parent_manager
        .get_entries()
        .iter()
        .rev()
        .find_map(|entry| match entry {
            pi_rust::coding_agent::session_manager::SessionEntry::ModelChange(change) => {
                Some((change.provider.clone(), change.model_id.clone()))
            }
            _ => None,
        });
    let parent_thinking = parent_manager
        .get_entries()
        .iter()
        .rev()
        .find_map(|entry| match entry {
            pi_rust::coding_agent::session_manager::SessionEntry::ThinkingLevelChange(change) => {
                Some(change.thinking_level.clone())
            }
            _ => None,
        });
    let parent_meta = state
        .session_meta
        .lock()
        .expect("session meta lock")
        .get(&id)
        .cloned()
        .unwrap_or_default();
    let new_id = format!("side-{}", product::new_id());
    let sessions_dir = std::path::Path::new(&state.agent_dir)
        .join("sessions")
        .to_string_lossy()
        .to_string();
    let mut manager = SessionManager::create(
        &parent_cwd,
        Some(&sessions_dir),
        Some(&NewSessionOptions {
            id: Some(new_id.clone()),
            parent_session: None,
        }),
    )
    .map_err(|error| ApiError::internal(error.to_string()))?;
    manager
        .append_session_info("临时侧聊")
        .map_err(|error| ApiError::internal(error.to_string()))?;
    if let Some((provider, model_id)) = parent_model {
        manager
            .append_model_change(&provider, &model_id)
            .map_err(|error| ApiError::internal(error.to_string()))?;
    }
    if let Some(thinking) = parent_thinking {
        manager
            .append_thinking_level_change(&thinking)
            .map_err(|error| ApiError::internal(error.to_string()))?;
    }
    session_api::persist_empty_session(&mut manager)?;
    let modified = std::fs::metadata(manager.get_session_file().unwrap_or_default())
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_else(|| product::now_ms() as i64);
    let mut summary = session_api::summary(&state, &manager, modified)?;
    let expires_at = iso_timestamp(now + SIDE_CHAT_TTL_MS);
    {
        let mut entries = state.session_meta.lock().expect("session meta lock");
        let entry = entries.entry(new_id.clone()).or_default();
        entry.side_chat = Some(SideChatMeta(json!({
            "version": 1,
            "parentSessionId": id,
            "lastActivityAt": iso_timestamp(now),
            "expiresAt": expires_at,
        })));
        entry.name = Some("临时侧聊".to_string());
        // 执行/权限/运行模式沿用父会话（release 继承链）。
        entry.execution_mode = parent_meta.execution_mode.clone();
        entry.permission_mode = parent_meta.permission_mode.clone();
        entry.run_mode = parent_meta.run_mode.clone();
    }
    session_api::save_metadata(&state)?;
    summary["metadata"] = json!({
        "manual": true,
        "sideChat": side_chat_meta(&state, &new_id).unwrap_or(Value::Null),
    });
    Ok(Json(json!({
        "session": summary,
        "expiresAt": expires_at,
        "created": true,
    })))
}

pub(crate) async fn side_chat_get(
    State(state): State<ArcAppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    respond_side_chat(state, id, false).await
}

pub(crate) async fn side_chat_create(
    State(state): State<ArcAppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    respond_side_chat(state, id, true).await
}

// --------------------------------------------------------------- tree nav

/// release navigateSessionTree：切换活跃叶子；`includeTree:false` 只返回导航结果。
pub(crate) async fn tree_navigate(
    State(state): State<ArcAppState>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let target = body
        .get("targetEntryId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::bad_request("targetEntryId 不能为空。"))?
        .to_string();
    if let Some(summarize) = body.get("summarize") {
        if !summarize.is_boolean() {
            return Err(ApiError::bad_request("summarize 必须是布尔值。"));
        }
    }
    if let Some(include_tree) = body.get("includeTree") {
        if !include_tree.is_boolean() {
            return Err(ApiError::bad_request("includeTree 必须是布尔值。"));
        }
    }
    let mutation = session_runtime::mutation(&state, &id).await?;
    let session = mutation.hosted.session();
    {
        let manager = session.session_manager.lock().expect("session manager lock");
        if manager.get_entry(&target).is_none() {
            return Err(ApiError::bad_request("会话树节点不存在。"));
        }
        if manager.get_leaf_id() == Some(target.as_str()) {
            let navigation = json!({"cancelled": false, "editorText": Value::Null});
            if body.get("includeTree") == Some(&json!(false)) {
                return Ok(Json(navigation));
            }
            let mut tree = session_api::project_tree(&manager, false)?;
            tree["cancelled"] = navigation["cancelled"].clone();
            tree["editorText"] = navigation["editorText"].clone();
            return Ok(Json(tree));
        }
    }
    let result = session
        .navigate_tree(
            &target,
            NavigateTreeOptions {
                summarize: Some(body["summarize"].as_bool().unwrap_or(false)),
                custom_instructions: None,
                replace_instructions: None,
                label: None,
            },
        )
        .await
        .map_err(|error| ApiError::internal(error.to_string()))?;
    let navigation = json!({
        "cancelled": result.cancelled,
        "editorText": result.editor_text,
    });
    if body.get("includeTree") == Some(&json!(false)) {
        return Ok(Json(navigation));
    }
    let manager = session.session_manager.lock().expect("session manager lock");
    let mut tree = session_api::project_tree(&manager, false)?;
    tree["cancelled"] = navigation["cancelled"].clone();
    tree["editorText"] = navigation["editorText"].clone();
    Ok(Json(tree))
}

/// release setSessionTreeLabel：空标签 = 删除（墓碑），返回更新后的树
/// （`{...tree, lineage}` — project_tree 已含 lineage 字段）。
pub(crate) async fn tree_label_set(
    State(state): State<ArcAppState>,
    Path((id, entry_id)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let Some(label) = body.get("label").and_then(Value::as_str) else {
        return Err(ApiError::bad_request("label 必须是字符串。"));
    };
    let normalized = label.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() > 80 {
        return Err(ApiError::bad_request("节点标签不能超过 80 个字符。"));
    }
    let mutation = session_runtime::mutation(&state, &id).await?;
    let session = mutation.hosted.session();
    let mut manager = session.session_manager.lock().expect("session manager lock");
    if manager.get_entry(&entry_id).is_none() {
        return Err(ApiError::bad_request("会话树节点不存在。"));
    }
    if manager.get_label(&entry_id).unwrap_or("") == normalized {
        return Ok(Json(session_api::project_tree(&manager, false)?));
    }
    manager
        .append_label_change(
            &entry_id,
            if normalized.is_empty() { None } else { Some(&normalized) },
        )
        .map_err(|error| ApiError::internal(error.to_string()))?;
    Ok(Json(session_api::project_tree(&manager, false)?))
}

// -------------------------------------------------------------- commands

fn command_scope(source_info: &Value) -> &'static str {
    if source_info["origin"] == "package" {
        return "package";
    }
    match source_info["scope"].as_str() {
        Some("project") => "project",
        Some("user") => "user",
        _ => "custom",
    }
}

fn clean_text(value: &str, maximum: usize) -> String {
    let cleaned: String = value
        .chars()
        .map(|character| {
            let code = character as u32;
            if code < 32 || code == 127 {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    cleaned.chars().take(maximum).collect()
}

fn valid_name(value: &str) -> String {
    let name = clean_text(value, MAX_COMMAND_NAME_CHARS);
    if name.is_empty() || name.chars().any(|c| c.is_whitespace() || c == '/') {
        String::new()
    } else {
        name
    }
}

/// release projectSessionCommands：prompt(/name) + skill(/skill:name) 统一命令表。
pub(crate) async fn session_commands(
    State(state): State<ArcAppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let cwd = match state.sessions.get(&id) {
        Some(hosted) => hosted.runtime.cwd(),
        None => {
            let path = session_api::find_session_path(&state, &id)?;
            let manager = SessionManager::open(&path, None, None)
                .map_err(|error| ApiError::internal(error.to_string()))?;
            manager.get_cwd().to_string()
        }
    };
    let services = state.runtime.services();
    let agent_dir = services.agent_dir.clone();
    let prompts = load_prompt_templates(LoadPromptTemplatesOptions {
        cwd: cwd.clone(),
        agent_dir: agent_dir.clone(),
        prompt_paths: vec![],
        include_defaults: true,
    });
    let skills = load_skills(LoadSkillsOptions {
        cwd: cwd.clone(),
        agent_dir,
        skill_paths: vec![],
        include_defaults: true,
    });
    let mut commands: Vec<Value> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for prompt in &prompts {
        let name = valid_name(&prompt.name);
        if name.is_empty() {
            continue;
        }
        let invocation = format!("/{name}");
        if !seen.insert(invocation.clone()) {
            continue;
        }
        let scope =
            command_scope(&serde_json::to_value(&prompt.source_info).unwrap_or_default());
        commands.push(json!({
            "name": name,
            "invocation": invocation,
            "description": clean_text(&prompt.description, MAX_COMMAND_DESCRIPTION_CHARS),
            "argumentHint": clean_text(
                prompt.argument_hint.as_deref().unwrap_or(""),
                MAX_ARGUMENT_HINT_CHARS,
            ),
            "source": "prompt",
            "scope": scope,
        }));
    }
    for skill in &skills.skills {
        let name = valid_name(&skill.name);
        if name.is_empty() {
            continue;
        }
        let invocation = format!("/skill:{name}");
        if !seen.insert(invocation.clone()) {
            continue;
        }
        let scope = command_scope(&serde_json::to_value(&skill.source_info).unwrap_or_default());
        commands.push(json!({
            "name": name,
            "invocation": invocation,
            "description": clean_text(&skill.description, MAX_COMMAND_DESCRIPTION_CHARS),
            "argumentHint": "",
            "source": "skill",
            "scope": scope,
        }));
    }
    commands.sort_by(|left, right| {
        let left_source = left["source"].as_str().unwrap_or("");
        let right_source = right["source"].as_str().unwrap_or("");
        if left_source == right_source {
            left["name"]
                .as_str()
                .unwrap_or("")
                .cmp(right["name"].as_str().unwrap_or(""))
        } else if left_source == "prompt" {
            std::cmp::Ordering::Less
        } else {
            std::cmp::Ordering::Greater
        }
    });
    let total = commands.len();
    let prompt_count = commands.iter().filter(|c| c["source"] == "prompt").count();
    let skill_count = commands.iter().filter(|c| c["source"] == "skill").count();
    Ok(Json(json!({
        "sessionId": id,
        "commands": commands,
        "counts": {
            "total": total,
            "prompts": prompt_count,
            "skills": skill_count,
            "diagnostics": 0,
        },
    })))
}

// ----------------------------------------------------------------- retry

/// release prepareLastTurnRetry：沿活跃路径向上找最近的用户消息，
/// 把叶子撤回到该轮之前的边界，取出文本与图片附件。
async fn prepare_last_turn_retry(
    state: &AppState,
    id: &str,
) -> Result<(String, Value), ApiError> {
    let mutation = session_runtime::mutation(state, id).await?;
    let session = mutation.hosted.session();
    let (message, attachments, parent_id) = {
        let manager = session.session_manager.lock().expect("session manager lock");
        let mut entry_id = manager.get_leaf_id().map(str::to_string);
        let mut user_entry: Option<Value> = None;
        while let Some(current) = entry_id.clone() {
            let Some(entry) = manager.get_entry(&current) else {
                break;
            };
            let value =
                serde_json::to_value(entry).map_err(|e| ApiError::internal(e.to_string()))?;
            if value["type"] == "message" && value["message"]["role"] == "user" {
                user_entry = Some(value);
                break;
            }
            entry_id = value["parentId"].as_str().map(str::to_string);
        }
        let Some(user_entry) = user_entry else {
            return Err(ApiError::bad_request("没有可重试的用户消息。"));
        };
        let Some(parent_id) = user_entry["parentId"].as_str().map(str::to_string) else {
            return Err(ApiError::bad_request("无法定位重试位置。"));
        };
        if manager.get_entry(&parent_id).is_none() {
            return Err(ApiError::bad_request("无法定位重试位置。"));
        }
        let content = &user_entry["message"]["content"];
        let parts: Vec<Value> = if content.is_array() {
            content.as_array().unwrap().clone()
        } else {
            vec![json!({
                "type": "text",
                "text": content.as_str().unwrap_or_default(),
            })]
        };
        let message = parts
            .iter()
            .filter(|part| part["type"] == "text")
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string();
        let attachments: Vec<Value> = parts
            .iter()
            .enumerate()
            .filter(|(_, part)| {
                part["type"] == "image" && part.get("data").map(Value::is_string).unwrap_or(false)
            })
            .map(|(index, part)| {
                json!({
                    "id": format!("retry-image-{index}"),
                    "kind": "image",
                    "name": format!("image-{}", index + 1),
                    "mimeType": part["mimeType"].as_str().unwrap_or("image/png"),
                    "data": part["data"],
                })
            })
            .collect();
        if message.is_empty() && attachments.is_empty() {
            return Err(ApiError::bad_request("没有可重试的用户消息。"));
        }
        (message, attachments, parent_id)
    };
    session
        .navigate_tree(
            &parent_id,
            NavigateTreeOptions {
                summarize: Some(false),
                custom_instructions: None,
                replace_instructions: None,
                label: None,
            },
        )
        .await
        .map_err(|error| ApiError::internal(error.to_string()))?;
    Ok((message, Value::Array(attachments)))
}

/// release POST /api/sessions/:id/retry：先完成树导航与输入恢复再开 SSE，
/// 无可重试内容时按普通错误返回。
pub(crate) async fn retry_session(
    State(state): State<ArcAppState>,
    Path(id): Path<String>,
) -> Result<axum::response::Response, ApiError> {
    let (message, attachments) = prepare_last_turn_retry(&state, &id).await?;
    crate::chat_stream::chat_owned(
        state,
        json!({"sessionId": id, "message": message, "attachments": attachments}),
        None,
    )
    .await
}

// ------------------------------------------------------- git/* and diff

/// release git/* 是 vcs/* 的完整别名（同一 GitChangesService）。
pub(crate) async fn git_changes(
    State(state): State<ArcAppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    crate::product::vcs_changes(State(state), Path(id)).await
}

pub(crate) async fn git_commit(
    State(state): State<ArcAppState>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    crate::product::vcs_commit(State(state), Path(id), Json(body)).await
}

pub(crate) async fn git_push(
    State(state): State<ArcAppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    crate::product::vcs_push(State(state), Path(id)).await
}

pub(crate) async fn git_revert(
    State(state): State<ArcAppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    crate::product::vcs_revert(State(state), Path(id)).await
}

/// 会话工作区 cwd：宿中的用运行时的，其余从会话文件读。
pub(crate) async fn session_cwd_for(state: &AppState, id: &str) -> Result<String, ApiError> {
    if let Some(hosted) = state.sessions.get(id) {
        return Ok(hosted.runtime.cwd());
    }
    let path = session_api::find_session_path(state, id)?;
    let manager = SessionManager::open(&path, None, None)
        .map_err(|error| ApiError::internal(error.to_string()))?;
    Ok(manager.get_cwd().to_string())
}

/// release getSessionFileDiff：单文件差异；非 VCS 工作区回退到会话内修改前快照。
pub(crate) async fn vcs_file_diff(
    State(state): State<ArcAppState>,
    Path(id): Path<String>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<Value>, ApiError> {
    let file_path = params.get("path").cloned().unwrap_or_default();
    let normalized = file_path.replace('\\', "/");
    let outside = normalized.is_empty()
        || normalized.starts_with('/')
        || normalized.contains(':')
        || normalized
            .split('/')
            .any(|segment| segment == ".." || segment.is_empty() && normalized.len() > 1);
    if outside {
        return Err(ApiError::bad_request("文件路径超出会话工作区范围。"));
    }
    let cwd = session_cwd_for(&state, &id).await?;
    let vcs = vcs_ops::file_diff(&cwd, &file_path).await;
    if vcs["isRepo"] == true && vcs["diff"].as_str().map(str::trim).unwrap_or("") != "" {
        let vcs_kind = vcs["vcs"].as_str().unwrap_or("").to_string();
        let mut value = vcs;
        value["source"] = json!(if vcs_kind.is_empty() { "vcs".to_string() } else { vcs_kind });
        value["canRevert"] = json!(false);
        return Ok(Json(value));
    }
    // 快照回退：会话内写工具的修改前快照（无 Git/SVN 也能预览）。
    let snapshot = state
        .file_changes
        .diff(&id, std::path::Path::new(&cwd), &file_path)
        .await?;
    if snapshot["found"] == true && snapshot["diff"].as_str().map(str::trim).unwrap_or("") != "" {
        return Ok(Json(json!({
            "isRepo": false,
            "vcs": "",
            "diff": snapshot["diff"],
            "diffTruncated": snapshot
                .get("diffTruncated")
                .cloned()
                .unwrap_or(json!(false)),
            "source": "snapshot",
            "canRevert": true,
        })));
    }
    Ok(Json(vcs))
}

// --------------------------------------------------- withdraw & mobile ops

/// release withdrawSessionMessage：从待发送队列移除一条输入。
/// pi-rs 队列存纯文本（无逐条 id），未知 id 走 release 的 removed:false 分支。
pub(crate) async fn withdraw_input(
    State(state): State<ArcAppState>,
    Path((id, input_id)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    if input_id.trim().is_empty() {
        return Err(ApiError::bad_request("待发送消息标识不能为空。"));
    }
    let hosted = session_runtime::hosted(&state, &id).await?;
    let active = hosted.session();
    Ok(Json(json!({
        "removed": false,
        "inputId": input_id,
        "pendingMessageCount": active.pending_message_count(),
        "queuedInputs": session_api::queued_inputs(&state, &id),
    })))
}

/// release resolveMobileOperation：移动 App 回传设备操作结果。
/// 本后端尚无移动 SSE 通道（诚实接缝），因此永远没有挂起操作 —— 与
/// release 对未知 operationId 的 404 `{accepted:false}` 分支字节一致。
pub(crate) async fn mobile_operation(
    State(state): State<ArcAppState>,
    Path((id, operation_id)): Path<(String, String)>,
    Json(_body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let _ = (&state, &id, &operation_id);
    Ok(Json(json!({"accepted": false})))
}
