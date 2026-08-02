use serde::Deserialize;
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
    #[serde(default, alias = "taskList")]
    pub plan: Option<Plan>,
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
    #[serde(default)]
    pub started_at: u64,
    #[serde(default)]
    pub finished_at: u64,
    #[serde(default)]
    pub agent: Option<Value>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessagePage {
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub context_usage: Option<ContextUsage>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextUsage {
    #[serde(default)]
    pub percent: Option<f64>,
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
    Stream(StreamEvent),
    StreamFailed(String),
}

#[cfg(test)]
mod tests {
    use super::SessionSummary;

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
