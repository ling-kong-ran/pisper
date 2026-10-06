//! 渠道私有状态和公开投影；模板内容沿用已有通知契约，凭据只保留在私有连接。
use super::super::{ChannelError, Resource, Result};
use crate::native_notifications::templates;
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Map, Value};
use std::cmp::Ordering;

pub(super) const PLATFORMS: [&str; 4] = ["feishu", "weixin", "qq", "telegram"];
pub(super) fn truthy(value: &Value) -> bool {
    templates::truthy(value)
}
pub(super) fn string(value: &Value) -> String {
    templates::js_string(value)
}
pub(super) fn text(value: &Value) -> String {
    if truthy(value) {
        string(value)
    } else {
        String::new()
    }
}
pub(super) fn trim(value: &str) -> &str {
    value.trim_matches(|value| matches!(value, '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'))
}
pub(super) fn clip(value: &str, count: usize) -> String {
    templates::clip(value, count)
}
pub(super) fn first(values: &[&Value]) -> Value {
    values
        .iter()
        .find(|value| truthy(value))
        .map(|value| (*value).clone())
        .unwrap_or(Value::Null)
}
pub(super) fn fallback(value: &Value, default: &str) -> Value {
    if truthy(value) {
        value.clone()
    } else {
        json!(default)
    }
}
pub(super) fn mode(value: &Value) -> &'static str {
    match text(value).as_str() {
        "workspace" | "workspace-write" => "workspace-write",
        "full-access" => "full-access",
        _ => "approval-required",
    }
}
pub(super) fn run_mode(value: &Value) -> &'static str {
    match value.as_str() {
        Some("goal") => "goal",
        Some("team") => "team",
        _ => "plan",
    }
}
pub(super) fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
pub(super) fn check_platform(platform: &str) -> Result<()> {
    if PLATFORMS.contains(&platform) {
        Ok(())
    } else {
        Err(ChannelError::new("不支持这个渠道。"))
    }
}
pub(super) fn default_templates() -> Value {
    normalize_templates(&Value::Null)
}
pub(super) fn normalize_templates(stored: &Value) -> Value {
    let definitions = templates::definitions();
    let mut result = json!({});
    for event in templates::EVENTS {
        let value = &stored[event];
        let mut variants = json!({});
        for platform in ["feishu", "weixin", "qq", "telegram", "browser"] {
            let variant = &value["channels"][platform];
            let content = if (variant.is_object() || variant.is_array())
                && !trim(&text(&variant["content"])).is_empty()
            {
                clip(&string(&variant["content"]), 12_000)
            } else {
                definitions[event]["defaultContent"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned()
            };
            variants[platform] = json!({"content":content});
        }
        result[event] = json!({"enabled":value["enabled"]!=false,"channels":variants});
    }
    result
}
fn private_templates(stored: &Value) -> Value {
    let normalized = normalize_templates(stored);
    let mut result = stored.as_object().cloned().unwrap_or_default();
    // 通知和渠道共用规范写入者；未知扩展只保留在私有存储，旧 targets 不再生效。
    for template in result.values_mut() {
        if let Some(channels) = template.get_mut("channels").and_then(Value::as_object_mut) {
            for variant in channels.values_mut() {
                if let Some(variant) = variant.as_object_mut() {
                    variant.remove("targets");
                }
            }
        }
    }
    for event in templates::EVENTS {
        let mut template = result
            .get(event)
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        template.insert("enabled".into(), normalized[event]["enabled"].clone());
        let mut channels = template
            .get("channels")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        for platform in ["feishu", "weixin", "qq", "telegram", "browser"] {
            let mut variant = stored[event]["channels"][platform]
                .as_object()
                .cloned()
                .unwrap_or_default();
            variant.remove("targets");
            variant.insert(
                "content".into(),
                normalized[event]["channels"][platform]["content"].clone(),
            );
            channels.insert(platform.into(), Value::Object(variant));
        }
        template.insert("channels".into(), Value::Object(channels));
        result.insert(event.into(), Value::Object(template));
    }
    Value::Object(result)
}
pub(super) fn normalize(stored: &Value) -> Value {
    let mut result = stored.as_object().cloned().unwrap_or_default();
    // 版本1的单向 webhook 已废弃，不能保留历史凭据或继续连接。
    let version = stored["version"].as_u64();
    if !matches!(version, Some(2..=5)) {
        result = Map::new();
    }
    result.remove("connection");
    result.remove("channels");
    result.insert("version".into(), json!(5));
    let mut connections = json!({"feishu":null,"weixin":null,"qq":null,"telegram":null});
    let mut scopes = json!({});
    let mut notification_templates = default_templates();
    if matches!(version, Some(3..=5)) {
        for platform in PLATFORMS {
            let connection = &stored["connections"][platform];
            if !(matches!(platform, "qq" | "telegram") && connection["mode"] == "personal")
                && truthy(connection)
            {
                connections[platform] = connection.clone();
            }
        }
        if stored["scopes"].is_object() || stored["scopes"].is_array() {
            scopes = stored["scopes"].clone();
        }
        notification_templates = private_templates(&stored["templates"]);
    } else if version == Some(2) {
        if truthy(&stored["connection"]) {
            connections["feishu"] = stored["connection"].clone();
        }
        for (peer, scope) in entries(&stored["scopes"]) {
            let mut scope = spread(&scope);
            scope.insert("platform".into(), json!("feishu"));
            scope.insert("peerId".into(), json!(peer));
            scopes[format!("feishu:{peer}")] = Value::Object(scope);
        }
    }
    result.insert("connections".into(), connections);
    result.insert("scopes".into(), scopes);
    result.insert("templates".into(), notification_templates);
    Value::Object(result)
}
pub(super) fn entries(value: &Value) -> Vec<(String, Value)> {
    match value {
        Value::Object(values) => values
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        Value::Array(values) => values
            .iter()
            .enumerate()
            .map(|(key, value)| (key.to_string(), value.clone()))
            .collect(),
        _ => Vec::new(),
    }
}
pub(super) fn spread(value: &Value) -> Map<String, Value> {
    match value {
        Value::Object(values) => values.clone(),
        Value::Array(values) => values
            .iter()
            .enumerate()
            .map(|(index, value)| (index.to_string(), value.clone()))
            .collect(),
        Value::String(value) => value
            .encode_utf16()
            .enumerate()
            .map(|(index, unit)| (index.to_string(), json!(String::from_utf16_lossy(&[unit]))))
            .collect(),
        _ => Map::new(),
    }
}
pub(super) fn scopes_mut(value: &mut Value) -> Result<&mut Map<String, Value>> {
    // JS 对 scopes 数组使用字符串属性；落盘时原数组会丢失这些属性。规范写入
    // 使用对象表示已有索引及新增作用域，避免丢掉真实会话归属。
    if !value["scopes"].is_object() {
        value["scopes"] = Value::Object(spread(&value["scopes"]));
    }
    value["scopes"]
        .as_object_mut()
        .ok_or_else(|| ChannelError::new("渠道作用域存储无效。"))
}
pub(super) fn merge_scope(value: &mut Value, key: &str, patch: Map<String, Value>) -> Result<()> {
    let scopes = scopes_mut(value)?;
    let mut scope = spread(scopes.get(key).unwrap_or(&Value::Null));
    scope.extend(patch);
    scopes.insert(key.into(), Value::Object(scope));
    Ok(())
}
pub(super) fn owner_key(platform: &str) -> &'static str {
    if platform == "feishu" {
        "ownerOpenId"
    } else {
        "ownerUserId"
    }
}
fn masked(value: &Value) -> String {
    let value = text(value);
    let units = value.encode_utf16().collect::<Vec<_>>();
    if units.len() > 10 {
        format!(
            "{}••••{}",
            String::from_utf16_lossy(&units[..7]),
            String::from_utf16_lossy(&units[units.len() - 4..])
        )
    } else {
        value
    }
}
pub(super) fn connection(platform: &str, connection: &Value, live: &Value, cwd: &str) -> Value {
    if !truthy(connection) {
        return Value::Null;
    }
    let bot = &live["bot"];
    let name = match platform {
        "feishu" => fallback(&bot["name"], "Pisper Agent"),
        "weixin" => json!("微信机器人"),
        "qq" => first(&[&bot["username"], &bot["nickname"], &json!("QQ 官方机器人")]),
        _ => first(&[&bot["username"], &bot["name"], &json!("Telegram Bot")]),
    };
    let account = match platform {
        "feishu" | "qq" => connection["appId"].clone(),
        "telegram" => first(&[&bot["id"], &connection["accountId"]]),
        _ => connection["accountId"].clone(),
    };
    json!({"id":platform,"type":platform,"name":first(&[&connection["name"],&name]),"enabled":connection["enabled"]!=false,
        "accountId":masked(&account),"accessMode":fallback(&connection["accessMode"],"owner"),"defaultCwd":fallback(&connection["defaultCwd"],cwd),"replyModel":first(&[&connection["replyModel"]]),
        "executionMode":mode(&connection["executionMode"]),"runMode":run_mode(&connection["runMode"]),"ownerConfigured":truthy(&connection[owner_key(platform)]),
        "bot":first(&[bot]),"status":live["state"],"lastError":fallback(&live["lastError"],""),"connectedAt":first(&[&live["connectedAt"]]),"lastEventAt":first(&[&live["lastEventAt"]])})
}
pub(super) fn scope(key: &str, value: &Value) -> Value {
    json!({"key":key,"platform":value["platform"],"peerId":value["peerId"],"chatType":fallback(&value["chatType"],"p2p"),"sessionId":fallback(&value["sessionId"],""),
        "title":first(&[&value["title"],&value["peerId"]]),"cwd":fallback(&value["cwd"],""),"model":fallback(&value["model"],""),"executionMode":mode(&value["executionMode"]),"runMode":run_mode(&value["runMode"]),"lastMessage":fallback(&value["lastMessage"],""),"updatedAt":first(&[&value["updatedAt"]])})
}
pub(super) fn timestamp(value: &Value) -> Option<i64> {
    if !truthy(value) {
        Some(0)
    } else if let Some(value) = value.as_i64() {
        Some(value)
    } else {
        value
            .as_str()
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.timestamp_millis())
    }
}
pub(super) fn scope_order(left: &Value, right: &Value) -> Ordering {
    match (
        timestamp(&left["updatedAt"]),
        timestamp(&right["updatedAt"]),
    ) {
        (Some(left), Some(right)) => right.cmp(&left),
        _ => Ordering::Equal,
    }
}
pub(super) fn latest_scope(state: &Value, platform: &str) -> Option<Value> {
    let mut latest: Option<Value> = None;
    for (_, scope) in entries(&state["scopes"]) {
        if scope["platform"] != platform {
            continue;
        }
        if latest.as_ref().is_none_or(|previous| {
            match (
                timestamp(&scope["updatedAt"]),
                timestamp(&previous["updatedAt"]),
            ) {
                (Some(next), Some(last)) => next >= last,
                _ => false,
            }
        }) {
            latest = Some(scope);
        }
    }
    latest
}
pub(super) fn providers() -> Value {
    json!([
    {"type":"feishu","name":"飞书应用机器人","description":"官方扫码创建，WebSocket 长连接，支持私聊与群聊 @"},
    {"type":"weixin","name":"微信","description":"腾讯 iLink Bot 扫码登录，支持个人微信私聊与媒体消息"},
    {"type":"qq","name":"QQ 官方机器人","description":"腾讯 QQ Bot Connector 扫码创建，使用官方 OpenAPI 与 WebSocket 接入","accessMode":"qr"},
    {"type":"telegram","name":"Telegram Bot","description":"使用 Telegram Bot API 长轮询接入","accessMode":"manual"}])
}
pub(super) fn attachment(resource: Resource) -> Option<Value> {
    let extension = std::path::Path::new(&resource.name)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_lowercase();
    if resource.kind == "image"
        || ["png", "jpg", "jpeg", "gif", "webp", "bmp"].contains(&extension.as_str())
    {
        let mime = resource
            .mime_type
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| {
                match extension.as_str() {
                    "jpg" | "jpeg" => "image/jpeg",
                    "webp" => "image/webp",
                    "gif" => "image/gif",
                    _ => "image/png",
                }
                .into()
            });
        Some(
            json!({"kind":"image","name":resource.name,"mimeType":mime,"size":resource.bytes.len(),"data":STANDARD.encode(resource.bytes)}),
        )
    } else if [
        "txt", "md", "json", "js", "jsx", "ts", "tsx", "css", "html", "xml", "yaml", "yml", "csv",
        "log", "py", "java", "go", "rs", "sh", "ps1", "toml", "sql",
    ]
    .contains(&extension.as_str())
    {
        Some(
            json!({"kind":"text","name":resource.name,"size":resource.bytes.len(),"text":String::from_utf8_lossy(&resource.bytes)}),
        )
    } else if [
        "pdf", "docx", "pptx", "xlsx", "odt", "odp", "ods", "rtf", "epub",
    ]
    .contains(&extension.as_str())
    {
        Some(
            json!({"kind":"document","name":resource.name,"size":resource.bytes.len(),"extension":format!(".{extension}"),"data":STANDARD.encode(resource.bytes)}),
        )
    } else {
        None
    }
}
pub(super) fn catalog(value: &Value) -> Value {
    templates::catalog(&normalize_templates(value))
}
pub(super) fn render(state: &Value, event: &str, platform: &str, data: &Value) -> Result<Value> {
    if !templates::EVENTS.contains(&event)
        || !["feishu", "weixin", "qq", "telegram", "browser"].contains(&platform)
    {
        return Err(ChannelError::new("通知模板不存在。"));
    }
    let definitions = templates::definitions();
    Ok(
        json!({"title":definitions[event]["name"],"content":templates::render(&text(&state["templates"][event]["channels"][platform]["content"]),data)}),
    )
}
pub(super) fn sample() -> Value {
    templates::sample()
}
