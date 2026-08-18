//! 与 Runtime HTTP API 对齐的数据契约（serde 反序列化模型）。
//!
//! Runtime 的 JSON 采用 camelCase，字段普遍是可选的（`#[serde(default)]`），
//! 以便向前兼容：Runtime 新增字段时旧版 TUI 不会反序列化失败。
//! 这里定义的所有类型都只做反序列化（`Deserialize`），不负责序列化回写。

use serde::{Deserialize, Deserializer};
use serde_json::Value;

/// 会话摘要：会话列表与当前会话的基础信息。
/// `plan` 兼容历史字段名 `taskList`（见 `plan_protocol` 模块）。
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub streaming: bool,
    #[serde(default)]
    pub execution_mode: String,
    #[serde(default)]
    pub thinking_level: String,
    #[serde(default)]
    pub modified: String,
    #[serde(default, alias = "taskList")]
    pub plan: Option<Plan>,
}

/// 会话工作区变更响应（`PUT /api/sessions/{id}/cwd`）。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionCwdUpdate {
    #[serde(default)]
    pub cwd: String,
}

/// 执行计划：进行中的任务列表。仅 UI 展示使用，不做逻辑约束。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    #[serde(default)]
    pub items: Vec<PlanItem>,
    #[serde(default)]
    pub counts: PlanCounts,
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// 单个计划项。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlanItem {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub title: String,
    /// 状态：`pending` / `in_progress` / `completed` / `blocked`。
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub assignee: String,
    /// 依赖的其他计划项 id。
    #[serde(default)]
    pub depends_on: Vec<String>,
}

/// 计划各项状态的计数汇总（用于面板标题的进度展示）。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlanCounts {
    #[serde(default)]
    pub pending: usize,
    #[serde(default)]
    pub in_progress: usize,
    #[serde(default)]
    pub completed: usize,
    #[serde(default)]
    pub blocked: usize,
    #[serde(default)]
    pub total: usize,
}

/// 单条对话消息。`run_activity` 携带 Agent 推理与工具活动明细。
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    /// 角色：`user` / `agent`。
    pub role: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub run_activity: Option<RunActivity>,
    #[serde(default)]
    pub attachments: Vec<MessageAttachment>,
}

/// 消息附件元信息（图片/文本/文档）。
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageAttachment {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub mime_type: String,
    #[serde(default)]
    pub size: u64,
}

/// 一次 Agent 运行的活动：思考文本、工具调用、子代理。
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunActivity {
    #[serde(default)]
    pub thinking_text: String,
    #[serde(default)]
    pub tools: Vec<ToolActivity>,
    /// 子代理信息（非结构化 Value，因字段不固定）。
    #[serde(default)]
    pub agents: Vec<Value>,
}

/// 工具调用的活动记录。
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolActivity {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// 状态：`running` / `done` / `error`。
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub output: String,
    #[serde(default)]
    pub args: Value,
    /// 开始时间（毫秒时间戳）。历史数据可能是 ISO 字符串，用自定义反序列化兜底。
    #[serde(default, deserialize_with = "deserialize_millis")]
    pub started_at: u64,
    #[serde(default, deserialize_with = "deserialize_millis")]
    pub finished_at: u64,
    /// 所属子代理（存在时表示工具由子代理执行）。
    #[serde(default)]
    pub agent: Option<Value>,
}

/// 时间戳反序列化兜底：新老 Runtime 分别用毫秒数字与 ISO 字符串；
/// 无法解析时返回 0（表示未知），绝不因时间字段拒绝整条消息。
fn deserialize_millis<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    Ok(match value {
        Value::Number(number) => number.as_u64().unwrap_or_default(),
        Value::String(value) => value.parse().unwrap_or_default(),
        _ => 0,
    })
}

/// 一页消息：包含会话用量与分页信息。
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessagePage {
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub context_usage: Option<ContextUsage>,
    #[serde(default)]
    pub session_usage: Option<SessionUsage>,
    #[serde(default)]
    pub page_info: PageInfo,
}

/// 消息分页信息：`start` 是下一页的游标，`has_more` 表示是否还有更早的消息。
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageInfo {
    #[serde(default)]
    pub start: u64,
    #[serde(default)]
    pub has_more: bool,
}

/// 上下文窗口占用比例（0-100）。
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextUsage {
    #[serde(default)]
    pub percent: Option<f64>,
}

/// 会话 token 用量统计（用于状态栏展示）。
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionUsage {
    #[serde(default)]
    pub input: u64,
    #[serde(default)]
    pub output: u64,
    #[serde(default)]
    pub cache_read: u64,
    #[serde(default)]
    pub cache_write: u64,
    #[serde(default)]
    pub reasoning: u64,
    #[serde(default)]
    pub total_tokens: u64,
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub requests: u64,
    #[serde(default)]
    pub cache_hit_rate: Option<f64>,
}

impl SessionUsage {
    /// 旧 Runtime 可能缺失命中率，或在已有缓存读取时错误保留零值；
    /// 此时使用原始 token 计数恢复与 Runtime 聚合规则一致的比例。
    pub fn effective_cache_hit_rate(&self) -> Option<f64> {
        let reported = self
            .cache_hit_rate
            .filter(|rate| rate.is_finite() && *rate >= 0.0);
        if reported.is_some_and(|rate| rate > 0.0)
            || (reported == Some(0.0) && self.cache_read == 0)
        {
            return reported.map(|rate| rate.min(100.0));
        }

        let prompt_tokens = if self.prompt_tokens > 0 {
            self.prompt_tokens
        } else {
            self.input
                .saturating_add(self.cache_read)
                .saturating_add(self.cache_write)
        };
        (prompt_tokens > 0).then(|| (self.cache_read as f64 / prompt_tokens as f64) * 100.0)
    }
}

/// 版本控制状态中的一个文件。
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VcsFile {
    pub path: String,
    /// 变更状态：`M`（修改）/ `A`（新增）/ `D`（删除）等。
    #[serde(default)]
    pub status: String,
}

/// 工作区版本控制变更汇总（Git/SVN）。
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VcsChanges {
    #[serde(default)]
    pub vcs: String,
    #[serde(default)]
    pub is_repo: bool,
    #[serde(default)]
    pub git_available: Option<bool>,
    #[serde(default)]
    pub svn_available: Option<bool>,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub branch: String,
    #[serde(default)]
    pub files: Vec<VcsFile>,
    #[serde(default)]
    pub diff: String,
    #[serde(default)]
    pub diff_truncated: bool,
    #[serde(default)]
    pub ahead: Option<u64>,
    #[serde(default)]
    pub error: String,
}

/// 会话列表响应。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct SessionsResponse {
    #[serde(default)]
    pub sessions: Vec<SessionSummary>,
}

/// 插件（应用工具）目录。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct PluginCatalog {
    #[serde(default)]
    pub tools: Vec<ToolDefinition>,
}

/// 应用工具定义（Slash 目录中的 `/tool` 项）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct ToolDefinition {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub enabled: bool,
}

/// MCP 工具目录。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct McpCatalog {
    #[serde(default)]
    pub tools: Vec<McpToolDefinition>,
}

/// MCP 工具定义：`pi_name` 是工具在 Slash 目录中使用的命令名。
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolDefinition {
    #[serde(default)]
    pub pi_name: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub available: bool,
}

/// Skill（技能）目录。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct SkillsCatalog {
    #[serde(default)]
    pub skills: Vec<SkillDefinition>,
}

/// Skill 定义：`command` 是激活该技能的命令（如 `/skill-name`）。
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillDefinition {
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub enabled: bool,
}

/// 可选模型项（模型选择器使用）。
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelOption {
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// 是否支持推理（reasoning）模式。
    #[serde(default)]
    pub reasoning: bool,
}

/// 支持的 Provider 协议（API 类型 → 展示名），
/// 供 `/provider` 对话框循环选择协议时使用。
pub const PROVIDER_APIS: [(&str, &str); 4] = [
    ("openai-responses", "OpenAI Responses"),
    ("openai-completions", "OpenAI Chat Completions"),
    ("anthropic-messages", "Anthropic Messages"),
    ("google-generative-ai", "Google Generative AI"),
];

/// Provider 选项（连接配置信息）。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderOption {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub provider_type: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub configured: bool,
    #[serde(default)]
    pub api: String,
    #[serde(default)]
    pub base_url: String,
}

/// Provider 连接更新响应。
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConnectionUpdate {
    #[serde(default)]
    pub connection_updated: bool,
    #[serde(default)]
    pub api_key_updated: bool,
    #[serde(default)]
    pub updated_provider_id: String,
}

/// 会话模型更新响应。
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionModelUpdate {
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub thinking_level: String,
    #[serde(default)]
    pub available_thinking_levels: Vec<String>,
    #[serde(default)]
    pub thinking_status: String,
    #[serde(default)]
    pub thinking_message: String,
    #[serde(default)]
    pub context_usage: Option<ContextUsage>,
}

/// 思考级别（thinking level）查询/更新结果。
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingLevelUpdate {
    #[serde(default)]
    pub thinking_level: String,
    #[serde(default)]
    pub available_levels: Vec<String>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub message: String,
}

/// 思考级别的可用性状态（由 TUI 本地推导，非 Runtime 返回）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum ThinkingAvailability {
    /// 尚未加载。
    #[default]
    Loading,
    /// 当前模型支持配置思考级别。
    Supported,
    /// 当前模型不支持配置思考级别。
    Unsupported,
    /// 查询失败，携带错误信息。
    Error(String),
}

/// 执行模式更新响应。
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionModeUpdate {
    #[serde(default)]
    pub execution_mode: String,
}

/// 一条已解码的 SSE 流事件。
#[derive(Clone, Debug)]
pub struct StreamEvent {
    /// 事件名（无 `event:` 字段时默认为 `message`）。
    pub name: String,
    /// 事件负载（JSON）。
    pub data: Value,
}

/// 事件循环中 TUI 主任务与后台任务之间的统一消息。
/// 每个变体对应一类异步请求的终态或一条流事件。
#[derive(Clone, Debug)]
pub enum RuntimeEvent {
    /// 启动数据：默认模型、思考级别、模型/Provider/工具/Skill 目录，
    /// 用于初始化草稿会话的默认值。
    StartupData {
        default_model: String,
        thinking_level: String,
        model_options: Vec<ModelOption>,
        provider_options: Vec<ProviderOption>,
        tools: Vec<ToolDefinition>,
        skills: Vec<SkillDefinition>,
    },
    /// 单条对话流事件。
    Stream(StreamEvent),
    /// 对话流整体失败（连接中断/协议错误），需结束当前运行。
    StreamFailed(String),
    /// 排队输入的结果（运行中发送的消息）。
    QueueInputFinished {
        session_id: String,
        message: String,
        result: Result<usize, String>,
    },
    /// 中止运行的结果。
    AbortFinished {
        session_id: String,
        result: Result<(), String>,
    },
    /// 执行模式变更的结果。
    ExecutionModeFinished {
        session_id: String,
        result: Result<ExecutionModeUpdate, String>,
    },
    /// 加载更早历史消息的结果。
    HistoryPage {
        before: u64,
        result: Result<MessagePage, String>,
    },
    /// 切换会话的结果（携带完整会话信息与消息页）。
    SessionLoaded {
        request_id: u64,
        session: Box<SessionSummary>,
        result: Result<MessagePage, String>,
    },
    /// 加载会话思考级别的结果。
    SessionThinkingLoaded {
        session_id: String,
        result: Result<ThinkingLevelUpdate, String>,
    },
    /// 上下文压缩（compact）的结果。
    CompactionFinished {
        context_usage: Option<ContextUsage>,
        error: Option<String>,
    },
    /// 审批（权限请求）解析的结果。
    ApprovalResolved {
        session_id: String,
        approval_id: String,
        approved: bool,
        result: Result<(), String>,
    },
    /// 版本控制查询/提交/推送/回退的结果。
    VcsResult {
        session_id: String,
        result: Result<VcsChanges, String>,
    },
}

#[cfg(test)]
mod tests {
    use super::{MessagePage, SessionSummary, SessionUsage, VcsChanges};

    /// 验证历史工具时间戳兼容 ISO 字符串与毫秒整数两种格式（非法 ISO 归零）。
    #[test]
    fn historical_tool_timestamps_accept_iso_strings_and_milliseconds() {
        let page: MessagePage = serde_json::from_value(serde_json::json!({
            "messages": [{
                "role": "assistant",
                "runActivity": {
                    "tools": [
                        {
                            "id": "iso",
                            "startedAt": "2026-08-03T01:14:48.734Z",
                            "finishedAt": null
                        },
                        {
                            "id": "millis",
                            "startedAt": 1000,
                            "finishedAt": "1512"
                        }
                    ]
                }
            }]
        }))
        .unwrap();
        let tools = &page.messages[0].run_activity.as_ref().unwrap().tools;
        assert_eq!(tools[0].started_at, 0);
        assert_eq!(tools[0].finished_at, 0);
        assert_eq!(tools[1].started_at, 1000);
        assert_eq!(tools[1].finished_at, 1512);
    }

    /// 验证会话用量与 VCS 变更的 camelCase 字段能正确反序列化。
    #[test]
    fn message_usage_and_vcs_contracts_deserialize_camel_case_fields() {
        let page: MessagePage = serde_json::from_value(serde_json::json!({
            "messages": [],
            "sessionUsage": {
                "input": 100,
                "output": 40,
                "cacheRead": 75,
                "cacheWrite": 25,
                "reasoning": 10,
                "totalTokens": 240,
                "promptTokens": 200,
                "requests": 2,
                "cacheHitRate": 37.5
            }
        }))
        .unwrap();
        let usage = page.session_usage.unwrap();
        assert_eq!(usage.cache_read, 75);
        assert_eq!(usage.total_tokens, 240);
        assert_eq!(usage.cache_hit_rate, Some(37.5));

        let changes: VcsChanges = serde_json::from_value(serde_json::json!({
            "vcs": "svn",
            "isRepo": true,
            "svnAvailable": true,
            "files": [{ "path": "docs/a & b.txt", "status": "M" }],
            "diff": "--- a/docs/a & b.txt\n+++ b/docs/a & b.txt"
        }))
        .unwrap();
        assert!(changes.is_repo);
        assert_eq!(changes.files[0].path, "docs/a & b.txt");
    }

    /// 验证缓存命中率：服务端缺省时由 token 计数推算，无缓存时返回 0。
    #[test]
    fn session_usage_recovers_cache_rate_from_token_counts() {
        let usage = SessionUsage {
            input: 100,
            cache_read: 75,
            cache_write: 25,
            prompt_tokens: 200,
            cache_hit_rate: Some(0.0),
            ..SessionUsage::default()
        };
        assert_eq!(usage.effective_cache_hit_rate(), Some(37.5));

        let legacy = SessionUsage {
            input: 100,
            cache_read: 75,
            cache_write: 25,
            ..SessionUsage::default()
        };
        assert_eq!(legacy.effective_cache_hit_rate(), Some(37.5));

        let uncached = SessionUsage {
            input: 100,
            ..SessionUsage::default()
        };
        assert_eq!(uncached.effective_cache_hit_rate(), Some(0.0));
    }

    /// 验证会话计划兼容新（plan）旧（taskList）两种负载字段。
    #[test]
    fn session_plan_deserializes_canonical_and_legacy_fields() {
        let canonical: SessionSummary = serde_json::from_value(serde_json::json!({
            "id": "canonical",
            "plan": {
                "items": [{
                    "id": "inspect",
                    "title": "Inspect",
                    "status": "in_progress",
                    "note": "Read files",
                    "assignee": "agent",
                    "dependsOn": ["setup"]
                }],
                "counts": { "inProgress": 1, "total": 1 },
                "updatedAt": "2026-08-02T00:00:00.000Z"
            }
        }))
        .unwrap();
        assert_eq!(canonical.plan.as_ref().unwrap().items[0].id, "inspect");
        assert_eq!(
            canonical.plan.as_ref().unwrap().items[0].depends_on,
            ["setup"]
        );

        let legacy: SessionSummary = serde_json::from_value(serde_json::json!({
            "id": "legacy",
            "taskList": { "items": [{ "id": "old", "title": "Old", "status": "pending" }] }
        }))
        .unwrap();
        assert_eq!(legacy.plan.as_ref().unwrap().items[0].id, "old");
    }
}
