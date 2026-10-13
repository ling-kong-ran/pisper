//! Security layer: secret redaction (ported from the Node runtime's
//! `runtime/security/secret-redaction.mjs`) plus the remote device-pairing
//! store. Redaction keeps API keys / tokens / passwords from leaking through
//! error messages, logs, and model input; pairing lets a remote client
//! exchange a short-lived code for a device bearer token.

use serde::{Deserialize, Serialize};
use std::sync::Mutex;

pub const REDACTED_SECRET: &str = "[REDACTED SECRET]";

const ALWAYS_SENSITIVE_KEYS: [&str; 14] = [
    "apikey",
    "appsecret",
    "authorization",
    "authtoken",
    "clientsecret",
    "cookie",
    "credential",
    "credentials",
    "password",
    "passwd",
    "refreshtoken",
    "secret",
    "setcookie",
    "accesstoken",
];

const KV_KEYS: [&str; 24] = [
    "apikey",
    "api-key",
    "api_key",
    "accesstoken",
    "access-token",
    "access_token",
    "refreshtoken",
    "refresh-token",
    "refresh_token",
    "authtoken",
    "auth-token",
    "auth_token",
    "clientsecret",
    "client-secret",
    "client_secret",
    "appsecret",
    "app-secret",
    "app_secret",
    "password",
    "passwd",
    "authorization",
    "credentials",
    "credential",
    "secret",
];

const VENDOR_KEY_PREFIXES: [&str; 11] = [
    "sk-",
    "rk-",
    "pk-",
    "pcl-",
    "ghp_",
    "github_pat-",
    "xoxb-",
    "xoxa-",
    "xoxp-",
    "xoxr-",
    "xoxs-",
];

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Upstream `looksLikeSecret`: bearer prefixes, JWT shape, vendor key
/// prefixes, or a >=20-char space-free mixed token.
pub fn looks_like_secret(value: &str) -> bool {
    let text = value.trim();
    if text.is_empty() || text == REDACTED_SECRET {
        return false;
    }
    let lower = text.to_lowercase();
    if lower.starts_with("bearer ") {
        return true;
    }
    let parts: Vec<&str> = text.split('.').collect();
    if parts.len() == 3
        && lower.starts_with("eyj")
        && parts.iter().all(|p| {
            p.len() >= 8
                && p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        })
    {
        return true;
    }
    for prefix in VENDOR_KEY_PREFIXES {
        if let Some(rest) = lower.strip_prefix(prefix) {
            if rest.len() >= 12
                && rest
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                return true;
            }
        }
    }
    text.len() >= 20
        && !text.contains(' ')
        && text.chars().any(|c| c.is_ascii_alphabetic())
        && text
            .chars()
            .any(|c| c.is_ascii_digit() || !c.is_ascii_alphanumeric())
}

/// Normalized-key sensitive check: the always-sensitive set, a sensitive
/// suffix, or a generic `token` key whose value looks like a secret.
pub fn sensitive_key(key: &str, content: Option<&str>) -> bool {
    let normalized: String = key
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    if normalized == "token" {
        return content.map(looks_like_secret).unwrap_or(false);
    }
    ALWAYS_SENSITIVE_KEYS.contains(&normalized.as_str())
        || [
            "apikey",
            "secret",
            "password",
            "passwd",
            "authorization",
            "credential",
            "accesstoken",
            "refreshtoken",
            "authtoken",
        ]
        .iter()
        .any(|suffix| normalized.ends_with(suffix))
}

fn redact_pem_blocks(text: &str) -> String {
    let mut result = String::new();
    let mut rest = text;
    loop {
        let Some(begin) = rest.find("-----BEGIN") else {
            result.push_str(rest);
            break;
        };
        if !rest[begin..].contains("PRIVATE KEY-----") {
            result.push_str(&rest[..begin + 10]);
            rest = &rest[begin + 10..];
            continue;
        }
        let Some(end_rel) = rest[begin..].find("-----END") else {
            result.push_str(rest);
            break;
        };
        let after_end = rest[begin + end_rel..]
            .find('\n')
            .map(|i| begin + end_rel + i + 1)
            .unwrap_or(rest.len());
        result.push_str(&rest[..begin]);
        result.push_str(REDACTED_SECRET);
        rest = &rest[after_end..];
    }
    result
}

fn is_secret_header(name: &str) -> bool {
    matches!(
        name.trim().to_lowercase().as_str(),
        "authorization" | "proxy-authorization" | "cookie" | "x-api-key"
    )
}

fn redact_header_lines(text: &str) -> String {
    text.split_inclusive('\n')
        .map(|line| {
            let trimmed = line.trim();
            let name = trimmed
                .split(':')
                .next()
                .unwrap_or("")
                .trim()
                .to_lowercase();
            if trimmed.contains(':')
                && is_secret_header(&name)
                && !trimmed.ends_with(REDACTED_SECRET)
            {
                let (header, _) = line.split_once(':').unwrap_or((line, ""));
                format!("{header}:{REDACTED_SECRET}")
            } else {
                line.to_string()
            }
        })
        .collect()
}

fn redact_bearer(text: &str) -> String {
    let mut result = String::new();
    let mut rest = text;
    loop {
        let Some(pos) = rest.to_lowercase().find("bearer ") else {
            result.push_str(rest);
            break;
        };
        let after = &rest[pos + "bearer ".len()..];
        let token_len = after
            .chars()
            .take_while(|c| {
                c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '~' | '+' | '/' | '-')
            })
            .map(|c| c.len_utf8())
            .sum();
        result.push_str(&rest[..pos + "bearer ".len()]);
        if token_len >= 8 {
            result.push_str(REDACTED_SECRET);
            rest = &after[token_len..];
        } else {
            result.push_str(&after[..token_len]);
            rest = &after[token_len..];
        }
    }
    result
}

fn redact_db_urls(text: &str) -> String {
    let mut out = text.to_string();
    let schemes = [
        "postgresql://",
        "postgres://",
        "mysql://",
        "mariadb://",
        "mongodb+srv://",
        "mongodb://",
    ];
    for scheme in schemes {
        while let Some(pos) = out.find(scheme) {
            let end: usize = out[pos..]
                .chars()
                .take_while(|c| !c.is_whitespace())
                .map(|c| c.len_utf8())
                .sum();
            out.replace_range(pos..pos + end, REDACTED_SECRET);
        }
    }
    out
}

fn value_end(text: &str) -> usize {
    text.chars()
        .take_while(|c| !matches!(c, ' ' | ',' | ';' | '}' | ']' | '\n' | '\r'))
        .map(|c| c.len_utf8())
        .sum()
}

const GENERIC_TOKEN: &str = "token";

fn redact_kv_pairs(text: &str) -> String {
    let mut out = text.to_string();
    let mut keys: Vec<&str> = KV_KEYS.to_vec();
    keys.push(GENERIC_TOKEN);
    keys.sort_by_key(|k| std::cmp::Reverse(k.len()));
    for key in keys {
        let mut search_from = 0usize;
        loop {
            let lower = out.to_lowercase();
            let Some(rel) = lower[search_from..].find(key) else {
                break;
            };
            let key_start = search_from + rel;
            let key_end = key_start + key.len();
            let boundary_ok = key_start == 0
                || !out[..key_start]
                    .chars()
                    .next_back()
                    .map(is_word_char)
                    .unwrap_or(false);
            let after_key = &out[key_end..];
            let value_start = key_end + (after_key.len() - after_key.trim_start().len());
            let Some(sep) = out[value_start..].chars().next() else {
                break;
            };
            if !boundary_ok || (sep != '=' && sep != ':') {
                search_from = key_end;
                continue;
            }
            let after_sep = &out[value_start + 1..];
            let value_start = value_start + 1 + (after_sep.len() - after_sep.trim_start().len());
            let value = &out[value_start..];
            if value.starts_with(REDACTED_SECRET) {
                search_from = value_start + REDACTED_SECRET.len();
                continue;
            }
            let quoted = value.starts_with('"') || value.starts_with('\'');
            let quote = if quoted {
                value.chars().next().unwrap()
            } else {
                ' '
            };
            let inner_len = if quoted {
                value[1..]
                    .find(quote)
                    .unwrap_or(value.len().saturating_sub(2))
            } else {
                value_end(value)
            };
            if inner_len == 0 {
                break;
            }
            let inner = if quoted {
                &value[1..1 + inner_len]
            } else {
                &value[..inner_len]
            };
            let replace_start = value_start + if quoted { 1 } else { 0 };
            if key == GENERIC_TOKEN && !looks_like_secret(inner) {
                search_from = replace_start + inner_len;
                continue;
            }
            out.replace_range(replace_start..replace_start + inner_len, REDACTED_SECRET);
            search_from = replace_start + REDACTED_SECRET.len();
        }
    }
    out
}

fn redact_query_secrets(text: &str) -> String {
    let mut out = text.to_string();
    let keys = [
        "access_token",
        "refresh_token",
        "auth_token",
        "api_key",
        "apikey",
        "token",
        "secret",
        "password",
        "auth",
        "credential",
        "key",
    ];
    for key in keys {
        for marker in [format!("?{key}="), format!("&{key}=")] {
            let mut search_from = 0usize;
            loop {
                let Some(rel) = out[search_from..].find(&marker) else {
                    break;
                };
                let eq = search_from + rel + marker.len();
                let value_len: usize = out[eq..]
                    .chars()
                    .take_while(|c| !matches!(c, '&' | '#' | ' ' | '\n' | '\r'))
                    .map(|c| c.len_utf8())
                    .sum();
                if value_len == 0 {
                    search_from = eq;
                    continue;
                }
                out.replace_range(eq..eq + value_len, REDACTED_SECRET);
                search_from = eq + REDACTED_SECRET.len();
            }
        }
    }
    out
}

fn redact_bare_secrets(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    loop {
        let token_end = rest
            .char_indices()
            .find(|(_, c)| {
                c.is_whitespace() || matches!(c, '"' | ',' | ';' | ')' | '(' | '}' | ']' | '=')
            })
            .map(|(i, _)| i)
            .unwrap_or(rest.len());
        let (word, tail) = rest.split_at(token_end);
        let lower = word.to_lowercase();
        let parts: Vec<&str> = word.split('.').collect();
        let is_jwt = lower.starts_with("eyj")
            && parts.len() == 3
            && parts.iter().all(|p| {
                p.len() >= 8
                    && p.chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            });
        let is_vendor_key = VENDOR_KEY_PREFIXES
            .iter()
            .any(|prefix| lower.starts_with(prefix) && word.len() >= prefix.len() + 12);
        if is_jwt || is_vendor_key {
            out.push_str(REDACTED_SECRET);
        } else {
            out.push_str(word);
        }
        let delim_len = tail
            .chars()
            .take_while(|c| {
                c.is_whitespace() || matches!(c, '"' | ',' | ';' | ')' | '(' | '}' | ']' | '=')
            })
            .map(|c| c.len_utf8())
            .sum();
        out.push_str(&tail[..delim_len]);
        rest = &tail[delim_len..];
        if rest.is_empty() {
            break;
        }
    }
    out
}

/// Redact obviously secret-bearing values in free text: PEM blocks,
/// sensitive header lines, bearer tokens, DB connection URLs, key=value
/// pairs, URL query secrets, bare JWTs and vendor-prefixed keys.
pub fn redact_secret_text(value: &str) -> String {
    let out = redact_pem_blocks(value);
    let out = redact_header_lines(&out);
    let out = redact_bearer(&out);
    let out = redact_db_urls(&out);
    let out = redact_kv_pairs(&out);
    let out = redact_query_secrets(&out);
    redact_bare_secrets(&out)
}

pub fn contains_secret_text(value: &str) -> bool {
    redact_secret_text(value) != value
}

/// Recursively redact a JSON value: strings through `redact_secret_text`,
/// object values under sensitive keys replaced whole, arrays walked.
pub fn redact_secret_value(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::String(text) => serde_json::Value::String(redact_secret_text(text)),
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(redact_secret_value).collect())
        }
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.iter()
                .map(|(key, val)| {
                    if sensitive_key(key, val.as_str()) {
                        (
                            key.clone(),
                            serde_json::Value::String(REDACTED_SECRET.into()),
                        )
                    } else {
                        (key.clone(), redact_secret_value(val))
                    }
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

// ------------------------------------------------------------ pairing store

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairedDevice {
    pub id: String,
    pub name: String,
    pub token_hash: String,
    pub created_at: String,
    pub last_seen_at: String,
    pub revoked_at: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Default)]
pub struct PairingStore {
    /// 待兑换配对码的 SHA-256 与失效时间；只保留一个有效码。
    pub pending: Mutex<Option<(String, u64)>>,
    pub devices: Mutex<Vec<PairedDevice>>,
}

// 直接读写 Node 的唯一配对文件，保留未知字段；从不落盘明文设备令牌。
static REMOTE_WRITE: Mutex<()> = Mutex::new(());

fn remote_document(data_dir: &str) -> Result<serde_json::Value, String> {
    let path = std::path::Path::new(data_dir).join("remote-access.json");
    match std::fs::read(path) {
        Ok(bytes) => {
            let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| {
                "Invalid remote-access.json; existing data was preserved".to_string()
            })?;
            if value.get("version").and_then(serde_json::Value::as_u64) != Some(1)
                || !value
                    .get("devices")
                    .is_some_and(serde_json::Value::is_array)
            {
                return Err(
                    "Invalid remote-access.json schema; existing data was preserved".into(),
                );
            }
            Ok(value)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(serde_json::json!({"version":1,"enabled":false,"devices":[],"pairingCode":null}))
        }
        Err(_) => Err("Unable to read remote-access.json".into()),
    }
}

fn update_remote(
    data_dir: &str,
    update: impl FnOnce(&mut serde_json::Value),
) -> Result<(), String> {
    use std::io::Write;
    let _guard = REMOTE_WRITE
        .lock()
        .map_err(|_| "Remote storage lock failed")?;
    let mut document = remote_document(data_dir)?;
    update(&mut document);
    let directory = std::path::Path::new(data_dir);
    let path = directory.join("remote-access.json");
    let temporary = directory.join(format!("remote-access.json.{}.tmp", uuid::Uuid::new_v4()));
    let bytes =
        serde_json::to_vec_pretty(&document).map_err(|_| "Remote storage encoding failed")?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| -> std::io::Result<()> {
        let mut file = options.open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, &path)
    })();
    if result.is_err() && temporary.exists() {
        std::fs::remove_file(&temporary).map_err(|_| "Remote storage cleanup failed")?;
    }
    result.map_err(|_| "Unable to write remote-access.json".into())
}

pub fn load_devices(data_dir: &str) -> Result<Vec<PairedDevice>, String> {
    serde_json::from_value(remote_document(data_dir)?["devices"].clone())
        .map_err(|_| "Invalid remote device records; existing data was preserved".into())
}

pub fn load_pairing_code(data_dir: &str) -> Result<Option<(String, u64)>, String> {
    let document = remote_document(data_dir)?;
    let value = &document["pairingCode"];
    if value.is_null() {
        return Ok(None);
    }
    let invalid = || "Invalid pairing code record; existing data was preserved".to_string();
    let hash = value["codeHash"]
        .as_str()
        .filter(|hash| hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or_else(invalid)?;
    let expires = value["expiresAt"]
        .as_str()
        .and_then(crate::session_ops::parse_timestamp_ms)
        .ok_or_else(invalid)?;
    Ok(Some((hash.to_string(), expires)))
}

pub fn save_pairing_code(data_dir: &str, hash: &str, expires: u64) -> Result<(), String> {
    update_remote(data_dir, |document| {
        document["pairingCode"] = serde_json::json!({
            "codeHash": hash, "expiresAt": crate::session_ops::iso_timestamp(expires)
        });
    })
}

pub fn redeem_pairing_code(data_dir: &str, devices: &[PairedDevice]) -> Result<(), String> {
    let value = serde_json::to_value(devices).map_err(|_| "Remote device encoding failed")?;
    // 设备登记与一次性码消费必须在原文件的一次原子写入中提交。
    update_remote(data_dir, |document| {
        document["devices"] = value;
        document["pairingCode"] = serde_json::Value::Null;
    })
}

pub fn save_devices(data_dir: &str, devices: &[PairedDevice]) -> Result<(), String> {
    let value = serde_json::to_value(devices).map_err(|_| "Remote device encoding failed")?;
    update_remote(data_dir, |document| document["devices"] = value)
}

pub(crate) fn revoke_device(
    data_dir: &str,
    devices: &mut Vec<PairedDevice>,
    id: &str,
) -> Result<(), crate::ApiError> {
    let mut next = devices.clone();
    let device = next
        .iter_mut()
        .find(|device| device.id == id && device.revoked_at.is_none())
        .ok_or_else(|| {
            crate::ApiError::new(
                axum::http::StatusCode::NOT_FOUND,
                "device_not_found",
                "设备不存在或已被吊销。",
            )
        })?;
    device.revoked_at = Some(crate::session_ops::iso_timestamp(crate::product::now_ms()));
    save_devices(data_dir, &next).map_err(crate::ApiError::internal)?;
    *devices = next;
    Ok(())
}

pub fn load_remote_enabled(data_dir: &str) -> Result<bool, String> {
    Ok(remote_document(data_dir)?["enabled"]
        .as_bool()
        .unwrap_or(false))
}

pub fn save_remote_enabled(data_dir: &str, enabled: bool) -> Result<(), String> {
    update_remote(data_dir, |document| {
        document["enabled"] = enabled.into();
        if !enabled {
            document["pairingCode"] = serde_json::Value::Null;
        }
    })
}

impl PairedDevice {
    pub fn new(id: String, name: String, token: &str, now: u64) -> Self {
        use sha2::{Digest, Sha256};
        Self {
            id,
            name,
            token_hash: Sha256::digest(token.as_bytes())
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
            created_at: crate::session_ops::iso_timestamp(now),
            last_seen_at: crate::session_ops::iso_timestamp(now),
            revoked_at: None,
            extra: Default::default(),
        }
    }
}

/// release remote-access-service 的配对申请（桌面端审批流）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingApproval {
    pub request_id: String,
    /// 申请方出示的 X-Pisper-Pairing-Secret，审批状态轮询凭它防枚举。
    pub secret: String,
    pub device_name: String,
    pub ip: String,
    pub created_at: u64,
    pub expires_at: u64,
    /// pending | approved | denied
    pub status: String,
    /// 批准时签发的设备凭据（请求方通过 GET 领取）。
    pub device_id: Option<String>,
    pub token: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_remote_store_survives_read_pair_revoke_and_restart() {
        let root = std::env::temp_dir().join(format!("pisper-remote-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let directory = root.to_str().unwrap();
        let path = root.join("remote-access.json");
        let original = serde_json::json!({"version":1,"enabled":true,"pairingCode":null,
            "futureSetting":{"keep":true},"devices":[{"id":"old-device","name":"旧手机",
            "tokenHash":"old-hash","createdAt":"2026-01-01T00:00:00.000Z",
            "lastSeenAt":"2026-01-02T00:00:00.000Z","revokedAt":null,"futureField":42}]});
        std::fs::write(&path, serde_json::to_vec(&original).unwrap()).unwrap();
        let mut devices = load_devices(directory).unwrap();
        assert!(load_remote_enabled(directory).unwrap());
        let code_hash = crate::remote_access::hash("ABCD1234");
        save_pairing_code(directory, &code_hash, 1_900_000_000_000).unwrap();
        assert_eq!(
            load_pairing_code(directory).unwrap(),
            Some((code_hash, 1_900_000_000_000))
        );
        devices.push(PairedDevice::new(
            "new-device".into(),
            "新手机".into(),
            "fixture-secret",
            0,
        ));
        redeem_pairing_code(directory, &devices).unwrap();
        assert_eq!(load_pairing_code(directory).unwrap(), None);
        revoke_device(directory, &mut devices, "old-device").unwrap();
        save_remote_enabled(directory, false).unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["futureSetting"], original["futureSetting"]);
        assert_eq!(saved["devices"][0]["futureField"], 42);
        assert_eq!(saved["devices"][0]["tokenHash"], "old-hash");
        assert!(saved["devices"][0]["revokedAt"].is_string());
        assert!(!saved.to_string().contains("fixture-secret"));
        assert_eq!(load_devices(directory).unwrap().len(), 2);
        assert!(!load_remote_enabled(directory).unwrap());
        assert!(!root.join("devices.json").exists());
        assert!(!root.join("pairing-requests.json").exists());
        std::fs::write(&path, b"{invalid").unwrap();
        assert!(load_devices(directory).is_err());
        assert!(save_devices(directory, &devices).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"{invalid");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn vendor_prefixed_keys_are_redacted() {
        let text = "my key is sk-1cat-bd1faee5117ee96347238efdb86daa65930d9342dc3f0a79 ok";
        assert_eq!(
            redact_secret_text(text),
            format!("my key is {REDACTED_SECRET} ok")
        );
        assert!(contains_secret_text(text));
    }

    #[test]
    fn bearer_tokens_are_redacted() {
        let text = "Authorization: Bearer abcdef1234567890";
        assert!(redact_secret_text(text).contains(REDACTED_SECRET));
        assert!(!contains_secret_text(&redact_secret_text(text)));
    }

    #[test]
    fn key_value_pairs_are_redacted() {
        let text = "password=hunter2secret\napi_key: sk-abcdefgh12345678";
        let redacted = redact_secret_text(text);
        assert!(!redacted.contains("hunter2secret"), "{redacted}");
        assert!(!redacted.contains("sk-abcdefgh12345678"), "{redacted}");
    }

    #[test]
    fn sensitive_object_keys_redact_whole_values() {
        let value = serde_json::json!({ "apiKey": "sk-abcdefgh12345678", "note": "safe" });
        let out = redact_secret_value(&value);
        assert_eq!(out["apiKey"], serde_json::json!(REDACTED_SECRET));
        assert_eq!(out["note"], serde_json::json!("safe"));
    }

    #[test]
    fn jwt_is_redacted() {
        let text = "token eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U end";
        assert!(redact_secret_text(text).contains(REDACTED_SECRET));
    }

    #[test]
    fn plain_words_survive() {
        let text = "The quick brown fox jumps over 123 lazy dogs";
        assert_eq!(redact_secret_text(text), text);
        assert!(!contains_secret_text(text));
    }
}
