use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;

pub type Result<T> = std::result::Result<T, CustomUiError>;

#[derive(Debug, Clone)]
pub struct CustomUiError {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
}
impl CustomUiError {
    pub fn new(status: u16, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }
    pub(crate) fn archive(code: &'static str) -> Self {
        Self::new(400, code, code)
    }
    pub(crate) fn missing_asset() -> Self {
        Self::new(404, "component_asset_missing", "组件资源不存在。")
    }
    pub(crate) fn expired_view() -> Self {
        Self::new(404, "component_view_expired", "组件预览不存在或已过期。")
    }
    pub(crate) fn io(_error: impl fmt::Display) -> Self {
        // 不把宿主绝对路径或用户文件名放入 HTTP 错误。
        Self::new(500, "component_storage_failed", "组件文件操作失败。")
    }
}
impl fmt::Display for CustomUiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for CustomUiError {}

pub const PERMISSIONS: &[&str] = &[
    "config.read",
    "sessions.read",
    "game-assets.read",
    "game-assets.write",
    "game-assets.run",
    "notify",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub entry: String,
    pub permissions: Vec<String>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Component {
    #[serde(flatten)]
    pub manifest: Manifest,
    pub entry_url: String,
    pub directory: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub built_in: Option<bool>,
}
#[derive(Debug, Clone)]
pub struct View {
    pub component_id: String,
    pub owner: String,
    pub origin: String,
    pub expires_at: i64,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewGrant {
    pub id: String,
    pub entry_url: String,
}

pub(crate) fn valid_component_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && (id.as_bytes()[0].is_ascii_lowercase() || id.as_bytes()[0].is_ascii_digit())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
}
pub(crate) fn valid_view_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn string(value: &Value) -> String {
    match value {
        Value::Null | Value::Bool(false) => String::new(),
        Value::Number(n) if n.as_f64() == Some(0.0) => String::new(),
        Value::String(s) => s.clone(),
        Value::Array(a) => a
            .iter()
            .map(|v| match v {
                Value::Null => String::new(),
                Value::Object(_) => "[object Object]".into(),
                Value::Array(_) => string(v),
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".into(),
        other => other.to_string(),
    }
}
fn truncate_utf16(s: &str, max: usize) -> String {
    let mut units = 0;
    s.chars()
        .take_while(|c| {
            units += c.len_utf16();
            units <= max
        })
        .collect()
}
pub(crate) fn normalize_manifest(id: &str, raw: &Value) -> Result<Manifest> {
    if !valid_component_id(id) || !raw.is_object() {
        return Err(CustomUiError::archive("component_manifest_invalid"));
    }
    let name = string(&raw["name"]).trim().to_owned();
    if name.is_empty() || name.encode_utf16().count() > 120 {
        return Err(CustomUiError::archive("component_manifest_invalid"));
    }
    let entry = string(&raw["entry"]).trim().to_owned();
    let entry = if entry.is_empty() {
        "index.html".to_owned()
    } else {
        entry
    };
    if entry.contains("..")
        || entry.starts_with(['/', '\\'])
        || std::path::Path::new(&entry).is_absolute()
        || entry.contains(':')
        || entry.contains('\0')
    {
        return Err(CustomUiError::archive("component_manifest_invalid"));
    }
    let mut permissions = Vec::new();
    for permission in raw["permissions"].as_array().into_iter().flatten() {
        let value = string(permission).trim().to_owned();
        if PERMISSIONS.contains(&value.as_str()) && !permissions.contains(&value) {
            permissions.push(value);
        }
    }
    Ok(Manifest {
        id: id.into(),
        name,
        entry,
        permissions,
        version: truncate_utf16(string(&raw["version"]).trim(), 64),
        description: truncate_utf16(string(&raw["description"]).trim(), 500),
    })
}

pub(crate) fn encode_path(path: &str) -> String {
    path.split('/')
        .map(|part| {
            let mut output = String::new();
            for byte in part.bytes() {
                if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
                    output.push(byte as char);
                } else {
                    use std::fmt::Write;
                    let _ = write!(output, "%{byte:02X}");
                }
            }
            output
        })
        .collect::<Vec<_>>()
        .join("/")
}
