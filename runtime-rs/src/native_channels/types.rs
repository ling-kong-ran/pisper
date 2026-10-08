//! 渠道领域边界：网关负责协议，Agent 端口负责会话；凭据不进入公共快照类型。
use futures::future::BoxFuture;
use serde_json::Value;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug)]
pub(crate) struct ChannelError {
    pub(crate) message: String,
    pub(crate) status: Option<u16>,
}
impl ChannelError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            status: None,
        }
    }
}
impl std::fmt::Display for ChannelError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}
impl std::error::Error for ChannelError {}
pub(crate) type Result<T> = std::result::Result<T, ChannelError>;
pub(crate) type MessageSink = Arc<dyn Fn(Value) + Send + Sync>;
pub(crate) type ValueSink = Arc<dyn Fn(Value) + Send + Sync>;
#[derive(Clone)]
pub(crate) struct GatewayCallbacks {
    pub(crate) on_message: MessageSink,
    pub(crate) on_status: ValueSink,
    pub(crate) on_sync: ValueSink,
}
pub(crate) struct Resource {
    pub(crate) name: String,
    pub(crate) kind: String,
    pub(crate) mime_type: Option<String>,
    pub(crate) bytes: Vec<u8>,
}
pub(crate) trait Gateway: Send + Sync {
    fn get_status(&self) -> Value;
    fn connect(&self, connection: Value) -> BoxFuture<'_, Result<Value>>;
    fn disconnect(&self) -> BoxFuture<'_, Result<()>>;
    fn send(&self, message: Value, payload: Value) -> BoxFuture<'_, Result<()>>;
    fn send_to_peer(
        &self,
        peer_id: String,
        payload: Value,
        scope: Value,
    ) -> BoxFuture<'_, Result<()>>;
    fn send_asset(&self, peer_id: String, asset: Value, scope: Value) -> BoxFuture<'_, Result<()>>;
    fn download_resources(&self, resources: Value) -> BoxFuture<'_, Result<Vec<Resource>>>;
}
pub(crate) type GatewayFactory = Arc<dyn Fn(GatewayCallbacks) -> Arc<dyn Gateway> + Send + Sync>;
pub(crate) type CompletedSink =
    Arc<dyn Fn(Value) -> BoxFuture<'static, Result<Value>> + Send + Sync>;
pub(crate) trait Onboarding: Send + Sync {
    fn start(&self, options: Value) -> BoxFuture<'_, Result<Value>>;
    fn get(&self, id: &str) -> Option<Value>;
    fn cancel(&self, id: &str) -> bool;
    fn verify(&self, id: &str, code: Value) -> Result<Option<Value>>;
    fn dispose(&self) -> BoxFuture<'_, Result<()>>;
}
pub(crate) type OnboardingFactory = Arc<dyn Fn(CompletedSink) -> Arc<dyn Onboarding> + Send + Sync>;
pub(crate) type AgentEventSink = Arc<dyn Fn(String, Value) + Send + Sync>;
pub(crate) struct PromptRequest {
    pub(crate) session_id: String,
    pub(crate) message: String,
    pub(crate) attachments: Vec<Value>,
    pub(crate) cwd: String,
    pub(crate) title: String,
    pub(crate) model: Value,
    pub(crate) execution_mode: String,
    pub(crate) goal_mode: bool,
    pub(crate) team_mode: bool,
    pub(crate) on_event: AgentEventSink,
    pub(crate) cancellation: CancellationToken,
}
pub(crate) struct PromptResult {
    pub(crate) session_id: String,
    pub(crate) cwd: String,
    pub(crate) model: String,
    pub(crate) text: String,
    pub(crate) assets: Vec<Value>,
}
#[derive(Clone)]
pub(crate) struct AgentPort {
    pub(crate) prompt:
        Arc<dyn Fn(PromptRequest) -> BoxFuture<'static, Result<PromptResult>> + Send + Sync>,
    pub(crate) validate_directory:
        Arc<dyn Fn(String) -> BoxFuture<'static, Result<String>> + Send + Sync>,
    pub(crate) set_cwd: Arc<dyn Fn(String, String) -> BoxFuture<'static, Result<()>> + Send + Sync>,
    pub(crate) set_execution_mode:
        Arc<dyn Fn(String, String) -> BoxFuture<'static, Result<()>> + Send + Sync>,
    pub(crate) set_run_mode:
        Arc<dyn Fn(String, String) -> BoxFuture<'static, Result<()>> + Send + Sync>,
    pub(crate) resolve_approval:
        Arc<dyn Fn(String, String, bool) -> BoxFuture<'static, Result<Value>> + Send + Sync>,
    pub(crate) abort: Arc<dyn Fn(String) -> BoxFuture<'static, Result<bool>> + Send + Sync>,
}

/// 与通知模板共用同一个规范状态所有者。读取返回完整版本5私有状态。
pub(crate) type StateMutation = Box<dyn FnOnce(&mut Value) -> Result<()> + Send>;
#[derive(Clone)]
pub(crate) struct StatePort {
    pub(crate) read: Arc<dyn Fn() -> Result<Value> + Send + Sync>,
    pub(crate) update: Arc<dyn Fn(StateMutation) -> Result<Value> + Send + Sync>,
}
