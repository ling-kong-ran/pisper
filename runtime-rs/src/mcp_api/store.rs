//! pisper-mcp.json 是唯一读写来源；仅在内存中适配 Pi 的配置接口。
use super::{ordered, validate_mcp_server_config, ApiError};
use pi_rust::coding_agent::extensions::mcp::config::{
    LoadedMcpConfig, McpConfigScope, McpServerConfigPatch, McpServerEntry,
};
use serde_json::{json, Map, Value};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
static WRITE: Mutex<()> = Mutex::new(());

pub(super) fn read(path: &Path) -> Result<Value, ApiError> {
    use std::io::Read;
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(json!({"version":1,"servers":[],"calls":[]}))
        }
        Err(_) => return Err(ApiError::internal("无法读取 MCP 配置。")),
    };
    let mut bytes = Vec::new();
    file.take(8 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ApiError::internal("无法读取 MCP 配置。"))?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err(ApiError::bad_request("MCP 配置超过大小限制。"));
    }
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| ApiError::bad_request("MCP JSON 无效，未修改文件。"))?;
    if value["version"] != 1 || !value["servers"].is_array() {
        return Err(ApiError::bad_request("MCP 配置格式无效。"));
    }
    Ok(value)
}

fn project(value: &Value) -> Result<Value, ApiError> {
    if value["version"] != 1 {
        return Err(ApiError::bad_request("MCP 配置版本无效。"));
    }
    let servers = value["servers"]
        .as_array()
        .ok_or_else(|| ApiError::bad_request("MCP servers 格式无效。"))?;
    let mut result = Map::new();
    for server in servers {
        let display = server["name"].as_str().unwrap_or("");
        // 保留旧 HTTP 服务 ID；名称单独显示，避免已有选择和引用失效。
        let name = server["id"]
            .as_str()
            .filter(|name| !name.is_empty())
            .ok_or_else(|| ApiError::bad_request("MCP 服务缺少有效标识。"))?;
        if result.contains_key(name) {
            return Err(ApiError::bad_request("MCP 服务标识重复。"));
        }
        if server["transport"] == "sse" {
            return Err(ApiError::bad_request("MCP 使用 SSE；当前 Rust 引擎不支持此传输，请先改用 Streamable HTTP。原文件未修改。"));
        }
        let mut config = json!({"enabled":server.get("enabled").cloned().unwrap_or(json!(true)),"exposure":server.get("exposure").cloned().unwrap_or(json!("codemode")), "pisperName":display});
        for field in ["command", "args", "cwd", "env", "url", "headers"] {
            if let Some(value) = server.get(field) {
                if field == "cwd" && value.as_str().is_some_and(|cwd| cwd.trim().is_empty()) {
                    continue;
                }
                config[field] = value.clone();
            }
        }
        if let Some(timeout) = server.get("requestTimeoutMs") {
            let timeout = timeout
                .as_f64()
                .filter(|value| *value > 0.0)
                .ok_or_else(|| ApiError::bad_request("MCP 超时设置无效。"))?;
            config["timeout"] = json!(timeout / 1000.0);
        }
        for (key, value) in server
            .as_object()
            .ok_or_else(|| ApiError::bad_request("MCP 服务格式无效。"))?
        {
            if ![
                "id",
                "name",
                "transport",
                "requestTimeoutMs",
                "tools",
                "toolStates",
                "createdAt",
                "updatedAt",
            ]
            .contains(&key.as_str())
            {
                if key == "cwd" && value.as_str().is_some_and(|cwd| cwd.trim().is_empty()) {
                    continue;
                }
                config[key] = value.clone();
            }
        }
        if let Some(states) = server.get("toolStates") {
            let states = states
                .as_object()
                .ok_or_else(|| ApiError::bad_request("MCP 工具设置无效。"))?;
            let mut exposure = config
                .get("toolExposure")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            for (tool, enabled) in states {
                let enabled = enabled
                    .as_bool()
                    .ok_or_else(|| ApiError::bad_request("MCP 工具开关无效。"))?;
                if !enabled {
                    exposure.insert(tool.clone(), json!("hidden"));
                } else {
                    exposure.entry(tool.clone()).or_insert(json!("codemode"));
                }
            }
            config["toolExposure"] = Value::Object(exposure);
        }
        validate_mcp_server_config(name, &ordered(&config)?)
            .map_err(|_| ApiError::bad_request("MCP 连接配置无效，原文件未修改。"))?;
        result.insert(name.into(), config);
    }
    // 调用历史和缓存工具描述不属于连接配置，仍留在原文件，不能当作当前在线状态。
    Ok(Value::Object(result))
}

pub(super) fn load(path: &Path) -> LoadedMcpConfig {
    let result = (|| -> Result<Vec<McpServerEntry>, ApiError> {
        let projected = project(&read(path)?)?;
        projected
            .as_object()
            .expect("projected map")
            .iter()
            .map(|(id, value)| {
                Ok(McpServerEntry {
                    name: id.clone(),
                    config: validate_mcp_server_config(id, &ordered(value)?)
                        .map_err(|_| ApiError::bad_request("MCP 配置无效。"))?,
                    source: path.to_string_lossy().into_owned(),
                    scope: Some(McpConfigScope::Global),
                    override_: None,
                })
            })
            .collect()
    })();
    match result {
        Ok(servers) => LoadedMcpConfig {
            servers,
            ..Default::default()
        },
        Err(error) => LoadedMcpConfig {
            errors: vec![error.message],
            ..Default::default()
        },
    }
}

fn edit(
    path: &Path,
    action: impl FnOnce(&mut Vec<Value>) -> Result<(), ApiError>,
) -> Result<(), ApiError> {
    let _write = WRITE
        .lock()
        .map_err(|_| ApiError::internal("MCP 配置锁不可用。"))?;
    let mut value = read(path)?;
    action(value["servers"].as_array_mut().expect("validated array"))?;
    project(&value)?;
    crate::config_storage::write_object(path, &value)
}

pub(super) fn add(path: &Path, definitions: &[(String, Value)]) -> Result<(), ApiError> {
    edit(path, |servers| {
        for (id, config) in definitions {
            if servers.iter().any(|server| server["id"] == *id) {
                return Err(ApiError::bad_request("MCP 服务已经存在。"));
            }
            validate_mcp_server_config(id, &ordered(config)?)
                .map_err(|_| ApiError::bad_request("MCP 连接配置无效。"))?;
            let mut server = json!({"id":id,"name":id,"transport":if config.get("command").is_some(){"stdio"}else{"http"},"enabled":config.get("enabled").cloned().unwrap_or(json!(true)),"requestTimeoutMs":config.get("timeout").and_then(Value::as_f64).unwrap_or(60.0)*1000.0,"toolStates":{},"tools":[],"createdAt":chrono::Utc::now().to_rfc3339(),"updatedAt":chrono::Utc::now().to_rfc3339()});
            for field in [
                "command", "args", "cwd", "env", "url", "headers", "exposure",
            ] {
                if let Some(value) = config.get(field) {
                    server[field] = value.clone();
                }
            }
            if let Some(exposure) = config.get("toolExposure").and_then(Value::as_object) {
                for (name, value) in exposure {
                    server["toolStates"][name] = json!(value != "hidden");
                }
            }
            for (key, value) in config.as_object().expect("validated object") {
                if !["timeout", "enabled", "id", "name"].contains(&key.as_str()) {
                    server[key] = value.clone();
                }
            }
            servers.push(server);
        }
        Ok(())
    })
}
pub(super) fn update(path: &Path, id: &str, patch: McpServerConfigPatch) -> Result<(), ApiError> {
    edit(path, |servers| {
        let server = servers
            .iter_mut()
            .find(|server| server["id"] == id)
            .ok_or_else(|| ApiError::bad_request("MCP 服务不存在。"))?;
        if let Some(enabled) = patch.enabled {
            server["enabled"] = json!(enabled);
        }
        if let Some(exposure) = patch.exposure {
            server["exposure"] = json!(super::exposure_name(exposure));
        }
        server["updatedAt"] = json!(chrono::Utc::now().to_rfc3339());
        Ok(())
    })
}
pub(super) fn set_tool(path: &Path, id: &str, tool: &str, enabled: bool) -> Result<(), ApiError> {
    edit(path, |servers| {
        let server = servers
            .iter_mut()
            .find(|server| server["id"] == id)
            .ok_or_else(|| ApiError::bad_request("MCP 服务不存在。"))?;
        if !server["toolStates"].is_object() {
            server["toolStates"] = json!({});
        }
        server["toolStates"][tool] = json!(enabled);
        if !server["toolExposure"].is_object() {
            server["toolExposure"] = json!({});
        }
        let exposure = server["exposure"]
            .as_str()
            .filter(|value| *value != "hidden")
            .unwrap_or("codemode")
            .to_owned();
        server["toolExposure"][tool] = json!(if enabled { exposure.as_str() } else { "hidden" });
        Ok(())
    })
}
pub(super) fn remove(path: &Path, id: &str) -> Result<(), ApiError> {
    edit(path, |servers| {
        servers.retain(|server| server["id"] != id);
        Ok(())
    })
}

pub(super) fn options(
    path: PathBuf,
) -> pi_rust::coding_agent::extensions::mcp::McpExtensionOptions {
    let source = path.clone();
    pi_rust::coding_agent::extensions::mcp::McpExtensionOptions {
        load_config: Some(Arc::new(move |_| load(&source))),
        update_config: Some(Arc::new(move |entry, patch| {
            update(&path, &entry.name, patch).map_err(|error| error.message)
        })),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Value {
        json!({"version":1,"future":"keep","servers":[{"id":"original-id","name":"Original name","transport":"stdio","command":"fixture","args":[],"cwd":"","env":{"SECRET":"synthetic"},"enabled":false,"requestTimeoutMs":12500,"toolStates":{"danger":false},"tools":[{"name":"cached"}],"custom":"keep"}],"calls":[{"id":"old-call","status":"ok"}]})
    }
    #[test]
    fn original_document_is_read_directly_and_all_edits_remain_in_original_format() {
        let root = std::env::temp_dir().join(format!("pisper-mcp-direct-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("pisper-mcp.json");
        let original = serde_json::to_vec(&fixture()).unwrap();
        std::fs::write(&path, &original).unwrap();
        std::fs::write(
            root.join("mcp.json"),
            br#"{"mcpServers":{"wrong":{"command":"wrong"}}}"#,
        )
        .unwrap();
        let loaded = load(&path);
        assert!(loaded.errors.is_empty());
        assert_eq!(loaded.servers.len(), 1);
        assert_eq!(loaded.servers[0].name, "original-id");
        assert!(!loaded.servers[0].config.enabled());
        assert_eq!(loaded.servers[0].config.timeout(), Some(12.5));
        assert!(loaded.servers[0].config.cwd().is_none());
        assert_eq!(
            std::fs::read(&path).unwrap(),
            original,
            "Reading must not migrate or rewrite"
        );
        update(
            &path,
            "original-id",
            McpServerConfigPatch {
                enabled: Some(true),
                exposure: None,
            },
        )
        .unwrap();
        set_tool(&path, "original-id", "danger", true).unwrap();
        add(&path,&[("new-id".into(),json!({"url":"https://example.invalid/mcp","headers":{"Authorization":"synthetic"},"fixtureUnknown":true,"exposure":"direct"}))]).unwrap();
        let saved = read(&path).unwrap();
        assert_eq!(saved["version"], 1);
        assert_eq!(saved["future"], "keep");
        assert_eq!(saved["calls"], fixture()["calls"]);
        assert_eq!(saved["servers"][0]["custom"], "keep");
        assert_eq!(saved["servers"][0]["toolStates"]["danger"], true);
        assert_eq!(saved["servers"][1]["fixtureUnknown"], true);
        assert!(saved.get("mcpServers").is_none());
        assert_eq!(load(&path).servers.len(), 2);
        remove(&path, "new-id").unwrap();
        assert_eq!(load(&path).servers.len(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn missing_and_invalid_files_do_not_create_a_replacement_config() {
        let root = std::env::temp_dir().join(format!(
            "pisper-mcp-direct-invalid-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("pisper-mcp.json");
        assert!(load(&path).servers.is_empty());
        assert!(!path.exists());
        std::fs::write(&path, b"broken").unwrap();
        assert!(!load(&path).errors.is_empty());
        assert!(add(&path, &[("x".into(), json!({"command":"fixture"}))]).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"broken");
        assert!(!root.join("mcp.json").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
