//! 飞书消息的公开字段与内容标记，按已锁定 SDK 的转换器实现。
use crate::native_channels::{ChannelError, Result};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
fn text(value: &Value) -> String {
    match value {
        Value::String(v) => v.clone(),
        Value::Number(v) => v.to_string(),
        Value::Bool(v) => v.to_string(),
        _ => String::new(),
    }
}
fn attr(value: &Value) -> String {
    text(value).replace('"', "&quot;")
}
fn duration(value: &Value) -> String {
    let Some(ms) = value.as_f64().filter(|ms| *ms >= 0.) else {
        return String::new();
    };
    if ms < 1000. {
        format!("{}ms", ms.round())
    } else if ms % 1000. == 0. {
        format!("{}s", ms / 1000.)
    } else {
        let value = ms / 1000.;
        if value >= 1e21 {
            return format!(
                "{}s",
                serde_json::Number::from_f64(value)
                    .map(|v| v.to_string())
                    .unwrap_or_default()
            );
        }
        let bits = value.to_bits();
        let exponent = ((bits >> 52) & 0x7ff) as i32 - 1023 - 52;
        let mantissa = ((bits & ((1u64 << 52) - 1)) | (1u64 << 52)) as u128 * 10;
        let rounded = if exponent >= 0 {
            mantissa.checked_shl(exponent as u32).unwrap_or(u128::MAX)
        } else {
            let shift = (-exponent) as u32;
            if shift >= 128 {
                0
            } else {
                let quotient = mantissa >> shift;
                let remainder = mantissa & ((1u128 << shift) - 1);
                quotient + u128::from(remainder >= (1u128 << (shift - 1)))
            }
        };
        format!("{}.{}s", rounded / 10, rounded % 10)
    }
}
fn calendar(value: &Value) -> Vec<String> {
    let mut result = Vec::new();
    if let Some(summary) = value["summary"].as_str() {
        result.push(format!("📅 {summary}"))
    }
    let date = |value: &Value| {
        value
            .as_i64()
            .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
            .filter(|value| *value > 0)
            .and_then(|value| chrono::DateTime::from_timestamp_millis(value + 8 * 3600 * 1000))
            .map(|value| value.format("%Y-%m-%d %H:%M").to_string())
    };
    match (date(&value["start_time"]), date(&value["end_time"])) {
        (Some(start), Some(end)) => result.push(format!("🕙 {start} ~ {end}")),
        (Some(start), None) => result.push(format!("🕙 {start}")),
        _ => {}
    }
    result
}
fn card(node: &Value, result: &mut Vec<String>) {
    match node {
        Value::Array(items) => {
            for item in items {
                card(item, result)
            }
        }
        Value::Object(map) => {
            let tag = node["tag"].as_str().unwrap_or("");
            if matches!(tag, "plain_text" | "lark_md" | "markdown") {
                if let Some(content) = node["content"].as_str() {
                    result.push(content.into())
                }
                return;
            }
            if let Some(title) = node["header"].get("title") {
                card(title, result)
            }
            for key in ["text", "label", "placeholder"] {
                if let Some(child) = map.get(key) {
                    card(child, result)
                }
            }
            if tag == "button" {
                card(&node["text"], result)
            }
            for option in node["options"].as_array().into_iter().flatten() {
                card(&option["text"], result)
            }
            for key in ["elements", "fields", "actions", "columns", "body"] {
                if let Some(child) = map.get(key) {
                    card(child, result)
                }
            }
        }
        _ => {}
    }
}
fn render_element(
    el: &Value,
    mentions: &HashMap<String, Value>,
    resources: &mut Vec<Value>,
) -> String {
    match el["tag"].as_str().unwrap_or("") {
        "text" => {
            let mut result = text(&el["text"]);
            let styles = el["style"].as_array().map(Vec::as_slice).unwrap_or(&[]);
            for (kind, open, close) in [
                ("bold", "**", "**"),
                ("italic", "*", "*"),
                ("underline", "<u>", "</u>"),
                ("lineThrough", "~~", "~~"),
                ("codeInline", "`", "`"),
            ] {
                if styles.iter().any(|v| {
                    v == kind
                        || (kind == "lineThrough" && v == "strikethrough")
                        || (kind == "codeInline" && v == "code")
                }) {
                    result = format!("{open}{result}{close}")
                }
            }
            result
        }
        "a" => {
            let href = text(&el["href"]);
            let label = el["text"].as_str().unwrap_or(&href);
            if href.is_empty() {
                label.into()
            } else {
                format!("[{label}]({href})")
            }
        }
        "at" => {
            let id = text(&el["user_id"]);
            if matches!(id.as_str(), "all" | "all_members") {
                return "@all".into();
            }
            if let Some(info) = mentions.get(&id) {
                return text(&info["key"]);
            }
            format!("@{}", el["user_name"].as_str().unwrap_or(&id))
        }
        "img" => {
            let key = text(&el["image_key"]);
            if key.is_empty() {
                String::new()
            } else {
                resources.push(json!({"type":"image","fileKey":key}));
                format!("![image]({key})")
            }
        }
        "media" => {
            let key = text(&el["file_key"]);
            if key.is_empty() {
                String::new()
            } else {
                resources.push(json!({"type":"file","fileKey":key}));
                format!("<file key=\"{key}\"/>")
            }
        }
        "code_block" => format!(
            "\n```{}\n{}\n```\n",
            text(&el["language"]),
            text(&el["text"])
        ),
        "hr" => "\n---\n".into(),
        _ => text(&el["text"]),
    }
}
pub(super) fn content(
    kind: &str,
    raw: &str,
    mentions: &HashMap<String, Value>,
) -> (String, Vec<Value>) {
    let value = serde_json::from_str::<Value>(raw).unwrap_or(Value::Null);
    let mut resources = Vec::new();
    let content = match kind {
        "text" => text(&value["text"]),
        "post" => {
            let body = if value.get("title").is_some() || value.get("content").is_some() {
                &value
            } else {
                ["zh_cn", "en_us", "ja_jp"]
                    .iter()
                    .find_map(|locale| value.get(*locale).filter(|v| v.is_object()))
                    .or_else(|| value.as_object().and_then(|v| v.values().next()))
                    .unwrap_or(&Value::Null)
            };
            let mut lines = Vec::new();
            if let Some(title) = body["title"].as_str().filter(|v| !v.is_empty()) {
                lines.push(format!("**{title}**"));
                lines.push(String::new())
            }
            for paragraph in body["content"].as_array().into_iter().flatten() {
                if let Some(elements) = paragraph.as_array() {
                    lines.push(
                        elements
                            .iter()
                            .map(|el| render_element(el, mentions, &mut resources))
                            .collect::<String>(),
                    )
                }
            }
            let result = lines.join("\n").trim().to_owned();
            if result.is_empty() {
                "[rich text message]".into()
            } else {
                result
            }
        }
        "image" => {
            let key = text(&value["image_key"]);
            if key.is_empty() {
                "[image]".into()
            } else {
                resources.push(json!({"type":"image","fileKey":key}));
                format!("![image]({key})")
            }
        }
        "file" | "folder" | "audio" | "video" | "media" | "sticker" => {
            let key = text(&value["file_key"]);
            let kind = if kind == "media" { "video" } else { kind };
            if key.is_empty() {
                format!("[{kind}]")
            } else {
                let name = if matches!(kind, "file" | "folder" | "video")
                    && !text(&value["file_name"]).is_empty()
                {
                    format!(" name=\"{}\"", attr(&value["file_name"]))
                } else {
                    String::new()
                };
                let duration = if matches!(kind, "audio" | "video") {
                    duration(&value["duration"])
                } else {
                    String::new()
                };
                let duration = if duration.is_empty() {
                    duration
                } else {
                    format!(" duration=\"{duration}\"")
                };
                if kind != "folder" {
                    let mut resource = json!({"type":kind,"fileKey":key});
                    if value.get("file_name").is_some() {
                        resource["fileName"] = value["file_name"].clone()
                    }
                    if matches!(kind, "audio" | "video") && value.get("duration").is_some() {
                        resource["durationMs"] = value["duration"].clone()
                    }
                    if kind == "video" && value.get("image_key").is_some() {
                        resource["coverImageKey"] = value["image_key"].clone()
                    }
                    resources.push(resource)
                }
                format!("<{kind} key=\"{key}\"{name}{duration}/>")
            }
        }
        "share_chat" => format!("<group_card id=\"{}\"/>", text(&value["chat_id"])),
        "share_user" => format!("<contact_card id=\"{}\"/>", text(&value["user_id"])),
        "interactive" => {
            let mut pieces = Vec::new();
            card(&value, &mut pieces);
            let mut seen = HashSet::new();
            let pieces = pieces
                .into_iter()
                .filter_map(|piece| {
                    let piece = piece.trim().to_owned();
                    if !piece.is_empty() && seen.insert(piece.clone()) {
                        Some(piece)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
            if pieces.is_empty() {
                "[interactive card]".into()
            } else {
                pieces.join("\n")
            }
        }
        "location" => {
            let name = if text(&value["name"]).is_empty() {
                String::new()
            } else {
                format!(" name=\"{}\"", attr(&value["name"]))
            };
            let coords = if value["latitude"].as_f64().unwrap_or(0.) != 0.
                && value["longitude"].as_f64().unwrap_or(0.) != 0.
            {
                format!(
                    " coords=\"lat:{},lng:{}\"",
                    text(&value["latitude"]),
                    text(&value["longitude"])
                )
            } else {
                String::new()
            };
            format!("<location{name}{coords}/>")
        }
        "hongbao" => {
            let text = if text(&value["text"]).is_empty() {
                String::new()
            } else {
                format!(" text=\"{}\"", attr(&value["text"]))
            };
            format!("<hongbao{text}/>")
        }
        "calendar" | "general_calendar" | "share_calendar_event" => {
            let tag = match kind {
                "calendar" => "calendar_invite",
                "general_calendar" => "calendar",
                _ => "calendar_share",
            };
            let lines = calendar(&value);
            format!(
                "<{tag}>\n{}\n</{tag}>",
                if lines.is_empty() {
                    "[calendar event]".into()
                } else {
                    lines.join("\n")
                }
            )
        }
        "video_chat" => {
            let mut lines = calendar(&value);
            if let Some(topic) = value["topic"].as_str() {
                lines.insert(0, format!("📹 {topic}"))
            }
            format!(
                "<meeting>\n{}\n</meeting>",
                if lines.is_empty() {
                    "[video chat]".into()
                } else {
                    lines.join("\n")
                }
            )
        }
        "vote" => {
            let mut lines = Vec::new();
            if let Some(topic) = value["topic"].as_str() {
                lines.push(topic.into())
            }
            for option in value["options"].as_array().into_iter().flatten() {
                lines.push(format!("• {}", text(option)))
            }
            format!(
                "<vote>\n{}\n</vote>",
                if lines.is_empty() {
                    "[vote]".into()
                } else {
                    lines.join("\n")
                }
            )
        }
        "system" => {
            let template = text(&value["template"]);
            let re = regex::Regex::new(r"\{([a-z_]+)\}").unwrap();
            let result = re
                .replace_all(&template, |capture: &regex::Captures<'_>| {
                    let field = &value[&capture[1]];
                    if let Some(items) = field.as_array() {
                        items.iter().map(text).collect::<Vec<_>>().join(", ")
                    } else if field.is_null() {
                        String::new()
                    } else if field.is_string() {
                        text(field)
                    } else {
                        capture[0].into()
                    }
                })
                .trim()
                .to_owned();
            if result.is_empty() {
                "[system message]".into()
            } else {
                result
            }
        }
        "todo" => {
            let mut lines = Vec::new();
            if let Some(title) = value["summary"]["title"].as_str() {
                lines.push(title.into())
            }
            for paragraph in value["summary"]["content"].as_array().into_iter().flatten() {
                let line = paragraph
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|el| matches!(el["tag"].as_str(), Some("text" | "a")))
                    .map(|el| text(&el["text"]))
                    .collect::<String>();
                if !line.is_empty() {
                    lines.push(line)
                }
            }
            if let Some(ms) = value["due_time"]
                .as_i64()
                .or_else(|| {
                    value["due_time"]
                        .as_str()
                        .and_then(|value| value.parse().ok())
                })
                .filter(|ms| *ms > 0)
            {
                if let Some(date) = chrono::DateTime::from_timestamp_millis(ms + 8 * 3600 * 1000) {
                    lines.push(format!("Due: {}", date.format("%Y-%m-%d %H:%M")))
                }
            }
            format!(
                "<todo>\n{}\n</todo>",
                if lines.is_empty() {
                    "[todo]".into()
                } else {
                    lines.join("\n")
                }
            )
        }
        "merge_forward" => "<forwarded_messages/>".into(),
        _ => value["text"]
            .as_str()
            .unwrap_or("[unsupported message]")
            .into(),
    };
    (content, resources)
}
pub(super) fn normalize(event: &Value, bot: &str) -> Result<Value> {
    let message = &event["message"];
    let mut by_id = HashMap::new();
    let mut mentions = Vec::new();
    let mut replacements = Vec::new();
    let mut mention_all = false;
    let mut mentioned_bot = false;
    for raw in message["mentions"].as_array().into_iter().flatten() {
        let key = text(&raw["key"]);
        if key == "@_all" {
            mention_all = true;
            replacements.push((key, text(&raw["name"]), false));
            continue;
        }
        let id = text(&raw["id"]["open_id"]);
        let is_bot = !bot.is_empty() && id == bot;
        mentioned_bot |= is_bot;
        let mut info = json!({"key":key,"isBot":is_bot});
        if let Some(name) = raw.get("name") {
            info["name"] = name.clone()
        }
        if !id.is_empty() {
            info["openId"] = json!(id);
            by_id.insert(id, info.clone());
        }
        if let Some(user) = raw["id"].get("user_id") {
            info["userId"] = user.clone()
        }
        mentions.push(info);
        replacements.push((key, text(&raw["name"]), is_bot));
    }
    let raw = text(&message["content"]);
    let kind = message["message_type"].as_str().unwrap_or("");
    let (mut content, resources) = content(kind, &raw, &by_id);
    mention_all |= regex::Regex::new(r"@_all\b").unwrap().is_match(&raw);
    for (key, name, is_bot) in &replacements {
        if *is_bot {
            let pattern = format!(r"\s?{}\s?", regex::escape(key));
            content = regex::Regex::new(&pattern)
                .unwrap()
                .replace_all(&content, " ")
                .into_owned()
        } else if !name.is_empty() {
            content = content.replace(key, &format!("@{name}"))
        }
    }
    if !replacements.is_empty() {
        content = regex::Regex::new(r"[ \t]{2,}")
            .unwrap()
            .replace_all(&content, " ")
            .trim()
            .to_owned()
    }
    let sender = event.get("sender").ok_or_else(|| {
        ChannelError::new("Cannot read properties of undefined (reading 'sender_id')")
    })?;
    if sender.is_null() {
        return Err(ChannelError::new(
            "Cannot read properties of null (reading 'sender_id')",
        ));
    }
    let sender_id = sender.get("sender_id").ok_or_else(|| {
        ChannelError::new("Cannot read properties of undefined (reading 'open_id')")
    })?;
    if sender_id.is_null() {
        return Err(ChannelError::new(
            "Cannot read properties of null (reading 'open_id')",
        ));
    }
    let sender = sender_id
        .get("open_id")
        .filter(|value| !value.is_null())
        .or_else(|| sender_id.get("user_id").filter(|value| !value.is_null()))
        .or_else(|| sender_id.get("union_id").filter(|value| !value.is_null()))
        .map(text)
        .unwrap_or_default();
    let time = message["create_time"]
        .as_i64()
        .or_else(|| message["create_time"].as_str().and_then(|v| v.parse().ok()))
        .unwrap_or(0);
    let mut result = json!({"messageId":message["message_id"],"chatId":message["chat_id"],"peerId":message["chat_id"],"chatType":message["chat_type"],"senderId":sender,"content":content,"rawContentType":kind,"resources":resources,"mentions":mentions,"mentionAll":mention_all,"mentionedBot":mentioned_bot,"createTime":time});
    for (source, target) in [
        ("root_id", "rootId"),
        ("thread_id", "threadId"),
        ("parent_id", "replyToMessageId"),
    ] {
        if let Some(value) = message.get(source) {
            result[target] = value.clone()
        }
    }
    Ok(result)
}
