//! 首次升级承接 Node 的 pisper-mcp.json；原文件保留，mcp.json 存在后不重复导入。
use super::{ordered, validate_mcp_server_config, ApiError};
use serde_json::{json, Map, Value};
use std::path::Path;

fn convert(value: &Value) -> Result<Value, ApiError> {
    if value["version"] != 1 {
        return Err(ApiError::bad_request(
            "旧版 MCP 配置版本无效，原文件未修改。",
        ));
    }
    let servers = value["servers"]
        .as_array()
        .ok_or_else(|| ApiError::bad_request("旧版 MCP servers 格式无效。"))?;
    let mut result = Map::new();
    for server in servers {
        let display = server["name"].as_str().unwrap_or("");
        // 保留旧 HTTP 服务 ID；名称单独显示，避免已有选择和引用失效。
        let name = server["id"]
            .as_str()
            .filter(|name| !name.is_empty())
            .ok_or_else(|| ApiError::bad_request("旧版 MCP 服务缺少有效标识。"))?;
        if result.contains_key(name) {
            return Err(ApiError::bad_request("旧版 MCP 服务标识重复。"));
        }
        if server["transport"] == "sse" {
            return Err(ApiError::bad_request("旧版 MCP 使用 SSE；当前 Rust 引擎不支持此传输，请先改用 Streamable HTTP。原文件未修改。"));
        }
        let mut config = json!({"enabled":server.get("enabled").cloned().unwrap_or(json!(true)),"exposure":"codemode", "pisperLegacyId":server["id"], "pisperLegacyName":display});
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
                .ok_or_else(|| ApiError::bad_request("旧版 MCP 超时设置无效。"))?;
            config["timeout"] = json!(timeout / 1000.0);
        }
        if let Some(states) = server.get("toolStates") {
            let states = states
                .as_object()
                .ok_or_else(|| ApiError::bad_request("旧版 MCP 工具设置无效。"))?;
            let mut exposure = Map::new();
            for (tool, enabled) in states {
                let enabled = enabled
                    .as_bool()
                    .ok_or_else(|| ApiError::bad_request("旧版 MCP 工具开关无效。"))?;
                exposure.insert(
                    tool.clone(),
                    json!(if enabled { "codemode" } else { "hidden" }),
                );
            }
            config["toolExposure"] = Value::Object(exposure);
        }
        validate_mcp_server_config(name, &ordered(&config)?)
            .map_err(|_| ApiError::bad_request("旧版 MCP 连接配置无效，原文件未修改。"))?;
        result.insert(name.into(), config);
    }
    // 调用历史和缓存工具描述不属于连接配置，仍留在原文件，不能当作当前在线状态。
    Ok(json!({"mcpServers":result,"pisperMigration":{"source":"pisper-mcp.json","version":1}}))
}

pub(crate) fn restore_legacy_config(agent_dir: &str, data_dir: &str) -> Result<bool, ApiError> {
    let target = Path::new(agent_dir).join("mcp.json");
    match std::fs::symlink_metadata(&target) {
        Ok(_) => return Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(ApiError::internal("无法检查 MCP 配置文件。")),
    }
    let source = Path::new(data_dir).join("pisper-mcp.json");
    match std::fs::symlink_metadata(&source) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => return Err(ApiError::bad_request("旧版 MCP 配置必须是常规文件。")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err(ApiError::internal("无法检查旧版 MCP 配置。")),
    }
    let bytes = match (|| -> std::io::Result<Vec<u8>> {
        use std::io::Read;
        let mut bytes = Vec::new();
        std::fs::File::open(&source)?
            .take(8 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        Ok(bytes)
    })() {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err(ApiError::internal("无法读取旧版 MCP 配置。")),
    };
    if bytes.len() > 8 * 1024 * 1024 {
        return Err(ApiError::bad_request("旧版 MCP 配置超过迁移大小限制。"));
    }
    let input: Value = serde_json::from_slice(&bytes)
        .map_err(|_| ApiError::bad_request("旧版 MCP JSON 损坏，原文件未修改。"))?;
    crate::config_storage::create_object(&target, &convert(&input)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Value {
        json!({"version":1,"servers":[{"id":"one","name":"local","transport":"stdio","command":"fixture-command","args":["--fixture"],"cwd":"fixture-dir","env":{"KEY":"synthetic"},"enabled":false,"requestTimeoutMs":12500,"toolStates":{"danger":false,"read":true}}, {"id":"two","name":"remote","transport":"http","url":"https://example.invalid/mcp","headers":{"Authorization":"synthetic"},"enabled":true}],"calls":[{"private":"history"}],"future":true})
    }
    #[test]
    fn legacy_connections_preserve_auth_disabled_tools_timeout_and_duplicate_display_names() {
        let mut input = fixture();
        input["servers"][1]["name"] = json!("local");
        let output = convert(&input).unwrap();
        assert_eq!(output["mcpServers"]["one"]["enabled"], false);
        assert_eq!(output["mcpServers"]["one"]["timeout"], 12.5);
        assert_eq!(
            output["mcpServers"]["one"]["toolExposure"]["danger"],
            "hidden"
        );
        assert_eq!(output["mcpServers"]["one"]["env"]["KEY"], "synthetic");
        assert_eq!(
            output["mcpServers"]["two"]["headers"]["Authorization"],
            "synthetic"
        );
        assert!(output.get("calls").is_none());
    }
    #[test]
    fn migration_is_atomic_private_repeatable_and_does_not_resurrect_deleted_servers() {
        let dir = std::env::temp_dir().join(format!("pisper-mcp-upgrade-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.to_str().unwrap();
        let original = serde_json::to_vec(&fixture()).unwrap();
        std::fs::write(dir.join("pisper-mcp.json"), &original).unwrap();
        assert!(restore_legacy_config(path, path).unwrap());
        let generated = std::fs::read(dir.join("mcp.json")).unwrap();
        assert!(!restore_legacy_config(path, path).unwrap());
        assert_eq!(std::fs::read(dir.join("mcp.json")).unwrap(), generated);
        assert_eq!(
            std::fs::read(dir.join("pisper-mcp.json")).unwrap(),
            original
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(dir.join("mcp.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        std::fs::write(dir.join("mcp.json"), b"{\"mcpServers\":{}}").unwrap();
        assert!(!restore_legacy_config(path, path).unwrap());
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn damaged_and_unsupported_sources_never_publish_partial_configuration() {
        let dir = std::env::temp_dir().join(format!("pisper-mcp-invalid-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.to_str().unwrap();
        assert!(!restore_legacy_config(path, path).unwrap());
        for input in [b"broken".to_vec(),serde_json::to_vec(&json!({"version":2,"servers":[]})).unwrap(),serde_json::to_vec(&json!({"version":1,"servers":[{"id":"s","name":"s","transport":"sse","url":"https://example.invalid"}]})).unwrap()] {
            std::fs::write(dir.join("pisper-mcp.json"),&input).unwrap();
            assert!(restore_legacy_config(path,path).is_err());
            assert!(!dir.join("mcp.json").exists());
            assert_eq!(std::fs::read(dir.join("pisper-mcp.json")).unwrap(),input);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
