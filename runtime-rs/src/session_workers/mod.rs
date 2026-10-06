//! 执行服务通过窄接口接入真实 Pi 会话；领域状态不持有 AppState 或全局引擎锁。
pub(crate) mod agents;
pub(crate) mod persistence;
pub(crate) mod team;
pub(crate) mod team_js;
pub(crate) mod team_workflow;
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

pub(crate) type EventSink = Arc<dyn Fn(&str, &Value) + Send + Sync>;
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct SessionScope {
    pub session_id: String,
    pub cwd: String,
    pub model: String,
    pub thinking_level: String,
    pub execution_mode: String,
    pub permission_mode: String,
    pub tool_names: Vec<String>,
    pub parent_session_id: Option<String>,
    pub owned_files: Vec<String>,
}
#[derive(Clone, Debug)]
pub(crate) struct ChildRequest {
    pub id: String,
    pub parent: SessionScope,
    pub system_prompt: String,
    pub tools: Vec<String>,
    pub owned_files: Vec<String>,
}
#[derive(Clone, Debug, Default)]
pub(crate) struct PromptRequest {
    pub session_id: String,
    pub text: String,
    pub internal: bool,
    pub images: Vec<pi_rust::ai::types::ImageContent>,
    pub context_prepared: bool,
    pub isolated: bool,
}
#[derive(Clone, Debug, Default)]
pub(crate) struct RunOutcome {
    pub output: String,
    pub usage: Value,
    pub error: Option<String>,
    pub aborted: bool,
}
#[derive(Clone, Copy, Debug)]
pub(crate) enum InputKind {
    Steer,
    FollowUp,
    Notification,
}
/// prompt 必须等真实引擎结束，并在返回前发布全部 turn_end。
/// turn_end.data 含 message.usage 与 elapsedSeconds；错误/取消不能伪装为正常结束。
pub(crate) trait SessionExecutor: Send + Sync {
    /// HTTP projections need an existence check, not a resident model runtime.
    /// Native adapters override this without opening or changing the journal.
    fn validate_session(&self, id: String) -> BoxFuture<'static, anyhow::Result<()>> {
        let scope = self.scope(id);
        Box::pin(async move { scope.await.map(|_| ()) })
    }
    fn scope(&self, id: String) -> BoxFuture<'static, anyhow::Result<SessionScope>>;
    fn create_child(
        &self,
        request: ChildRequest,
    ) -> BoxFuture<'static, anyhow::Result<SessionScope>>;
    fn prompt(
        &self,
        request: PromptRequest,
        events: EventSink,
    ) -> BoxFuture<'static, anyhow::Result<RunOutcome>>;
    fn enqueue(
        &self,
        id: String,
        text: String,
        kind: InputKind,
    ) -> BoxFuture<'static, anyhow::Result<()>>;
    fn abort(&self, id: String) -> BoxFuture<'static, anyhow::Result<()>>;
    fn dispose(&self, id: String) -> BoxFuture<'static, anyhow::Result<()>>;
}

pub(crate) fn tool_result(value: Value) -> pi_rust::agent_core::types::AgentToolResult {
    serde_json::from_value(serde_json::json!({
        "content":[{"type":"text","text":serde_json::to_string_pretty(&value).expect("tool JSON")}],
        "details":value,
    }))
    .expect("native tool result")
}
pub(crate) fn native_tool(
    name: &str,
    description: &str,
    parameters: Value,
    handler: Arc<dyn Fn(Value) -> BoxFuture<'static, anyhow::Result<Value>> + Send + Sync>,
) -> pi_rust::agent_core::types::AgentTool {
    use pi_rust::agent_core::types::AgentTool;
    AgentTool {
        name: name.into(),
        label: name.into(),
        description: description.into(),
        parameters,
        constrained_sampling: None,
        prepare_arguments: None,
        replay: None,
        execution_mode: None,
        execute: Arc::new(move |_, args, signal, _| {
            let handler = handler.clone();
            Box::pin(async move {
                let cancelled = async {
                    if let Some(signal) = signal {
                        signal.cancelled().await
                    } else {
                        std::future::pending::<()>().await
                    }
                };
                tokio::select! { biased;
                    _ = cancelled => Err(anyhow::anyhow!("Tool execution was cancelled.")),
                    result = handler(args) => result.map(tool_result),
                }
            })
        }),
    }
}
