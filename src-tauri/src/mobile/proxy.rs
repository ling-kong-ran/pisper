//! 本地回环代理：WebView 只访问 `http://127.0.0.1:<port>`（明文、仅回环），
//! 代理负责把请求转发到当前激活的桌面端 endpoint——TLS 指纹锁定、
//! 注入 Bearer 令牌、SSE 字节流透传（禁缓冲）。
//!
//! 这个设计让前端代码零改动：相对路径的 API/SSE 请求天然走代理；
//! 未来 T3 隧道（Iroh/WebRTC）只需实现同样的字节转发即可插入。
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

use super::pinning::pinned_client;
use super::store::{ServerProfile, SharedStore};

/// 上游地址缓存有效期：避免每个请求都探测；网络切换后最多 20 秒内自愈。
const UPSTREAM_CACHE_TTL: Duration = Duration::from_secs(20);
/// 端点健康探测超时：局域网内健康检查应在毫秒级返回。
const PROBE_TIMEOUT: Duration = Duration::from_millis(2500);

type ProxyBody = UnsyncBoxBody<Bytes, Infallible>;

struct UpstreamCache {
    url: String,
    checked_at: Instant,
}

pub struct ProxyHandle {
    pub port: u16,
    store: Arc<SharedStore>,
    upstream: Mutex<Option<UpstreamCache>>,
    /// 指纹变化（换服务器）时重建客户端。
    client_cache: Mutex<Option<(String, reqwest::Client)>>,
}

impl ProxyHandle {
    fn active_profile(&self) -> Option<ServerProfile> {
        self.store.lock().ok()?.active().cloned()
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

    /// 解析当前可用上游：缓存有效直接用；否则按优先级逐个健康探测。
    async fn resolve_upstream(&self, profile: &ServerProfile) -> Result<String, String> {
        if let Some(cache) = self
            .upstream
            .lock()
            .ok()
            .and_then(|c| c.as_ref().map(|c| (c.url.clone(), c.checked_at)))
        {
            if cache.1.elapsed() < UPSTREAM_CACHE_TTL {
                return Ok(cache.0);
            }
        }
        let client = self.client_for(&profile.fingerprint)?;
        for endpoint in &profile.endpoints {
            let base = endpoint.url.trim_end_matches('/').to_string();
            let probe = client
                .get(format!("{base}/api/health"))
                .bearer_auth(&profile.token)
                .timeout(PROBE_TIMEOUT)
                .send()
                .await;
            if matches!(probe, Ok(response) if response.status().is_success()) {
                if let Ok(mut cache) = self.upstream.lock() {
                    *cache = Some(UpstreamCache {
                        url: base.clone(),
                        checked_at: Instant::now(),
                    });
                }
                return Ok(base);
            }
        }
        Err("无法连接到桌面端，请确认电脑在线且与手机处于同一网络。".to_string())
    }
}

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
        .body(BodyExt::boxed_unsync(body))
        .expect("static response")
}

async fn forward(
    proxy: &Arc<ProxyHandle>,
    request: Request<Incoming>,
) -> Result<Response<ProxyBody>, Infallible> {
    let Some(profile) = proxy.active_profile() else {
        return Ok(text_response(
            StatusCode::BAD_GATEWAY,
            "尚未配对桌面端，请先在连接页完成配对。",
        ));
    };
    let upstream = match proxy.resolve_upstream(&profile).await {
        Ok(upstream) => upstream,
        Err(error) => return Ok(text_response(StatusCode::BAD_GATEWAY, &error)),
    };
    let client = match proxy.client_for(&profile.fingerprint) {
        Ok(client) => client,
        Err(error) => return Ok(text_response(StatusCode::INTERNAL_SERVER_ERROR, &error)),
    };

    let (parts, body) = request.into_parts();
    let path_and_query = parts
        .uri
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or("/");
    let url = format!("{upstream}{path_and_query}");

    // 请求体整体读入（runtime 本身限制附件 ≤32MB），响应体则流式透传。
    let body_bytes = match body.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(_) => {
            return Ok(text_response(StatusCode::BAD_REQUEST, "读取请求体失败。"));
        }
    };

    let mut outgoing = client
        .request(parts.method, &url)
        .bearer_auth(&profile.token);
    for (name, value) in &parts.headers {
        let lower = name.as_str().to_ascii_lowercase();
        if HOP_BY_HOP.contains(&lower.as_str()) {
            continue;
        }
        outgoing = outgoing.header(name, value);
    }

    let response = match outgoing.body(body_bytes).send().await {
        Ok(response) => response,
        Err(error) => {
            // 连接失败时清掉上游缓存，下个请求会重新探测。
            if let Ok(mut cache) = proxy.upstream.lock() {
                *cache = None;
            }
            return Ok(text_response(
                StatusCode::BAD_GATEWAY,
                &format!("连接桌面端失败：{error}"),
            ));
        }
    };

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
    let stream = response
        .bytes_stream()
        .take_while(|result| std::future::ready(result.is_ok()))
        .filter_map(|result| async move { result.ok() })
        .map(|chunk| Ok::<_, Infallible>(Frame::data(chunk)));
    let body = StreamBody::new(stream).boxed_unsync();
    Ok(builder
        .body(body)
        .unwrap_or_else(|_| text_response(StatusCode::INTERNAL_SERVER_ERROR, "构造响应失败。")))
}

/// 启动回环代理（绑定随机端口），返回句柄。调用方需持有 Arc 以保持运行。
pub async fn start_proxy(store: Arc<SharedStore>) -> Result<Arc<ProxyHandle>, String> {
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
    });
    let server = handle.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                continue;
            };
            let proxy = server.clone();
            tauri::async_runtime::spawn(async move {
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
mod tests {
    //! 代理集成测试：自建带自签证书的 TLS 上游，验证
    //! ① 未配对返回 502；② Bearer 注入与转发；③ 指纹不匹配拒绝连接；④ SSE 逐帧透传。
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
                                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n",
                                        body.len()
                                    )
                                    .as_bytes(),
                                )
                                .await
                                .unwrap();
                            stream.write_all(body).await.unwrap();
                        }
                        "sse" => {
                            stream
                                .write_all(
                                    b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n",
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

    use std::sync::OnceLock;
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
            endpoints: vec![ServerEndpoint {
                kind: "lan".into(),
                url: url.into(),
            }],
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
        }
        let proxy = start_proxy(Arc::new(Mutex::new(store))).await.unwrap();
        proxy.port
    }

    fn fast_id() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
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
    async fn unpaired_returns_502() {
        let port = spawn_proxy(None).await;
        let (status, _) = raw_get(port, "/api/health").await;
        assert!(status.contains("502"), "unexpected status: {status}");
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
        eprintln!("[sse-test] spawning upstream");
        let (url, fingerprint) = spawn_upstream("sse").await;
        eprintln!("[sse-test] upstream at {url}");
        let port = spawn_proxy(Some(profile_for(&url, &fingerprint))).await;
        eprintln!("[sse-test] proxy at {port}");

        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        stream
            .write_all(b"GET /api/chat HTTP/1.1\r\nhost: 127.0.0.1\r\nconnection: close\r\n\r\n")
            .await
            .unwrap();
        eprintln!("[sse-test] request sent");
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
        eprintln!(
            "[sse-test] first frame received: {}",
            String::from_utf8_lossy(&received).replace('\n', "|")
        );
        let text = String::from_utf8_lossy(&received);
        assert!(text.contains("event: run"), "应先收到第一帧");
        assert!(!text.contains("event: done"), "第二帧此刻不应到达");
        // 放行上游发第二帧，随后应能读到 done。
        SSE_GATE.get().unwrap().add_permits(1);
        eprintln!("[sse-test] gate released");
        let mut rest = Vec::new();
        let read_rest = async { stream.read_to_end(&mut rest).await.unwrap() };
        tokio::time::timeout(std::time::Duration::from_secs(10), read_rest)
            .await
            .expect("读取剩余帧超时");
        assert!(String::from_utf8_lossy(&rest).contains("event: done"));
    }
}
