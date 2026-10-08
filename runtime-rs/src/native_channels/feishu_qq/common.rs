//! 飞书与 QQ 的传输生命周期；凭据只留在网关实例，不进入公开状态。
use crate::native_channels::{ChannelError, GatewayCallbacks, Result};
use futures::StreamExt;
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

pub(super) fn text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(value) => value.clone(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        _ => String::new(),
    }
}
pub(super) fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
pub(super) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
pub(super) fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_default()
}
pub(super) fn clip(value: &str, length: usize) -> String {
    String::from_utf16_lossy(&value.encode_utf16().take(length).collect::<Vec<_>>())
}
pub(super) fn cancelled() -> ChannelError {
    ChannelError::new("渠道连接已停止。")
}
pub(super) async fn response(
    request: reqwest::RequestBuilder,
    token: &CancellationToken,
) -> Result<reqwest::Response> {
    tokio::select! { biased;
        _ = token.cancelled() => Err(cancelled()),
        response = request.send() => response.map_err(|error| ChannelError::new(error.without_url().to_string()))
    }
}
pub(super) async fn json_response(
    request: reqwest::RequestBuilder,
    token: &CancellationToken,
    label: &str,
) -> Result<Value> {
    let response = response(request, token).await?;
    let status = response.status();
    let value: Value = tokio::select! { biased;
        _ = token.cancelled() => return Err(cancelled()),
        value = response.json() => value.map_err(|_| ChannelError::new(format!("{label} 返回了无效响应（HTTP {}）。",status.as_u16())))?
    };
    if !status.is_success() {
        let reason = value
            .get("message")
            .or_else(|| value.get("msg"))
            .map(text)
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| format!("HTTP {}", status.as_u16()));
        return Err(ChannelError::new(format!(
            "{label} 失败：{}",
            clip(&reason, 500)
        )));
    }
    Ok(value)
}
pub(super) async fn bytes(
    response: reqwest::Response,
    token: &CancellationToken,
    limit: usize,
) -> Result<Vec<u8>> {
    let mut stream = response.bytes_stream();
    let mut result = Vec::new();
    loop {
        let part = tokio::select! {biased; _=token.cancelled()=>return Err(cancelled()), part=stream.next()=>part};
        let Some(part) = part else { return Ok(result) };
        let part = part.map_err(|error| ChannelError::new(error.without_url().to_string()))?;
        if result.len().saturating_add(part.len()) > limit {
            return Err(ChannelError::new("附件总大小超过 24 MB。"));
        }
        result.extend_from_slice(&part);
    }
}
pub(super) fn segment(value: &str) -> String {
    let mut url = reqwest::Url::parse("https://localhost/").unwrap();
    url.path_segments_mut().unwrap().pop_if_empty().push(value);
    url.path()[1..].to_owned()
}
pub(super) struct Status {
    value: Mutex<Value>,
    callbacks: GatewayCallbacks,
}
impl Status {
    pub(super) fn new(callbacks: GatewayCallbacks, extra: Value) -> Arc<Self> {
        let mut value = json!({"state":"idle","lastError":"","connectedAt":null});
        if let (Some(value), Some(extra)) = (value.as_object_mut(), extra.as_object()) {
            value.extend(extra.clone());
        }
        Arc::new(Self {
            value: Mutex::new(value),
            callbacks,
        })
    }
    pub(super) fn get(&self) -> Value {
        self.value
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
    pub(super) fn patch(&self, patch: Value) {
        let value = {
            let mut value = self
                .value
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let (Some(value), Some(patch)) = (value.as_object_mut(), patch.as_object()) {
                value.extend(patch.clone())
            };
            value.clone()
        };
        (self.callbacks.on_status)(value);
    }
    pub(super) fn message(&self, value: Value) {
        (self.callbacks.on_message)(value)
    }
    pub(super) fn sync(&self, value: Value) {
        (self.callbacks.on_sync)(value)
    }
}
pub(super) struct Live {
    pub(super) token: CancellationToken,
    active: AtomicUsize,
    settled: Notify,
}
impl Live {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            token: CancellationToken::new(),
            active: AtomicUsize::new(0),
            settled: Notify::new(),
        })
    }
    pub(super) fn enter(self: &Arc<Self>) -> Result<Operation> {
        if self.token.is_cancelled() {
            return Err(cancelled());
        }
        self.active.fetch_add(1, Ordering::AcqRel);
        let operation = Operation(self.clone());
        if self.token.is_cancelled() {
            drop(operation);
            Err(cancelled())
        } else {
            Ok(operation)
        }
    }
    pub(super) async fn stop(&self) {
        self.token.cancel();
        loop {
            let settled = self.settled.notified();
            if self.active.load(Ordering::Acquire) == 0 {
                return;
            }
            settled.await
        }
    }
}
pub(super) struct Operation(Arc<Live>);
impl Drop for Operation {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::AcqRel);
        self.0.settled.notify_waiters();
    }
}
