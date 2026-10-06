//! 微信同步缓冲区长轮询；上下文 token 按消息传给会话领域，不在网关推断用户状态。
use super::weixin_protocol::{
    error, media_items, now_iso, now_ms, pause, string, text, text_from_items, truthy, Operations,
    WeixinProtocol,
};
use super::{Gateway, GatewayCallbacks, Resource, Result};
use futures::future::BoxFuture;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tokio::{sync::Mutex as AsyncMutex, task::JoinHandle};
use tokio_util::sync::CancellationToken;
struct State {
    connection: Option<Value>,
    cancellation: CancellationToken,
    status: Value,
}
struct Inner {
    state: Mutex<State>,
    protocol: Arc<WeixinProtocol>,
    callbacks: GatewayCallbacks,
    operations: Arc<Operations>,
}
pub(crate) struct WeixinGateway {
    inner: Arc<Inner>,
    monitor: AsyncMutex<Option<JoinHandle<()>>>,
}
impl Inner {
    fn status(&self) -> Value {
        self.state.lock().unwrap().status.clone()
    }
    fn set_status(&self, patch: Value) {
        let value = {
            let mut state = self.state.lock().unwrap();
            for (key, value) in patch.as_object().into_iter().flatten() {
                state.status[key] = value.clone();
            }
            state.status.clone()
        };
        (self.callbacks.on_status)(value);
    }
    fn connection(&self) -> Result<(Value, CancellationToken)> {
        let state = self.state.lock().unwrap();
        state
            .connection
            .clone()
            .filter(|_| !state.cancellation.is_cancelled())
            .map(|value| (value, state.cancellation.clone()))
            .ok_or_else(|| error("微信机器人尚未连接。"))
    }
    async fn monitor(self: Arc<Self>, mut connection: Value, token: CancellationToken) {
        let mut sync = if truthy(&connection["syncBuf"]) {
            connection["syncBuf"].clone()
        } else {
            json!("")
        };
        let mut timeout = 35_000;
        let mut failures = 0;
        while !token.is_cancelled() {
            let result = self
                .protocol
                .get_updates(&connection, &sync, &token, timeout)
                .await
                .and_then(|value| {
                    if (truthy(&value["ret"]) && value["ret"] != 0)
                        || (truthy(&value["errcode"]) && value["errcode"] != 0)
                    {
                        let code = if truthy(&value["errcode"]) {
                            &value["errcode"]
                        } else {
                            &value["ret"]
                        };
                        Err(error(if truthy(&value["errmsg"]) {
                            text(&value, "errmsg")
                        } else {
                            format!("微信同步失败（{}）", string(code))
                        }))
                    } else {
                        Ok(value)
                    }
                });
            match result {
                Ok(result) => {
                    if token.is_cancelled() {
                        return;
                    }
                    failures = 0;
                    self.set_status(
                        json!({"state":"connected","lastEventAt":now_iso(),"lastError":""}),
                    );
                    if let Some(next) = result["longpolling_timeout_ms"].as_u64().filter(|v| *v > 0)
                    {
                        timeout = next;
                    }
                    if truthy(&result["get_updates_buf"]) && result["get_updates_buf"] != sync {
                        sync = result["get_updates_buf"].clone();
                        connection["syncBuf"] = sync.clone();
                        if let Some(current) = self.state.lock().unwrap().connection.as_mut() {
                            current["syncBuf"] = sync.clone();
                        }
                        (self.callbacks.on_sync)(sync.clone());
                    }
                    for raw in result["msgs"].as_array().into_iter().flatten() {
                        if let Some(message) = map_message(raw) {
                            (self.callbacks.on_message)(message);
                        }
                    }
                    // 本地立即响应夹具与极快的空轮询仍应给取消/发送任务调度机会。
                    tokio::task::yield_now().await;
                }
                Err(e) => {
                    if token.is_cancelled() {
                        return;
                    }
                    failures += 1;
                    self.set_status(json!({"state":if failures>=3{"reconnecting"}else{"connected"},"lastError":e.message}));
                    pause(&token, if failures >= 3 { 30_000 } else { 2_000 }).await;
                }
            }
        }
    }
}
pub(crate) fn map_message(raw: &Value) -> Option<Value> {
    if raw["message_type"] == 2 || !truthy(&raw["from_user_id"]) {
        return None;
    }
    let message_id = if truthy(&raw["message_id"]) {
        string(&raw["message_id"])
    } else if truthy(&raw["client_id"]) {
        string(&raw["client_id"])
    } else {
        format!(
            "{}-{}",
            string(&raw["from_user_id"]),
            if truthy(&raw["create_time_ms"]) {
                string(&raw["create_time_ms"])
            } else {
                now_ms().to_string()
            }
        )
    };
    Some(
        json!({"messageId":message_id,"peerId":raw["from_user_id"],"senderId":raw["from_user_id"],"senderName":"","chatType":"p2p","content":text_from_items(&raw["item_list"]),"resources":media_items(&raw["item_list"]),"contextToken":if truthy(&raw["context_token"]){raw["context_token"].clone()}else{json!("")}}),
    )
}
impl WeixinGateway {
    pub(crate) fn new(callbacks: GatewayCallbacks) -> Arc<Self> {
        Self::with_protocol(callbacks, Arc::new(WeixinProtocol::new()))
    }
    pub(crate) fn with_protocol(
        callbacks: GatewayCallbacks,
        protocol: Arc<WeixinProtocol>,
    ) -> Arc<Self> {
        Arc::new(Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State {
                    connection: None,
                    cancellation: CancellationToken::new(),
                    status: json!({"state":"idle","lastError":"","connectedAt":null,"lastEventAt":null}),
                }),
                protocol,
                callbacks,
                operations: Arc::new(Operations::default()),
            }),
            monitor: AsyncMutex::new(None),
        })
    }
    async fn stop(&self) -> Result<()> {
        self.inner.state.lock().unwrap().cancellation.cancel();
        let mut monitor = self.monitor.lock().await;
        self.inner.state.lock().unwrap().cancellation.cancel();
        if let Some(handle) = monitor.take() {
            handle.await.map_err(|e| error(e.to_string()))?;
        }
        self.inner.operations.join().await;
        let connection = self.inner.state.lock().unwrap().connection.take();
        if let Some(connection) = connection {
            let _ = self.inner.protocol.notify_stop(&connection).await;
        }
        self.inner
            .set_status(json!({"state":"idle","connectedAt":null}));
        Ok(())
    }
}
impl Gateway for WeixinGateway {
    fn get_status(&self) -> Value {
        self.inner.status()
    }
    fn connect(&self, connection: Value) -> BoxFuture<'_, Result<Value>> {
        Box::pin(async move {
            self.stop().await?;
            let mut monitor = self.monitor.lock().await;
            let cancellation = CancellationToken::new();
            {
                let mut state = self.inner.state.lock().unwrap();
                state.connection = Some(connection.clone());
                state.cancellation = cancellation.clone();
            }
            self.inner
                .set_status(json!({"state":"connecting","lastError":""}));
            match self
                .inner
                .protocol
                .notify_start(&connection, &cancellation)
                .await
            {
                Ok(_) => {
                    self.inner.set_status(
                        json!({"state":"connected","connectedAt":now_iso(),"lastError":""}),
                    );
                    let inner = self.inner.clone();
                    *monitor = Some(tokio::spawn(async move {
                        inner.monitor(connection, cancellation).await
                    }));
                    Ok(self.inner.status())
                }
                Err(e) => {
                    self.inner
                        .set_status(json!({"state":"failed","lastError":e.message}));
                    Err(e)
                }
            }
        })
    }
    fn disconnect(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(self.stop())
    }
    fn send(&self, message: Value, payload: Value) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let _lease = self.inner.operations.lease();
            let (connection, token) = self.inner.connection()?;
            let content = if truthy(&payload["markdown"]) {
                payload["markdown"].clone()
            } else {
                payload["text"].clone()
            };
            self.inner
                .protocol
                .send_text(
                    &connection,
                    message["peerId"].clone(),
                    content,
                    message["contextToken"].clone(),
                    &token,
                )
                .await?;
            Ok(())
        })
    }
    fn send_to_peer(
        &self,
        peer_id: String,
        payload: Value,
        scope: Value,
    ) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let _lease = self.inner.operations.lease();
            let (connection, token) = self.inner.connection()?;
            if truthy(&payload["path"]) {
                self.inner
                    .protocol
                    .send_media(
                        &connection,
                        json!(peer_id),
                        &payload,
                        scope["contextToken"].clone(),
                        &token,
                    )
                    .await?;
            } else {
                let content = if truthy(&payload["markdown"]) {
                    payload["markdown"].clone()
                } else {
                    payload["text"].clone()
                };
                self.inner
                    .protocol
                    .send_text(
                        &connection,
                        json!(peer_id),
                        content,
                        scope["contextToken"].clone(),
                        &token,
                    )
                    .await?;
            }
            Ok(())
        })
    }
    fn send_asset(&self, peer_id: String, asset: Value, scope: Value) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let _lease = self.inner.operations.lease();
            let (connection, token) = self.inner.connection()?;
            self.inner
                .protocol
                .send_media(
                    &connection,
                    json!(peer_id),
                    &asset,
                    scope["contextToken"].clone(),
                    &token,
                )
                .await?;
            Ok(())
        })
    }
    fn download_resources(&self, resources: Value) -> BoxFuture<'_, Result<Vec<Resource>>> {
        Box::pin(async move {
            let _lease = self.inner.operations.lease();
            let (connection, token) = self.inner.connection()?;
            let mut out = Vec::new();
            let mut total = 0usize;
            for item in resources.as_array().into_iter().flatten().take(8) {
                if let Some(resource) = self
                    .inner
                    .protocol
                    .download_item(&connection, item, &token)
                    .await?
                {
                    total = total.saturating_add(resource.bytes.len());
                    if total > 24 * 1024 * 1024 {
                        return Err(error("微信附件总大小超过 24 MB。"));
                    }
                    out.push(resource);
                }
            }
            Ok(out)
        })
    }
}
