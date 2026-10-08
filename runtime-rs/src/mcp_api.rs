//! MCP 的配置管理、真实连接测试与前端投影。
//! 原生扩展没有公开其连接表；只把本模块实际持有的测试连接报告为在线。

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, patch, post},
    Json, Router,
};
use pi_rust::{
    ai::types::ordered_map::OrderedMap,
    coding_agent::{
        core::mcp_servers::{mcp_namespace, validate_mcp_server_config, McpExposure},
        extensions::{
            mcp::{
                config::{
                    add_mcp_server_config, load_mcp_config, remove_mcp_server_config,
                    update_mcp_server_config, LoadedMcpConfigOptions, McpServerConfigPatch,
                    McpServerEntry,
                },
                oauth::McpOAuthCredentialStore,
                runtime::{
                    create_default_transport, McpServerConnection, McpServerConnectionOptions,
                    ServerState,
                },
                tools::create_mcp_tool_name,
            },
            types::ToolExposure,
        },
    },
    mcp::transports::McpTransport,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::{ApiError, AppState};

struct Probe {
    signature: String,
    connection: Arc<McpServerConnection>,
    timestamp: String,
    latency_ms: u128,
    transports: Arc<Mutex<Vec<Arc<dyn McpTransport>>>>,
}

fn probes() -> &'static Mutex<HashMap<String, Probe>> {
    static STORE: OnceLock<Mutex<HashMap<String, Probe>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/mcp", get(dashboard).post(add))
        .route("/api/mcp/{id}", patch(set_enabled).delete(remove))
        .route("/api/mcp/{id}/tools/{tool}", patch(set_tool_enabled))
        .route("/api/mcp/{id}/test", post(test))
}

pub(crate) async fn shutdown(state: &AppState) {
    let prefix = format!("{}\0", state.agent_dir);
    let owned = {
        let mut store = probes().lock().expect("MCP probe lock");
        let keys: Vec<_> = store
            .keys()
            .filter(|key| key.starts_with(&prefix))
            .cloned()
            .collect();
        keys.into_iter()
            .filter_map(|key| store.remove(&key))
            .collect::<Vec<_>>()
    };
    futures::future::join_all(
        owned
            .iter()
            .map(|probe| close_owned(&probe.connection, &probe.transports)),
    )
    .await;
}

async fn close_owned(
    connection: &Arc<McpServerConnection>,
    transports: &Arc<Mutex<Vec<Arc<dyn McpTransport>>>>,
) {
    let owned = { std::mem::take(&mut *transports.lock().expect("MCP transport lock")) };
    let result = tokio::time::timeout(Duration::from_secs(4), async {
        let _ = tokio::join!(
            connection.close(),
            futures::future::join_all(owned.iter().map(|transport| transport.close()))
        );
    })
    .await;
    if result.is_err() {
        tracing::warn!("MCP owned transport cleanup exceeded its shutdown budget");
    }
}

fn key(state: &AppState, id: &str) -> String {
    format!("{}\0{id}", state.agent_dir)
}
fn config_path(state: &AppState) -> PathBuf {
    PathBuf::from(&state.agent_dir).join("mcp.json")
}
fn session_cwd(state: &AppState) -> String {
    state
        .runtime
        .session()
        .session_manager
        .lock()
        .expect("session manager lock")
        .get_cwd()
        .to_string()
}
fn exposure_name(exposure: McpExposure) -> &'static str {
    match exposure {
        McpExposure::Codemode => "codemode",
        McpExposure::Deferred => "deferred",
        McpExposure::Direct => "direct",
        McpExposure::Hidden => "hidden",
    }
}
fn ordered(value: &Value) -> Result<OrderedMap<Value>, ApiError> {
    let object = value
        .as_object()
        .ok_or_else(|| ApiError::bad_request("MCP 服务配置必须是对象。"))?;
    Ok(OrderedMap::from_pairs(
        object.iter().map(|(k, v)| (k.clone(), v.clone())),
    ))
}
fn signature(entry: &McpServerEntry) -> String {
    let value: serde_json::Map<String, Value> = entry
        .config
        .raw()
        .iter()
        .filter(|(k, _)| !matches!(k.as_str(), "exposure" | "toolExposure"))
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect();
    hex_digest(&serde_json::to_vec(&value).unwrap_or_default())
}
fn matches_tool_name(server: &str, original: &str, pi_name: &str) -> bool {
    create_mcp_tool_name(server, original, |_| false) == pi_name
        || create_mcp_tool_name(server, original, |_| true) == pi_name
}
fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn entries(state: &AppState) -> Vec<McpServerEntry> {
    load_mcp_config(LoadedMcpConfigOptions {
        agent_dir: state.agent_dir.clone(),
        cwd: session_cwd(state),
        project_trusted: false,
    })
    .servers
}
fn find(state: &AppState, id: &str) -> Result<McpServerEntry, ApiError> {
    entries(state)
        .into_iter()
        .find(|entry| entry.name == id)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "mcp_not_found", "MCP 服务不存在。"))
}

fn public_endpoint(url: &str) -> String {
    let without_query = url.split(['?', '#']).next().unwrap_or("");
    let Some((scheme, rest)) = without_query.split_once("://") else {
        return String::new();
    };
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let authority = authority.rsplit('@').next().unwrap_or("");
    format!(
        "{scheme}://{authority}{}",
        if path.is_empty() {
            String::new()
        } else {
            format!("/{path}")
        }
    )
}

fn safe_error(entry: &McpServerEntry, error: &str) -> String {
    let mut result = error.to_string();
    for section in ["headers", "env"] {
        for (_, secret) in entry.config.string_record(section).unwrap_or_default() {
            if !secret.is_empty() {
                result = result.replace(&secret, "[REDACTED]");
            }
            if let Some(token) = secret.strip_prefix("Bearer ") {
                if !token.is_empty() {
                    result = result.replace(token, "[REDACTED]");
                }
            }
        }
    }
    if let Some(oauth) = entry.config.oauth() {
        if let Some(secret) = oauth.client_secret {
            if !secret.is_empty() {
                result = result.replace(&secret, "[REDACTED]");
            }
        }
    }
    if let Some(url) = entry.config.url() {
        result = result.replace(url, &public_endpoint(url));
    }
    crate::security::redact_secret_text(&result)
}

fn snapshot(state: &AppState) -> Value {
    let configured = entries(state);
    let tools = state.runtime.session().get_all_tools();
    let probe_store = probes().lock().expect("MCP probe lock");
    let mut services = Vec::new();
    let mut tool_rows = Vec::new();
    let mut online = 0;
    for entry in &configured {
        let probe = probe_store
            .get(&key(state, &entry.name))
            .filter(|probe| probe.signature == signature(entry));
        let enabled = entry.config.enabled();
        let status = if !enabled {
            "disabled"
        } else {
            match probe.map(|probe| probe.connection.state()) {
                Some(ServerState::Connected) => {
                    online += 1;
                    "online"
                }
                Some(ServerState::Connecting) => "connecting",
                Some(ServerState::NeedsAuth) => "unauthorized",
                Some(ServerState::Failed | ServerState::Disconnected | ServerState::Closed) => {
                    "offline"
                }
                None => "unverified",
            }
        };
        let headers = entry.config.string_record("headers").unwrap_or_default();
        let environment = entry.config.string_record("env").unwrap_or_default();
        services.push(json!({
            "id": entry.name, "name": entry.name, "status": status,
            "transport": if entry.config.command().is_some() { "stdio" } else { "http" },
            "endpoint": entry.config.url().map(public_endpoint).unwrap_or_default(),
            "command": entry.config.command().unwrap_or(""),
            "workingDirectory": entry.config.cwd().unwrap_or(""), "enabled": enabled,
            "error": probe.and_then(|probe| probe.connection.error()).map(|error| safe_error(entry, &error)).unwrap_or_default(),
            "latencyMs": probe.map(|probe| probe.latency_ms),
            "lastPingAt": probe.map(|probe| probe.timestamp.as_str()).unwrap_or(""),
            "auth": if !headers.is_empty() { "headers" } else if !environment.is_empty() { "environment" } else if entry.config.command().is_some() { "local" } else { "none" },
            "authCount": headers.len() + environment.len(),
            "statusSource": if probe.is_some() { "explicit-connection-test" } else { "not-observable" },
            "exposure": exposure_name(entry.config.exposure()),
        }));
        let namespace = mcp_namespace(&entry.name);
        for tool in tools.iter().filter(|tool| {
            tool.namespace
                .as_ref()
                .is_some_and(|ns| ns.name == namespace)
        }) {
            let catalog_name = probe.and_then(|probe| {
                probe
                    .connection
                    .tools()
                    .into_iter()
                    .find(|candidate| matches_tool_name(&entry.name, &candidate.name, &tool.name))
                    .map(|candidate| candidate.name)
            });
            let original_name = catalog_name.as_deref().unwrap_or(&tool.name);
            tool_rows.push(json!({
                "serviceId": entry.name, "serviceName": entry.name, "name": original_name,
                "piName": tool.name, "description": safe_error(entry, &tool.description),
                "enabled": enabled && tool.exposure != ToolExposure::Hidden,
                "serviceEnabled": enabled, "risk": "unknown", "source": "engine-tool-catalog",
            }));
        }
    }
    let available = tool_rows
        .iter()
        .filter(|tool| tool["enabled"] == true)
        .count();
    json!({"services": services, "tools": tool_rows, "calls": [], "metrics": {"totalServices": configured.len(), "onlineServices": online, "availableTools": available, "restrictedTools": tool_rows.len() - available, "errorRate": 0}, "runtimeStatusObservable": false})
}

async fn dashboard(State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(snapshot(&state))
}

fn engine_guard(state: &AppState) -> Result<tokio::sync::RwLockWriteGuard<'_, ()>, ApiError> {
    let guard = state.engine_mutation.try_write().map_err(|_| {
        ApiError::new(
            StatusCode::CONFLICT,
            "session_busy",
            "当前会话正在运行，请稍后修改 MCP。",
        )
    })?;
    if state.sessions.any_busy() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "session_busy",
            "当前会话正在运行，请稍后修改 MCP。",
        ));
    }
    Ok(guard)
}

async fn invalidate_probe(state: &AppState, id: &str) {
    let old = probes()
        .lock()
        .expect("MCP probe lock")
        .remove(&key(state, id));
    if let Some(old) = old {
        close_owned(&old.connection, &old.transports).await;
    }
}

async fn reload(state: &AppState) -> Result<(), ApiError> {
    let result = tokio::time::timeout(
        Duration::from_secs(20),
        state.runtime.session().reload(None),
    )
    .await;
    crate::attach_event_listener(state);
    match result {
        Ok(Ok(())) => {
            for hosted in state.sessions.all() {
                tokio::time::timeout(Duration::from_secs(20), hosted.session().reload(None))
                    .await
                    .map_err(|_| ApiError::internal("MCP 配置已保存，但会话重新加载超时。"))?
                    .map_err(|_| ApiError::internal("MCP 配置已保存，但会话重新加载失败。"))?;
                hosted.rebind(state);
            }
            Ok(())
        }
        Ok(Err(_)) => Err(ApiError::internal(
            "MCP 配置已保存，但引擎重新加载失败。请重启应用后重试。",
        )),
        Err(_) => Err(ApiError::internal(
            "MCP 配置已保存，但引擎重新加载超时。请重启应用后重试。",
        )),
    }
}

fn split_command(input: &str) -> Result<Vec<String>, ApiError> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if let Some(delimiter) = quote {
            if ch == delimiter {
                quote = None;
            } else if ch == '\\' && chars.peek() == Some(&delimiter) {
                word.push(chars.next().expect("peeked"));
            } else {
                word.push(ch);
            }
        } else if ch == '\'' || ch == '"' {
            quote = Some(ch);
        } else if ch.is_whitespace() {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
        } else {
            word.push(ch);
        }
    }
    if quote.is_some() {
        return Err(ApiError::bad_request("MCP 命令的引号未闭合。"));
    }
    if !word.is_empty() {
        words.push(word);
    }
    Ok(words)
}

fn parse_spec(spec: &str) -> Result<Vec<(String, Value)>, ApiError> {
    let spec = spec.trim();
    if spec.is_empty() || spec.len() > 12_000 {
        return Err(ApiError::bad_request(
            "请输入有效的 MCP URL、命令或 JSON 配置。",
        ));
    }
    let id = format!("mcp-{}", &hex_digest(spec.as_bytes())[..12]);
    let result = if spec.starts_with('{') {
        let mut value: Value = serde_json::from_str(spec)
            .map_err(|_| ApiError::bad_request("MCP 配置 JSON 无效。"))?;
        if value.get("command").is_some() || value.get("url").is_some() {
            let name = value
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(&id)
                .to_string();
            value.as_object_mut().expect("parsed object").remove("name");
            vec![(name, value)]
        } else {
            let servers = value
                .get("mcpServers")
                .unwrap_or(&value)
                .as_object()
                .ok_or_else(|| ApiError::bad_request("MCP JSON 必须包含服务配置。"))?;
            servers
                .iter()
                .map(|(name, config)| (name.clone(), config.clone()))
                .collect()
        }
    } else if spec.starts_with("http://") || spec.starts_with("https://") {
        vec![(id, json!({"url":spec,"exposure":"codemode"}))]
    } else {
        let words = split_command(spec)?;
        if words.is_empty() {
            return Err(ApiError::bad_request("MCP 命令为空。"));
        }
        if words.get(1).is_some_and(|word| word == "--url") {
            if words.len() != 3 {
                return Err(ApiError::bad_request("HTTP 配置格式为：服务名 --url URL。"));
            }
            vec![(
                words[0].clone(),
                json!({"url":words[2],"exposure":"codemode"}),
            )]
        } else {
            let (name, start) = if words.get(1).is_some_and(|word| word == "--") {
                (words[0].clone(), 2)
            } else {
                (id, 0)
            };
            let command = words
                .get(start)
                .ok_or_else(|| ApiError::bad_request("MCP 命令为空。"))?;
            vec![(
                name,
                json!({"command":command,"args":words[start+1..],"exposure":"codemode"}),
            )]
        }
    };
    if result.is_empty() {
        return Err(ApiError::bad_request("MCP 配置中没有服务。"));
    }
    for (name, config) in &result {
        validate_mcp_server_config(name, &ordered(config)?).map_err(|_| {
            ApiError::bad_request(format!(
                "MCP 服务 {name} 的配置无效，请检查 command、args、url、headers 和 env。"
            ))
        })?;
    }
    Ok(result)
}

async fn add(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let _guard = engine_guard(&state)?;
    let definitions = parse_spec(body["spec"].as_str().unwrap_or(""))?;
    let existing = entries(&state);
    for (name, _) in &definitions {
        if existing.iter().any(|entry| entry.name == *name) {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "mcp_exists",
                "同名 MCP 服务已存在，请先删除或修改现有服务。",
            ));
        }
    }
    let path = config_path(&state);
    // 官方编辑器保留根文档和其他服务的未知字段；失败时恢复写入前的字节。
    let previous = std::fs::read(&path).ok();
    for (name, config) in &definitions {
        if add_mcp_server_config(&path.to_string_lossy(), name, &ordered(config)?).is_err() {
            if let Some(bytes) = &previous {
                let _ = std::fs::write(&path, bytes);
            } else {
                let _ = std::fs::remove_file(&path);
            }
            return Err(ApiError::internal("无法保存 MCP 配置。"));
        }
    }
    reload(&state).await?;
    Ok(Json(snapshot(&state)))
}

async fn set_enabled(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let _guard = engine_guard(&state)?;
    let entry = find(&state, &id)?;
    let enabled = body["enabled"]
        .as_bool()
        .ok_or_else(|| ApiError::bad_request("enabled 必须是布尔值。"))?;
    update_mcp_server_config(
        &entry.source,
        &id,
        McpServerConfigPatch {
            enabled: Some(enabled),
            exposure: None,
        },
        false,
    )
    .map_err(|_| ApiError::internal("无法保存 MCP 启停设置。"))?;
    invalidate_probe(&state, &id).await;
    reload(&state).await?;
    Ok(Json(snapshot(&state)))
}

async fn set_tool_enabled(
    State(state): State<Arc<AppState>>,
    Path((id, tool)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let _guard = engine_guard(&state)?;
    let entry = find(&state, &id)?;
    let enabled = body["enabled"]
        .as_bool()
        .ok_or_else(|| ApiError::bad_request("enabled 必须是布尔值。"))?;
    if tool.is_empty() || tool.len() > 256 {
        return Err(ApiError::bad_request("工具名称无效。"));
    }
    // Pi 工具名会转义与截短，不能猜原名写权限；测试目录给出经过协议确认的原名。
    let original_name = {
        let store = probes().lock().expect("MCP probe lock");
        store
            .get(&key(&state, &id))
            .filter(|probe| probe.signature == signature(&entry))
            .and_then(|probe| {
                probe
                    .connection
                    .tools()
                    .into_iter()
                    .find(|candidate| {
                        candidate.name == tool || matches_tool_name(&id, &candidate.name, &tool)
                    })
                    .map(|candidate| candidate.name)
            })
    }
    .ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_IMPLEMENTED,
            "mcp_tool_identity_unverified",
            "请先测试连接以确认工具原名，再修改工具权限。",
        )
    })?;
    let bytes =
        std::fs::read(&entry.source).map_err(|_| ApiError::internal("无法读取 MCP 配置。"))?;
    let mut document: Value =
        serde_json::from_slice(&bytes).map_err(|_| ApiError::internal("MCP 配置 JSON 无效。"))?;
    let root = if document.get("mcpServers").is_some() {
        &mut document["mcpServers"]
    } else {
        &mut document
    };
    let config = root
        .get_mut(&id)
        .and_then(Value::as_object_mut)
        .ok_or_else(|| ApiError::internal("MCP 配置中缺少该服务。"))?;
    let mut exposure = config
        .get("toolExposure")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if enabled {
        exposure.insert(
            original_name,
            json!(exposure_name(match entry.config.exposure() {
                McpExposure::Hidden => McpExposure::Codemode,
                exposure => exposure,
            })),
        );
    } else {
        exposure.insert(original_name, json!("hidden"));
    }
    config.insert("toolExposure".into(), Value::Object(exposure));
    let config = Value::Object(config.clone());
    validate_mcp_server_config(&id, &ordered(&config)?)
        .map_err(|_| ApiError::bad_request("工具权限配置无效。"))?;
    add_mcp_server_config(&entry.source, &id, &ordered(&config)?)
        .map_err(|_| ApiError::internal("无法保存 MCP 工具设置。"))?;
    reload(&state).await?;
    Ok(Json(snapshot(&state)))
}

async fn remove(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let _guard = engine_guard(&state)?;
    let entry = find(&state, &id)?;
    remove_mcp_server_config(&entry.source, &id)
        .map_err(|_| ApiError::internal("无法删除 MCP 服务。"))?;
    invalidate_probe(&state, &id).await;
    reload(&state).await?;
    Ok(Json(snapshot(&state)))
}

async fn test(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let _guard = engine_guard(&state)?;
    let entry = find(&state, &id)?;
    if !entry.config.enabled() {
        return Err(ApiError::bad_request("请先启用 MCP 服务。"));
    }
    if entry.config.auth_provider().is_some() {
        return Err(ApiError::new(
            StatusCode::NOT_IMPLEMENTED,
            "mcp_provider_auth_probe_unsupported",
            "此测试暂不支持复用模型 Provider 的认证；原生 Agent 连接仍使用其真实认证。",
        ));
    }
    // 初始化尚未完成时 Pi 的 connection.client 为空，显式保留 transport 才能回收子进程。
    let transports: Arc<Mutex<Vec<Arc<dyn McpTransport>>>> = Arc::new(Mutex::new(Vec::new()));
    let tracked = transports.clone();
    let connection = McpServerConnection::new(McpServerConnectionOptions {
        entry: entry.clone(),
        cwd: session_cwd(&state),
        create_transport: Arc::new(move |entry, cwd, auth| {
            let created = create_default_transport(entry, cwd, auth)?;
            tracked
                .lock()
                .expect("MCP transport lock")
                .push(created.transport.clone());
            Ok(created)
        }),
        credentials: Arc::new(McpOAuthCredentialStore::with_paths(
            PathBuf::from(&state.agent_dir)
                .join("mcp-auth.json")
                .to_string_lossy()
                .to_string(),
            Some(state.agent_dir.clone()),
        )),
        provider_token: None,
        on_tools: Arc::new(|_| {}),
        on_change: None,
        log: None,
    });
    let started = Instant::now();
    let mut opening = Box::pin(connection.get_client());
    let result = tokio::time::timeout(Duration::from_secs(15), opening.as_mut()).await;
    let timed_out = result.is_err();
    let error = match result {
        Ok(Ok(_)) => None,
        Ok(Err(error)) => Some(safe_error(&entry, &error)),
        Err(_) => Some("MCP 连接测试超时。".to_string()),
    };
    if let Some(error) = error {
        close_owned(&connection, &transports).await;
        if timed_out {
            // Transport 关闭会唤醒待决请求；让已开始的 future 清除 Pi 的 opening 缓存。
            let _ = tokio::time::timeout(Duration::from_secs(2), opening.as_mut()).await;
        }
        return Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            "mcp_connection_failed",
            error,
        ));
    }
    drop(opening);
    invalidate_probe(&state, &id).await;
    let timestamp = pi_rust::agent_core::harness::session::jsonl::iso8601::format_iso8601_utc(
        crate::product::now_ms() as i64,
    );
    let tool_count = connection.tools().len();
    probes().lock().expect("MCP probe lock").insert(
        key(&state, &id),
        Probe {
            signature: signature(&entry),
            connection,
            timestamp,
            latency_ms: started.elapsed().as_millis(),
            transports,
        },
    );
    let mut data = snapshot(&state);
    data["test"] = json!({"ok":true,"serviceId":id,"toolCount":tool_count,"source":"real-mcp-initialize-and-tools-list"});
    Ok(Json(data))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn spec_preserves_credentials_but_public_projection_redacts_endpoint() {
        let spec = r#"{"mcpServers":{"fixture":{"url":"https://user:secret@example.test/mcp?token=hidden","headers":{"Authorization":"Bearer synthetic-secret"},"futureField":{"kept":true}}}}"#;
        let parsed = parse_spec(spec).unwrap();
        assert_eq!(parsed[0].0, "fixture");
        assert_eq!(
            parsed[0].1["headers"]["Authorization"],
            "Bearer synthetic-secret"
        );
        assert_eq!(parsed[0].1["futureField"]["kept"], true);
        assert_eq!(
            public_endpoint(parsed[0].1["url"].as_str().unwrap()),
            "https://example.test/mcp"
        );
    }
    #[test]
    fn windows_stdio_spec_retains_path_and_quoted_arguments() {
        let parsed =
            parse_spec(r#"fixture -- "C:\Program Files\node.exe" "C:\fixture\server.mjs""#)
                .unwrap();
        assert_eq!(parsed[0].0, "fixture");
        assert_eq!(parsed[0].1["command"], r#"C:\Program Files\node.exe"#);
        assert_eq!(parsed[0].1["args"][0], r#"C:\fixture\server.mjs"#);
        assert!(parse_spec("\"unclosed").is_err());
    }
}
