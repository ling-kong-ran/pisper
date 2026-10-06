//! release `services/mcp-host-service.mjs` 的管理面移植：
//! 内置 MCP 服务开关、凭据读取与令牌轮换（持久化到 pisper-mcp-host.json）。
//!
//! 诚实接缝：对外 MCP 协议监听（Streamable HTTP + 会话工具注册）依赖
//! MCP server 实现（pi-rs 的 mcp 模块目前只有客户端），本模块如实报告
//! `listening:false` 并在 error 字段说明；管理面状态与令牌生命周期与
//! release 逐字段对齐。

use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};
use sha2::Digest;
use std::path::Path;
use std::sync::Arc;

use crate::{product, ApiError, AppState};

type ArcAppState = Arc<AppState>;

const STATE_VERSION: u32 = 1;
const MCP_HOST_PORT: u32 = 5175;

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
    let mut buffer = [0u8; 32];
    // product::new_id 每次调用都有系统熵；拼接多次保证 ≥32 字节素材。
    let mut material = String::new();
    while material.len() < 96 {
        material.push_str(&product::new_id());
    }
    let digest = sha2::Sha256::digest(material.as_bytes());
    buffer.copy_from_slice(&digest);
    buffer.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn status_view(mcp_state: &McpHostState) -> Value {
    json!({
        "enabled": mcp_state.enabled,
        "listening": false,
        "host": "127.0.0.1",
        "port": MCP_HOST_PORT,
        "url": "",
        "error": if mcp_state.enabled {
            json!("内置 MCP 服务监听尚未启动。")
        } else {
            Value::Null
        },
    })
}

/// release GET /api/mcp-host
pub(crate) async fn status(State(state): State<ArcAppState>) -> Json<Value> {
    Json(status_view(&load_state(&state)))
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
    Ok(Json(status_view(&mcp_state)))
}

/// release POST /api/mcp-host/credentials：未监听 → 409 mcp_host_not_listening。
pub(crate) async fn credentials(State(state): State<ArcAppState>) -> Result<Json<Value>, ApiError> {
    let mcp_state = load_state(&state);
    // 监听尚未实现（诚实接缝），凭据永远按 release 的 not_listening 分支返回。
    let _ = &mcp_state;
    Err(ApiError::new(
        axum::http::StatusCode::CONFLICT,
        "mcp_host_not_listening",
        "内置 MCP 服务尚未就绪。",
    ))
}

/// release POST /api/mcp-host/rotate-token：未开启 → 400；未监听 → 503。
pub(crate) async fn rotate_token(State(state): State<ArcAppState>) -> Result<Json<Value>, ApiError> {
    let mut mcp_state = load_state(&state);
    if !mcp_state.enabled {
        return Err(ApiError::bad_request("请先开启 Pisper MCP 服务。"));
    }
    mcp_state.token = generate_token();
    save_state(&state, &mcp_state)?;
    Err(ApiError::new(
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        "mcp_host_not_listening",
        "内置 MCP 服务尚未就绪。",
    ))
}
