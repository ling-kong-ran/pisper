use super::{templates, Result, PLATFORMS};
use crate::ApiError;
use serde_json::{json, Value};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Component, Path},
};
const MAX_JSON_BYTES: u64 = 16 * 1024 * 1024;

pub(crate) fn check_path(path: &Path) -> Result<()> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .map_err(|_| ApiError::internal("通知存储目录无效。"))?
            .join(path)
    };
    let mut current = std::path::PathBuf::new();
    for part in absolute.components() {
        current.push(part.as_os_str());
        if matches!(part, Component::Prefix(_)) {
            continue;
        }
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                #[cfg(windows)]
                let reparse = {
                    use std::os::windows::fs::MetadataExt;
                    metadata.file_attributes() & 0x400 != 0
                };
                #[cfg(not(windows))]
                let reparse = false;
                if metadata.file_type().is_symlink() || reparse {
                    return Err(ApiError::internal("通知存储路径不能是链接。"));
                }
                if current != absolute && !metadata.is_dir() {
                    return Err(ApiError::internal("通知存储上级路径无效。"));
                }
            }
            Err(error)
                if [
                    std::io::ErrorKind::NotFound,
                    std::io::ErrorKind::NotADirectory,
                ]
                .contains(&error.kind()) => {}
            Err(_) => return Err(ApiError::internal("通知存储路径无法访问。")),
        }
    }
    Ok(())
}
pub(crate) fn read_json(path: &Path, fallback: Value) -> Result<Value> {
    check_path(path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error)
            if [
                std::io::ErrorKind::NotFound,
                std::io::ErrorKind::NotADirectory,
            ]
            .contains(&error.kind()) =>
        {
            return Ok(fallback)
        }
        Err(_) => return Err(ApiError::internal("通知记录无法读取。")),
    };
    let before = file
        .metadata()
        .map_err(|_| ApiError::internal("通知记录无法读取。"))?;
    if !before.is_file() || before.len() > MAX_JSON_BYTES {
        return Err(ApiError::internal("通知记录过大或类型无效。"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_JSON_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ApiError::internal("通知记录无法读取。"))?;
    check_path(path)?;
    if bytes.len() as u64 > MAX_JSON_BYTES {
        return Err(ApiError::internal("通知记录过大。"));
    }
    serde_json::from_slice(&bytes).map_err(|_| ApiError::internal("通知记录 JSON 无效。"))
}
pub(crate) fn write_json(path: &Path, value: &Value) -> Result<()> {
    check_path(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| ApiError::internal("通知存储目录无效。"))?;
    fs::create_dir_all(parent).map_err(|_| ApiError::internal("通知存储目录无法创建。"))?;
    check_path(path)?;
    let mut bytes =
        serde_json::to_vec_pretty(value).map_err(|_| ApiError::internal("通知记录无法序列化。"))?;
    bytes.push(b'\n');
    if bytes.len() as u64 > MAX_JSON_BYTES {
        return Err(ApiError::internal("通知记录过大。"));
    }
    let temporary = parent.join(format!(".notification-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .map_err(|_| ApiError::internal("通知记录无法写入。"))?;
        file.write_all(&bytes)
            .map_err(|_| ApiError::internal("通知记录无法写入。"))?;
        file.sync_all()
            .map_err(|_| ApiError::internal("通知记录无法保存。"))?;
        drop(file);
        check_path(path)?;
        fs::rename(&temporary, path).map_err(|_| ApiError::internal("通知记录无法原子替换。"))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}
pub(crate) fn normalized_channels(stored: &Value) -> Value {
    let mut result = json!({"version":5,"connections":{"feishu":null,"weixin":null,"qq":null,"telegram":null},"scopes":{},"templates":templates::normalized(&json!({}))});
    match stored["version"].as_u64() {
        Some(3..=5) => {
            for platform in &PLATFORMS[..4] {
                let connection = &stored["connections"][platform];
                if !(["qq", "telegram"].contains(platform) && connection["mode"] == "personal") {
                    result["connections"][platform] = connection.clone();
                }
            }
            if stored["scopes"].is_object() {
                result["scopes"] = stored["scopes"].clone();
            }
            result["templates"] = templates::normalized(&stored["templates"]);
        }
        Some(2) => {
            result["connections"]["feishu"] = stored["connection"].clone();
            if let Some(scopes) = stored["scopes"].as_object() {
                for (peer, scope) in scopes {
                    if scope.is_object() {
                        let mut value = scope.clone();
                        value["platform"] = json!("feishu");
                        value["peerId"] = json!(peer);
                        result["scopes"][format!("feishu:{peer}")] = value;
                    }
                }
            }
        }
        _ => {}
    }
    result
}
fn text<'a>(value: &'a Value, key: &str, fallback: &'a str) -> &'a str {
    value[key]
        .as_str()
        .filter(|v| !v.is_empty())
        .unwrap_or(fallback)
}
fn execution_mode(value: &Value) -> &str {
    match value.as_str() {
        Some("workspace") => "workspace-write",
        Some("workspace-write") => "workspace-write",
        Some("full-access") => "full-access",
        _ => "approval-required",
    }
}
fn run_mode(value: &Value) -> &str {
    match value.as_str() {
        Some("goal") => "goal",
        Some("team") => "team",
        _ => "plan",
    }
}
pub(crate) fn timestamp(value: &Value) -> Option<i64> {
    if value.is_null() {
        Some(0)
    } else if let Some(n) = value.as_i64() {
        Some(n)
    } else {
        value
            .as_str()
            .and_then(|text| chrono::DateTime::parse_from_rfc3339(text).ok())
            .map(|date| date.timestamp_millis())
    }
}
pub(crate) fn latest_scope(channels: &Value, platform: &str) -> Option<Value> {
    let mut latest = None::<Value>;
    for scope in channels["scopes"].as_object()?.values() {
        if scope["platform"] != platform {
            continue;
        }
        if latest.as_ref().is_none_or(|previous| {
            match (
                timestamp(&scope["updatedAt"]),
                timestamp(&previous["updatedAt"]),
            ) {
                (Some(a), Some(b)) => a >= b,
                _ => false,
            }
        }) {
            latest = Some(scope.clone());
        }
    }
    latest
}
pub(crate) fn public_state(
    channels: &Value,
    app: &Value,
    transport: Option<&dyn super::NotificationTransport>,
) -> Value {
    let mut connections = json!({});
    for platform in &PLATFORMS[..4] {
        let connection = &channels["connections"][platform];
        let supported = transport.is_some_and(|transport| transport.supports(platform));
        if connection.is_null() {
            connections[platform] = Value::Null;
            continue;
        }
        let default_name = match *platform {
            "feishu" => "Pisper Agent",
            "weixin" => "微信机器人",
            "qq" => "QQ 官方机器人",
            _ => "Telegram Bot",
        };
        let owner_key = if *platform == "feishu" {
            "ownerOpenId"
        } else {
            "ownerUserId"
        };
        let account_key = if ["feishu", "qq"].contains(platform) {
            "appId"
        } else {
            "accountId"
        };
        let id = templates::js_string(&if templates::truthy(&connection[account_key]) {
            connection[account_key].clone()
        } else {
            json!("")
        });
        let masked = if id.encode_utf16().count() > 10 {
            format!(
                "{}••••{}",
                templates::clip(&id, 7),
                String::from_utf16_lossy(
                    &id.encode_utf16()
                        .skip(id.encode_utf16().count() - 4)
                        .collect::<Vec<_>>()
                )
            )
        } else {
            id
        };
        connections[platform] = json!({"id":platform,"type":platform,"name":text(connection,"name",default_name),"enabled":connection["enabled"]!=false,"accountId":masked,"accessMode":text(connection,"accessMode","owner"),"defaultCwd":text(connection,"defaultCwd",""),"replyModel":connection["replyModel"],"executionMode":execution_mode(&connection["executionMode"]),"runMode":run_mode(&connection["runMode"]),"ownerConfigured":templates::truthy(&connection[owner_key]),"bot":null,"status":"disconnected","lastError":if supported{""}else{"原生通知渠道发送服务尚未接入。"},"connectedAt":null,"lastEventAt":null,"supported":supported});
    }
    let mut scopes = Vec::new();
    if let Some(items) = channels["scopes"].as_object() {
        for (key, scope) in items {
            let peer = text(scope, "peerId", "");
            scopes.push(json!({"key":key,"platform":scope["platform"],"peerId":peer,"chatType":text(scope,"chatType","p2p"),"sessionId":text(scope,"sessionId",""),"title":text(scope,"title",peer),"cwd":text(scope,"cwd",""),"model":text(scope,"model",""),"executionMode":execution_mode(&scope["executionMode"]),"runMode":run_mode(&scope["runMode"]),"lastMessage":text(scope,"lastMessage",""),"updatedAt":scope["updatedAt"]}));
        }
    }
    scopes.sort_by(|a, b| {
        timestamp(&b["updatedAt"])
            .unwrap_or(0)
            .cmp(&timestamp(&a["updatedAt"]).unwrap_or(0))
    });
    let supported = PLATFORMS
        .iter()
        .filter(|platform| {
            **platform == "browser"
                || transport.is_some_and(|transport| transport.supports(platform))
        })
        .collect::<Vec<_>>();
    let unsupported = PLATFORMS[..4]
        .iter()
        .filter(|platform| !transport.is_some_and(|transport| transport.supports(platform)))
        .collect::<Vec<_>>();
    json!({"providers":[{"type":"feishu","name":"飞书应用机器人","description":"官方扫码创建，WebSocket 长连接，支持私聊与群聊 @"},{"type":"weixin","name":"微信","description":"腾讯 iLink Bot 扫码登录，支持个人微信私聊与媒体消息"},{"type":"qq","name":"QQ 官方机器人","description":"腾讯 QQ Bot Connector 扫码创建，使用官方 OpenAPI 与 WebSocket 接入","accessMode":"qr"},{"type":"telegram","name":"Telegram Bot","description":"使用 Telegram Bot API 长轮询接入","accessMode":"manual"}],"connections":connections,"scopes":scopes,"templates":templates::catalog(&channels["templates"]),"browser":{"enabled":super::browser_enabled(app)},"supportedChannels":supported,"unsupportedChannels":unsupported})
}
