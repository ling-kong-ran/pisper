use serde::{Deserialize, Deserializer};
use serde_json::Value;

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

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionCwdUpdate {
    #[serde(default)]
    pub cwd: String,
}

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

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlanItem {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub assignee: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
}

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

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    pub role: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub run_activity: Option<RunActivity>,
    #[serde(default)]
    pub attachments: Vec<MessageAttachment>,
}

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

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunActivity {
    #[serde(default)]
    pub thinking_text: String,
    #[serde(default)]
    pub tools: Vec<ToolActivity>,
    #[serde(default)]
    pub agents: Vec<Value>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolActivity {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub output: String,
    #[serde(default)]
    pub args: Value,
    #[serde(default, deserialize_with = "deserialize_millis")]
    pub started_at: u64,
    #[serde(default, deserialize_with = "deserialize_millis")]
    pub finished_at: u64,
    #[serde(default)]
    pub agent: Option<Value>,
}

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

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageInfo {
    #[serde(default)]
    pub start: u64,
    #[serde(default)]
    pub has_more: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextUsage {
    #[serde(default)]
    pub percent: Option<f64>,
}

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

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VcsFile {
    pub path: String,
    #[serde(default)]
    pub status: String,
}

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

#[derive(Clone, Debug, Default, Deserialize)]
pub struct SessionsResponse {
    #[serde(default)]
    pub sessions: Vec<SessionSummary>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct PluginCatalog {
    #[serde(default)]
    pub tools: Vec<ToolDefinition>,
}

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

#[derive(Clone, Debug, Default, Deserialize)]
pub struct McpCatalog {
    #[serde(default)]
    pub tools: Vec<McpToolDefinition>,
}

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

#[derive(Clone, Debug, Default, Deserialize)]
pub struct SkillsCatalog {
    #[serde(default)]
    pub skills: Vec<SkillDefinition>,
}

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

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelOption {
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub reasoning: bool,
}

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

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum ThinkingAvailability {
    #[default]
    Loading,
    Supported,
    Unsupported,
    Error(String),
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionModeUpdate {
    #[serde(default)]
    pub execution_mode: String,
}

#[derive(Clone, Debug)]
pub struct StreamEvent {
    pub name: String,
    pub data: Value,
}

#[derive(Clone, Debug)]
pub enum RuntimeEvent {
    StartupData {
        default_model: String,
        thinking_level: String,
        model_options: Vec<ModelOption>,
        tools: Vec<ToolDefinition>,
        skills: Vec<SkillDefinition>,
    },
    Stream(StreamEvent),
    StreamFailed(String),
    HistoryPage {
        before: u64,
        result: Result<MessagePage, String>,
    },
    CompactionFinished {
        context_usage: Option<ContextUsage>,
        error: Option<String>,
    },
    VcsResult {
        session_id: String,
        result: Result<VcsChanges, String>,
    },
}

#[cfg(test)]
mod tests {
    use super::{MessagePage, SessionSummary, VcsChanges};

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
