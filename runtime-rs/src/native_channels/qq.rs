//! QQ 官方机器人：访问令牌、WebSocket 会话恢复与 OpenAPI 共用连接取消所有者。
#[path = "feishu_qq/common.rs"]
mod common;
use super::{ChannelError, Gateway, GatewayCallbacks, Resource, Result};
use common::{Live, Status};
use futures::{future::BoxFuture, SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::Mutex as AsyncMutex;
use tokio_tungstenite::tungstenite::Message;

#[derive(Clone)]
pub(crate) struct Endpoints {
    pub(crate) api: String,
    pub(crate) token: String,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            api: "https://api.sgroup.qq.com".into(),
            token: "https://bots.qq.com/app/getAppAccessToken".into(),
        }
    }
}
struct Authentication {
    token: String,
    until: Instant,
}
struct Session {
    native_protocol: bool,
    config: Value,
    endpoints: Endpoints,
    client: reqwest::Client,
    authentication: AsyncMutex<Authentication>,
    live: Arc<Live>,
}
struct QqGateway {
    native_protocol: bool,
    status: Arc<Status>,
    endpoints: Endpoints,
    session: Mutex<Option<Arc<Session>>>,
    task: AsyncMutex<Option<tokio::task::JoinHandle<()>>>,
    connecting: AsyncMutex<()>,
}
pub(crate) fn new(callbacks: GatewayCallbacks) -> Arc<dyn Gateway> {
    new_with_endpoints(callbacks, Endpoints::default())
}
pub(crate) fn new_with_endpoints(
    callbacks: GatewayCallbacks,
    endpoints: Endpoints,
) -> Arc<dyn Gateway> {
    new_protocol_with_endpoints(callbacks, endpoints, false)
}
fn new_protocol_with_endpoints(
    callbacks: GatewayCallbacks,
    endpoints: Endpoints,
    native_protocol: bool,
) -> Arc<dyn Gateway> {
    Arc::new(QqGateway {
        native_protocol,
        status: Status::new(callbacks, json!({"lastEventAt":null,"bot":null})),
        endpoints,
        session: Mutex::new(None),
        task: AsyncMutex::new(None),
        connecting: AsyncMutex::new(()),
    })
}
impl Session {
    async fn access_token(&self) -> Result<String> {
        let mut authentication = self.authentication.lock().await;
        if !authentication.token.is_empty() && Instant::now() < authentication.until {
            return Ok(authentication.token.clone());
        }
        let secret = common::text(&self.config["appSecret"]);
        if secret.is_empty() {
            return Err(ChannelError::new(
                "QQ 未提供可用的 App Secret/Token，无法鉴权。",
            ));
        }
        let value = common::json_response(
            self.client
                .post(&self.endpoints.token)
                .json(&json!({"appId":common::text(&self.config["appId"]),"clientSecret":secret})),
            &self.live.token,
            "QQ 获取访问令牌",
        )
        .await?;
        let token = common::text(&value["access_token"]);
        if token.is_empty() {
            return Err(ChannelError::new("QQ 鉴权响应缺少 access_token。"));
        }
        let expiry = value["expires_in"]
            .as_u64()
            .or_else(|| {
                value["expires_in"]
                    .as_str()
                    .and_then(|value| value.parse().ok())
            })
            .unwrap_or(7200);
        authentication.token = token.clone();
        authentication.until =
            Instant::now() + Duration::from_secs(expiry.saturating_sub(60).max(1));
        Ok(token)
    }
    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
        label: &str,
    ) -> Result<Value> {
        let token = self.access_token().await?;
        let mut request = self
            .client
            .request(
                method,
                format!("{}{}", self.endpoints.api.trim_end_matches('/'), path),
            )
            .header("authorization", format!("QQBot {token}"));
        if self.native_protocol {
            request = request.header("X-Union-Appid", common::text(&self.config["appId"]));
        }
        if let Some(body) = body {
            request = request.json(&body)
        }
        common::json_response(request, &self.live.token, label).await
    }
    async fn message(&self, peer: &str, chat_type: &str, body: Value) -> Result<()> {
        let kind = match chat_type {
            "channel" => "channels",
            "group" => "groups",
            _ => "users",
        };
        self.request(
            reqwest::Method::POST,
            &format!("/v2/{kind}/{}/messages", common::segment(peer)),
            Some(body),
            "QQ 发送消息",
        )
        .await?;
        Ok(())
    }
}
impl QqGateway {
    fn current(&self) -> Result<Arc<Session>> {
        self.session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or_else(|| ChannelError::new("QQ 机器人尚未连接。"))
    }
    async fn stop(&self) {
        let session = self
            .session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(session) = &session {
            session.live.token.cancel()
        }
        if let Some(task) = self.task.lock().await.take() {
            let _ = task.await;
        }
        if let Some(session) = session {
            session.live.stop().await
        }
        self.status
            .patch(json!({"state":"idle","connectedAt":null,"bot":null}));
    }
    async fn connect_owned(&self, config: Value) -> Result<Value> {
        let _connecting = self.connecting.lock().await;
        self.stop().await;
        if common::text(&config["appId"]).trim().is_empty() {
            return Err(ChannelError::new("QQ App ID 不能为空。"));
        }
        if common::text(&config["appSecret"]).trim().is_empty()
            && common::text(&config["token"]).trim().is_empty()
        {
            return Err(ChannelError::new("QQ App Secret/Token 不能为空。"));
        }
        let mut endpoints = self.endpoints.clone();
        if let Some(base) = config["baseUrl"].as_str().filter(|value| !value.is_empty()) {
            endpoints.api = base.to_owned()
        }
        let session = Arc::new(Session {
            native_protocol: self.native_protocol,
            authentication: AsyncMutex::new(Authentication {
                token: if common::text(&config["appSecret"]).is_empty() {
                    common::text(&config["token"])
                } else {
                    String::new()
                },
                until: Instant::now() + Duration::from_secs(24 * 60 * 60),
            }),
            config,
            endpoints,
            client: common::client(),
            live: Live::new(),
        });
        *self
            .session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(session.clone());
        self.status
            .patch(json!({"state":"connecting","lastError":"","bot":null}));
        let result=async{
            let _operation=session.live.enter()?;
            session.access_token().await?;
            let bot=session.request(reqwest::Method::GET,"/users/@me",None,"QQ 获取机器人身份").await.unwrap_or(Value::Null);
            let value=session.request(reqwest::Method::GET,"/gateway",None,"QQ 获取 WebSocket 地址").await?;
            let url=value["url"].as_str().filter(|value|!value.is_empty()).ok_or_else(||ChannelError::new("QQ 网关响应缺少 WebSocket 地址。"))?.to_owned();
            let socket=tokio::select!{biased;_=session.live.token.cancelled()=>return Err(common::cancelled()),socket=tokio::time::timeout(Duration::from_secs(15),tokio_tungstenite::connect_async(&url))=>socket.map_err(|_|ChannelError::new("QQ WebSocket 连接超时。"))?.map_err(|error|ChannelError::new(format!("QQ WebSocket 连接失败：{error}")))?.0};
            self.status.patch(json!({"state":"connected","connectedAt":common::now(),"lastError":"","bot":bot}));
            let status=self.status.clone();let session=session.clone();
            *self.task.lock().await=Some(tokio::spawn(async move {gateway_loop(session,status,url,socket).await}));
            Ok(self.status.get())
        }.await;
        if let Err(error) = &result {
            self.stop().await;
            self.status
                .patch(json!({"state":"failed","lastError":safe_error(&error.message)}));
        }
        result
    }
    async fn send_owned(
        &self,
        peer: String,
        payload: Value,
        scope: Value,
        reply: Option<String>,
    ) -> Result<()> {
        let session = self.current()?;
        let _operation = session.live.enter()?;
        let content = payload
            .get("markdown")
            .filter(|v| !common::text(v).is_empty())
            .or_else(|| payload.get("text"))
            .map(common::text)
            .unwrap_or_default();
        let mut body = json!({"content":common::clip(&content,4000),"msg_type":0});
        if let Some(reply) = reply.filter(|value| !value.is_empty()) {
            body["msg_id"] = json!(reply)
        }
        session
            .message(&peer, scope["chatType"].as_str().unwrap_or("p2p"), body)
            .await
    }
    async fn asset_owned(&self, peer: String, asset: Value, scope: Value) -> Result<()> {
        let session = self.current()?;
        let _operation = session.live.enter()?;
        let chat_type = scope["chatType"].as_str().unwrap_or("p2p");
        if !session.native_protocol {
            let url = asset["url"].as_str().filter(|value| !value.is_empty());
            let content = if let Some(url) = url {
                url.to_owned()
            } else if common::text(&asset["mimeType"]).starts_with("image/") {
                "图片附件暂不支持直接上传，请使用图片 URL。".into()
            } else {
                format!(
                    "文件：{}",
                    asset["name"]
                        .as_str()
                        .filter(|value| !value.is_empty())
                        .unwrap_or("附件")
                )
            };
            return session
                .message(&peer, chat_type, json!({"content":content,"msg_type":0}))
                .await;
        }
        if let Some(url) = asset["url"].as_str().filter(|v| !v.is_empty()) {
            let fallback = if common::text(&asset["mimeType"]).starts_with("image/") {
                url.to_owned()
            } else {
                url.to_owned()
            };
            return session
                .message(&peer, chat_type, json!({"content":fallback,"msg_type":0}))
                .await;
        }
        let path = common::text(&asset["path"]);
        if path.is_empty() {
            let text = if common::text(&asset["mimeType"]).starts_with("image/") {
                "图片附件暂不支持直接上传，请使用图片 URL。".into()
            } else {
                format!("文件：{}", asset["name"].as_str().unwrap_or("附件"))
            };
            return session
                .message(&peer, chat_type, json!({"content":text,"msg_type":0}))
                .await;
        }
        let data = tokio::select! {biased;_=session.live.token.cancelled()=>return Err(common::cancelled()),data=tokio::fs::read(&path)=>data.map_err(|error|ChannelError::new(error.to_string()))?};
        let mime = common::text(&asset["mimeType"]);
        let file_type = if mime.starts_with("image/") {
            1
        } else if mime.starts_with("video/") {
            2
        } else if mime.starts_with("audio/") {
            3
        } else {
            4
        };
        let kind = if chat_type == "group" {
            "groups"
        } else {
            "users"
        };
        use base64::Engine;
        let value=session.request(reqwest::Method::POST,&format!("/v2/{kind}/{}/files",common::segment(&peer)),Some(json!({"file_type":file_type,"file_data":base64::engine::general_purpose::STANDARD.encode(data),"srv_send_msg":false})),"QQ 上传文件").await?;
        let info = value["file_info"]
            .as_str()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| ChannelError::new("QQ 上传文件响应缺少 file_info。"))?;
        session
            .message(
                &peer,
                chat_type,
                json!({"msg_type":7,"media":{"file_info":info}}),
            )
            .await
    }
}
impl Gateway for QqGateway {
    fn get_status(&self) -> Value {
        self.status.get()
    }
    fn connect(&self, connection: Value) -> BoxFuture<'_, Result<Value>> {
        Box::pin(self.connect_owned(connection))
    }
    fn disconnect(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            if let Some(session) = self
                .session
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
            {
                session.live.token.cancel()
            }
            let _connecting = self.connecting.lock().await;
            self.stop().await;
            Ok(())
        })
    }
    fn send(&self, message: Value, payload: Value) -> BoxFuture<'_, Result<()>> {
        Box::pin(self.send_owned(
            common::text(&message["peerId"]),
            payload,
            message.clone(),
            Some(common::text(&message["messageId"])),
        ))
    }
    fn send_to_peer(
        &self,
        peer_id: String,
        payload: Value,
        scope: Value,
    ) -> BoxFuture<'_, Result<()>> {
        Box::pin(self.send_owned(peer_id, payload, scope, None))
    }
    fn send_asset(&self, peer_id: String, asset: Value, scope: Value) -> BoxFuture<'_, Result<()>> {
        Box::pin(self.asset_owned(peer_id, asset, scope))
    }
    fn download_resources(&self, resources: Value) -> BoxFuture<'_, Result<Vec<Resource>>> {
        Box::pin(async move {
            if !self.native_protocol {
                return Ok(Vec::new());
            }
            let session = self.current()?;
            let _operation = session.live.enter()?;
            let mut result = Vec::new();
            let mut total = 0;
            for resource in resources.as_array().into_iter().flatten().take(8) {
                let Some(url) = resource["url"].as_str() else {
                    continue;
                };
                let response =
                    common::response(session.client.get(url), &session.live.token).await?;
                if !response.status().is_success() {
                    return Err(ChannelError::new(format!(
                        "QQ 下载附件失败：HTTP {}",
                        response.status()
                    )));
                }
                let bytes =
                    common::bytes(response, &session.live.token, 24 * 1024 * 1024 - total).await?;
                total += bytes.len();
                result.push(Resource {
                    name: resource["fileName"].as_str().unwrap_or("附件").into(),
                    kind: resource["type"].as_str().unwrap_or("file").into(),
                    mime_type: resource["mimeType"].as_str().map(str::to_owned),
                    bytes,
                });
            }
            Ok(result)
        })
    }
}
fn safe_error(message: &str) -> String {
    static PATTERN: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let pattern = PATTERN.get_or_init(|| {
        regex::Regex::new(r"(?i)(Bot\s+)?\d+\.[^\s)]+").expect("QQ credential pattern")
    });
    common::clip(&pattern.replace_all(message, "Bot ***"), 1000)
}
pub(crate) fn map_message(kind: &str, raw: &Value) -> Option<Value> {
    map_protocol_message(kind, raw, false)
}
fn map_protocol_message(kind: &str, raw: &Value, native_protocol: bool) -> Option<Value> {
    if !matches!(
        kind,
        "C2C_MESSAGE_CREATE" | "GROUP_AT_MESSAGE_CREATE" | "AT_MESSAGE_CREATE"
    ) {
        return None;
    }
    let sender = raw["author"]
        .get("user_openid")
        .or_else(|| raw["author"].get("id"))
        .map(common::text)
        .unwrap_or_default();
    let peer = if kind == "C2C_MESSAGE_CREATE" {
        sender.clone()
    } else {
        raw.get("group_openid")
            .or_else(|| raw.get("group_id"))
            .or_else(|| raw.get("channel_id"))
            .map(common::text)
            .unwrap_or_default()
    };
    if peer.is_empty() || sender.is_empty() {
        return None;
    }
    let id = raw["id"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{sender}-{}", common::now_ms()));
    let resources = if !native_protocol {
        Vec::new()
    } else {
        raw["attachments"].as_array().into_iter().flatten().filter_map(|resource|{let url=resource["url"].as_str()?;let mime=resource["content_type"].as_str().unwrap_or("");Some(json!({"type":if mime.starts_with("image/"){"image"}else{"file"},"url":if url.starts_with("//"){format!("https:{url}")}else{url.to_owned()},"fileName":resource["filename"],"mimeType":mime,"size":resource["size"]}))}).collect::<Vec<_>>()
    };
    Some(
        json!({"messageId":id,"peerId":peer,"senderId":sender,"senderName":raw["author"].get("member_openid").or_else(||raw["author"].get("username")).map(common::text).unwrap_or_default(),"chatType":if kind=="AT_MESSAGE_CREATE"{"channel"}else if kind=="GROUP_AT_MESSAGE_CREATE"{"group"}else{"p2p"},"content":common::text(&raw["content"]).trim(),"resources":resources}),
    )
}
async fn gateway_loop(
    session: Arc<Session>,
    status: Arc<Status>,
    mut url: String,
    mut socket: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) {
    let mut sequence = Value::Null;
    let mut session_id = String::new();
    let mut seen = HashMap::<String, Instant>::new();
    loop {
        let mut heartbeat = Duration::from_millis(41250);
        let mut next = tokio::time::Instant::now() + heartbeat;
        let mut acknowledged = true;
        let mut terminal = false;
        loop {
            let item = tokio::select! {biased;_=session.live.token.cancelled()=>{let _=socket.close(None).await;return},_=tokio::time::sleep_until(next)=>{
                if session.native_protocol && !acknowledged{status.patch(json!({"lastError":"QQ 心跳确认超时。"}));break}
                if socket.send(Message::Text(json!({"op":1,"d":sequence}).to_string().into())).await.is_err(){break}
                acknowledged=false;next=tokio::time::Instant::now()+heartbeat;continue
            },item=socket.next()=>item};
            let Some(item) = item else { break };
            let packet = match item {
                Ok(Message::Text(text)) => serde_json::from_str::<Value>(&text),
                Ok(Message::Binary(bytes)) => serde_json::from_slice::<Value>(&bytes),
                Ok(Message::Ping(bytes)) => {
                    let _ = socket.send(Message::Pong(bytes)).await;
                    continue;
                }
                Ok(Message::Close(_)) | Err(_) => break,
                _ => continue,
            };
            let packet = match packet {
                Ok(packet) => packet,
                Err(error) => {
                    status.patch(json!({"lastError":format!("QQ WebSocket 消息解析失败：{}",safe_error(&error.to_string()))}));
                    continue;
                }
            };
            if !packet["s"].is_null() {
                sequence = packet["s"].clone();
            }
            match packet["op"].as_i64() {
                Some(10) => {
                    heartbeat = Duration::from_millis(
                        packet["d"]["heartbeat_interval"]
                            .as_u64()
                            .unwrap_or(41250)
                            .max(5000),
                    );
                    next = tokio::time::Instant::now() + heartbeat;
                    acknowledged = true;
                    let token = match session.access_token().await {
                        Ok(token) => token,
                        Err(error) => {
                            status.patch(json!({"lastError":safe_error(&error.message)}));
                            break;
                        }
                    };
                    let identify = if session.native_protocol && !session_id.is_empty() {
                        json!({"op":6,"d":{"token":format!("QQBot {token}"),"session_id":session_id,"seq":sequence}})
                    } else {
                        json!({"op":2,"d":{"token":format!("QQBot {token}"),"intents":session.config["intents"].as_u64().filter(|v|*v!=0).unwrap_or((1<<25)|(1<<26)|(1<<30)),"shard":[0,1],"properties":{"$os":if cfg!(windows){"win32"}else{std::env::consts::OS},"$browser":"pisper","$device":"pisper"}}})
                    };
                    if socket
                        .send(Message::Text(identify.to_string().into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Some(11) => acknowledged = true,
                Some(1) => {
                    if !session.native_protocol {
                        continue;
                    }
                    if socket
                        .send(Message::Text(
                            json!({"op":1,"d":sequence}).to_string().into(),
                        ))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Some(7) => {
                    status
                        .patch(json!({"state":"reconnecting","lastError":"QQ 网关要求重新连接。"}));
                    break;
                }
                Some(9) => {
                    if !session.native_protocol {
                        status.patch(json!({"state":"failed","lastError":"QQ 网关鉴权失败：请检查 App ID、App Secret/Token 与 intents 配置。"}));
                        continue;
                    }
                    session_id.clear();
                    sequence = Value::Null;
                    if packet["d"] != true {
                        status.patch(json!({"state":"failed","lastError":"QQ 网关鉴权失败：请检查 App ID、App Secret/Token 与 intents 配置。"}));
                        terminal = true;
                    }
                    break;
                }
                Some(0) => {
                    let kind = packet["t"].as_str().unwrap_or("");
                    if kind == "READY" {
                        session_id = common::text(&packet["d"]["session_id"]);
                        if let Some(next_url) = packet["d"]["resume_gateway_url"].as_str() {
                            url = next_url.to_owned()
                        }
                        if session.native_protocol {
                            status.sync(json!({"sessionId":session_id,"sequence":sequence}));
                        }
                    }
                    if session.native_protocol && kind == "RESUMED" {
                        status.patch(json!({"state":"connected","lastError":""}));
                    }
                    if let Some(message) =
                        map_protocol_message(kind, &packet["d"], session.native_protocol)
                    {
                        seen.retain(|_, at| at.elapsed() < Duration::from_secs(300));
                        let id = common::text(&message["messageId"]);
                        if session.native_protocol && seen.contains_key(&id) {
                            continue;
                        }
                        seen.insert(id, Instant::now());
                        status.patch(
                            json!({"state":"connected","lastEventAt":common::now(),"lastError":""}),
                        );
                        status.message(message);
                    }
                }
                _ => {}
            }
        }
        let _ = socket.close(None).await;
        if terminal || session.live.token.is_cancelled() {
            return;
        }
        status.patch(json!({"state":"reconnecting","lastError":"QQ WebSocket 已断开。"}));
        tokio::select! {biased;_=session.live.token.cancelled()=>return,_=tokio::time::sleep(Duration::from_secs(1))=>{}}
        loop {
            let mut refreshed_bot = Value::Null;
            if !session.native_protocol {
                status.patch(json!({"state":"idle","connectedAt":null,"bot":null}));
                status.patch(json!({"state":"connecting","lastError":"","bot":null}));
                if !common::text(&session.config["appSecret"]).is_empty() {
                    session.authentication.lock().await.until = Instant::now();
                }
                let refreshed = async {
                    session.access_token().await?;
                    refreshed_bot = session
                        .request(
                            reqwest::Method::GET,
                            "/users/@me",
                            None,
                            "QQ 获取机器人身份",
                        )
                        .await
                        .unwrap_or(Value::Null);
                    let gateway = session
                        .request(
                            reqwest::Method::GET,
                            "/gateway",
                            None,
                            "QQ 获取 WebSocket 地址",
                        )
                        .await?;
                    url = gateway["url"]
                        .as_str()
                        .filter(|value| !value.is_empty())
                        .ok_or_else(|| ChannelError::new("QQ 网关响应缺少 WebSocket 地址。"))?
                        .to_owned();
                    Ok::<(), ChannelError>(())
                }
                .await;
                if let Err(error) = refreshed {
                    session.live.token.cancel();
                    status.patch(
                        json!({"state":"failed","lastError":safe_error(&error.message),"bot":null}),
                    );
                    return;
                }
            }
            let connected = tokio::select! {biased;_=session.live.token.cancelled()=>return,value=tokio::time::timeout(Duration::from_secs(15),tokio_tungstenite::connect_async(&url))=>value};
            match connected {
                Ok(Ok((next_socket, _))) => {
                    socket = next_socket;
                    if session.native_protocol {
                        status.patch(json!({"state":"connected","lastError":""}));
                    } else {
                        status.patch(json!({"state":"connected","connectedAt":common::now(),"lastError":"","bot":refreshed_bot}));
                    }
                    break;
                }
                _ => {
                    if !session.native_protocol {
                        session.live.token.cancel();
                        status.patch(json!({"state":"failed","lastError":"QQ WebSocket 连接失败。","bot":null}));
                        return;
                    }
                    status.patch(json!({"state":"reconnecting","lastError":"QQ 重连失败。"}));
                    tokio::select! {biased;_=session.live.token.cancelled()=>return,_=tokio::time::sleep(Duration::from_secs(1))=>{}}
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "feishu_qq/qq_tests.rs"]
mod tests;
