//! 转写区的折叠/展开与运行计时：渲染层记录可点击区域，事件循环按行号反查并切换展开状态。
//! 展开键不使用消息序号——加载更早历史或裁剪旧消息都会移动序号，改用工具 id 与内容哈希保持稳定。

use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    time::Duration,
};

use serde_json::Value;

use crate::model::ToolActivity;

/// 可展开块的稳定标识。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ExpandKey {
    /// 单个工具调用：展开后显示完整命令、参数与输出尾部。
    Tool(String),
    /// 已提交消息的思考过程（按内容哈希）。
    Thinking(u64),
    /// 正在流式输出回合的思考过程。
    LiveThinking,
    /// 已提交消息的工具组：展开后不再折叠为「N earlier」。
    ToolGroup(u64),
    /// 正在流式输出回合的工具组。
    LiveToolGroup,
}

/// 渲染时记录的可点击区域，行号相对整个转写区（未滚动前）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClickRegion {
    pub start_row: usize,
    pub rows: usize,
    pub key: ExpandKey,
}

/// 按转写区行号查找被点击的展开键。
pub fn region_at(regions: &[ClickRegion], row: usize) -> Option<&ExpandKey> {
    regions
        .iter()
        .find(|region| row >= region.start_row && row < region.start_row + region.rows.max(1))
        .map(|region| &region.key)
}

fn hash_of(value: impl Hash) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

/// 工具的展开键：优先用 Runtime 下发的工具 id，旧数据缺 id 时退回名称与参数的哈希。
pub fn tool_key(tool: &ToolActivity) -> ExpandKey {
    if tool.id.is_empty() {
        ExpandKey::Tool(format!(
            "{}:{}:{}",
            tool.name,
            tool.started_at,
            hash_of(tool.args.to_string())
        ))
    } else {
        ExpandKey::Tool(tool.id.clone())
    }
}

pub fn thinking_key(thinking: &str) -> ExpandKey {
    ExpandKey::Thinking(hash_of(thinking))
}

pub fn tool_group_key(tools: &[ToolActivity]) -> ExpandKey {
    ExpandKey::ToolGroup(hash_of(
        tools
            .iter()
            .map(|tool| match tool_key(tool) {
                ExpandKey::Tool(id) => id,
                _ => String::new(),
            })
            .collect::<Vec<_>>(),
    ))
}

/// 工具的主命令：Shell 类工具取 `command`，其余取第一个字符串参数。
pub fn tool_command(tool: &ToolActivity) -> Option<String> {
    let object = tool.args.as_object()?;
    if let Some(command) = object.get("command").and_then(Value::as_str) {
        return Some(command.to_owned());
    }
    None
}

/// 展开后列出的参数（不含已作为主命令展示的 `command`），值统一压成单行文本。
pub fn tool_arguments(tool: &ToolActivity) -> Vec<(String, String)> {
    let Some(object) = tool.args.as_object() else {
        return Vec::new();
    };
    object
        .iter()
        .filter(|(key, _)| key.as_str() != "command")
        .filter_map(|(key, value)| {
            let text = match value {
                Value::Null => return None,
                Value::String(text) if text.is_empty() => return None,
                Value::String(text) => text.clone(),
                other => other.to_string(),
            };
            Some((key.clone(), text))
        })
        .collect()
}

/// 用时文本：`42s`、`1m 03s`、`1h 02m`。
pub fn format_elapsed(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{}h {:02}m", seconds / 3600, (seconds % 3600) / 60)
    }
}

/// 一轮运行的结束摘要，状态栏在结束后短暂展示。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RunFinish {
    pub elapsed: Duration,
    pub ok: bool,
}

/// 运行结束摘要在状态栏保留的时长。
pub const FINISH_NOTICE: Duration = Duration::from_secs(10);

/// 流式回合超过该时长没有任何事件且没有工具在运行时，提示正在等待模型。
pub const STALL_AFTER: Duration = Duration::from_secs(15);

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn tool(id: &str, args: Value) -> ToolActivity {
        ToolActivity {
            id: id.to_owned(),
            name: "bash".to_owned(),
            args,
            ..ToolActivity::default()
        }
    }

    #[test]
    fn regions_resolve_rows_including_wrapped_lines() {
        let regions = vec![
            ClickRegion {
                start_row: 2,
                rows: 1,
                key: ExpandKey::LiveThinking,
            },
            ClickRegion {
                start_row: 5,
                rows: 2,
                key: ExpandKey::Tool("a".to_owned()),
            },
        ];
        assert_eq!(region_at(&regions, 2), Some(&ExpandKey::LiveThinking));
        assert_eq!(
            region_at(&regions, 6),
            Some(&ExpandKey::Tool("a".to_owned()))
        );
        assert_eq!(region_at(&regions, 3), None);
        assert_eq!(region_at(&regions, 7), None);
    }

    #[test]
    fn tool_keys_are_stable_without_runtime_ids() {
        let first = tool("", json!({ "command": "npm test" }));
        assert_eq!(tool_key(&first), tool_key(&first.clone()));
        assert_ne!(
            tool_key(&first),
            tool_key(&tool("", json!({ "command": "npm run build" })))
        );
        assert_eq!(
            tool_key(&tool("call-1", json!({}))),
            ExpandKey::Tool("call-1".to_owned())
        );
    }

    #[test]
    fn command_is_separated_from_other_arguments() {
        let shell = tool(
            "1",
            json!({ "command": "npm test", "timeout": 30, "cwd": "", "note": null }),
        );
        assert_eq!(tool_command(&shell).as_deref(), Some("npm test"));
        assert_eq!(
            tool_arguments(&shell),
            vec![("timeout".to_owned(), "30".to_owned())]
        );
        let read = tool("2", json!({ "path": "src/app.rs", "limit": 40 }));
        assert_eq!(tool_command(&read), None);
        assert_eq!(tool_arguments(&read).len(), 2);
    }

    #[test]
    fn elapsed_text_scales_with_duration() {
        assert_eq!(format_elapsed(Duration::from_millis(900)), "0s");
        assert_eq!(format_elapsed(Duration::from_secs(42)), "42s");
        assert_eq!(format_elapsed(Duration::from_secs(63)), "1m 03s");
        assert_eq!(format_elapsed(Duration::from_secs(3720)), "1h 02m");
    }
}
