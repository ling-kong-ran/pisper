//! Telegram Bot API 长轮询；任务句柄与共享协议状态分开，断开会加入正在发送的请求。
use super::weixin_protocol::{
    cancelled, error, now_iso, now_ms, pause, string, text, truthy, Operations,
};
use super::{Gateway, GatewayCallbacks, Resource, Result};
use futures::future::BoxFuture;
use reqwest::{
    multipart::{Form, Part},
    Client,
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tokio::{sync::Mutex as AsyncMutex, task::JoinHandle};
use tokio_util::sync::CancellationToken;

pub(crate) const TELEGRAM_API_BASE: &str = "https://api.telegram.org";
const FILE_BASE: &str = "https://api.telegram.org/file";
struct State {
    connection: Option<Value>,
    cancellation: CancellationToken,
    offset: i64,
    status: Value,
}
struct Inner {
    state: Mutex<State>,
    client: Client,
    callbacks: GatewayCallbacks,
    operations: Arc<Operations>,
}
pub(crate) struct TelegramGateway {
    inner: Arc<Inner>,
    monitor: AsyncMutex<Option<JoinHandle<()>>>,
}

fn valid_token(value: &Value) -> Result<String> {
    let token = if truthy(value) {
        string(value).trim().to_owned()
    } else {
        String::new()
    };
    if !regex::Regex::new(r"^\d+:[A-Za-z0-9_-]+$")
        .unwrap()
        .is_match(&token)
    {
        return Err(error("Telegram Bot Token 格式无效。"));
    }
    Ok(token)
}
fn safe_error(message: &str) -> String {
    let redacted = regex::Regex::new(r"(?i)bot\d+:[A-Za-z0-9_-]+")
        .unwrap()
        .replace_all(message, "bot***:***");
    String::from_utf16_lossy(&redacted.encode_utf16().take(1000).collect::<Vec<_>>())
}
fn truncate_utf16(value: &str, limit: usize) -> String {
    String::from_utf16_lossy(&value.encode_utf16().take(limit).collect::<Vec<_>>())
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
            .ok_or_else(|| error("Telegram 机器人尚未连接。"))
    }
    async fn request(
        &self,
        connection: &Value,
        method: &str,
        body: Value,
        form: Option<Form>,
        token: &CancellationToken,
    ) -> Result<Value> {
        let base = text(connection, "baseUrl");
        let base = if base.is_empty() {
            TELEGRAM_API_BASE
        } else {
            &base
        };
        let url = format!(
            "{}/bot{}/{}",
            base.strip_suffix('/').unwrap_or(base),
            valid_token(&connection["token"])?,
            method
        );
        cancelled(token, 0, async {
            let request = self.client.post(url);
            let request = if let Some(form) = form {
                request.multipart(form)
            } else {
                request
                    .header("content-type", "application/json")
                    .body(body.to_string())
            };
            let response = request.send().await.map_err(|e| {
                error(format!(
                    "Telegram {method} 请求失败：{}",
                    safe_error(&e.to_string())
                ))
            })?;
            let status = response.status();
            let payload = response.json::<Value>().await.map_err(|_| {
                error(format!(
                    "Telegram {method} 返回了无效响应（HTTP {}）。",
                    status.as_u16()
                ))
            })?;
            if !status.is_success() || payload["ok"] != true {
                let description = if truthy(&payload["description"]) {
                    string(&payload["description"])
                } else {
                    format!("HTTP {}", status.as_u16())
                };
                return Err(error(format!(
                    "Telegram {method} 失败：{}",
                    truncate_utf16(&description, 500)
                )));
            }
            Ok(payload["result"].clone())
        })
        .await
    }
    async fn monitor(self: Arc<Self>, connection: Value, token: CancellationToken) {
        let mut failures: u64 = 0;
        while !token.is_cancelled() {
            let offset = self.state.lock().unwrap().offset;
            match self
                .request(
                    &connection,
                    "getUpdates",
                    json!({"offset":offset,"timeout":30,"allowed_updates":["message"]}),
                    None,
                    &token,
                )
                .await
            {
                Ok(updates) => {
                    if token.is_cancelled() {
                        return;
                    }
                    failures = 0;
                    self.set_status(
                        json!({"state":"connected","lastEventAt":now_iso(),"lastError":""}),
                    );
                    let items = updates.as_array().cloned().unwrap_or_default();
                    for update in &items {
                        // API 的 offset 是下一次请求边界；消息去重归渠道服务所有。
                        if let Some(id) = update["update_id"].as_i64() {
                            self.state.lock().unwrap().offset = id.saturating_add(1);
                        }
                        if let Some(message) = map_message(&update["message"]) {
                            (self.callbacks.on_message)(message);
                        }
                    }
                    pause(&token, if items.is_empty() { 50 } else { 0 }).await;
                }
                Err(e) => {
                    if token.is_cancelled() {
                        return;
                    }
                    failures += 1;
                    self.set_status(json!({"state":if failures>=3{"reconnecting"}else{"connected"},"lastError":safe_error(&e.message)}));
                    pause(&token, (failures * 2000).min(30000)).await;
                }
            }
        }
    }
    async fn send_message(
        &self,
        peer: Value,
        payload: &Value,
        reply: Option<&Value>,
    ) -> Result<()> {
        let _lease = self.operations.lease();
        let (connection, token) = self.connection()?;
        let content = if truthy(&payload["markdown"]) {
            &payload["markdown"]
        } else {
            &payload["text"]
        };
        let mut body = json!({"chat_id":string(&peer),"text":truncate_utf16(&if truthy(content){string(content)}else{String::new()},4096)});
        if let Some(reply) = reply.filter(|reply| truthy(reply)) {
            let numeric = match reply {
                Value::Number(v) => v.as_f64(),
                Value::String(v) => v.trim().parse::<f64>().ok(),
                Value::Bool(v) => Some(if *v { 1.0 } else { 0.0 }),
                _ => None,
            };
            let numeric = numeric
                .filter(|v| v.is_finite())
                .map(|v| {
                    if v.fract() == 0.0 && v >= i64::MIN as f64 && v < i64::MAX as f64 {
                        json!(v as i64)
                    } else {
                        json!(v)
                    }
                })
                .unwrap_or(Value::Null);
            body["reply_parameters"] = json!({"message_id":numeric});
        }
        self.request(&connection, "sendMessage", body, None, &token)
            .await?;
        Ok(())
    }
}

pub(crate) fn map_message(raw: &Value) -> Option<Value> {
    if !truthy(&raw["chat"]["id"]) || !truthy(&raw["from"]["id"]) {
        return None;
    }
    let mut resources = Vec::new();
    if let Some(photo) = raw["photo"].as_array().and_then(|v| v.last()) {
        resources.push(json!({"type":"image","fileId":photo["file_id"],"name":format!("telegram-{}.jpg",string(&photo["file_id"]))}));
    }
    if truthy(&raw["document"]["file_id"]) {
        resources.push(json!({"type":"file","fileId":raw["document"]["file_id"],"name":if truthy(&raw["document"]["file_name"]){text(&raw["document"],"file_name")}else{"telegram-document".into()}}));
    }
    let names = [&raw["from"]["first_name"], &raw["from"]["last_name"]]
        .into_iter()
        .filter(|v| truthy(v))
        .map(string)
        .collect::<Vec<_>>()
        .join(" ");
    let message_id = if truthy(&raw["message_id"]) {
        string(&raw["message_id"])
    } else {
        format!(
            "{}-{}",
            string(&raw["chat"]["id"]),
            if truthy(&raw["date"]) {
                string(&raw["date"])
            } else {
                now_ms().to_string()
            }
        )
    };
    Some(
        json!({"messageId":message_id,"peerId":string(&raw["chat"]["id"]),"senderId":string(&raw["from"]["id"]),"senderName":if names.is_empty(){text(&raw["from"],"username")}else{names},"chatType":if raw["chat"]["type"]=="private"{"p2p"}else{"group"},"content":if truthy(&raw["text"]){text(raw,"text")}else{text(raw,"caption")},"resources":resources}),
    )
}
impl TelegramGateway {
    pub(crate) fn new(callbacks: GatewayCallbacks) -> Arc<Self> {
        Self::with_client(callbacks, Client::new())
    }
    pub(crate) fn with_client(callbacks: GatewayCallbacks, client: Client) -> Arc<Self> {
        Arc::new(Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State {
                    connection: None,
                    cancellation: CancellationToken::new(),
                    offset: 0,
                    status: json!({"state":"idle","lastError":"","connectedAt":null,"lastEventAt":null,"bot":null}),
                }),
                client,
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
        self.inner.state.lock().unwrap().connection = None;
        self.inner
            .set_status(json!({"state":"idle","connectedAt":null,"bot":null}));
        Ok(())
    }
}
impl Gateway for TelegramGateway {
    fn get_status(&self) -> Value {
        self.inner.status()
    }
    fn connect(&self, mut connection: Value) -> BoxFuture<'_, Result<Value>> {
        Box::pin(async move {
            self.stop().await?;
            let token = valid_token(&connection["token"])?;
            connection["token"] = json!(token);
            let mut monitor = self.monitor.lock().await;
            let cancellation = CancellationToken::new();
            {
                let mut state = self.inner.state.lock().unwrap();
                state.offset = connection["offset"].as_i64().unwrap_or(0);
                state.connection = Some(connection.clone());
                state.cancellation = cancellation.clone();
            }
            self.inner
                .set_status(json!({"state":"connecting","lastError":"","bot":null}));
            match self
                .inner
                .request(&connection, "getMe", json!({}), None, &cancellation)
                .await
            {
                Ok(bot) if truthy(&bot["id"]) => {
                    self.inner.set_status(json!({"state":"connected","connectedAt":now_iso(),"lastError":"","bot":{"id":string(&bot["id"]),"name":if truthy(&bot["first_name"]){text(&bot,"first_name")}else if truthy(&bot["username"]){text(&bot,"username")}else{"Telegram Bot".into()},"username":text(&bot,"username")}}));
                    let inner = self.inner.clone();
                    *monitor = Some(tokio::spawn(async move {
                        inner.monitor(connection, cancellation).await
                    }));
                    Ok(self.inner.status())
                }
                result => {
                    let failure = result
                        .err()
                        .unwrap_or_else(|| error("Telegram getMe 未返回机器人身份。"));
                    self.inner.set_status(
                        json!({"state":"failed","lastError":safe_error(&failure.message)}),
                    );
                    self.inner.state.lock().unwrap().connection = None;
                    Err(failure)
                }
            }
        })
    }
    fn disconnect(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(self.stop())
    }
    fn send(&self, message: Value, payload: Value) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            self.inner
                .send_message(
                    message["peerId"].clone(),
                    &payload,
                    Some(&message["messageId"]),
                )
                .await
        })
    }
    fn send_to_peer(
        &self,
        peer_id: String,
        payload: Value,
        _scope: Value,
    ) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            self.inner
                .send_message(json!(peer_id), &payload, None)
                .await
        })
    }
    fn send_asset(
        &self,
        peer_id: String,
        asset: Value,
        _scope: Value,
    ) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let _lease = self.inner.operations.lease();
            let (connection, token) = self.inner.connection()?;
            let bytes = cancelled(&token, 0, async {
                tokio::fs::read(text(&asset, "path"))
                    .await
                    .map_err(|e| error(e.to_string()))
            })
            .await?;
            let mime = text(&asset, "mimeType");
            let image = mime.starts_with("image/");
            let mime = if mime.is_empty() {
                "application/octet-stream"
            } else {
                &mime
            };
            let name = text(&asset, "name");
            let name = if name.is_empty() {
                if image {
                    "image"
                } else {
                    "document"
                }
            } else {
                &name
            };
            let part = Part::bytes(bytes)
                .mime_str(mime)
                .map_err(|e| error(e.to_string()))?
                .file_name(name.to_owned());
            let form = Form::new()
                .text("chat_id", peer_id)
                .part(if image { "photo" } else { "document" }, part);
            self.inner
                .request(
                    &connection,
                    if image { "sendPhoto" } else { "sendDocument" },
                    Value::Null,
                    Some(form),
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
            let mut result = Vec::new();
            let mut total = 0usize;
            for resource in resources.as_array().into_iter().flatten().take(8) {
                if !truthy(&resource["fileId"]) {
                    continue;
                }
                let file = self
                    .inner
                    .request(
                        &connection,
                        "getFile",
                        json!({"file_id":resource["fileId"]}),
                        None,
                        &token,
                    )
                    .await?;
                if !truthy(&file["file_path"]) {
                    continue;
                }
                let base = text(&connection, "fileBaseUrl");
                let base = if base.is_empty() { FILE_BASE } else { &base };
                let url = format!(
                    "{}/bot{}/{}",
                    base.strip_suffix('/').unwrap_or(base),
                    valid_token(&connection["token"])?,
                    text(&file, "file_path")
                );
                let (bytes, mime) = cancelled(&token, 0, async {
                    let response = self.inner.client.get(url).send().await.map_err(|e| {
                        error(format!(
                            "Telegram 附件下载请求失败：{}",
                            safe_error(&e.to_string())
                        ))
                    })?;
                    if !response.status().is_success() {
                        return Err(error(format!(
                            "Telegram 附件下载失败（HTTP {}）。",
                            response.status().as_u16()
                        )));
                    }
                    let mime = response
                        .headers()
                        .get("content-type")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("")
                        .to_owned();
                    let bytes = response
                        .bytes()
                        .await
                        .map_err(|e| error(e.to_string()))?
                        .to_vec();
                    Ok((bytes, mime))
                })
                .await?;
                total = total.saturating_add(bytes.len());
                if total > 24 * 1024 * 1024 {
                    return Err(error("Telegram 附件总大小超过 24 MB。"));
                }
                result.push(Resource {
                    name: text(resource, "name"),
                    kind: text(resource, "type"),
                    mime_type: Some(mime),
                    bytes,
                });
            }
            Ok(result)
        })
    }
}
