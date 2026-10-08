use super::{config::unique_strings, PluginError, Result, MANIFEST_FILE, MAX_MANIFEST_BYTES};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    fs,
    path::{Component, Path},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolManifest {
    pub name: String,
    pub label: String,
    pub description: String,
    pub scope: String,
    pub parameters: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub schema_version: u8,
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub entry: String,
    pub permissions: Vec<String>,
    pub tools: Vec<ToolManifest>,
}
pub(crate) fn js_string(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(v) => v.clone(),
        Value::Bool(false) => String::new(),
        Value::Number(v) if v.as_f64() == Some(0.0) => String::new(),
        other => value_string(other),
    }
}
pub(crate) fn value_string(value: &Value) -> String {
    match value {
        Value::String(v) => v.clone(),
        Value::Array(items) => items
            .iter()
            .map(|v| {
                if v.is_null() {
                    String::new()
                } else {
                    value_string(v)
                }
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".into(),
        other => other.to_string(),
    }
}
fn sliced(value: &str, limit: usize) -> String {
    let mut length = 0;
    value
        .chars()
        .take_while(|c| {
            length += c.len_utf16();
            length <= limit
        })
        .collect()
}
pub(crate) fn valid_id(id: &str) -> bool {
    (1..=96).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b".-".contains(&b))
        && id.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
        && id.as_bytes().last().is_some_and(u8::is_ascii_alphanumeric)
}
fn valid_tool(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name.as_bytes()[0].is_ascii_lowercase()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}
pub(crate) fn relative_path(value: &Value, field: &str) -> Result<String> {
    let normalized = js_string(value).replace('\\', "/");
    if normalized.is_empty()
        || Path::new(&normalized).is_absolute()
        || Path::new(&normalized).components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::Prefix(_) | Component::RootDir
            )
        })
        || normalized.contains('\0')
    {
        return Err(PluginError::new(format!(
            "{field} 必须是插件目录内的相对路径。"
        )));
    }
    Ok(normalized)
}
pub(crate) fn normalize(raw: &Value) -> Result<Manifest> {
    if !raw.is_object() {
        return Err(PluginError::new("插件清单必须是 JSON 对象。"));
    }
    let schema = match &raw["schemaVersion"] {
        Value::Null | Value::Bool(false) => Some(1.0),
        Value::Number(v) if v.as_f64() == Some(0.0) => Some(1.0),
        Value::String(s) if s.is_empty() => Some(1.0),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        Value::Bool(true) => Some(1.0),
        Value::Number(v) => v.as_f64(),
        _ => None,
    };
    if schema != Some(1.0) {
        return Err(PluginError::new(format!(
            "不支持插件清单版本 {}。",
            js_string(&raw["schemaVersion"])
        )));
    }
    let id = js_string(&raw["id"]).trim().to_owned();
    if !valid_id(&id) {
        return Err(PluginError::new(
            "插件 id 必须为 1-96 位小写字母、数字、点或连字符。",
        ));
    }
    let name = js_string(&raw["name"]).trim().to_owned();
    if name.is_empty() || name.encode_utf16().count() > 100 {
        return Err(PluginError::new("插件 name 必须为 1-100 个字符。"));
    }
    let version = js_string(&raw["version"]).trim().to_owned();
    if !regex::Regex::new(r"^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$")
        .expect("version regex")
        .is_match(&version)
    {
        return Err(PluginError::new(
            "插件 version 必须是有效的语义版本，例如 1.0.0。",
        ));
    }
    let entry = relative_path(&raw["entry"], "entry")?;
    if !Path::new(&entry)
        .extension()
        .and_then(|v| v.to_str())
        .is_some_and(|v| ["js", "mjs", "cjs"].contains(&v.to_ascii_lowercase().as_str()))
    {
        return Err(PluginError::new(
            "插件 entry 仅支持 .js、.mjs 或 .cjs 文件。",
        ));
    }
    let mut tools = Vec::new();
    let mut seen = HashSet::new();
    for (index, tool) in raw["tools"].as_array().into_iter().flatten().enumerate() {
        if !tool.is_object() {
            return Err(PluginError::new(format!("tools[{index}] 必须是对象。")));
        }
        let name = js_string(&tool["name"]).trim().to_owned();
        if !valid_tool(&name) {
            return Err(PluginError::new(format!(
                "工具名称 {} 无效；必须以小写字母开头，且只能包含小写字母、数字和下划线。",
                if name.is_empty() {
                    format!("(index {index})")
                } else {
                    name.clone()
                }
            )));
        }
        if !seen.insert(name.clone()) {
            return Err(PluginError::new("插件工具名称不能重复。"));
        }
        let description = js_string(&tool["description"]).trim().to_owned();
        if description.is_empty() {
            return Err(PluginError::new(format!("工具 {name} 缺少 description。")));
        }
        let parameters = if tool["parameters"].is_object() {
            tool["parameters"].clone()
        } else {
            json!({"type":"object","properties":{}})
        };
        if parameters["type"] != "object" {
            return Err(PluginError::new(format!(
                "工具 {name} 的 parameters.type 必须为 object。"
            )));
        }
        if !parameters["properties"].is_null() && !parameters["properties"].is_object() {
            return Err(PluginError::new(format!(
                "工具 {name} 的 parameters.properties 必须为对象。"
            )));
        }
        if !parameters["required"].is_null()
            && !parameters["required"]
                .as_array()
                .is_some_and(|v| v.iter().all(Value::is_string))
        {
            return Err(PluginError::new(format!(
                "工具 {name} 的 parameters.required 必须为字符串数组。"
            )));
        }
        jsonschema::validator_for(&parameters).map_err(|_| {
            PluginError::new(format!(
                "工具 {name} 的 parameters 不是有效的 JSON Schema。"
            ))
        })?;
        let label = if js_string(&tool["label"]).is_empty() {
            name.clone()
        } else {
            js_string(&tool["label"])
        };
        let scope = if js_string(&tool["scope"]).is_empty() {
            description.clone()
        } else {
            js_string(&tool["scope"])
        };
        tools.push(ToolManifest {
            name,
            label: sliced(label.trim(), 100),
            description: sliced(&description, 1000),
            scope: sliced(scope.trim(), 500),
            parameters,
        });
    }
    if tools.is_empty() || tools.len() > 32 {
        return Err(PluginError::new("插件必须声明 1-32 个工具。"));
    }
    Ok(Manifest {
        schema_version: 1,
        id,
        name,
        version,
        entry,
        tools,
        description: sliced(js_string(&raw["description"]).trim(), 1000),
        permissions: unique_strings(&raw["permissions"])
            .into_iter()
            .take(32)
            .collect(),
    })
}
pub(crate) fn read(root: &Path) -> Result<Manifest> {
    let path = root.join(MANIFEST_FILE);
    let metadata = fs::symlink_metadata(&path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            PluginError::new(format!("目录中缺少 {MANIFEST_FILE}。"))
        } else {
            error.into()
        }
    })?;
    if super::store::is_link(&metadata) || !metadata.is_file() {
        return Err(PluginError::new(format!(
            "{MANIFEST_FILE} 必须是普通文件。"
        )));
    }
    if metadata.len() > MAX_MANIFEST_BYTES as u64 {
        return Err(PluginError::new(format!(
            "{MANIFEST_FILE} 超过 256 KB 限制。"
        )));
    }
    let bytes = fs::read(path)?;
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(PluginError::new(format!(
            "{MANIFEST_FILE} 超过 256 KB 限制。"
        )));
    }
    let raw: Value = serde_json::from_slice(&bytes)
        .map_err(|_| PluginError::new(format!("{MANIFEST_FILE} 不是有效的 JSON。")))?;
    normalize(&raw)
}
