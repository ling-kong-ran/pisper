//! 会话消息资产归属规则与 release session-assets.mjs 保持同一线协议。
use serde_json::{json, Value};
use std::collections::HashSet;
fn string<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or("")
}
pub fn identity(attachment: &Value) -> String {
    let path = attachment["path"]
        .as_str()
        .filter(|path| !path.is_empty())
        .or_else(|| attachment["filePath"].as_str())
        .unwrap_or("")
        .trim()
        .replace('\\', "/")
        .to_lowercase();
    if !path.is_empty() {
        return format!("path:{path}");
    }
    let id = string(attachment, "id").trim();
    if id.is_empty() {
        String::new()
    } else {
        format!("id:{id}")
    }
}
pub fn dedupe(attachments: &[Value]) -> Vec<Value> {
    let mut seen = HashSet::new();
    attachments
        .iter()
        .filter(|item| {
            let key = identity(item);
            !key.is_empty() && seen.insert(key)
        })
        .cloned()
        .collect()
}
fn encode_id(value: &str) -> String {
    let mut result = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            result.push(byte as char)
        } else {
            result.push_str(&format!("%{byte:02X}"));
        }
    }
    result
}
pub fn attachment(asset: &Value) -> Value {
    let mime = string(asset, "mimeType");
    let id = encode_id(string(asset, "id"));
    let mut value = json!({"id":asset["id"],"kind":if mime.starts_with("image/"){ "image" }else if mime.starts_with("video/"){ "video" }else{ "file" },"name":asset["name"],"mimeType":mime,"size":asset["size"].as_u64().unwrap_or(0),"url":format!("/api/assets/{id}/download?inline=1"),"downloadUrl":format!("/api/assets/{id}/download")});
    if !string(asset, "filePath").is_empty() {
        value["path"] = asset["filePath"].clone();
    }
    value
}
fn timestamp(value: &Value) -> Option<f64> {
    if let Some(value) = value.as_f64() {
        return Some(value);
    }
    let value = value.as_str()?;
    if let Ok(number) = value.parse::<f64>() {
        if number != 0.0 {
            return Some(number);
        }
    }
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|date| date.timestamp_millis() as f64)
}
pub fn attach_generated_assets(messages: &[Value], assets: &[Value]) -> Vec<Value> {
    let mut result = messages.to_vec();
    for message in &mut result {
        message["attachments"] = json!(dedupe(
            message["attachments"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or(&[])
        ));
    }
    let agents = result
        .iter()
        .enumerate()
        .filter_map(|(index, message)| (message["role"] == "agent").then_some(index))
        .collect::<Vec<_>>();
    let Some(first) = agents.first() else {
        return result;
    };
    for asset in assets {
        let created = timestamp(
            asset
                .get("created")
                .filter(|value| !value.is_null() && value.as_str() != Some(""))
                .unwrap_or(&asset["modified"]),
        );
        let mut target = *first;
        for &index in &agents {
            if timestamp(&result[index]["timestamp"])
                .zip(created)
                .is_some_and(|(message, created)| message <= created)
            {
                target = index
            } else {
                break;
            }
        }
        let item = attachment(asset);
        let key = identity(&item);
        if let Some(attachments) = result[target]["attachments"].as_array_mut() {
            if !attachments.iter().any(|item| identity(item) == key) {
                attachments.push(item);
            }
        }
    }
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generated_assets_belong_to_the_prior_agent_turn_and_dedupe_canonical_paths() {
        let messages = vec![
            json!({"role":"agent","timestamp":1000,"attachments":[{"id":"old","path":"C:\\Fixture\\image.png"},{"id":"alias","path":"c:/fixture/image.png"}]}),
            json!({"role":"user","timestamp":1500}),
            json!({"role":"agent","timestamp":2000}),
        ];
        let assets = vec![
            json!({"id":"image id","name":"image.png","mimeType":"image/png","filePath":"c:/fixture/image.png","created":1800}),
            json!({"id":"late","name":"video.mp4","mimeType":"video/mp4","created":2500}),
        ];
        let result = attach_generated_assets(&messages, &assets);
        assert_eq!(result[0]["attachments"].as_array().unwrap().len(), 1);
        assert_eq!(result[2]["attachments"][0]["kind"], "video");
        assert_eq!(
            attachment(&assets[0])["url"],
            "/api/assets/image%20id/download?inline=1"
        );
        assert_eq!(result[1]["attachments"], json!([]));
    }
}
