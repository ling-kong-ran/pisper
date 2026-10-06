//! 飞书原生长连接：SDK1.72.0 的端点发现、pbbp2 帧、消息安全策略和 OpenAPI。
#[path = "feishu_qq/common.rs"]
mod common;
#[path = "feishu_qq/frame.rs"]
mod frame;
#[path = "feishu_qq/http_tunnel.rs"]
mod http_tunnel;
#[path = "feishu_qq/markdown.rs"]
mod markdown;
#[path = "feishu_qq/media_proxy.rs"]
mod media_proxy;
#[path = "feishu_qq/normalize.rs"]
mod normalize;
use super::{ChannelError, Gateway, GatewayCallbacks, Resource, Result};
use base64::Engine;
use common::{Live, Status};
use frame::{Frame, Header};
use futures::{future::BoxFuture, SinkExt, StreamExt};
use markdown::{optimize_markdown, split_markdown};
use prost::Message as ProtobufMessage;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::Mutex as AsyncMutex;
use tokio_tungstenite::tungstenite::Message;
type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
#[derive(Clone, Copy, PartialEq)]
enum SendFailure {
    Unknown,
    RateLimited,
    Permission,
    Format,
    Revoked,
    Timeout,
    Cancelled,
}
struct DiscoveryFailure {
    error: ChannelError,
    retryable: bool,
}
impl From<DiscoveryFailure> for ChannelError {
    fn from(value: DiscoveryFailure) -> Self {
        value.error
    }
}
#[derive(Clone)]
pub(crate) struct Endpoints {
    pub(crate) api: Option<String>,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self { api: None }
    }
}
struct Token {
    value: String,
    until: Instant,
}
struct Session {
    config: Value,
    base: String,
    client: reqwest::Client,
    token: AsyncMutex<Token>,
    live: Arc<Live>,
    bot: Mutex<String>,
    fragment_epoch: Mutex<tokio::time::Instant>,
}
struct FeishuGateway {
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
    Arc::new(FeishuGateway {
        status: Status::new(
            callbacks,
            json!({"reconnectAttempts":0,"lastConnectTime":null,"bot":null}),
        ),
        endpoints,
        session: Mutex::new(None),
        task: AsyncMutex::new(None),
        connecting: AsyncMutex::new(()),
    })
}
impl Session {
    async fn send_request(
        &self,
        path: &str,
        body: Value,
    ) -> std::result::Result<Value, (ChannelError, SendFailure)> {
        let token = self
            .authorization()
            .await
            .map_err(|error| (error, SendFailure::Unknown))?;
        let request = self
            .client
            .post(format!("{}{}", self.base, path))
            .bearer_auth(token)
            .json(&body);
        let response = tokio::select! {biased;_=self.live.token.cancelled()=>return Err((common::cancelled(),SendFailure::Cancelled)),response=request.send()=>response.map_err(|error|{let class=if error.is_timeout(){SendFailure::Timeout}else{SendFailure::Unknown};(ChannelError::new(error.without_url().to_string()),class)})?};
        let status = response.status().as_u16();
        let value: Value = tokio::select! {biased;_=self.live.token.cancelled()=>return Err((common::cancelled(),SendFailure::Cancelled)),value=response.json()=>value.map_err(|_|(ChannelError::new("飞书接口返回无效 JSON。"),SendFailure::Unknown))?};
        let code = value["code"].as_i64().unwrap_or(0);
        if (200..300).contains(&status) && code == 0 {
            return Ok(value);
        }
        let class = match code {
            230020 | 230017 => SendFailure::Revoked,
            99991400 | 99991401 => SendFailure::Permission,
            230002 | 230001 => SendFailure::Format,
            _ => match status {
                429 => SendFailure::RateLimited,
                401 | 403 => SendFailure::Permission,
                400 => SendFailure::Format,
                404 => SendFailure::Revoked,
                _ => SendFailure::Unknown,
            },
        };
        Err((
            ChannelError::new(
                value["msg"]
                    .as_str()
                    .or_else(|| value["message"].as_str())
                    .unwrap_or("飞书接口失败。"),
            ),
            class,
        ))
    }
    async fn authorization(&self) -> Result<String> {
        let mut token = self.token.lock().await;
        if !token.value.is_empty() && Instant::now() < token.until {
            return Ok(token.value.clone());
        }
        let value=self.raw(self.client.post(format!("{}/open-apis/auth/v3/tenant_access_token/internal",self.base)).json(&json!({"app_id":self.config["appId"],"app_secret":self.config["appSecret"]})),"飞书获取访问令牌").await?;
        let value_token = value["tenant_access_token"]
            .as_str()
            .filter(|v| !v.is_empty())
            .ok_or_else(|| ChannelError::new("飞书鉴权响应缺少 tenant_access_token。"))?
            .to_owned();
        token.value = value_token.clone();
        token.until = Instant::now()
            + Duration::from_secs(
                value["expire"]
                    .as_u64()
                    .unwrap_or(7200)
                    .saturating_sub(180)
                    .max(1),
            );
        Ok(value_token)
    }
    async fn raw(&self, request: reqwest::RequestBuilder, label: &str) -> Result<Value> {
        let value = common::json_response(
            request.header(
                "User-Agent",
                "larksuiteoapi/node-sdk/1.72.0 source/pisper channel",
            ),
            &self.live.token,
            label,
        )
        .await?;
        if value["code"].as_i64().unwrap_or(0) != 0 {
            return Err(ChannelError::new(format!(
                "{} (code {})",
                value["msg"].as_str().unwrap_or("飞书接口失败。"),
                value["code"]
            )));
        }
        Ok(value)
    }
    async fn api(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> Result<Value> {
        let token = self.authorization().await?;
        let mut request = self
            .client
            .request(method, format!("{}{}", self.base, path))
            .bearer_auth(token);
        if let Some(body) = body {
            request = request.json(&body)
        }
        self.raw(request, "飞书接口").await
    }
    async fn discover(&self) -> std::result::Result<(String, Value, i32), DiscoveryFailure> {
        let value = common::json_response(
            self.client
                .post(format!("{}/callback/ws/endpoint", self.base))
                .header("locale", "zh")
                .header(
                    "User-Agent",
                    "larksuiteoapi/node-sdk/1.72.0 source/pisper channel",
                )
                .json(&json!({"AppID":self.config["appId"],"AppSecret":self.config["appSecret"]})),
            &self.live.token,
            "飞书 WebSocket 地址",
        )
        .await
        .map_err(|error| DiscoveryFailure {
            retryable: !self.live.token.is_cancelled(),
            error,
        })?;
        let code = value["code"].as_i64().unwrap_or(-1);
        if code != 0 {
            let reason = if code == 1 {
                "system busy"
            } else {
                value["msg"].as_str().unwrap_or("")
            };
            return Err(DiscoveryFailure {
                retryable: code == 1000040343,
                error: ChannelError::new(format!(
                    "pullConnectConfig failed: code={code}, msg={reason}"
                )),
            });
        }
        let url = value["data"]["URL"]
            .as_str()
            .filter(|v| !v.is_empty())
            .ok_or_else(|| DiscoveryFailure {
                retryable: true,
                error: ChannelError::new("飞书网关响应缺少 WebSocket 地址。"),
            })?
            .to_owned();
        let service = reqwest::Url::parse(&url)
            .ok()
            .and_then(|url| {
                url.query_pairs()
                    .find(|(key, _)| key == "service_id")
                    .and_then(|(_, value)| value.parse().ok())
            })
            .unwrap_or(0);
        Ok((url, value["data"]["ClientConfig"].clone(), service))
    }
    async fn open(&self, url: &str) -> Result<Socket> {
        tokio::select! {biased;_=self.live.token.cancelled()=>Err(common::cancelled()),socket=tokio::time::timeout(Duration::from_secs(15),tokio_tungstenite::connect_async(url))=>socket.map_err(|_|ChannelError::new("WebSocket handshake did not complete within 15000ms"))?.map(|value|value.0).map_err(|error|ChannelError::new(format!("WebSocket connect failed: {error}")))}
    }
    async fn send_one(
        &self,
        to: &str,
        kind: &str,
        content: Value,
        reply: Option<&str>,
    ) -> Result<()> {
        let id_type = if to.starts_with("oc_") {
            "chat_id"
        } else if to.starts_with("ou_") {
            "open_id"
        } else if to.starts_with("on_") {
            "union_id"
        } else if to.contains('@') {
            "email"
        } else {
            "user_id"
        };
        if to.is_empty() {
            return Err(ChannelError::new("empty receive_id"));
        }
        let mut reply = reply.map(str::to_owned);
        let mut kind = kind.to_owned();
        let mut content = content;
        let mut attempt = 0;
        loop {
            let (path, body) = if let Some(reply) = &reply {
                (
                    format!("/open-apis/im/v1/messages/{}/reply", common::segment(reply)),
                    json!({"msg_type":kind,"content":content.to_string()}),
                )
            } else {
                (
                    format!("/open-apis/im/v1/messages?receive_id_type={id_type}"),
                    json!({"receive_id":to,"msg_type":kind,"content":content.to_string()}),
                )
            };
            match self.send_request(&path, body).await {
                Ok(value) => {
                    if value["data"]["message_id"]
                        .as_str()
                        .filter(|v| !v.is_empty())
                        .is_none()
                    {
                        return Err(ChannelError::new("message_id missing from send response"));
                    }
                    return Ok(());
                }
                Err((error, class)) => {
                    if reply.is_some() && class == SendFailure::Revoked {
                        reply = None;
                        attempt = 0;
                        continue;
                    }
                    if kind == "post" && class == SendFailure::Format {
                        kind = "text".into();
                        let text = post_plain(&content);
                        content = json!({"text":if text.is_empty(){"[message]".into()}else{text}});
                        attempt = 0;
                        continue;
                    }
                    attempt += 1;
                    if attempt >= 3
                        || !matches!(class, SendFailure::RateLimited | SendFailure::Unknown)
                    {
                        return Err(error);
                    }
                    tokio::select! {biased;_=self.live.token.cancelled()=>return Err(common::cancelled()),_=tokio::time::sleep(Duration::from_millis(500*3u64.pow(attempt-1)))=>{}}
                }
            }
        }
    }
    async fn materialize(&self, source: &str) -> Result<Vec<u8>> {
        if source
            .get(..7)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("http://"))
            || source
                .get(..8)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
        {
            let url = reqwest::Url::parse(source)
                .map_err(|_| ChannelError::new("source URL is invalid"))?;
            let host = url
                .host_str()
                .ok_or_else(|| ChannelError::new("source URL has no host"))?;
            let port = url.port_or_known_default().unwrap_or(443);
            let addresses = tokio::select! {biased;_=self.live.token.cancelled()=>return Err(common::cancelled()),addresses=tokio::net::lookup_host((host,port))=>addresses.map_err(|_|ChannelError::new("source URL could not be resolved"))?.collect::<Vec<_>>()};
            // The SDK rejects the entire DNS answer if any record is blocked.
            let address = addresses
                .first()
                .copied()
                .filter(|_| addresses.iter().all(|address| public_ip(address.ip())))
                .ok_or_else(|| {
                    ChannelError::new("URL blocked: private or reserved network address")
                })?;
            return self.fetch_source_url(url, Some(address.ip())).await;
        }
        let path = tokio::select! {biased;_=self.live.token.cancelled()=>return Err(common::cancelled()),path=tokio::fs::canonicalize(source)=>path.map_err(|_|ChannelError::new("source is neither an http(s) URL nor a readable local file"))?};
        if !cfg!(windows)
            && ["/etc", "/proc", "/sys", "/dev", "/private/etc"]
                .iter()
                .any(|prefix| path.starts_with(prefix))
        {
            return Err(ChannelError::new("file path is not allowed"));
        }
        tokio::select! {biased;_=self.live.token.cancelled()=>Err(common::cancelled()),data=tokio::fs::read(path)=>data.map_err(|error|ChannelError::new(error.to_string()))}
    }
    async fn fetch_source_url(
        &self,
        url: reqwest::Url,
        pinned: Option<std::net::IpAddr>,
    ) -> Result<Vec<u8>> {
        self.fetch_source_url_with_environment(url, pinned, &media_proxy::current(), None)
            .await
    }
    async fn fetch_source_url_with_environment(
        &self,
        mut url: reqwest::Url,
        pinned: Option<std::net::IpAddr>,
        environment: &media_proxy::Environment,
        extra_root: Option<&[u8]>,
    ) -> Result<Vec<u8>> {
        // SDK media uses Node's default TLS trust plus NODE_EXTRA_CA_CERTS.
        // Missing/invalid extra files are ignored by Node, leaving base roots.
        let environment_roots = if extra_root.is_none() {
            if let Some(path) = environment.get("NODE_EXTRA_CA_CERTS") {
                tokio::select! {biased;_=self.live.token.cancelled()=>return Err(common::cancelled()),pem=tokio::fs::read(path)=>pem.ok().filter(|pem| http_tunnel::certificates(pem).is_ok())}
            } else {
                None
            }
        } else {
            None
        };
        let extra_root = extra_root.or(environment_roots.as_deref());
        let protocol = url.scheme().to_owned();
        let mut tunnel_proxy: Option<reqwest::Url> = None;
        let fetch = async {
            // axios/follow-redirects defaults to 21 redirects. Its per-request
            // pinned agent also pins redirected DNS names to the initial IP.
            for redirects in 0..=21 {
                if !matches!(url.scheme(), "http" | "https") {
                    return Err(ChannelError::new("fetch source URL failed"));
                }
                let selected_proxy = media_proxy::select(&url, environment)?;
                let retained_proxy = tunnel_proxy.clone();
                let proxy = if url.scheme() == "https" {
                    if selected_proxy.is_some() {
                        tunnel_proxy = selected_proxy;
                    }
                    // Axios leaves its installed HTTPS tunnel in agents.https
                    // even when NO_PROXY bypasses a subsequent HTTPS redirect.
                    tunnel_proxy.clone()
                } else {
                    selected_proxy
                };
                let wire_protocol = proxy
                    .as_ref()
                    .filter(|_| url.scheme() == "http")
                    .map(|proxy| proxy.scheme())
                    .unwrap_or(url.scheme());
                let connect_host = proxy
                    .as_ref()
                    .filter(|_| url.scheme() == "http")
                    .and_then(|proxy| proxy.host_str())
                    .or_else(|| url.host_str())
                    .unwrap_or("");
                let tunneled = url.scheme() == "https" && proxy.is_some();
                let nested =
                    url.scheme() == "http" && wire_protocol == "https" && retained_proxy.is_some();
                if pinned.is_some()
                    && !nested
                    && !tunneled
                    && (wire_protocol != protocol
                        || connect_host
                            .trim_matches(['[', ']'])
                            .parse::<std::net::IpAddr>()
                            .is_err())
                {
                    // The release's Node 24 default autoSelectFamily asks the
                    // SDK's single-address lookup for all:true, and its pinned
                    // Agent fails DNS-name connections (ERR_INVALID_IP_ADDRESS).
                    return Err(ChannelError::new("fetch source URL failed"));
                }
                if nested {
                    let response = http_tunnel::get(
                        &url,
                        proxy
                            .as_ref()
                            .ok_or_else(|| ChannelError::new("fetch source URL failed"))?,
                        retained_proxy
                            .as_ref()
                            .ok_or_else(|| ChannelError::new("fetch source URL failed"))?,
                        extra_root,
                    )
                    .await?;
                    if (300..400).contains(&response.status) {
                        if let Some(location) = response.location {
                            if redirects == 21 {
                                return Err(ChannelError::new("fetch source URL failed"));
                            }
                            url = url
                                .join(&location)
                                .map_err(|_| ChannelError::new("fetch source URL failed"))?;
                            continue;
                        }
                    }
                    if !(200..300).contains(&response.status) {
                        return Err(ChannelError::new("fetch source URL failed"));
                    }
                    return Ok(response.body);
                }
                let mut builder = reqwest::Client::builder()
                    // Disable reqwest's different environment interpretation;
                    // install the actual per-hop Axios-selected proxy below.
                    .no_proxy()
                    .gzip(false)
                    .brotli(false)
                    .deflate(false)
                    .redirect(reqwest::redirect::Policy::none())
                    .timeout(Duration::from_secs(15));
                if let Some(root) = extra_root {
                    for der in http_tunnel::certificates(root)? {
                        builder = builder.add_root_certificate(
                            reqwest::Certificate::from_der(&der)
                                .map_err(|_| ChannelError::new("fetch source URL failed"))?,
                        );
                    }
                }
                if let Some(proxy) = &proxy {
                    let mut address = proxy.clone();
                    address
                        .set_username("")
                        .map_err(|_| ChannelError::new("fetch source URL failed"))?;
                    address
                        .set_password(None)
                        .map_err(|_| ChannelError::new("fetch source URL failed"))?;
                    let mut configured = reqwest::Proxy::all(address.as_str())
                        .map_err(|_| ChannelError::new("fetch source URL failed"))?;
                    if !proxy.username().is_empty() {
                        let encoded = base64::engine::general_purpose::STANDARD.encode(format!(
                            "{}:{}",
                            proxy.username(),
                            proxy.password().unwrap_or("")
                        ));
                        configured = configured.custom_http_auth(
                            reqwest::header::HeaderValue::from_str(&format!("Basic {encoded}"))
                                .map_err(|_| ChannelError::new("fetch source URL failed"))?,
                        );
                    }
                    builder = builder.proxy(configured);
                }
                if let Some(ip) = pinned {
                    let host = url
                        .host_str()
                        .ok_or_else(|| ChannelError::new("fetch source URL failed"))?;
                    builder = builder.resolve(
                        host,
                        std::net::SocketAddr::new(ip, url.port_or_known_default().unwrap_or(443)),
                    );
                }
                let client = builder
                    .build()
                    .map_err(|_| ChannelError::new("fetch source URL failed"))?;
                let response = common::response(
                    client.get(url.clone()).header(
                        reqwest::header::ACCEPT_ENCODING,
                        "gzip, compress, deflate, br",
                    ),
                    &self.live.token,
                )
                .await
                .map_err(|error| {
                    if self.live.token.is_cancelled() {
                        error
                    } else {
                        ChannelError::new("fetch source URL failed")
                    }
                })?;
                if response.status().is_redirection() {
                    if let Some(location) = response.headers().get(reqwest::header::LOCATION) {
                        if redirects == 21 {
                            return Err(ChannelError::new("fetch source URL failed"));
                        }
                        let location = location
                            .to_str()
                            .map_err(|_| ChannelError::new("fetch source URL failed"))?;
                        url = url
                            .join(location)
                            .map_err(|_| ChannelError::new("fetch source URL failed"))?;
                        continue;
                    }
                }
                if !response.status().is_success() {
                    return Err(ChannelError::new("fetch source URL failed"));
                }
                let empty = response.status().as_u16() == 204;
                let encoding = response
                    .headers()
                    .get(reqwest::header::CONTENT_ENCODING)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned);
                let bytes = common::bytes(response, &self.live.token, 50 * 1024 * 1024)
                    .await
                    .map_err(|error| {
                        if self.live.token.is_cancelled() {
                            error
                        } else {
                            ChannelError::new("fetch source URL failed")
                        }
                    })?;
                return if empty {
                    Ok(Vec::new())
                } else {
                    http_tunnel::decode(bytes, encoding.as_deref())
                };
            }
            Err(ChannelError::new("fetch source URL failed"))
        };
        tokio::select! {biased;_=self.live.token.cancelled()=>Err(common::cancelled()),result=tokio::time::timeout(Duration::from_secs(15),fetch)=>result.map_err(|_|ChannelError::new("fetch source URL failed"))?}
    }
    async fn upload(&self, kind: &str, input: &Value) -> Result<String> {
        let data = self.materialize(&common::text(&input["source"])).await?;
        let token = self.authorization().await?;
        let (path, form, key) = if kind == "image" {
            (
                "/open-apis/im/v1/images",
                reqwest::multipart::Form::new()
                    .text("image_type", "message")
                    .part(
                        "image",
                        reqwest::multipart::Part::bytes(data).file_name("image"),
                    ),
                "image_key",
            )
        } else {
            let file_type = match kind {
                "video" => "mp4",
                "audio" => "opus",
                _ => "stream",
            };
            let name = input["fileName"].as_str().unwrap_or(match kind {
                "video" => "video.mp4",
                "audio" => "voice.opus",
                _ => "upload.bin",
            });
            let duration = if matches!(kind, "video" | "audio") {
                input["duration"]
                    .as_u64()
                    .filter(|v| *v > 0)
                    .or_else(|| media_duration(kind, &data))
            } else {
                None
            };
            if matches!(kind, "video" | "audio") && duration.is_none() {
                return Err(ChannelError::new(format!(
                    "duration could not be determined for {kind}; pass it explicitly"
                )));
            }
            let mut form = reqwest::multipart::Form::new()
                .text("file_type", file_type)
                .text("file_name", name.to_owned())
                .part(
                    "file",
                    reqwest::multipart::Part::bytes(data).file_name(name.to_owned()),
                );
            if let Some(duration) = duration {
                form = form.text("duration", duration.to_string())
            }
            ("/open-apis/im/v1/files", form, "file_key")
        };
        let value = self
            .raw(
                self.client
                    .post(format!("{}{}", self.base, path))
                    .bearer_auth(token)
                    .multipart(form),
                "飞书上传附件",
            )
            .await?;
        value[key]
            .as_str()
            .or_else(|| value["data"][key].as_str())
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| ChannelError::new(format!("{key} missing in upload response")))
    }
}
impl FeishuGateway {
    fn current(&self) -> Result<Arc<Session>> {
        self.session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or_else(|| ChannelError::new("飞书机器人尚未连接。"))
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
        self.status.patch(json!({"state":"idle","connectedAt":null,"reconnectAttempts":0,"lastConnectTime":null,"bot":null}));
    }
    async fn connect_owned(&self, config: Value) -> Result<Value> {
        let _connecting = self.connecting.lock().await;
        self.stop().await;
        self.status
            .patch(json!({"state":"connecting","lastError":""}));
        let base = self.endpoints.api.clone().unwrap_or_else(|| {
            if config["domain"] == "lark" {
                "https://open.larksuite.com".into()
            } else {
                "https://open.feishu.cn".into()
            }
        });
        let session = Arc::new(Session {
            config,
            base: base.trim_end_matches('/').into(),
            client: common::client(),
            token: AsyncMutex::new(Token {
                value: String::new(),
                until: Instant::now(),
            }),
            live: Live::new(),
            bot: Mutex::new(String::new()),
            fragment_epoch: Mutex::new(tokio::time::Instant::now()),
        });
        *self
            .session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(session.clone());
        let result: Result<Value>=async{
            let _operation=session.live.enter()?;let bot=session.api(reqwest::Method::GET,"/open-apis/bot/v3/info",None).await?;let open_id=bot["bot"]["open_id"].as_str().filter(|v|!v.is_empty()).ok_or_else(||ChannelError::new("could not resolve bot identity via /open-apis/bot/v3/info — required for channel to function"))?;
            *session.bot.lock().unwrap_or_else(std::sync::PoisonError::into_inner)=open_id.into();self.status.patch(json!({"bot":{"openId":open_id,"name":bot["bot"]["app_name"].as_str().unwrap_or("bot")}}));
            // LarkChannel constructs WSClient/DataCache after bot identity.
            *session.fragment_epoch.lock().unwrap_or_else(std::sync::PoisonError::into_inner)=tokio::time::Instant::now();
            let deadline=tokio::time::Instant::now()+Duration::from_secs(15);let mut retry_configuration=json!({"ReconnectNonce":30,"ReconnectInterval":120,"ReconnectCount":-1});let mut attempt=0u32;
            let(socket,configuration,service)=loop{
                self.status.patch(json!({"lastConnectTime":common::now_ms()}));
                let connection=async{
                    let(url,configuration,service)=session.discover().await?;retry_configuration=configuration.clone();
                    let socket=session.open(&url).await.map_err(|error|DiscoveryFailure{retryable:!session.live.token.is_cancelled(),error})?;
                    Ok::<_,DiscoveryFailure>((socket,configuration,service))
                };
                match tokio::time::timeout_at(deadline,connection).await{
                    Ok(Ok(value))=>break value,
                    Ok(Err(failure))if !failure.retryable=>return Err(ChannelError::new(format!("WebSocket connect failed: {}",failure.error.message))),
                    Err(_)=>return Err(ChannelError::new("WebSocket handshake did not complete within 15000ms")),
                    _=>{}
                }
                attempt+=1;
                let mut random=[0u8;4];let _=getrandom::getrandom(&mut random);
                let seconds=if attempt==1{retry_configuration["ReconnectNonce"].as_f64().unwrap_or(30.)*(u32::from_le_bytes(random)as f64/(u32::MAX as f64+1.))}else{retry_configuration["ReconnectInterval"].as_f64().unwrap_or(120.)};
                let delay=Duration::from_secs_f64(seconds.max(0.));
                tokio::select!{biased;_=session.live.token.cancelled()=>return Err(common::cancelled()),_=tokio::time::sleep_until(deadline)=>return Err(ChannelError::new("WebSocket handshake did not complete within 15000ms")),_=tokio::time::sleep(delay)=>{}}
            };
            self.status.patch(json!({"state":"connected","connectedAt":common::now(),"lastError":""}));let status=self.status.clone();let session=session.clone();
            *self.task.lock().await=Some(tokio::spawn(async move{event_loop(session,status,socket,configuration,service).await}));Ok(self.status.get())
        }.await;
        if let Err(error) = &result {
            let bot = self.status.get()["bot"].clone();
            self.stop().await;
            self.status
                .patch(json!({"state":"failed","lastError":error.message,"bot":bot}));
        }
        result
    }
    async fn send_owned(&self, to: String, input: Value, reply: Option<String>) -> Result<()> {
        let session = self.current()?;
        let _operation = session.live.enter()?;
        if let Some(md) = input.get("markdown") {
            for (index, chunk) in split_markdown(&common::text(md), 3500)
                .into_iter()
                .enumerate()
            {
                let post = json!({"zh_cn":{"title":"","content":[[{"tag":"md","text":optimize_markdown(&chunk)}]]}});
                session
                    .send_one(
                        &to,
                        "post",
                        post,
                        if index == 0 { reply.as_deref() } else { None },
                    )
                    .await?
            }
            return Ok(());
        }
        if let Some(text) = input.get("text") {
            let units = common::text(text).encode_utf16().collect::<Vec<_>>();
            if units.is_empty() {
                return session
                    .send_one(&to, "text", json!({"text":""}), reply.as_deref())
                    .await;
            }
            for (index, chunk) in units.chunks(3500).enumerate() {
                session
                    .send_one(
                        &to,
                        "text",
                        json!({"text":String::from_utf16_lossy(chunk)}),
                        if index == 0 { reply.as_deref() } else { None },
                    )
                    .await?
            }
            return Ok(());
        }
        for kind in [
            "post",
            "image",
            "file",
            "audio",
            "video",
            "card",
            "shareChat",
            "shareUser",
            "sticker",
        ] {
            if let Some(value) = input.get(kind) {
                let (msg_type, content) = match kind {
                    "post" => ("post", value.clone()),
                    "card" => ("interactive", value.clone()),
                    "shareChat" => ("share_chat", json!({"chat_id":value["chatId"]})),
                    "shareUser" => ("share_user", json!({"user_id":value["userId"]})),
                    "sticker" => ("sticker", json!({"file_key":value["fileKey"]})),
                    "image" => (
                        "image",
                        json!({"image_key":session.upload("image",value).await?}),
                    ),
                    "video" => {
                        let mut content = json!({"file_key":session.upload("video",value).await?});
                        if let Some(cover) = value.get("coverImageKey") {
                            content["image_key"] = cover.clone()
                        }
                        ("media", content)
                    }
                    "audio" => (
                        "audio",
                        json!({"file_key":session.upload("audio",value).await?}),
                    ),
                    _ => (
                        "file",
                        json!({"file_key":session.upload("file",value).await?}),
                    ),
                };
                return session
                    .send_one(&to, msg_type, content, reply.as_deref())
                    .await;
            }
        }
        Err(ChannelError::new("unrecognized SendInput shape"))
    }
}
impl Gateway for FeishuGateway {
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
            Some(common::text(&message["messageId"])),
        ))
    }
    fn send_to_peer(
        &self,
        peer_id: String,
        payload: Value,
        _scope: Value,
    ) -> BoxFuture<'_, Result<()>> {
        Box::pin(self.send_owned(peer_id, payload, None))
    }
    fn send_asset(
        &self,
        peer_id: String,
        asset: Value,
        _scope: Value,
    ) -> BoxFuture<'_, Result<()>> {
        let mime = common::text(&asset["mimeType"]);
        let input = if mime.starts_with("image/") {
            json!({"image":{"source":asset["path"]}})
        } else if mime.starts_with("video/") {
            json!({"video":{"source":asset["path"]}})
        } else {
            json!({"file":{"source":asset["path"],"fileName":asset["name"]}})
        };
        Box::pin(self.send_owned(peer_id, input, None))
    }
    fn download_resources(&self, resources: Value) -> BoxFuture<'_, Result<Vec<Resource>>> {
        Box::pin(async move {
            let session = self.current()?;
            let _operation = session.live.enter()?;
            let token = session.authorization().await?;
            let mut total = 0;
            let mut result = Vec::new();
            for resource in resources.as_array().into_iter().flatten().take(8) {
                let kind = if resource["type"] == "image" {
                    "images"
                } else {
                    "files"
                };
                let key = common::text(&resource["fileKey"]);
                let response = common::response(
                    session
                        .client
                        .get(format!(
                            "{}/open-apis/im/v1/{kind}/{}",
                            session.base,
                            common::segment(&key)
                        ))
                        .bearer_auth(&token),
                    &session.live.token,
                )
                .await?;
                if !response.status().is_success() {
                    return Err(ChannelError::new(format!(
                        "飞书下载附件失败：HTTP {}",
                        response.status()
                    )));
                }
                let bytes =
                    common::bytes(response, &session.live.token, 24 * 1024 * 1024 - total).await?;
                total += bytes.len();
                let kind = resource["type"].as_str().unwrap_or("file");
                result.push(Resource {
                    name: resource["fileName"]
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| format!("{kind}-{}", result.len() + 1)),
                    kind: kind.into(),
                    mime_type: None,
                    bytes,
                });
            }
            Ok(result)
        })
    }
}
struct Fragments {
    at: i64,
    sum: usize,
    parts: HashMap<usize, Vec<u8>>,
}
fn sweep_fragments(fragments: &mut HashMap<String, Fragments>, now: i64) {
    // SDK DataCache uses strictly greater than 10s, on interval ticks only.
    fragments.retain(|_, parts| now.saturating_sub(parts.at) <= 10000);
}
struct FragmentSweep(tokio::task::JoinHandle<()>);
impl Drop for FragmentSweep {
    fn drop(&mut self) {
        self.0.abort();
    }
}
fn merge_fragment(
    fragments: &mut HashMap<String, Fragments>,
    id: String,
    sum: usize,
    seq: usize,
    payload: Vec<u8>,
) -> Option<Vec<u8>> {
    let entry = fragments.entry(id.clone()).or_insert_with(|| Fragments {
        at: common::now_ms(),
        sum,
        parts: HashMap::new(),
    });
    if entry.sum != sum {
        fragments.remove(&id);
        return None;
    }
    entry.parts.insert(seq, payload);
    if entry.parts.len() != sum {
        return None;
    }
    let entry = fragments.remove(&id)?;
    Some(
        (0..sum)
            .flat_map(|index| entry.parts.get(&index).into_iter().flatten().copied())
            .collect(),
    )
}
async fn write_frame(socket: &mut Socket, frame: &Frame, session: &Session) -> Result<()> {
    tokio::select! {biased;_=session.live.token.cancelled()=>Err(common::cancelled()),value=tokio::time::timeout(Duration::from_secs(15),socket.send(Message::Binary(frame.encode_to_vec().into())))=>value.map_err(|_|ChannelError::new("飞书 WebSocket 发送超时。"))?.map_err(|error|ChannelError::new(error.to_string()))}
}
async fn event_loop(
    session: Arc<Session>,
    status: Arc<Status>,
    mut socket: Socket,
    mut configuration: Value,
    mut service: i32,
) {
    let mut seen = HashMap::<String, Instant>::new();
    let mut order = VecDeque::new();
    let fragments = Arc::new(Mutex::new(HashMap::<String, Fragments>::new()));
    let swept_fragments = fragments.clone();
    let sweep_token = session.live.token.clone();
    let sweep_epoch = *session
        .fragment_epoch
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _sweep = FragmentSweep(tokio::spawn(async move {
        let mut interval = tokio::time::interval_at(
            sweep_epoch + Duration::from_secs(10),
            Duration::from_secs(10),
        );
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {biased;_=sweep_token.cancelled()=>return,_=interval.tick()=>{
                sweep_fragments(&mut swept_fragments.lock().unwrap_or_else(std::sync::PoisonError::into_inner), common::now_ms());
            }}
        }
    }));
    let mut attempts = 0u64;
    loop {
        let mut next_ping = tokio::time::Instant::now();
        loop {
            let item = tokio::select! {biased;_=session.live.token.cancelled()=>{let _=socket.close(None).await;return},_=tokio::time::sleep_until(next_ping)=>{
                let ping=Frame{seq_id:0,log_id:0,service,method:0,headers:vec![Header{key:"type".into(),value:"ping".into()}],..Frame::default()};if write_frame(&mut socket,&ping,&session).await.is_err(){break}next_ping=tokio::time::Instant::now()+Duration::from_secs(configuration["PingInterval"].as_u64().unwrap_or(120).max(1));continue
            },item=socket.next()=>item};
            let Some(item) = item else { break };
            let frame = match item {
                Ok(Message::Binary(bytes)) => {
                    match Frame::decode(bytes.as_ref()) {
                        Ok(frame) => frame,
                        Err(error) => {
                            status.patch(json!({"lastError":format!("飞书 WebSocket 消息解析失败：{error}")}));
                            continue;
                        }
                    }
                }
                Ok(Message::Ping(bytes)) => {
                    let _ = socket.send(Message::Pong(bytes)).await;
                    continue;
                }
                Ok(Message::Close(_)) | Err(_) => break,
                _ => continue,
            };
            if frame.method == 0 {
                if frame.header("type") == "pong" {
                    if let Some(payload) = &frame.payload {
                        if let Ok(value) = serde_json::from_slice(payload) {
                            configuration = value
                        }
                    }
                }
                continue;
            }
            if frame.method != 1 || frame.header("type") != "event" {
                continue;
            }
            let id = frame.header("message_id").to_owned();
            let sum = frame.header("sum").parse::<usize>().unwrap_or(1);
            let seq = frame.header("seq").parse::<usize>().unwrap_or(0);
            if sum == 0 || seq >= sum {
                status.patch(json!({"lastError":"飞书 WebSocket 分片编号无效。"}));
                continue;
            }
            let data = merge_fragment(
                &mut fragments
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
                id,
                sum,
                seq,
                frame.payload.clone().unwrap_or_default(),
            );
            let Some(data) = data else {
                continue;
            };
            let mut code = 200;
            match serde_json::from_slice::<Value>(&data) {
                Ok(value) => {
                    if value["header"]["event_type"] == "im.message.receive_v1" {
                        let bot = session
                            .bot
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .clone();
                        let event = &value["event"];
                        match normalize::normalize(event, &bot) {
                            Err(error) => status.patch(json!({"lastError":error.message})),
                            Ok(mut message) => {
                                if message["rawContentType"] == "merge_forward" {
                                    if let Ok(items) = session
                                        .api(
                                            reqwest::Method::GET,
                                            &format!(
                                                "/open-apis/im/v1/messages/{}",
                                                common::segment(&common::text(
                                                    &message["messageId"]
                                                ))
                                            ),
                                            None,
                                        )
                                        .await
                                    {
                                        message["content"] = json!(forwarded(
                                            &items["data"]["items"],
                                            &common::text(&message["messageId"])
                                        ));
                                    }
                                }
                                let create = message["createTime"].as_i64().unwrap_or(0);
                                let group = message["chatType"] == "group";
                                let allowed = (create == 0
                                    || common::now_ms() - create <= 5 * 60 * 1000)
                                    && (!group
                                        || (message["mentionedBot"] == true
                                            && message["mentionAll"] != true));
                                seen.retain(|_, at| at.elapsed() < Duration::from_secs(12 * 3600));
                                let mid = common::text(&message["messageId"]);
                                if allowed && !seen.contains_key(&mid) {
                                    seen.insert(mid.clone(), Instant::now());
                                    order.push_back(mid);
                                    while seen.len() > 5000 {
                                        if let Some(first) = order.pop_front() {
                                            seen.remove(&first);
                                        } else {
                                            break;
                                        }
                                    }
                                    status.message(message)
                                }
                            }
                        }
                    }
                }
                Err(error) => {
                    code = 500;
                    status.patch(json!({"lastError":format!("飞书事件 JSON 无效：{error}")}));
                }
            }
            let mut response = frame;
            response.headers.push(Header {
                key: "biz_rt".into(),
                value: "0".into(),
            });
            response.payload = Some(json!({"code":code}).to_string().into_bytes());
            if write_frame(&mut socket, &response, &session).await.is_err() {
                break;
            }
        }
        if session.live.token.is_cancelled() {
            return;
        }
        status.patch(json!({"state":"reconnecting"}));
        let _ = socket.close(None).await;
        loop {
            attempts += 1;
            status.patch(json!({"reconnectAttempts":attempts}));
            let limit = configuration["ReconnectCount"].as_i64().unwrap_or(-1);
            if limit >= 0 && attempts > (limit as u64).max(1) {
                status.patch(json!({"state":"failed","lastError":format!("WebSocket reconnect exhausted after {limit} attempts")}));
                return;
            }
            let delay = if attempts == 1 {
                let nonce = configuration["ReconnectNonce"]
                    .as_f64()
                    .unwrap_or(30.)
                    .max(0.);
                let mut random = [0u8; 4];
                let _ = getrandom::getrandom(&mut random);
                Duration::from_secs_f64(
                    nonce * (u32::from_le_bytes(random) as f64 / (u32::MAX as f64 + 1.)),
                )
            } else {
                Duration::from_secs(configuration["ReconnectInterval"].as_u64().unwrap_or(120))
            };
            tokio::select! {biased;_=session.live.token.cancelled()=>return,_=tokio::time::sleep(delay)=>{}}
            status.patch(json!({"lastConnectTime":common::now_ms()}));
            match session.discover().await {
                Ok((url, next_configuration, next_service)) => match session.open(&url).await {
                    Ok(next_socket) => {
                        socket = next_socket;
                        configuration = next_configuration;
                        service = next_service;
                        attempts = 0;
                        status.patch(
                            json!({"state":"connected","lastError":"","reconnectAttempts":0}),
                        );
                        break;
                    }
                    Err(error) => status.patch(json!({"lastError":error.message})),
                },
                Err(failure) => {
                    status.patch(json!({"lastError":failure.error.message}));
                    if !failure.retryable {
                        status.patch(json!({"state":"failed"}));
                        return;
                    }
                }
            }
        }
    }
}
fn public_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(ip) => {
            !ip.is_private()
                && !ip.is_loopback()
                && !ip.is_link_local()
                && !ip.is_broadcast()
                && !ip.is_documentation()
                && !ip.is_unspecified()
                && !ip.is_multicast()
                && ip.octets()[0] != 0
                && ip.octets()[0] < 224
                && !(ip.octets()[0] == 100 && (64..=127).contains(&ip.octets()[1]))
                && !(ip.octets()[0] == 192 && ip.octets()[1] == 0 && ip.octets()[2] == 0)
                && !(ip.octets()[0] == 198 && matches!(ip.octets()[1], 18 | 19))
        }
        std::net::IpAddr::V6(ip) => {
            let segments = ip.segments();
            if segments[..6] == [0, 0, 0, 0, 0, 0]
                || segments[..6] == [0, 0, 0, 0, 0, 0xffff]
                || segments[..6] == [0x64, 0xff9b, 0, 0, 0, 0]
            {
                return public_ip(
                    std::net::Ipv4Addr::from(
                        (u32::from(segments[6]) << 16) | u32::from(segments[7]),
                    )
                    .into(),
                );
            }
            !ip.is_loopback()
                && !ip.is_unspecified()
                && !ip.is_multicast()
                && (ip.segments()[0] & 0xfe00) != 0xfc00
                && (ip.segments()[0] & 0xffc0) != 0xfe80
                && segments[..4] != [0x100, 0, 0, 0]
                && segments[..2] != [0x2001, 0xdb8]
                && segments[..2] != [0x2001, 0]
        }
    }
}
fn post_plain(value: &Value) -> String {
    value["zh_cn"]["content"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|line| {
            line.as_array()
                .into_iter()
                .flatten()
                .map(|el| common::text(&el["text"]))
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .into()
}
fn media_duration(kind: &str, data: &[u8]) -> Option<u64> {
    if kind == "audio" {
        let header = data.windows(8).position(|v| v == b"OpusHead")?;
        let pre = u16::from_le_bytes(data.get(header + 10..header + 12)?.try_into().ok()?) as u64;
        let mut at = 0;
        let mut last = 0u64;
        while at + 27 <= data.len() {
            if &data[at..at + 4] != b"OggS" {
                return None;
            }
            let count = *data.get(at + 26)? as usize;
            let payload = data
                .get(at + 27..at + 27 + count)?
                .iter()
                .map(|v| *v as usize)
                .sum::<usize>();
            let granule = u64::from_le_bytes(data[at + 6..at + 14].try_into().ok()?);
            if granule != u64::MAX {
                last = granule
            }
            at = at.checked_add(27 + count + payload)?;
        }
        return Some(last.saturating_sub(pre) * 1000 / 48000);
    }
    let at = data.windows(4).position(|v| v == b"mvhd")?;
    let version = *data.get(at + 4)?;
    let (timescale, duration) = if version == 1 {
        (
            u32::from_be_bytes(data.get(at + 24..at + 28)?.try_into().ok()?) as u64,
            u64::from_be_bytes(data.get(at + 28..at + 36)?.try_into().ok()?),
        )
    } else {
        (
            u32::from_be_bytes(data.get(at + 16..at + 20)?.try_into().ok()?) as u64,
            u32::from_be_bytes(data.get(at + 20..at + 24)?.try_into().ok()?) as u64,
        )
    };
    if timescale == 0 {
        None
    } else {
        duration.checked_mul(1000).map(|v| v / timescale)
    }
}
fn forwarded(items: &Value, root: &str) -> String {
    let items = items.as_array().map(Vec::as_slice).unwrap_or(&[]);
    let mut children = HashMap::<String, Vec<&Value>>::new();
    for item in items.iter().take(50) {
        let id = common::text(&item["message_id"]);
        let parent = item["upper_message_id"].as_str().unwrap_or(root);
        if id == root && item.get("upper_message_id").is_none() {
            continue;
        }
        children.entry(parent.into()).or_default().push(item)
    }
    for children in children.values_mut() {
        children.sort_by_key(|item| {
            common::text(&item["create_time"])
                .parse::<i64>()
                .unwrap_or(0)
        });
    }
    fn tree(
        parent: &str,
        map: &HashMap<String, Vec<&Value>>,
        visited: &mut std::collections::HashSet<String>,
        truncated: bool,
    ) -> String {
        if !visited.insert(parent.into()) {
            return "<forwarded_messages/>".into();
        }
        let Some(items) = map.get(parent) else {
            return "<forwarded_messages/>".into();
        };
        let mut lines = Vec::new();
        for item in items {
            let content = if item["msg_type"] == "merge_forward" {
                tree(&common::text(&item["message_id"]), map, visited, false)
            } else {
                normalize::content(
                    item["msg_type"].as_str().unwrap_or("text"),
                    item["body"]["content"].as_str().unwrap_or("{}"),
                    &HashMap::new(),
                )
                .0
            };
            let ms = common::text(&item["create_time"])
                .parse::<i64>()
                .unwrap_or(0);
            let timestamp = chrono::DateTime::from_timestamp_millis(ms + 8 * 3600 * 1000)
                .filter(|_| ms > 0)
                .map(|v| v.format("%Y-%m-%dT%H:%M:%S+08:00").to_string())
                .unwrap_or_else(|| "unknown".into());
            lines.push(format!(
                "[{timestamp}] {}:\n{}",
                item["sender"]["id"].as_str().unwrap_or("unknown"),
                content
                    .lines()
                    .map(|line| format!("    {line}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }
        format!(
            "<forwarded_messages>\n{}{}\n</forwarded_messages>",
            lines.join("\n"),
            if truncated { "\n... (truncated)" } else { "" }
        )
    }
    tree(
        root,
        &children,
        &mut std::collections::HashSet::new(),
        items.len() > 50,
    )
}

#[cfg(test)]
#[path = "feishu_qq/sdk_edge_tests.rs"]
mod sdk_edge_tests;
#[cfg(test)]
#[path = "feishu_qq/sdk_proxy_tests.rs"]
mod sdk_proxy_tests;
#[cfg(test)]
#[path = "feishu_qq/feishu_tests.rs"]
mod tests;
