//! release `services/mcp-host-service.mjs` + `mcp-host-tools.mjs` 的原生移植：
//! 内置 MCP 服务的开关/凭据管理（持久化 pisper-mcp-host.json）与
//! Streamable-HTTP JSON-RPC 端点（/mcp，Bearer 令牌鉴权）。
//!
//! 工具后端通过回环自调用 /api（桌面令牌 Cookie），与三个前端消费的
//! 契约保持同一实现；`pisper_send_message` 以后台运行 + `pisper_get_run`
//! 轮询的 release 语义暴露。
//!
//! 拓扑差异（诚实记录）：release 在独立端口监听 MCP；本后端把 /mcp 挂在
//! 主监听端口上（sidecar 端口由宿主分配，无法保证第二端口可用），
//! 协议、鉴权与工具行为一致。

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};
use sha2::Digest;
use std::path::Path;
use std::sync::Arc;

use crate::{product, security, ApiError, AppState};

type ArcAppState = Arc<AppState>;

const STATE_VERSION: u32 = 1;
const MCP_PROTOCOL_VERSION: &str = "2025-03-26";
const MAX_RESULT_BYTES: usize = 96 * 1024;
const MAX_ACTIVE_RUNS: usize = 4;
const MAX_RUNS: usize = 100;
const MAX_RUN_AGE_MS: u64 = 30 * 60_000;
const MAX_TEXT_CHARS: usize = 32_000;

#[derive(Debug, Clone)]
struct McpHostState {
    enabled: bool,
    token: String,
}

fn state_path(state: &AppState) -> std::path::PathBuf {
    Path::new(&state.data_dir).join("pisper-mcp-host.json")
}

fn load_state(state: &AppState) -> McpHostState {
    std::fs::read_to_string(state_path(state))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .map(|value| McpHostState {
            enabled: value["enabled"].as_bool().unwrap_or(false),
            token: value["token"].as_str().unwrap_or("").to_string(),
        })
        .unwrap_or(McpHostState {
            enabled: false,
            token: String::new(),
        })
}

fn save_state(state: &AppState, mcp_state: &McpHostState) -> Result<(), ApiError> {
    std::fs::create_dir_all(&state.data_dir).map_err(|e| ApiError::internal(e.to_string()))?;
    let value = json!({
        "version": STATE_VERSION,
        "enabled": mcp_state.enabled,
        "token": mcp_state.token,
    });
    std::fs::write(
        state_path(state),
        serde_json::to_string_pretty(&value).map_err(|e| ApiError::internal(e.to_string()))?,
    )
    .map_err(|e| ApiError::internal(e.to_string()))
}

/// 32 字节 → 64 位十六进制；release 使用 32 字节 base64url（43 字符），
/// 两者都满足其令牌校验 /^[A-Za-z0-9_-]{43,128}$/。
fn generate_token() -> String {
    let mut material = String::new();
    while material.len() < 96 {
        material.push_str(&product::new_id());
    }
    let digest = sha2::Sha256::digest(material.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn self_base(state: &AppState) -> Option<String> {
    state
        .self_base
        .lock()
        .expect("self base lock")
        .clone()
}

fn status_view(state: &AppState, mcp_state: &McpHostState) -> Value {
    let base = self_base(state);
    let listening = mcp_state.enabled && base.is_some();
    let (port, url) = match (&base, listening) {
        (Some(base), true) => {
            let port = base.rsplit(':').next().and_then(|p| p.parse::<u32>().ok());
            (port, format!("{base}/mcp"))
        }
        _ => (Some(5175), String::new()),
    };
    json!({
        "enabled": mcp_state.enabled,
        "listening": listening,
        "host": "127.0.0.1",
        "port": port,
        "url": url,
        "error": if mcp_state.enabled && !listening {
            json!("内置 MCP 服务监听尚未启动。")
        } else {
            Value::Null
        },
    })
}

/// release GET /api/mcp-host
pub(crate) async fn status(State(state): State<ArcAppState>) -> Json<Value> {
    Json(status_view(&state, &load_state(&state)))
}

/// release PATCH /api/mcp-host {enabled}
pub(crate) async fn set_enabled(
    State(state): State<ArcAppState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let enabled = body
        .get("enabled")
        .and_then(Value::as_bool)
        .ok_or_else(|| ApiError::bad_request("MCP 服务开关必须是布尔值。"))?;
    let mut mcp_state = load_state(&state);
    mcp_state.enabled = enabled;
    if enabled && mcp_state.token.is_empty() {
        mcp_state.token = generate_token();
    }
    save_state(&state, &mcp_state)?;
    Ok(Json(status_view(&state, &mcp_state)))
}

/// release POST /api/mcp-host/credentials：未监听 → 409 mcp_host_not_listening。
pub(crate) async fn credentials(State(state): State<ArcAppState>) -> Result<Json<Value>, ApiError> {
    let mcp_state = load_state(&state);
    let base = self_base(&state);
    match (mcp_state.enabled, base) {
        (true, Some(base)) => Ok(Json(json!({
            "url": format!("{base}/mcp"),
            "token": mcp_state.token,
        }))),
        _ => Err(ApiError::new(
            StatusCode::CONFLICT,
            "mcp_host_not_listening",
            "内置 MCP 服务尚未就绪。",
        )),
    }
}

/// release POST /api/mcp-host/rotate-token：未开启 → 400；未监听 → 503。
pub(crate) async fn rotate_token(State(state): State<ArcAppState>) -> Result<Json<Value>, ApiError> {
    let mut mcp_state = load_state(&state);
    if !mcp_state.enabled {
        return Err(ApiError::bad_request("请先开启 Pisper MCP 服务。"));
    }
    mcp_state.token = generate_token();
    save_state(&state, &mcp_state)?;
    if self_base(&state).is_none() {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "mcp_host_not_listening",
            "内置 MCP 服务尚未就绪。",
        ));
    }
    let base = self_base(&state).expect("checked");
    Ok(Json(json!({
        "url": format!("{base}/mcp"),
        "token": mcp_state.token,
    })))
}

// ------------------------------------------------------------ MCP endpoint

fn tool_failed(message: &str) -> Value {
    json!({
        "content": [{"type":"text","text":json!({"error":"tool_failed","message":message}).to_string()}],
        "isError": true,
    })
}

fn tool_result(value: Value) -> Value {
    let redacted = security::redact_secret_value(&value);
    match serde_json::to_string(&redacted) {
        Ok(text) if text.len() <= MAX_RESULT_BYTES => {
            json!({"content":[{"type":"text","text":text}], "isError": false})
        }
        Ok(_) => json!({
            "content":[{"type":"text","text":json!({
                "error":"result_too_large",
                "message":"结果超过大小限制。请缩小查询范围或减少返回条数。"
            }).to_string()}],
            "isError": true,
        }),
        Err(_) => json!({
            "content":[{"type":"text","text":"{\"error\":\"result_not_serializable\"}"}],
            "isError": true,
        }),
    }
}

/// 回环自调用 /api：带桌面令牌 Cookie；返回解析后的 JSON 或错误信息。
async fn api_call(
    state: &AppState,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Result<(StatusCode, Value), String> {
    let base = self_base(state).ok_or_else(|| "Runtime 尚未就绪。".to_string())?;
    let client = crate::desktop_ops::http_client();
    let mut request = match method {
        "GET" => client.get(format!("{base}{path}")),
        "DELETE" => client.delete(format!("{base}{path}")),
        _ => client.request(reqwest::Method::from_bytes(method.as_bytes()).unwrap_or(reqwest::Method::POST), format!("{base}{path}")),
    };
    if let Some(token) = &state.desktop_token {
        request = request.header("Cookie", format!("__pisper_desktop={token}"));
    }
    if let Some(body) = &body {
        request = request.header("Content-Type", "application/json").json(body);
    }
    let response = request
        .timeout(std::time::Duration::from_secs(180))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let status = StatusCode::from_u16(response.status().as_u16())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let text = response.text().await.map_err(|error| error.to_string())?;
    let value = serde_json::from_str::<Value>(&text).unwrap_or_else(|_| json!(text));
    Ok((status, value))
}

fn require_ok(status: StatusCode, value: &Value) -> Result<(), String> {
    if status.is_success() {
        Ok(())
    } else {
        Err(value["error"]
            .as_str()
            .unwrap_or("请求失败。")
            .to_string())
    }
}

/// release mcp-host-tools 的工具目录（name/description/inputSchema）。
fn tool_definitions() -> Vec<Value> {
    let session_id = json!({"type":"string","minLength":1,"maxLength":160});
    let run_id = json!({"type":"string","minLength":1,"maxLength":160});
    vec![
        json!({"name":"pisper_capabilities","description":"Read Pisper runtime capabilities.","inputSchema":{"type":"object","properties":{}}}),
        json!({"name":"pisper_list_sessions","description":"List Pisper agent sessions.","inputSchema":{"type":"object","properties":{}}}),
        json!({"name":"pisper_create_session","description":"Create a Pisper agent session.","inputSchema":{"type":"object","properties":{"name":{"type":"string","maxLength":160},"cwd":{"type":"string","maxLength":2000}}}}),
        json!({"name":"pisper_send_message","description":"Start a Pisper agent turn. Returns a run ID immediately; poll pisper_get_run. Pisper session permissions and approvals still apply.","inputSchema":{"type":"object","properties":{"sessionId":session_id,"message":{"type":"string","minLength":1,"maxLength":12000},"name":{"type":"string","maxLength":160},"cwd":{"type":"string","maxLength":2000},"goalMode":{"type":"boolean"},"teamMode":{"type":"boolean"}},"required":["message"]}}),
        json!({"name":"pisper_get_run","description":"Poll a Pisper turn started by pisper_send_message.","inputSchema":{"type":"object","properties":{"runId":run_id},"required":["runId"]}}),
        json!({"name":"pisper_get_session_messages","description":"Read the projected messages of a Pisper session.","inputSchema":{"type":"object","properties":{"sessionId":session_id},"required":["sessionId"]}}),
        json!({"name":"pisper_get_session_live","description":"Read the live projection of a Pisper session.","inputSchema":{"type":"object","properties":{"sessionId":session_id},"required":["sessionId"]}}),
        json!({"name":"pisper_get_session_changes","description":"List workspace file changes recorded by a Pisper session.","inputSchema":{"type":"object","properties":{"sessionId":session_id},"required":["sessionId"]}}),
        json!({"name":"pisper_get_usage_today","description":"Read today's aggregate model usage.","inputSchema":{"type":"object","properties":{}}}),
        json!({"name":"pisper_rename_session","description":"Rename a Pisper session.","inputSchema":{"type":"object","properties":{"sessionId":session_id,"name":{"type":"string","maxLength":160}},"required":["sessionId","name"]}}),
        json!({"name":"pisper_abort_session","description":"Abort a running Pisper session.","inputSchema":{"type":"object","properties":{"sessionId":session_id},"required":["sessionId"]}}),
        json!({"name":"pisper_list_assets","description":"List workspace assets archived by Pisper.","inputSchema":{"type":"object","properties":{}}}),
        json!({"name":"pisper_search_memory","description":"Search the Pisper memory store.","inputSchema":{"type":"object","properties":{"query":{"type":"string","maxLength":400}}}}),
        json!({"name":"pisper_list_schedules","description":"List Pisper schedules.","inputSchema":{"type":"object","properties":{}}}),
        json!({"name":"pisper_run_schedule","description":"Run a Pisper schedule now.","inputSchema":{"type":"object","properties":{"scheduleId":session_id},"required":["scheduleId"]}}),
        json!({"name":"pisper_list_workflows","description":"List Pisper workflows.","inputSchema":{"type":"object","properties":{}}}),
        json!({"name":"pisper_get_workflow_run","description":"Read one Pisper workflow run.","inputSchema":{"type":"object","properties":{"runId":run_id},"required":["runId"]}}),
        json!({"name":"pisper_run_workflow","description":"Run a Pisper workflow.","inputSchema":{"type":"object","properties":{"workflowId":session_id,"inputs":{"type":"object"}},"required":["workflowId"]}}),
        json!({"name":"pisper_stop_workflow_run","description":"Stop a running Pisper workflow.","inputSchema":{"type":"object","properties":{"runId":run_id},"required":["runId"]}}),
        json!({"name":"pisper_get_goal","description":"Read a session's Pisper goal state.","inputSchema":{"type":"object","properties":{"sessionId":session_id},"required":["sessionId"]}}),
        json!({"name":"pisper_pause_goal","description":"Pause a session's Pisper goal.","inputSchema":{"type":"object","properties":{"sessionId":session_id},"required":["sessionId"]}}),
    ]
}

async fn call_tool(state: Arc<AppState>, name: &str, input: Value) -> Value {
    let (method, path, body): (&str, String, Option<Value>) = match name {
        "pisper_capabilities" => ("GET", "/api/runtime/capabilities".into(), None),
        "pisper_list_sessions" => ("GET", "/api/sessions".into(), None),
        "pisper_create_session" => (
            "POST",
            "/api/sessions".into(),
            Some(json!({
                "name": input.get("name").cloned().unwrap_or(json!("MCP 会话")),
                "cwd": input.get("cwd").cloned().unwrap_or(Value::Null),
            })),
        ),
        "pisper_get_session_messages" => (
            "GET",
            format!("/api/sessions/{}", input["sessionId"].as_str().unwrap_or(""))+"/messages",
            None,
        ),
        "pisper_get_session_live" => (
            "GET",
            format!("/api/sessions/{}", input["sessionId"].as_str().unwrap_or(""))+"/live",
            None,
        ),
        "pisper_get_session_changes" => (
            "GET",
            format!("/api/sessions/{}", input["sessionId"].as_str().unwrap_or(""))+"/file-changes",
            None,
        ),
        "pisper_get_usage_today" => ("GET", "/api/usage/today".into(), None),
        "pisper_rename_session" => (
            "PATCH",
            format!("/api/sessions/{}", input["sessionId"].as_str().unwrap_or("")),
            Some(json!({"name": input["name"]})),
        ),
        "pisper_abort_session" => (
            "POST",
            format!("/api/sessions/{}", input["sessionId"].as_str().unwrap_or(""))+"/abort",
            Some(json!({})),
        ),
        "pisper_list_assets" => ("GET", "/api/assets".into(), None),
        "pisper_search_memory" => ("GET", format!("/api/memory?query={}", input["query"].as_str().unwrap_or("")), None),
        "pisper_list_schedules" => ("GET", "/api/schedules".into(), None),
        "pisper_run_schedule" => (
            "POST",
            format!("/api/schedules/{}/run", input["scheduleId"].as_str().unwrap_or("")),
            Some(json!({})),
        ),
        "pisper_list_workflows" => ("GET", "/api/workflows".into(), None),
        "pisper_get_workflow_run" => (
            "GET",
            format!("/api/workflow-runs/{}", input["runId"].as_str().unwrap_or("")),
            None,
        ),
        "pisper_run_workflow" => (
            "POST",
            format!("/api/workflows/{}/run", input["workflowId"].as_str().unwrap_or("")),
            Some(json!({"inputs": input.get("inputs").cloned().unwrap_or(json!({}))})),
        ),
        "pisper_stop_workflow_run" => (
            "POST",
            format!("/api/workflow-runs/{}/stop", input["runId"].as_str().unwrap_or("")),
            Some(json!({})),
        ),
        "pisper_get_goal" => (
            "GET",
            format!("/api/sessions/{}", input["sessionId"].as_str().unwrap_or(""))+"/goal",
            None,
        ),
        "pisper_pause_goal" => (
            "PATCH",
            format!("/api/sessions/{}", input["sessionId"].as_str().unwrap_or(""))+"/goal",
            Some(json!({"action":"pause"})),
        ),
        "pisper_send_message" => return send_message(state.clone(), input).await,
        "pisper_get_run" => return get_run(&state, input).await,
        _ => return tool_failed("未知的 Pisper MCP 工具。"),
    };
    match api_call(&state, method, &path, body).await {
        Ok((status, value)) => match require_ok(status, &value) {
            Ok(()) => tool_result(value),
            Err(message) => tool_failed(&message),
        },
        Err(message) => tool_failed(&message),
    }
}

/// release pisper_send_message：立即返回运行 id，后台执行，get_run 轮询。
async fn send_message(state: Arc<AppState>, input: Value) -> Value {
    // 解析会话：显式 id 或新建（MCP 会话）。
    let session_id = match input.get("sessionId").and_then(Value::as_str) {
        Some(id) if !id.is_empty() => {
            match crate::session_api::find_session_path(&state, id) {
                Ok(_) => id.to_string(),
                Err(_) => return tool_failed("会话不存在。"),
            }
        }
        _ => {
            let body = json!({
                "name": input.get("name").cloned().unwrap_or(json!("MCP 会话")),
                "cwd": input.get("cwd").cloned().unwrap_or(Value::Null),
            });
            match api_call(&state, "POST", "/api/sessions", Some(body)).await {
                Ok((status, value)) if status.is_success() => {
                    let id = value["id"].as_str().unwrap_or_default().to_string();
                    // release 的新会话跟随全局默认模型（setDefaultModelAndProvider）；
                    // 创建后按当前默认显式对齐，避免继承启动会话的旧模型。
                    if let Ok((_, config)) =
                        api_call(&state, "GET", "/api/config", None).await
                    {
                        let provider = config["provider"].as_str().unwrap_or("");
                        let model = config["model"].as_str().unwrap_or("");
                        if !provider.is_empty() && !model.is_empty() {
                            let _ = api_call(
                                &state,
                                "PUT",
                                &format!("/api/sessions/{id}/model"),
                                Some(json!({"provider": provider, "model": model})),
                            )
                            .await;
                        }
                    }
                    id
                }
                _ => return tool_failed("会话不存在。"),
            }
        }
    };
    if session_id.is_empty() {
        return tool_failed("会话不存在。");
    }
    // 活跃运行数上限。
    {
        let mut runs = state.mcp_runs.lock().expect("mcp runs lock");
        let now = product::now_ms();
        runs.retain(|_, run| {
            run["status"] == "running"
                || now.saturating_sub(run["endedAtMs"].as_u64().unwrap_or(now)) < MAX_RUN_AGE_MS
        });
        if runs.values().filter(|run| run["status"] == "running").count() >= MAX_ACTIVE_RUNS {
            return tool_failed("外部会话运行数量已达到上限，请等待已有任务完成。");
        }
        while runs.len() >= MAX_RUNS {
            let oldest = runs
                .iter()
                .filter(|(_, run)| run["status"] != "running")
                .map(|(id, _)| id.clone())
                .next();
            match oldest {
                Some(id) => {
                    runs.remove(&id);
                }
                None => break,
            }
        }
    }
    let run_id_spawned = format!("mcp_run_{}", product::new_id());
    let run_id = run_id_spawned.clone();
    let run = json!({
        "id": run_id,
        "sessionId": session_id,
        "status": "running",
        "text": "",
        "error": "",
        "needsApproval": false,
        "startedAt": crate::session_ops::iso_timestamp(product::now_ms()),
        "endedAt": "",
        "endedAtMs": 0,
    });
    state
        .mcp_runs
        .lock()
        .expect("mcp runs lock")
        .insert(run_id.clone(), run);
    // 后台执行：POST /api/chat 消费 SSE，聚合 text_delta，error 帧置失败。
    let state_task = state.clone();
    let message = input["message"].as_str().unwrap_or_default().to_string();
    let goal_mode = input["goalMode"].as_bool().unwrap_or(false)
        || input["teamMode"].as_bool().unwrap_or(false);
    let session_id_spawned = session_id.clone();
    tokio::spawn(async move {
        let session_id = session_id_spawned;
        let run_id = run_id_spawned;
        let body = json!({"sessionId": session_id, "message": message, "goalMode": goal_mode});
        let result = run_prompt_via_chat(&state_task, &session_id, &body).await;
        let mut runs = state_task.mcp_runs.lock().expect("mcp runs lock");
        if let Some(run) = runs.get_mut(&run_id) {
            match result {
                Ok(text) => {
                    run["status"] = json!("completed");
                    run["text"] = json!(text.chars().rev().take(MAX_TEXT_CHARS).collect::<String>()
                        .chars()
                        .rev()
                        .collect::<String>());
                }
                Err(error) => {
                    run["status"] = json!("failed");
                    run["error"] = json!(security::redact_secret_text(&error));
                }
            }
            run["endedAt"] = json!(crate::session_ops::iso_timestamp(product::now_ms()));
            run["endedAtMs"] = json!(product::now_ms());
        }
    });
    tool_result(json!({"runId": run_id, "sessionId": session_id, "status": "running"}))
}

/// 经 POST /api/chat 执行一轮对话并聚合最终文本（SSE 消费）。
async fn run_prompt_via_chat(
    state: &AppState,
    session_id: &str,
    body: &Value,
) -> Result<String, String> {
    let base = self_base(state).ok_or_else(|| "Runtime 尚未就绪。".to_string())?;
    let client = crate::desktop_ops::http_client();
    let mut request = client
        .post(format!("{base}/api/chat"))
        .header("Content-Type", "application/json")
        .json(body);
    if let Some(token) = &state.desktop_token {
        request = request.header("Cookie", format!("__pisper_desktop={token}"));
    }
    let response = request
        .timeout(std::time::Duration::from_secs(600))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        let text = response.text().await.unwrap_or_default();
        let value = serde_json::from_str::<Value>(&text).unwrap_or(json!({}));
        return Err(value["error"]
            .as_str()
            .unwrap_or("会话运行失败。")
            .to_string());
    }
    let mut text = String::new();
    let mut stream = response.bytes_stream();
    let mut buffer = Vec::new();
    use futures::StreamExt;
    let mut done_text: Option<String> = None;
    let mut current_event = String::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| error.to_string())?;
        buffer.extend_from_slice(&chunk);
        while let Some(position) = buffer.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = buffer.drain(..=position).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim_end();
            // SSE 事件名在 `event:` 行；data 载荷是 {cursor,event,data} 帧的 data 部分。
            if let Some(name) = line.strip_prefix("event: ") {
                current_event = name.trim().to_string();
                continue;
            }
            let Some(data) = line.strip_prefix("data: ") else {
                continue;
            };
            let Ok(frame) = serde_json::from_str::<Value>(data) else {
                continue;
            };
            match current_event.as_str() {
                "text_delta" => {
                    if let Some(delta) = frame["delta"].as_str() {
                        text.push_str(delta);
                    }
                }
                "done" => {
                    if let Some(final_text) = frame["text"].as_str() {
                        done_text = Some(final_text.to_string());
                    }
                }
                "error" => {
                    return Err(frame["message"]
                        .as_str()
                        .unwrap_or("会话运行失败。")
                        .to_string());
                }
                _ => {}
            }
        }
        if text.chars().count() > MAX_TEXT_CHARS * 2 {
            text = text
                .chars()
                .skip(text.chars().count() - MAX_TEXT_CHARS)
                .collect();
        }
    }
    Ok(done_text.unwrap_or(text))
}

/// release pisper_get_run：读取运行记录；不存在 → not_found。
async fn get_run(state: &AppState, input: Value) -> Value {
    let run_id = input["runId"].as_str().unwrap_or("");
    let runs = state.mcp_runs.lock().expect("mcp runs lock");
    match runs.get(run_id) {
        Some(run) => {
            let mut public = run.clone();
            public.as_object_mut().map(|object| object.remove("endedAtMs"));
            tool_result(public)
        }
        None => json!({
            "content":[{"type":"text","text":json!({"error":"not_found","message":"运行不存在。"}).to_string()}],
            "isError": true,
        }),
    }
}

fn secure_equal(left: &str, right: &str) -> bool {
    use subtle::ConstantTimeEq;
    left.as_bytes().ct_eq(right.as_bytes()).into()
}

/// release MCP Streamable-HTTP JSON-RPC 端点（无状态模式）。
pub(crate) async fn mcp_endpoint(
    State(state): State<ArcAppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let mcp_state = load_state(&state);
    let presented = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or("");
    if !mcp_state.enabled
        || mcp_state.token.is_empty()
        || presented.len() != mcp_state.token.len()
        || !secure_equal(presented, &mcp_state.token)
    {
        return (
            StatusCode::UNAUTHORIZED,
            axum::Json(json!({"error":"unauthorized","code":"mcp_token_required"})),
        )
            .into_response();
    }
    let request: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                axum::Json(json!({"error":"MCP request body must be valid JSON.","code":"bad_request"})),
            )
                .into_response()
        }
    };
    let method = request["method"].as_str().unwrap_or("");
    let id = request["id"].clone();
    // 通知（无 id）：202 Accepted，无响应体（Streamable HTTP 语义）。
    if id.is_null() {
        return StatusCode::ACCEPTED.into_response();
    }
    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": {"name": "Pisper", "version": env!("CARGO_PKG_VERSION")},
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tool_definitions()})),
        "tools/call" => {
            let name = request["params"]["name"].as_str().unwrap_or("").to_string();
            let arguments = request["params"]["arguments"].clone();
            if name.is_empty() {
                Err(tool_failed("缺少工具名。"))
            } else {
                Ok(call_tool(state.clone(), &name, arguments).await)
            }
        }
        other => Err(json!({
            "jsonrpc":"2.0","id":id,
            "error":{"code":-32601,"message":format!("Method not found: {other}")}
        })),
    };
    match result {
        Ok(result) => Json(json!({"jsonrpc":"2.0","id":id,"result":result})).into_response(),
        Err(error) => {
            if error.get("error").is_some() && error["error"].is_object() {
                Json(error).into_response()
            } else {
                Json(json!({"jsonrpc":"2.0","id":id,"result":error})).into_response()
            }
        }
    }
}
