//! 本地回环代理：WebView 只访问 `http://127.0.0.1:<port>`（明文、仅回环）。
//! 签名包内 Runtime 始终提供 React UI；远程模式仅把 `/api/*` 转发到当前桌面端，
//! 并执行 TLS 指纹锁定、Bearer 注入与 SSE 字节流透传。
use std::{
    convert::Infallible,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use bytes::Bytes;
use futures_util::StreamExt;
use http_body_util::{combinators::UnsyncBoxBody, BodyExt, StreamBody};
use hyper::body::{Frame, Incoming};
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;

use crate::iroh_tunnel::TunnelBridgePool;

use super::pinning::pinned_client;
use super::store::{ServerEndpoint, ServerProfile, SharedStore};

/// 上游地址缓存有效期：避免每个请求都探测；网络切换后最多 20 秒内自愈。
const UPSTREAM_CACHE_TTL: Duration = Duration::from_secs(5);
/// 远程 HTTP 只限制收到响应头的时间；响应体可能是长期 SSE 流，不能设置整体超时。
const RESPONSE_HEADERS_TIMEOUT: Duration = Duration::from_secs(15);
/// 端点健康探测超时：局域网内健康检查应在毫秒级返回。
const PROBE_TIMEOUT: Duration = Duration::from_millis(2500);
/// 首次 Iroh 建连可能包含 relay 协商，需给足握手时间。
const IROH_PROBE_TIMEOUT: Duration = Duration::from_secs(8);

type ProxyBody = UnsyncBoxBody<Bytes, Infallible>;

struct UpstreamCache {
    url: String,
    kind: String,
    fingerprint: String,
    checked_at: Instant,
}

#[derive(Clone)]
struct LocalRuntime {
    base_url: String,
    cookie: String,
}

pub struct ProxyHandle {
    pub port: u16,
    store: Arc<SharedStore>,
    upstream: Mutex<Option<UpstreamCache>>,
    /// 指纹变化（换服务器）时重建客户端。
    client_cache: Mutex<Option<(String, reqwest::Client)>>,
    tunnels: Option<Arc<TunnelBridgePool>>,
    local_runtime: Mutex<Option<LocalRuntime>>,
}

impl ProxyHandle {
    pub fn active_transport(&self) -> Option<String> {
        self.upstream
            .lock()
            .ok()
            .and_then(|cache| cache.as_ref().map(|cache| cache.kind.clone()))
    }

    fn active_profile(&self) -> Option<ServerProfile> {
        self.store.lock().ok()?.active().cloned()
    }

    pub fn invalidate_remote_upstream(&self) {
        if let Ok(mut cache) = self.upstream.lock() {
            *cache = None;
        }
    }

    pub fn configure_local_runtime(&self, bootstrap_url: &str) -> Result<(), String> {
        let url = tauri::Url::parse(bootstrap_url)
            .map_err(|error| format!("本机 Runtime 启动地址无效：{error}"))?;
        if url.scheme() != "http"
            || url.host_str() != Some("127.0.0.1")
            || url.port().is_none()
            || url.path() != "/_pisper/desktop/bootstrap"
        {
            return Err("本机 Runtime 启动地址不受信任。".into());
        }
        let token = url
            .query_pairs()
            .find_map(|(key, value)| (key == "token").then(|| value.into_owned()))
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "本机 Runtime 启动地址缺少认证令牌。".to_string())?;
        let base_url = format!("http://127.0.0.1:{}", url.port().unwrap_or_default());
        let local = LocalRuntime {
            base_url,
            cookie: format!("__pisper_desktop={token}"),
        };
        *self
            .local_runtime
            .lock()
            .map_err(|_| "local Runtime cache poisoned".to_string())? = Some(local);
        Ok(())
    }

    fn use_remote_api(&self) -> bool {
        self.store
            .lock()
            .is_ok_and(|store| store.last_mode() == Some("remote") && store.active().is_some())
    }

    fn client_for(&self, fingerprint: &str) -> Result<reqwest::Client, String> {
        let mut cache = self
            .client_cache
            .lock()
            .map_err(|_| "client cache poisoned")?;
        if let Some((fp, client)) = cache.as_ref() {
            if fp == fingerprint {
                return Ok(client.clone());
            }
        }
        let client = pinned_client(fingerprint)?;
        *cache = Some((fingerprint.to_string(), client.clone()));
        Ok(client)
    }

    async fn endpoint_url(&self, endpoint: &ServerEndpoint) -> Result<String, String> {
        if endpoint.kind == "iroh" {
            let tunnels = self
                .tunnels
                .as_deref()
                .ok_or_else(|| "Iroh 桥接尚未启动。".to_string())?;
            return tunnels.bridge_url(endpoint.tunnel_endpoint()?).await;
        }
        if !endpoint.url.starts_with("https://") {
            return Err("远程端点必须使用 HTTPS。".into());
        }
        Ok(endpoint.url.trim_end_matches('/').to_string())
    }

    /// 解析当前可用上游：缓存有效直接用；否则按优先级逐个健康探测。
    async fn resolve_upstream(&self, profile: &ServerProfile) -> Result<String, String> {
        if let Some(cache) = self.upstream.lock().ok().and_then(|c| {
            c.as_ref()
                .map(|c| (c.url.clone(), c.fingerprint.clone(), c.checked_at))
        }) {
            // 切换配对档案后不能复用旧桌面端的地址；否则短暂期间请求会发往错误服务器。
            if cache.2.elapsed() < UPSTREAM_CACHE_TTL && cache.1 == profile.fingerprint {
                return Ok(cache.0);
            }
        }
        let client = self.client_for(&profile.fingerprint)?;
        for endpoint in &profile.endpoints {
            let Ok(base) = self.endpoint_url(endpoint).await else {
                continue;
            };
            let probe_timeout = if endpoint.kind == "iroh" {
                IROH_PROBE_TIMEOUT
            } else {
                PROBE_TIMEOUT
            };
            let probe = client
                .get(format!("{base}/api/health"))
                .bearer_auth(&profile.token)
                .timeout(probe_timeout)
                .send()
                .await;
            let healthy = match probe {
                Ok(response) => {
                    let status = response.status();
                    // 必须消费探测响应体释放连接，否则紧接着的真实请求可能卡在连接池中。
                    let body_read = response.bytes().await.is_ok();
                    status.is_success() && body_read
                }
                Err(_) => false,
            };
            if !healthy && endpoint.kind == "iroh" {
                // 网络切换后旧桥仍可能保留失败的监听任务，下一次探测必须重建它。
                if let Some(tunnels) = self.tunnels.as_deref() {
                    if let Ok(remote) = endpoint.tunnel_endpoint() {
                        tunnels.invalidate(&remote).await;
                    }
                }
            }
            if healthy {
                if let Ok(mut cache) = self.upstream.lock() {
                    *cache = Some(UpstreamCache {
                        url: base.clone(),
                        kind: endpoint.kind.clone(),
                        fingerprint: profile.fingerprint.clone(),
                        checked_at: Instant::now(),
                    });
                }
                return Ok(base);
            }
        }
        Err("无法连接到桌面端，请确认电脑在线且远程访问已启用。".to_string())
    }

    /// 从桌面端读取 Provider 配置并写入本机 Runtime，配对成功后自动执行。
    pub async fn sync_model_config(&self) -> Result<(), String> {
        let Some(profile) = self.active_profile() else {
            return Err("尚未配对桌面端。".into());
        };
        let local = self
            .local_runtime
            .lock()
            .map_err(|_| "local Runtime cache poisoned".to_string())?
            .clone()
            .ok_or_else(|| "本机 Runtime 尚未就绪。".to_string())?;
        let upstream = self.resolve_upstream(&profile).await?;
        let client = self.client_for(&profile.fingerprint)?;
        let response = client
            .get(format!("{upstream}/api/providers/export"))
            .bearer_auth(&profile.token)
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|error| format!("读取桌面端模型配置失败：{error}"))?;
        if !response.status().is_success() {
            return Err(format!(
                "桌面端模型配置返回 HTTP {}。",
                response.status().as_u16()
            ));
        }
        let body = response
            .bytes()
            .await
            .map_err(|error| format!("读取桌面端模型配置失败：{error}"))?;
        if body.len() > 4 * 1024 * 1024 {
            return Err("桌面端模型配置过大。".into());
        }
        let local_client = reqwest::Client::new();
        let result = local_client
            .post(format!("{}/api/providers/import", local.base_url))
            .header(reqwest::header::COOKIE, local.cookie)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|error| format!("写入本机模型配置失败：{error}"))?;
        if !result.status().is_success() {
            return Err(format!(
                "本机模型配置返回 HTTP {}。",
                result.status().as_u16()
            ));
        }
        Ok(())
    }
}

/// 移动前端资源必须完整缓冲后再交给 WebView，避免 Runtime 暂停时把截断模块伪装成成功响应。
const MAX_FRONTEND_RESPONSE_BYTES: u64 = 8 * 1024 * 1024;

/// 逐跳 header 名单：转发时必须剥离，由两端连接各自管理。
const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "host",
];

fn text_response(status: StatusCode, message: &str) -> Response<ProxyBody> {
    let body = http_body_util::Full::new(Bytes::from(format!("{message}\n")));
    Response::builder()
        .status(status)
        .header("Content-Type", "text/plain; charset=utf-8")
        // 错误响应必须让连接收尾：客户端读到 EOF 而不是干等超时。
        .header("Connection", "close")
        .body(BodyExt::boxed_unsync(body))
        .expect("static response")
}

async fn forward_remote(
    proxy: &Arc<ProxyHandle>,
    request: Request<Incoming>,
) -> Result<Response<ProxyBody>, Infallible> {
    let Some(profile) = proxy.active_profile() else {
        return Ok(text_response(
            StatusCode::BAD_GATEWAY,
            "尚未配对桌面端，请先在设置 -> 服务器中完成配对。",
        ));
    };
    let (parts, body) = request.into_parts();
    let path_and_query = parts
        .uri
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or("/");
    // 请求体整体读入（runtime 本身限制附件 ≤32MB），响应体则流式透传。
    let body_bytes = match body.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(_) => {
            return Ok(text_response(StatusCode::BAD_REQUEST, "读取请求体失败。"));
        }
    };

    // 真实请求也允许一次重选端点：探测成功后网络可能立即切换，不能把备用端点留给下一个请求。
    let mut response = None;
    let mut last_error = None;
    for attempt in 0..2 {
        let upstream = match proxy.resolve_upstream(&profile).await {
            Ok(upstream) => upstream,
            Err(error) => {
                last_error = Some(error);
                break;
            }
        };
        let client = match proxy.client_for(&profile.fingerprint) {
            Ok(client) => client,
            Err(error) => return Ok(text_response(StatusCode::INTERNAL_SERVER_ERROR, &error)),
        };
        let url = format!("{upstream}{path_and_query}");
        let mut outgoing = client
            .request(parts.method.clone(), &url)
            .bearer_auth(&profile.token);
        for (name, value) in &parts.headers {
            let lower = name.as_str().to_ascii_lowercase();
            if HOP_BY_HOP.contains(&lower.as_str()) {
                continue;
            }
            outgoing = outgoing.header(name, value);
        }
        // 标记流量来源：runtime/前端据此把设置页换成移动端形态（服务器切换而非发码管理）。
        outgoing = outgoing.header("X-Pisper-Client", "mobile-app");
        match tokio::time::timeout(
            RESPONSE_HEADERS_TIMEOUT,
            outgoing.body(body_bytes.clone()).send(),
        )
        .await
        {
            Ok(Ok(value)) => {
                response = Some(value);
                break;
            }
            Ok(Err(error)) => {
                last_error = Some(format!("连接桌面端失败：{error}"));
            }
            Err(_) => {
                last_error = Some("等待桌面端响应超时。".to_string());
            }
        }
        if let Ok(mut cache) = proxy.upstream.lock() {
            *cache = None;
        }
        if attempt == 0 {
            continue;
        }
    }
    let response = match response {
        Some(response) => response,
        None => {
            let message = last_error.as_deref().unwrap_or("无法连接到桌面端。");
            let status = if message.contains("超时") {
                StatusCode::GATEWAY_TIMEOUT
            } else {
                StatusCode::BAD_GATEWAY
            };
            return Ok(text_response(status, message));
        }
    };

    if response.status().is_server_error() || response.status() == StatusCode::REQUEST_TIMEOUT {
        if let Ok(mut cache) = proxy.upstream.lock() {
            *cache = None;
        }
    }

    let mut builder = Response::builder().status(response.status());
    for (name, value) in response.headers() {
        let lower = name.as_str().to_ascii_lowercase();
        if HOP_BY_HOP.contains(&lower.as_str()) || lower == "content-length" {
            continue;
        }
        builder = builder.header(name, value);
    }
    // SSE 字节流逐帧透传：reqwest 的 bytes_stream 到达即写，不做任何缓冲。
    // 上游流出错时提前终止流（等效于连接中断，客户端会按游标重连）。
    let stream_proxy = Arc::clone(proxy);
    let stream = response
        .bytes_stream()
        .filter_map(move |result| {
            let stream_proxy = Arc::clone(&stream_proxy);
            async move {
                match result {
                    Ok(chunk) => Some(chunk),
                    Err(_) => {
                        // SSE 断流通常意味着网络已切换；清缓存才能让重连重新选择端点。
                        stream_proxy.invalidate_remote_upstream();
                        None
                    }
                }
            }
        })
        .map(|chunk| Ok::<_, Infallible>(Frame::data(chunk)));
    let body = StreamBody::new(stream).boxed_unsync();
    Ok(builder
        .body(body)
        .unwrap_or_else(|_| text_response(StatusCode::INTERNAL_SERVER_ERROR, "构造响应失败。")))
}

async fn forward_local(
    proxy: &Arc<ProxyHandle>,
    request: Request<Incoming>,
) -> Result<Response<ProxyBody>, Infallible> {
    let local = proxy
        .local_runtime
        .lock()
        .ok()
        .and_then(|runtime| runtime.clone());
    let Some(local) = local else {
        return Ok(text_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "App 内置 Runtime 尚未就绪。",
        ));
    };

    let (parts, body) = request.into_parts();
    let path_and_query = parts
        .uri
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or("/");
    let url = format!("{}{path_and_query}", local.base_url);
    let body_bytes = match body.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(_) => return Ok(text_response(StatusCode::BAD_REQUEST, "读取请求体失败。")),
    };

    let is_frontend = !path_and_query.starts_with("/api/");
    let client = reqwest::Client::new();
    let mut outgoing = client
        .request(parts.method, &url)
        .header(reqwest::header::COOKIE, &local.cookie);
    if is_frontend {
        // 前端模块不能无限等待：后台恢复期间若 Node 连接悬挂，必须尽快交给入口恢复逻辑。
        outgoing = outgoing.timeout(Duration::from_secs(15));
    }
    for (name, value) in &parts.headers {
        let lower = name.as_str().to_ascii_lowercase();
        if HOP_BY_HOP.contains(&lower.as_str()) || lower == "cookie" || lower == "origin" {
            continue;
        }
        outgoing = outgoing.header(name, value);
    }
    outgoing = outgoing.header("X-Pisper-Client", "mobile-app");

    let response = match outgoing.body(body_bytes).send().await {
        Ok(response) => response,
        Err(error) => {
            let message = if is_frontend {
                format!("读取 App 前端资源失败：{error}")
            } else {
                format!("连接 App 内置 Runtime 失败：{error}")
            };
            return Ok(text_response(StatusCode::BAD_GATEWAY, &message));
        }
    };
    let status = response.status();
    let headers = response.headers().clone();

    if is_frontend {
        if response
            .content_length()
            .is_some_and(|length| length > MAX_FRONTEND_RESPONSE_BYTES)
        {
            return Ok(text_response(
                StatusCode::BAD_GATEWAY,
                "App 前端资源超过安全大小限制。",
            ));
        }
        // 前端 HTML、JS 与 CSS 必须先完整读取；上游若在后台冻结期间断流，
        // reqwest 会返回错误，代理改发 502，而不是让 WebView 执行残缺的 200 模块。
        let body = match response.bytes().await {
            Ok(body) if body.len() as u64 <= MAX_FRONTEND_RESPONSE_BYTES => body,
            Ok(_) => {
                return Ok(text_response(
                    StatusCode::BAD_GATEWAY,
                    "App 前端资源超过安全大小限制。",
                ));
            }
            Err(error) => {
                return Ok(text_response(
                    StatusCode::BAD_GATEWAY,
                    &format!("读取 App 前端资源失败：{error}"),
                ));
            }
        };
        let mut builder = Response::builder().status(status);
        for (name, value) in &headers {
            let lower = name.as_str().to_ascii_lowercase();
            if HOP_BY_HOP.contains(&lower.as_str())
                || lower == "content-length"
                || lower == "set-cookie"
                || lower == "cache-control"
            {
                continue;
            }
            builder = builder.header(name, value);
        }
        return Ok(builder
            .header("Cache-Control", "no-store")
            .header("Content-Length", body.len())
            .body(BodyExt::boxed_unsync(http_body_util::Full::new(body)))
            .unwrap_or_else(|_| {
                text_response(StatusCode::INTERNAL_SERVER_ERROR, "构造响应失败。")
            }));
    }

    let mut builder = Response::builder().status(status);
    for (name, value) in &headers {
        let lower = name.as_str().to_ascii_lowercase();
        if HOP_BY_HOP.contains(&lower.as_str())
            || lower == "content-length"
            || lower == "set-cookie"
        {
            continue;
        }
        builder = builder.header(name, value);
    }
    let stream = response
        .bytes_stream()
        .take_while(|result| std::future::ready(result.is_ok()))
        .filter_map(|result| async move { result.ok() })
        .map(|chunk| Ok::<_, Infallible>(Frame::data(chunk)));
    Ok(builder
        .body(StreamBody::new(stream).boxed_unsync())
        .unwrap_or_else(|_| text_response(StatusCode::INTERNAL_SERVER_ERROR, "构造响应失败。")))
}

fn route_to_remote(path: &str, remote_mode: bool) -> bool {
    path.starts_with("/api/") && remote_mode
}

async fn forward(
    proxy: &Arc<ProxyHandle>,
    request: Request<Incoming>,
) -> Result<Response<ProxyBody>, Infallible> {
    let remote_api = route_to_remote(request.uri().path(), proxy.use_remote_api());
    if remote_api {
        forward_remote(proxy, request).await
    } else {
        forward_local(proxy, request).await
    }
}

/// 启动回环代理（绑定随机端口），返回句柄。调用方需持有 Arc 以保持运行。
pub async fn start_proxy(
    store: Arc<SharedStore>,
    tunnels: Option<Arc<TunnelBridgePool>>,
) -> Result<Arc<ProxyHandle>, String> {
    let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .map_err(|error| format!("本地代理监听失败：{error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| error.to_string())?
        .port();
    let handle = Arc::new(ProxyHandle {
        port,
        store,
        upstream: Mutex::new(None),
        client_cache: Mutex::new(None),
        tunnels,
        local_runtime: Mutex::new(None),
    });
    let server = handle.clone();
    // 监听器绑定在哪个 Tokio reactor，就必须留在哪个运行时驱动；
    // 测试运行时与 Tauri 全局运行时不同，跨运行时移动会在部分平台卡住 I/O。
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                continue;
            };
            let proxy = server.clone();
            tokio::spawn(async move {
                let io = TokioIo::new(stream);
                let service = service_fn(move |request| {
                    let proxy = proxy.clone();
                    async move { forward(&proxy, request).await }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(io, service)
                    .await;
            });
        }
    });
    Ok(handle)
}

#[cfg(test)]
mod routing_tests {
    use super::route_to_remote;

    #[test]
    fn every_mobile_build_routes_only_runtime_apis_to_the_remote_server() {
        assert!(route_to_remote("/api/health", true));
        assert!(!route_to_remote("/api/health", false));
        assert!(!route_to_remote("/", true));
        assert!(!route_to_remote("/assets/index.js", true));
        assert!(!route_to_remote("/release-notes.json", true));
    }
}

#[cfg(all(test, not(feature = "mobile-store")))]
mod tests {
    //! 代理集成测试：验证本地 Runtime 就绪门禁，并用自签 TLS 上游覆盖
    //! Bearer 注入、指纹拒绝、Iroh 回退与 SSE 逐帧透传。
    use super::*;
    use crate::mobile::store::{ServerEndpoint, ServerProfile};
    use rcgen::generate_simple_self_signed;
    use rustls::pki_types::PrivatePkcs8KeyDer;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_rustls::TlsAcceptor;

    fn sha256_hex(bytes: &[u8]) -> String {
        use sha2::Digest;
        sha2::Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect()
    }

    /// 启动一个最小 TLS 上游：读请求头后按行为脚本响应。
    /// 返回 (地址, 证书指纹)。behavior 决定响应体写法。
    async fn spawn_upstream(behavior: &'static str) -> (String, String) {
        ensure_crypto_provider();
        let certified = generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let cert_der = certified.cert.der().clone();
        let fingerprint = sha256_hex(cert_der.as_ref());
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let tls_config = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![cert_der],
                PrivatePkcs8KeyDer::from(certified.key_pair.serialize_der()).into(),
            )
            .unwrap();
        let acceptor = TlsAcceptor::from(Arc::new(tls_config));
        let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    continue;
                };
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    let mut stream = acceptor.accept(stream).await.unwrap();
                    // 读请求头（本测试不涉及请求体）。
                    let mut head = Vec::new();
                    let mut buf = [0u8; 1024];
                    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                        let n = stream.read(&mut buf).await.unwrap();
                        if n == 0 {
                            return;
                        }
                        head.extend_from_slice(&buf[..n]);
                        if head.len() > 64 * 1024 {
                            return;
                        }
                    }
                    let request_text = String::from_utf8_lossy(&head);
                    assert!(
                        request_text.contains("authorization: Bearer pst_test"),
                        "代理必须注入 Bearer 头，实际请求：{request_text}"
                    );
                    // 按路径分流：/api/health 是代理的探测请求，其余走 SSE 脚本。
                    let behavior = if request_text.starts_with("GET /api/health ") {
                        "health"
                    } else {
                        behavior
                    };
                    match behavior {
                        "health" => {
                            let body = b"{\"ok\":true}";
                            stream
                                .write_all(
                                    format!(
                                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                                        body.len()
                                    )
                                    .as_bytes(),
                                )
                                .await
                                .unwrap();
                            stream.write_all(body).await.unwrap();
                            stream.flush().await.unwrap();
                            stream.shutdown().await.unwrap();
                        }
                        "sse" => {
                            stream
                                .write_all(
                                    b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n",
                                )
                                .await
                                .unwrap();
                            let frame = b"event: run\ndata: {\"runId\":\"r1\"}\n\n";
                            stream
                                .write_all(format!("{:x}\r\n", frame.len()).as_bytes())
                                .await
                                .unwrap();
                            stream.write_all(frame).await.unwrap();
                            stream.write_all(b"\r\n").await.unwrap();
                            stream.flush().await.unwrap();
                            // 等测试端确认收到第一帧后再发第二帧：确定性证明不缓冲。
                            if let Some(gate) = SSE_GATE.get() {
                                let _ = gate.acquire().await;
                            }
                            let frame2 = b"event: done\ndata: {}\n\n";
                            stream
                                .write_all(format!("{:x}\r\n", frame2.len()).as_bytes())
                                .await
                                .unwrap();
                            stream.write_all(frame2).await.unwrap();
                            stream.write_all(b"\r\n0\r\n\r\n").await.unwrap();
                        }
                        _ => unreachable!(),
                    }
                });
            }
        });
        (format!("https://{addr}"), fingerprint)
    }

    use std::sync::{
        atomic::{AtomicU64, Ordering},
        OnceLock,
    };

    // macOS 的系统时钟精度不足以单独生成并发测试文件名，原子序号保证进程内唯一。
    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(0);
    static SSE_GATE: OnceLock<tokio::sync::Semaphore> = OnceLock::new();

    // rustls 的 with_single_cert 会走进程级默认 provider 加载私钥；
    // reqwest 同时拉入了 ring 与 aws-lc-rs，必须显式安装一个。
    fn ensure_crypto_provider() {
        static ONCE: OnceLock<()> = OnceLock::new();
        ONCE.get_or_init(|| {
            let _ = rustls::crypto::ring::default_provider().install_default();
        });
    }

    fn profile_for(url: &str, fingerprint: &str) -> ServerProfile {
        ServerProfile {
            id: "srv_test".into(),
            name: "测试".into(),
            endpoints: vec![ServerEndpoint::lan(url.into())],
            fingerprint: fingerprint.into(),
            device_id: "dev_test".into(),
            token: "pst_test".into(),
            paired_at: "0".into(),
        }
    }

    async fn spawn_proxy(profile: Option<ServerProfile>) -> u16 {
        let path = std::env::temp_dir().join(format!("pisper-proxy-test-{}.json", fast_id()));
        let mut store = crate::mobile::store::ProfileStore::load(&path);
        if let Some(profile) = profile {
            store.upsert(profile).unwrap();
            store.set_last_mode("remote").unwrap();
        }
        let proxy = start_proxy(Arc::new(Mutex::new(store)), None)
            .await
            .unwrap();
        proxy.port
    }

    fn fast_id() -> u128 {
        let sequence = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed) as u128;
        ((std::process::id() as u128) << 64) | sequence
    }

    async fn raw_get(port: u16, path: &str) -> (String, Vec<u8>) {
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        stream
            .write_all(
                format!("GET {path} HTTP/1.1\r\nhost: 127.0.0.1\r\nconnection: close\r\n\r\n")
                    .as_bytes(),
            )
            .await
            .unwrap();
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).await.unwrap();
        let text = String::from_utf8_lossy(&raw).to_string();
        let status = text.lines().next().unwrap_or_default().to_string();
        (status, raw)
    }

    #[tokio::test]
    async fn local_runtime_not_configured_returns_503() {
        let port = spawn_proxy(None).await;
        let (status, raw) = raw_get(port, "/api/health").await;
        assert!(status.contains("503"), "unexpected status: {status}");
        assert!(String::from_utf8_lossy(&raw).contains("App 内置 Runtime 尚未就绪"));
    }

    #[tokio::test]
    async fn truncated_frontend_module_is_rejected_before_reaching_the_webview() {
        let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request).await;
            // 声明比实际更多的字节，模拟 Runtime 在后台冻结/恢复时前端响应被截断。
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\\r\\ncontent-type: text/javascript\\r\\ncontent-length: 128\\r\\nconnection: close\\r\\n\\r\\nexport const broken =",
                )
                .await
                .unwrap();
            stream.shutdown().await.unwrap();
        });

        let path = std::env::temp_dir().join(format!("pisper-proxy-test-{}.json", fast_id()));
        let proxy = start_proxy(
            Arc::new(Mutex::new(crate::mobile::store::ProfileStore::load(&path))),
            None,
        )
        .await
        .unwrap();
        proxy
            .configure_local_runtime(&format!(
                "http://{}/_pisper/desktop/bootstrap?token=test-token",
                address
            ))
            .unwrap();

        let (status, raw) = raw_get(proxy.port, "/assets/broken.js").await;
        assert!(status.contains("502"), "unexpected status: {status}");
        assert!(String::from_utf8_lossy(&raw).contains("读取 App 前端资源失败"));
    }

    #[tokio::test]
    async fn forwards_with_bearer_and_pinned_fingerprint() {
        let (url, fingerprint) = spawn_upstream("health").await;
        let port = spawn_proxy(Some(profile_for(&url, &fingerprint))).await;
        let (status, raw) = raw_get(port, "/api/health").await;
        assert!(status.contains("200"), "unexpected status: {status}");
        assert!(String::from_utf8_lossy(&raw).contains("{\"ok\":true}"));
    }

    #[tokio::test]
    async fn falls_back_from_lan_to_iroh() {
        let (url, fingerprint) = spawn_upstream("health").await;
        let target = url.trim_start_matches("https://").parse().unwrap();
        let tunnel_server = crate::iroh_tunnel::start_server(
            target,
            iroh::SecretKey::generate(),
            iroh::RelayMode::Disabled,
        )
        .await
        .unwrap();
        let remote = crate::iroh_tunnel::loopback_endpoint(
            tunnel_server.node_id(),
            tunnel_server.local_port().unwrap(),
        );
        let tunnels = Arc::new(
            TunnelBridgePool::start(iroh::SecretKey::generate(), iroh::RelayMode::Disabled)
                .await
                .unwrap(),
        );
        let mut profile = profile_for("https://127.0.0.1:9", &fingerprint);
        profile.endpoints.push(ServerEndpoint::iroh(remote));
        let path = std::env::temp_dir().join(format!("pisper-proxy-test-{}.json", fast_id()));
        let mut store = crate::mobile::store::ProfileStore::load(&path);
        store.upsert(profile).unwrap();
        store.set_last_mode("remote").unwrap();
        let proxy = start_proxy(Arc::new(Mutex::new(store)), Some(tunnels))
            .await
            .unwrap();

        let (status, raw) = raw_get(proxy.port, "/api/health").await;
        assert!(status.contains("200"), "unexpected status: {status}");
        assert!(String::from_utf8_lossy(&raw).contains("{\"ok\":true}"));
        tunnel_server.close().await;
    }

    #[tokio::test]
    async fn rejects_fingerprint_mismatch() {
        let (url, _fingerprint) = spawn_upstream("health").await;
        // 故意用不匹配的指纹。
        let wrong = "A".repeat(64);
        let port = spawn_proxy(Some(profile_for(&url, &wrong))).await;
        let (status, raw) = raw_get(port, "/api/health").await;
        assert!(status.contains("502"), "unexpected status: {status}");
        assert!(String::from_utf8_lossy(&raw).contains("无法连接"));
    }

    #[tokio::test]
    async fn sse_streams_incrementally() {
        SSE_GATE.set(tokio::sync::Semaphore::new(0)).ok();
        let (url, fingerprint) = spawn_upstream("sse").await;
        let port = spawn_proxy(Some(profile_for(&url, &fingerprint))).await;

        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        stream
            .write_all(b"GET /api/chat HTTP/1.1\r\nhost: 127.0.0.1\r\nconnection: close\r\n\r\n")
            .await
            .unwrap();
        // 只读到第一个 SSE 帧就停：若代理缓冲整个响应，这里会永远等不到。
        // 整体加超时：挂死要变成测试失败而不是卡住套件。
        let mut received = Vec::new();
        let mut buf = [0u8; 512];
        let read_first_frame = async {
            loop {
                let n = stream.read(&mut buf).await.unwrap();
                if n == 0 {
                    break;
                }
                received.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&received);
                if text.contains("event: run") && text.contains("}\n\n") {
                    break;
                }
                if received.len() > 64 * 1024 {
                    panic!("响应过大，疑似被缓冲");
                }
            }
        };
        tokio::time::timeout(std::time::Duration::from_secs(10), read_first_frame)
            .await
            .expect("读取第一帧超时");
        let text = String::from_utf8_lossy(&received);
        assert!(text.contains("event: run"), "应先收到第一帧");
        assert!(!text.contains("event: done"), "第二帧此刻不应到达");
        // 放行上游发第二帧，随后应能读到 done。
        SSE_GATE.get().unwrap().add_permits(1);
        let mut rest = Vec::new();
        let read_rest = async { stream.read_to_end(&mut rest).await.unwrap() };
        tokio::time::timeout(std::time::Duration::from_secs(10), read_rest)
            .await
            .expect("读取剩余帧超时");
        assert!(String::from_utf8_lossy(&rest).contains("event: done"));
    }
}
