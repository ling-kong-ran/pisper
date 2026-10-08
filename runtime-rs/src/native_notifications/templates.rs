use super::{Result, PLATFORMS};
use crate::ApiError;
use serde_json::{json, Value};
use std::sync::OnceLock;

pub(crate) const EVENTS: [&str; 6] = [
    "chat.completed",
    "chat.waiting",
    "schedule.completed",
    "schedule.failed",
    "workflow.completed",
    "workflow.failed",
];
pub(crate) fn definitions() -> Value {
    json!({
        "chat.completed":{"name":"对话完成","description":"Agent 完成回复后发送","variables":["chat.title","chat.summary","chat.model"],"defaultContent":"💬 对话「{{chat.title}}」已完成\n\n{{chat.summary}}\n\n模型：{{chat.model}}"},
        "chat.waiting":{"name":"等待用户确认","description":"Agent 暂停运行并等待用户确认时发送","variables":["chat.title","chat.tool","chat.reason","chat.model"],"defaultContent":"⏸️ 对话「{{chat.title}}」正在等待你的确认\n\n操作：{{chat.tool}}\n原因：{{chat.reason}}\n\n模型：{{chat.model}}"},
        "schedule.completed":{"name":"定时任务完成","description":"定时任务正常完成后发送","variables":["task.name","task.summary","task.duration","task.nextRun"],"defaultContent":"✅ 定时任务「{{task.name}}」已完成\n\n{{task.summary}}\n\n耗时：{{task.duration}}\n下次运行：{{task.nextRun}}"},
        "schedule.failed":{"name":"定时任务失败","description":"定时任务执行失败后发送","variables":["task.name","task.error","task.duration","task.nextRun"],"defaultContent":"❌ 定时任务「{{task.name}}」执行失败\n\n错误：{{task.error}}\n耗时：{{task.duration}}\n下次运行：{{task.nextRun}}"},
        "workflow.completed":{"name":"工作流完成","description":"工作流所有节点执行完成后发送","variables":["workflow.name","workflow.summary","workflow.duration","workflow.runId"],"defaultContent":"✅ 工作流「{{workflow.name}}」已完成\n\n{{workflow.summary}}\n\n耗时：{{workflow.duration}}\n运行 ID：{{workflow.runId}}"},
        "workflow.failed":{"name":"工作流失败","description":"工作流中断或节点失败后发送","variables":["workflow.name","workflow.node","workflow.error","workflow.runId"],"defaultContent":"❌ 工作流「{{workflow.name}}」执行失败\n\n节点：{{workflow.node}}\n错误：{{workflow.error}}\n运行 ID：{{workflow.runId}}"}
    })
}
pub(crate) fn sample() -> Value {
    json!({
        "chat":{"title":"修复渠道通知","summary":"实现已完成，测试和构建均已通过。","tool":"bash","reason":"需要确认后才能执行此操作。","model":"openai/gpt-5.4"},
        "task":{"name":"每日代码巡检","summary":"发现 2 个待处理问题，报告已归档。","duration":"2 分 18 秒","nextRun":"明天 09:00","error":"测试进程超时"},
        "workflow":{"name":"发布前检查","summary":"测试、构建和安全检查均已通过。","duration":"6 分 42 秒","runId":"run_20260718_001","node":"端到端测试","error":"浏览器启动失败"}
    })
}
pub(crate) fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(v) => *v,
        Value::Number(v) => v.as_f64().is_some_and(|n| n != 0.0),
        Value::String(v) => !v.is_empty(),
        _ => true,
    }
}
pub(crate) fn js_string(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(v) => v.to_string(),
        Value::Number(v) => v.to_string(),
        Value::String(v) => v.clone(),
        Value::Array(v) => v
            .iter()
            .map(|v| {
                if v.is_null() {
                    String::new()
                } else {
                    js_string(v)
                }
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".into(),
    }
}
pub(crate) fn clip(value: &str, limit: usize) -> String {
    String::from_utf16_lossy(&value.encode_utf16().take(limit).collect::<Vec<_>>())
}
pub(crate) fn normalized(stored: &Value) -> Value {
    let definitions = definitions();
    let mut result = json!({});
    for event in EVENTS {
        let value = &stored[event];
        let mut channels = json!({});
        for platform in PLATFORMS {
            let content = &value["channels"][platform]["content"];
            let content = if truthy(content) {
                js_string(content)
            } else {
                String::new()
            };
            channels[platform] = json!({"content":if content.trim().is_empty(){definitions[event]["defaultContent"].as_str().unwrap().to_owned()}else{clip(&content,12_000)}});
        }
        result[event] = json!({"enabled":value["enabled"]!=false,"channels":channels});
    }
    result
}
pub(crate) fn catalog(templates: &Value) -> Value {
    let definitions = definitions();
    json!(EVENTS
        .iter()
        .map(|event| {
            let mut value = definitions[event].clone();
            value["id"] = json!(event);
            value["enabled"] = templates[event]["enabled"].clone();
            value["channels"] = templates[event]["channels"].clone();
            value["sample"] = sample();
            value
        })
        .collect::<Vec<_>>())
}
pub(crate) fn validate_target(event: &str, platform: &str) -> Result<()> {
    if !EVENTS.contains(&event) || !PLATFORMS.contains(&platform) {
        Err(ApiError::bad_request("通知模板类型不存在。"))
    } else {
        Ok(())
    }
}
pub(crate) fn render(content: &str, data: &Value) -> String {
    static TOKENS: OnceLock<regex::Regex> = OnceLock::new();
    TOKENS
        .get_or_init(|| regex::Regex::new(r"\{\{\s*([A-Za-z0-9_.]+)\s*\}\}").unwrap())
        .replace_all(content, |capture: &regex::Captures<'_>| {
            let path = &capture[1];
            let mut current = Some(data);
            for key in path.split('.') {
                current = current.and_then(|value| {
                    if let Some(array) = value.as_array() {
                        key.parse::<usize>().ok().and_then(|index| array.get(index))
                    } else {
                        value.get(key)
                    }
                });
            }
            match current {
                None | Some(Value::Null) => format!("{{{{{path}}}}}"),
                Some(value) => js_string(value),
            }
        })
        .into_owned()
}
pub(crate) fn rendered(
    templates: &Value,
    event: &str,
    platform: &str,
    data: &Value,
) -> Result<(String, String)> {
    validate_target(event, platform)?;
    let defs = definitions();
    Ok((
        defs[event]["name"].as_str().unwrap().into(),
        render(
            templates[event]["channels"][platform]["content"]
                .as_str()
                .unwrap_or(""),
            data,
        ),
    ))
}
