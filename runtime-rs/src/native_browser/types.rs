//! 浏览器领域的窄驱动接口；会话、计时器与截图归档由各自的拥有者管理。
use futures::future::BoxFuture;
use serde_json::Value;
use std::{path::PathBuf, sync::Arc};
use tokio_util::sync::CancellationToken;

pub type BrowserResult<T> = Result<T, String>;
pub type BrowserProgress = Arc<dyn Fn(String) + Send + Sync>;

pub trait BrowserDriver: Send + Sync {
    /// 输入已经过服务层的 release 规则处理，含 viewport 与截图 outputPath。
    fn execute(
        &self,
        input: Value,
        on_progress: Option<BrowserProgress>,
    ) -> BoxFuture<'static, BrowserResult<Value>>;
    fn close(&self) -> BoxFuture<'static, BrowserResult<()>>;
}
pub type BrowserFactory =
    Arc<dyn Fn(Value) -> BoxFuture<'static, BrowserResult<Arc<dyn BrowserDriver>>> + Send + Sync>;

#[derive(Clone)]
pub struct BrowserContext {
    pub session_id: String,
    pub cwd: PathBuf,
}
pub type BrowserContextPort =
    Arc<dyn Fn(String, PathBuf) -> BoxFuture<'static, BrowserResult<BrowserContext>> + Send + Sync>;
pub type BrowserGeneratedFilePort =
    Arc<dyn Fn(BrowserContext, Value) -> BoxFuture<'static, BrowserResult<()>> + Send + Sync>;

#[derive(Clone, Default)]
pub struct BrowserOptions {
    pub cancellation: CancellationToken,
    pub on_progress: Option<BrowserProgress>,
}
