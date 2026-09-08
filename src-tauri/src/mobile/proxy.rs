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

/// 上游地址缓存有效期：避免每个请求都探测；恢复通知会立即使缓存失效。
const UPSTREAM_CACHE_TTL: Duration = Duration::from_secs(5);
/// 在前端默认 30 秒超时前返回明确错误；SSE 和下载响应交付后不受此预算约束。
const REMOTE_REQUEST_TIMEOUT: Duration = Duration::from_secs(25);
/// 缓存连接失效后，为 LAN 探测、Iroh 握手与真实请求重试保留总预算。
const RESPONSE_HEADERS_TIMEOUT: Duration = Duration::from_secs(10);
/// 所有 LAN 共用探测预算，避免 DROP 端点逐个耗尽 Iroh 的连接机会。
const PROBE_TIMEOUT: Duration = Duration::from_millis(2500);
const MAX_PARALLEL_PROBES: usize = 8;
/// 首次 Iroh 建连可能包含 relay 协商，需给足握手时间。
const IROH_PROBE_TIMEOUT: Duration = Duration::from_secs(8);

type ProxyBody = UnsyncBoxBody<Bytes, Infallible>;

struct UpstreamCache {
    url: String,
    kind: String,
    fingerprint: String,
    checked_at: Instant,
    tunnel_generation: Option<u64>,
}

#[derive(Default)]
struct RemoteState {
    generation: u64,
    upstream: Option<UpstreamCache>,
    client_cache: Option<(String, reqwest::Client)>,
}

struct ResolvedUpstream {
    url: String,
    generation: u64,
    client: reqwest::Client,
}

struct ProbedUpstream<'a> {
    url: String,
    endpoint: &'a ServerEndpoint,
    tunnel_generation: Option<u64>,
}

#[derive(Default)]
struct ProbeFailure {
    transport: bool,
    tunnel_generation: Option<u64>,
}

#[derive(Default)]
struct ProbeGroup<'a> {
    healthy: Option<ProbedUpstream<'a>>,
    recovery_generation: Option<u64>,
    rejected: bool,
}

/// rustls 错误也会被 reqwest 标记为 connect，必须先检查真实错误链再认定网络故障。
fn is_transport_error(error: &reqwest::Error) -> bool {
    if error.is_builder() || error.is_redirect() || error.is_status() || error.is_decode() {
        return false;
    }
    let mut transport = error.is_connect() || error.is_timeout();
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(error) = source {
        if error.is::<rustls::Error>() {
            return false;
        }
        if let Some(io) = error.downcast_ref::<std::io::Error>() {
            transport |= matches!(
                io.kind(),
                std::io::ErrorKind::ConnectionRefused
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::NotConnected
                    | std::io::ErrorKind::TimedOut
                    | std::io::ErrorKind::UnexpectedEof
                    | std::io::ErrorKind::NetworkUnreachable
                    | std::io::ErrorKind::HostUnreachable
            );
            // io::Error::source 可能跳过内层包装，get_ref 才能看到原始 rustls 错误。
            if let Some(inner) = io.get_ref() {
                source = Some(inner);
                continue;
            }
        }
        if let Some(error) = error.downcast_ref::<hyper::Error>() {
            transport |= error.is_closed() || error.is_incomplete_message();
        }
        source = error.source();
    }
    transport
}

#[derive(Clone)]
struct LocalRuntime {
    base_url: String,
    cookie: String,
}

pub struct ProxyHandle {
    pub port: u16,
    store: Arc<SharedStore>,
    /// 地址、连接池和世代必须一起切换，避免恢复后旧探测重新写入缓存。
    remote: Mutex<RemoteState>,
    /// 启动 API 共用一次探测；等待者拿到锁后重新读取健康缓存。
    resolution: tokio::sync::Mutex<()>,
    tunnels: Option<Arc<TunnelBridgePool>>,
    local_runtime: Mutex<Option<LocalRuntime>>,
}

impl ProxyHandle {
    pub fn active_transport(&self) -> Option<String> {
        self.remote
            .lock()
            .ok()
            .and_then(|state| state.upstream.as_ref().map(|cache| cache.kind.clone()))
    }

    fn active_profile(&self) -> Option<ServerProfile> {
        self.store.lock().ok()?.active().cloned()
    }

    pub fn invalidate_remote_upstream(&self) {
        if let Ok(mut state) = self.remote.lock() {
            Self::reset_remote(&mut state);
        }
    }

    pub async fn resume_remote_network(&self) {
        self.invalidate_remote_upstream();
        if let Some(tunnels) = self.tunnels.as_deref() {
            // 通知只负责重新检查网络；不能阻塞前台恢复，也不销毁现有 Iroh 身份与桥。
            let _ = tokio::time::timeout(Duration::from_secs(1), tunnels.network_change()).await;
        }
    }

    fn reset_remote(state: &mut RemoteState) {
        state.generation = state.generation.wrapping_add(1);
        state.upstream = None;
        // 丢弃池所有者；在途 SSE 可自然收尾，新请求不会再租用旧网络的空闲连接。
        state.client_cache = None;
    }

    fn remote_generation(&self) -> u64 {
        self.remote
            .lock()
            .map(|state| state.generation)
            .unwrap_or(0)
    }

    fn invalidate_remote_generation(&self, generation: u64) {
        if let Ok(mut state) = self.remote.lock() {
            // 旧请求的超时和断流不能清掉恢复后选出的地址与新连接池。
            if state.generation == generation {
                Self::reset_remote(&mut state);
            }
        }
    }

    fn recover_tunnel_generation(&self, generation: u64, tunnel_generation: u64) -> bool {
        let Ok(mut state) = self.remote.lock() else {
            return false;
        };
        if state.generation != generation {
            return false;
        }
        let recovering = self
            .tunnels
            .as_ref()
            .is_some_and(|pool| pool.recover_if_current(tunnel_generation));
        if recovering {
            Self::reset_remote(&mut state);
        }
        recovering
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

    fn client_for(state: &mut RemoteState, fingerprint: &str) -> Result<reqwest::Client, String> {
        if let Some((fp, client)) = state.client_cache.as_ref() {
            if fp == fingerprint {
                return Ok(client.clone());
            }
        }
        let client = pinned_client(fingerprint)?;
        if state.client_cache.is_some() {
            Self::reset_remote(state);
        }
        state.client_cache = Some((fingerprint.to_string(), client.clone()));
        Ok(client)
    }

    async fn endpoint_url(
        &self,
        endpoint: &ServerEndpoint,
    ) -> Result<(String, Option<u64>), String> {
        if endpoint.kind == "iroh" {
            let tunnels = self
                .tunnels
                .as_deref()
                .ok_or_else(|| "Iroh 桥接尚未启动。".to_string())?;
            return tunnels
                .bridge_url_with_generation(endpoint.tunnel_endpoint()?)
                .await
                .map(|(url, generation)| (url, Some(generation)));
        }
        if !endpoint.url.starts_with("https://") {
            return Err("远程端点必须使用 HTTPS。".into());
        }
        Ok((endpoint.url.trim_end_matches('/').to_string(), None))
    }

    async fn probe_endpoint<'a>(
        &self,
        profile: &ServerProfile,
        client: &reqwest::Client,
        endpoint: &'a ServerEndpoint,
        budget: Duration,
        deadline: tokio::time::Instant,
    ) -> Result<ProbedUpstream<'a>, ProbeFailure> {
        // 等待已启动的恢复不属于网络失败；只有实际尝试 HTTP 后的超时才可作为证据。
        let (base, tunnel_generation) = self
            .endpoint_url(endpoint)
            .await
            .map_err(|_| ProbeFailure::default())?;
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Err(ProbeFailure::default());
        }
        let result = tokio::time::timeout_at(deadline.min(now + budget), async {
            let response = client
                .get(format!("{base}/api/health"))
                .bearer_auth(&profile.token)
                .send()
                .await
                .map_err(|error| is_transport_error(&error))?;
            if !response.status().is_success() {
                // HTTP 错误证明传输已到达服务端，不能据此回收 Endpoint。
                return Err(false);
            }
            // 完整消费健康响应，真实请求才能复用同一条健康 TLS 连接。
            response
                .bytes()
                .await
                .map_err(|error| is_transport_error(&error))?;
            Ok(())
        })
        .await
        .unwrap_or(Err(true));
        match result {
            Ok(()) => Ok(ProbedUpstream {
                url: base,
                endpoint,
                tunnel_generation,
            }),
            Err(transport) => Err(ProbeFailure {
                transport,
                tunnel_generation,
            }),
        }
    }

    /// LAN 优先；每个探测共享绝对截止点，超时仍返回分类而非取消整组后丢失证据。
    async fn probe_group<'a>(
        &self,
        profile: &'a ServerProfile,
        client: &reqwest::Client,
        iroh: bool,
    ) -> ProbeGroup<'a> {
        let budget = if iroh {
            IROH_PROBE_TIMEOUT
        } else {
            PROBE_TIMEOUT
        };
        let deadline = tokio::time::Instant::now() + budget;
        let endpoints = profile
            .endpoints
            .iter()
            .filter(|endpoint| (endpoint.kind == "iroh") == iroh)
            .collect::<Vec<_>>();
        let batches = u32::try_from(endpoints.len().div_ceil(MAX_PARALLEL_PROBES))
            .unwrap_or(u32::MAX)
            .max(1);
        let endpoint_budget = if iroh { budget } else { budget / batches };
        let mut endpoints = endpoints.into_iter();
        let mut probes = futures_util::stream::FuturesUnordered::new();
        let mut group = ProbeGroup::default();
        for endpoint in endpoints.by_ref().take(MAX_PARALLEL_PROBES) {
            probes.push(self.probe_endpoint(profile, client, endpoint, endpoint_budget, deadline));
        }
        while let Some(result) = probes.next().await {
            match result {
                Ok(healthy) => {
                    group.healthy = Some(healthy);
                    return group;
                }
                Err(failure) => {
                    group.rejected |= !failure.transport;
                    if let Some(generation) =
                        failure.tunnel_generation.filter(|_| failure.transport)
                    {
                        group.rejected |= group
                            .recovery_generation
                            .is_some_and(|old| old != generation);
                        group.recovery_generation = Some(generation);
                    }
                }
            }
            if let Some(endpoint) = endpoints.next() {
                probes.push(self.probe_endpoint(
                    profile,
                    client,
                    endpoint,
                    endpoint_budget,
                    deadline,
                ));
            }
        }
        group
    }

    async fn resolve_upstream(&self, profile: &ServerProfile) -> Result<ResolvedUpstream, String> {
        let _resolution = self.resolution.lock().await;
        // 业务发送前仍只允许两轮；恢复任务独立运行，等待计入调用者原有的总预算。
        'resolve: for _ in 0..2 {
            let tunnel_generation = self.tunnels.as_deref().map(TunnelBridgePool::generation);
            let (generation, client) = {
                let mut state = self.remote.lock().map_err(|_| "remote cache poisoned")?;
                if state.upstream.as_ref().is_some_and(|cache| {
                    cache.tunnel_generation.is_some()
                        && cache.tunnel_generation != tunnel_generation
                }) {
                    Self::reset_remote(&mut state);
                }
                let client = Self::client_for(&mut state, &profile.fingerprint)?;
                if let Some(cache) = state.upstream.as_ref() {
                    if cache.checked_at.elapsed() < UPSTREAM_CACHE_TTL
                        && cache.fingerprint == profile.fingerprint
                    {
                        return Ok(ResolvedUpstream {
                            url: cache.url.clone(),
                            generation: state.generation,
                            client,
                        });
                    }
                }
                (state.generation, client)
            };
            let mut recovery_generation = None;
            let mut rejected = false;
            for iroh in [false, true] {
                if self.remote_generation() != generation {
                    continue 'resolve;
                }
                let group = self.probe_group(profile, &client, iroh).await;
                rejected |= group.rejected;
                if group.recovery_generation.is_some() {
                    recovery_generation = group.recovery_generation;
                }
                if let Some(healthy) = group.healthy {
                    let mut state = self.remote.lock().map_err(|_| "remote cache poisoned")?;
                    if state.generation != generation {
                        continue 'resolve;
                    }
                    state.upstream = Some(UpstreamCache {
                        url: healthy.url.clone(),
                        kind: healthy.endpoint.kind.clone(),
                        fingerprint: profile.fingerprint.clone(),
                        checked_at: Instant::now(),
                        tunnel_generation: healthy.tunnel_generation,
                    });
                    return Ok(ResolvedUpstream {
                        url: healthy.url,
                        generation,
                        client,
                    });
                }
            }
            if self.remote_generation() != generation {
                continue 'resolve;
            }
            if !rejected
                && recovery_generation.is_some_and(|tunnel_generation| {
                    self.recover_tunnel_generation(generation, tunnel_generation)
                })
            {
                continue 'resolve;
            }
            self.invalidate_remote_generation(generation);
            return Err("无法连接到桌面端，请确认电脑在线且远程访问已启用。".to_string());
        }
        Err("网络连接已更新，请重试。".into())
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
        let response = upstream
            .client
            .get(format!("{}/api/providers/export", upstream.url))
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

struct RemoteTrace {
    enabled: bool,
    started: Instant,
    request_id: u64,
    stage: &'static str,
    attempt: usize,
    generation: u64,
}

impl RemoteTrace {
    fn new(generation: u64) -> Self {
        static NEXT_REQUEST_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let enabled = std::env::var("PISPER_MOBILE_NETWORK_TRACE").is_ok_and(|value| value == "1");
        Self {
            enabled,
            started: Instant::now(),
            request_id: if enabled {
                NEXT_REQUEST_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            } else {
                0
            },
            stage: "request_start",
            attempt: 0,
            generation,
        }
    }

    fn record(&mut self, stage: &'static str, attempt: usize, proxy: &ProxyHandle, status: u16) {
        self.stage = stage;
        self.attempt = attempt;
        if !self.enabled {
            return;
        }
        // 仅允许固定传输类别，不能把配对档案或错误文本中的地址、令牌带入设备日志。
        let transport = match proxy.active_transport().as_deref() {
            Some("lan") => "lan",
            Some("iroh") => "iroh",
            _ => "none",
        };
        eprintln!(
            "[pisper-mobile-proxy] request_id={} stage={} elapsed_ms={} attempt={} transport={} status={}",
            self.request_id, stage, self.started.elapsed().as_millis(), attempt, transport, status
        );
    }
}

async fn forward_remote(
    proxy: &Arc<ProxyHandle>,
    request: Request<Incoming>,
) -> Result<Response<ProxyBody>, Infallible> {
    forward_remote_with_timeouts(
        proxy,
        request,
        REMOTE_REQUEST_TIMEOUT,
        RESPONSE_HEADERS_TIMEOUT,
    )
    .await
}

async fn forward_remote_with_timeouts(
    proxy: &Arc<ProxyHandle>,
    request: Request<Incoming>,
    budget: Duration,
    headers_timeout: Duration,
) -> Result<Response<ProxyBody>, Infallible> {
    // 只包住响应构造阶段：探测、重试和 JSON 读取不能各自重新获得完整预算。
    // SSE 与下载返回的是惰性流，后续逐帧传输不在这个超时作用域内。
    let mut trace = RemoteTrace::new(proxy.remote_generation());
    trace.record("request_start", 0, proxy, 0);
    let response = match tokio::time::timeout(
        budget,
        forward_remote_response(proxy, request, headers_timeout, &mut trace),
    )
    .await
    {
        Ok(response) => response,
        Err(_) => {
            let stage = match trace.stage {
                "resolve_start" => "resolve_timeout",
                "headers_start" => "headers_timeout",
                "json_body_start" => "json_body_timeout",
                _ => "request_timeout",
            };
            // 等待共享探测或读取入站请求体超时，不能撤销其他请求的健康连接。
            if matches!(trace.stage, "headers_start" | "json_body_start") {
                proxy.invalidate_remote_generation(trace.generation);
            }
            trace.record(stage, trace.attempt, proxy, 504);
            Ok(text_response(
                StatusCode::GATEWAY_TIMEOUT,
                "等待桌面端响应超时。",
            ))
        }
    };
    let status = match &response {
        Ok(response) => response.status().as_u16(),
        Err(never) => match *never {},
    };
    // 流式响应这里只表示响应头交付完成，不表示 SSE 或下载已经结束。
    trace.record("request_end", trace.attempt, proxy, status);
    response
}

fn is_json_response(headers: &reqwest::header::HeaderMap) -> bool {
    headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .and_then(|value| value.trim().split_once('/'))
        .is_some_and(|(kind, subtype)| {
            kind.eq_ignore_ascii_case("application")
                && (subtype.eq_ignore_ascii_case("json")
                    || subtype.to_ascii_lowercase().ends_with("+json"))
        })
}

async fn forward_remote_response(
    proxy: &Arc<ProxyHandle>,
    request: Request<Incoming>,
    headers_timeout: Duration,
    trace: &mut RemoteTrace,
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
    // 请求体整体读入（runtime 本身限制附件 ≤32MB）；JSON 响应完整读取，其余仍流式透传。
    let body_bytes = match body.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(_) => {
            return Ok(text_response(StatusCode::BAD_REQUEST, "读取请求体失败。"));
        }
    };

    // 仅安全方法允许重选端点后重试；首部丢失并不代表 POST 尚未执行，不能重复提交聊天。
    let attempts = if matches!(
        parts.method,
        hyper::Method::GET | hyper::Method::HEAD | hyper::Method::OPTIONS
    ) {
        2
    } else {
        1
    };
    let mut response = None;
    let mut last_error = None;
    for attempt in 1..=attempts {
        trace.generation = proxy.remote_generation();
        trace.record("resolve_start", attempt, proxy, 0);
        let upstream = match proxy.resolve_upstream(&profile).await {
            Ok(upstream) => {
                trace.record("resolve_ok", attempt, proxy, 0);
                upstream
            }
            Err(error) => {
                trace.record("resolve_failed", attempt, proxy, 502);
                last_error = Some(error);
                break;
            }
        };
        trace.generation = upstream.generation;
        let url = format!("{}{path_and_query}", upstream.url);
        let mut outgoing = upstream
            .client
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
        trace.record("headers_start", attempt, proxy, 0);
        match tokio::time::timeout(headers_timeout, outgoing.body(body_bytes.clone()).send()).await
        {
            Ok(Ok(value)) => {
                trace.record("headers_ok", attempt, proxy, value.status().as_u16());
                response = Some((value, upstream.generation));
                break;
            }
            Ok(Err(error)) => {
                trace.record("headers_failed", attempt, proxy, 502);
                last_error = Some(format!("连接桌面端失败：{error}"));
                if !is_transport_error(&error) {
                    break;
                }
            }
            Err(_) => {
                trace.record("headers_timeout", attempt, proxy, 504);
                last_error = Some("等待桌面端响应超时。".to_string());
            }
        }
        proxy.invalidate_remote_generation(upstream.generation);
    }
    let (response, generation) = match response {
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

    let mut builder = Response::builder().status(response.status());
    for (name, value) in response.headers() {
        let lower = name.as_str().to_ascii_lowercase();
        if HOP_BY_HOP.contains(&lower.as_str()) || lower == "content-length" {
            continue;
        }
        builder = builder.header(name, value);
    }
    if is_json_response(response.headers()) {
        // ProxyBody 无法在发出响应头后报告读取错误，必须先确认 JSON 传输完整。
        let upstream_status = response.status().as_u16();
        trace.record("json_body_start", trace.attempt, proxy, upstream_status);
        let body = match response.bytes().await {
            Ok(body) => {
                trace.record("json_body_ok", trace.attempt, proxy, upstream_status);
                body
            }
            Err(error) => {
                let status = if error.is_timeout() {
                    StatusCode::GATEWAY_TIMEOUT
                } else {
                    StatusCode::BAD_GATEWAY
                };
                trace.record("json_body_failed", trace.attempt, proxy, status.as_u16());
                proxy.invalidate_remote_generation(generation);
                return Ok(text_response(
                    status,
                    &format!("读取桌面端 JSON 响应失败：{error}"),
                ));
            }
        };
        return Ok(builder
            .header("Content-Length", body.len())
            .body(BodyExt::boxed_unsync(http_body_util::Full::new(body)))
            .unwrap_or_else(|_| {
                text_response(StatusCode::INTERNAL_SERVER_ERROR, "构造响应失败。")
            }));
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
                        stream_proxy.invalidate_remote_generation(generation);
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
        remote: Mutex::new(RemoteState::default()),
        resolution: tokio::sync::Mutex::new(()),
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
        spawn_scripted_upstream(
            behavior,
            Duration::ZERO,
            Duration::ZERO,
            Arc::new(AtomicU64::new(0)),
        )
        .await
    }

    fn test_tls_acceptor() -> (TlsAcceptor, String) {
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
        (TlsAcceptor::from(Arc::new(tls_config)), fingerprint)
    }

    async fn spawn_scripted_upstream(
        behavior: &'static str,
        probe_delay: Duration,
        headers_delay: Duration,
        requests: Arc<AtomicU64>,
    ) -> (String, String) {
        let (acceptor, fingerprint) = test_tls_acceptor();
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
                let requests = requests.clone();
                tokio::spawn(async move {
                    let mut stream = acceptor.accept(stream).await.unwrap();
                    // 仅解析请求头；POST 测试统计提交次数，不检查业务负载。
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
                    // 按路径区分探测与真实请求，计数只记录可能产生业务副作用的真实请求。
                    let behavior = if request_text.starts_with("GET /api/health ") {
                        tokio::time::sleep(probe_delay).await;
                        "health"
                    } else {
                        requests.fetch_add(1, Ordering::SeqCst);
                        assert!(request_text.contains("x-pisper-client: mobile-app"));
                        tokio::time::sleep(headers_delay).await;
                        behavior
                    };
                    match behavior {
                        "health" | "json_suffix" => {
                            let content_type = if behavior == "json_suffix" {
                                "Application/Problem+JSON; charset=utf-8"
                            } else {
                                "application/json"
                            };
                            let body = b"{\"ok\":true}";
                            stream
                                .write_all(
                                    format!(
                                        "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\nx-upstream: preserved\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
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
                        "stalled_json" | "truncated_json" | "truncated_chunked_json" => {
                            let framing = if behavior == "truncated_chunked_json" {
                                "transfer-encoding: chunked"
                            } else {
                                "content-length: 128"
                            };
                            let partial = if behavior == "truncated_chunked_json" {
                                "6\r\n{\"ok\":\r\n"
                            } else {
                                "{\"ok\":"
                            };
                            stream.write_all(format!(
                                "HTTP/1.1 200 OK\r\ncontent-type: application/problem+json; charset=utf-8\r\n{framing}\r\nconnection: close\r\n\r\n{partial}"
                            ).as_bytes()).await.unwrap();
                            stream.flush().await.unwrap();
                            if behavior == "stalled_json" {
                                std::future::pending::<()>().await;
                            }
                            stream.shutdown().await.unwrap();
                        }
                        "disconnect" => {
                            stream.shutdown().await.unwrap();
                        }
                        "download" => {
                            stream.write_all(b"HTTP/1.1 200 OK\r\ncontent-type: application/octet-stream\r\ncontent-length: 6\r\nconnection: close\r\n\r\none").await.unwrap();
                            stream.flush().await.unwrap();
                            tokio::time::sleep(Duration::from_millis(350)).await;
                            stream.write_all(b"two").await.unwrap();
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

    #[derive(Debug)]
    enum TestReply {
        Health,
        TruncatedJson,
        ServerError,
        Unauthorized,
        RequestTimeout,
    }

    struct TlsExchange {
        connection: u64,
        path: String,
        reply: tokio::sync::oneshot::Sender<TestReply>,
    }

    struct ControlledUpstream {
        url: String,
        fingerprint: String,
        exchanges: tokio::sync::mpsc::UnboundedReceiver<TlsExchange>,
    }

    impl ControlledUpstream {
        async fn next(&mut self) -> TlsExchange {
            tokio::time::timeout(Duration::from_secs(6), self.exchanges.recv())
                .await
                .expect("TLS 上游必须收到请求")
                .expect("TLS 上游不能提前退出")
        }
    }

    /// 由测试逐次放行真实 TLS 响应，稳定制造旧请求晚到并观察连接复用。
    async fn spawn_controlled_upstream() -> ControlledUpstream {
        let (acceptor, fingerprint) = test_tls_acceptor();
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let url = format!("https://{}", listener.local_addr().unwrap());
        let (sender, exchanges) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut connection = 0;
            while let Ok((stream, _)) = listener.accept().await {
                connection += 1;
                let acceptor = acceptor.clone();
                let sender = sender.clone();
                tokio::spawn(async move {
                    let Ok(mut stream) = acceptor.accept(stream).await else {
                        return;
                    };
                    loop {
                        let mut head = Vec::new();
                        let mut byte = [0u8; 1];
                        while !head.ends_with(b"\r\n\r\n") {
                            if stream.read_exact(&mut byte).await.is_err() {
                                return;
                            }
                            head.push(byte[0]);
                            assert!(head.len() < 64 * 1024);
                        }
                        let text = String::from_utf8(head).unwrap();
                        assert!(text.contains("authorization: Bearer pst_test"));
                        let path = text.split_whitespace().nth(1).unwrap().to_string();
                        let (reply, receive) = tokio::sync::oneshot::channel();
                        if sender
                            .send(TlsExchange {
                                connection,
                                path,
                                reply,
                            })
                            .is_err()
                        {
                            return;
                        }
                        let Ok(reply) = receive.await else {
                            return;
                        };
                        let (bytes, close): (&[u8], bool) = match reply {
                            TestReply::Health => (
                                b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 11\r\n\r\n{\"ok\":true}",
                                false,
                            ),
                            TestReply::ServerError => (
                                b"HTTP/1.1 500 Internal Server Error\r\ncontent-type: application/json\r\ncontent-length: 2\r\n\r\n{}",
                                false,
                            ),
                            TestReply::Unauthorized => (
                                b"HTTP/1.1 401 Unauthorized\r\ncontent-type: application/json\r\ncontent-length: 2\r\n\r\n{}",
                                false,
                            ),
                            TestReply::RequestTimeout => (
                                b"HTTP/1.1 408 Request Timeout\r\ncontent-type: application/json\r\ncontent-length: 2\r\n\r\n{}",
                                false,
                            ),
                            TestReply::TruncatedJson => (
                                b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 128\r\nconnection: close\r\n\r\n{\"ok\":",
                                true,
                            ),
                        };
                        if stream.write_all(bytes).await.is_err() || stream.flush().await.is_err() {
                            return;
                        }
                        if close {
                            let _ = stream.shutdown().await;
                            return;
                        }
                    }
                });
            }
        });
        ControlledUpstream {
            url,
            fingerprint,
            exchanges,
        }
    }

    /// 接受 TCP 后持续吞掉 TLS 字节，模拟 DROP 时没有 RST/EOF 的等待路径。
    async fn spawn_silent_endpoint() -> (String, Arc<AtomicU64>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let url = format!("https://{}", listener.local_addr().unwrap());
        let connections = Arc::new(AtomicU64::new(0));
        let observed = connections.clone();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                observed.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move {
                    let _ = tokio::io::copy(&mut stream, &mut tokio::io::sink()).await;
                });
            }
        });
        (url, connections)
    }

    fn start_resolution(
        proxy: &Arc<ProxyHandle>,
        profile: &ServerProfile,
    ) -> tokio::task::JoinHandle<Result<ResolvedUpstream, String>> {
        let proxy = proxy.clone();
        let profile = profile.clone();
        tokio::spawn(async move { proxy.resolve_upstream(&profile).await })
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

    async fn spawn_proxy_with_timeouts(
        profile: ServerProfile,
        budget: Duration,
        headers_timeout: Duration,
    ) -> Arc<ProxyHandle> {
        spawn_proxy_with_tunnels(profile, budget, headers_timeout, None).await
    }

    async fn spawn_proxy_with_tunnels(
        profile: ServerProfile,
        budget: Duration,
        headers_timeout: Duration,
        tunnels: Option<Arc<TunnelBridgePool>>,
    ) -> Arc<ProxyHandle> {
        let path = std::env::temp_dir().join(format!("pisper-proxy-test-{}.json", fast_id()));
        let mut store = crate::mobile::store::ProfileStore::load(&path);
        store.upsert(profile).unwrap();
        store.set_last_mode("remote").unwrap();
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let proxy = Arc::new(ProxyHandle {
            port: listener.local_addr().unwrap().port(),
            store: Arc::new(Mutex::new(store)),
            remote: Mutex::new(RemoteState::default()),
            resolution: tokio::sync::Mutex::new(()),
            tunnels,
            local_runtime: Mutex::new(None),
        });
        let server = proxy.clone();
        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let proxy = server.clone();
                tokio::spawn(async move {
                    let service = service_fn(move |request| {
                        let proxy = proxy.clone();
                        async move {
                            forward_remote_with_timeouts(&proxy, request, budget, headers_timeout)
                                .await
                        }
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        proxy
    }

    async fn spawn_iroh_proxy(
        headers_timeout: Duration,
    ) -> (
        ControlledUpstream,
        crate::iroh_tunnel::TunnelServer,
        Arc<TunnelBridgePool>,
        Arc<ProxyHandle>,
    ) {
        let server = spawn_controlled_upstream().await;
        let target = server.url.trim_start_matches("https://").parse().unwrap();
        let tunnel = crate::iroh_tunnel::start_server(
            target,
            iroh::SecretKey::generate(),
            iroh::RelayMode::Disabled,
        )
        .await
        .unwrap();
        let remote =
            crate::iroh_tunnel::loopback_endpoint(tunnel.node_id(), tunnel.local_port().unwrap());
        let pool = Arc::new(
            TunnelBridgePool::start(iroh::SecretKey::generate(), iroh::RelayMode::Disabled)
                .await
                .unwrap(),
        );
        let mut profile = profile_for(&server.url, &server.fingerprint);
        profile.endpoints = vec![ServerEndpoint::iroh(remote)];
        let proxy = spawn_proxy_with_tunnels(
            profile,
            REMOTE_REQUEST_TIMEOUT,
            headers_timeout,
            Some(pool.clone()),
        )
        .await;
        (server, tunnel, pool, proxy)
    }

    #[tokio::test]
    async fn closed_iroh_endpoint_recovers_once_for_concurrent_real_tls_requests() {
        let (mut server, tunnel, pool, proxy) = spawn_iroh_proxy(RESPONSE_HEADERS_TIMEOUT).await;
        let old = pool.endpoint_for_test();
        old.close().await;
        let mut requests = tokio::task::JoinSet::new();
        for _ in 0..8 {
            requests.spawn(raw_get(proxy.port, "/api/closed"));
        }
        for index in 0..9 {
            let exchange = server.next().await;
            assert_eq!(
                exchange.path,
                if index == 0 {
                    "/api/health"
                } else {
                    "/api/closed"
                }
            );
            exchange.reply.send(TestReply::Health).unwrap();
        }
        while let Some(result) = requests.join_next().await {
            assert!(result.unwrap().0.contains("200"));
        }
        assert_eq!(pool.wait_ready().await.unwrap(), 1);
        assert_eq!(pool.endpoint_for_test().id(), old.id());
        assert_eq!(proxy.active_transport().as_deref(), Some("iroh"));
        pool.endpoint_for_test().close().await;
        tunnel.close().await;
    }

    #[tokio::test]
    async fn iroh_headers_timeout_then_failed_reprobe_recovers_within_existing_retry_limit() {
        let (mut server, tunnel, pool, proxy) = spawn_iroh_proxy(Duration::from_millis(150)).await;
        let old = pool.endpoint_for_test();
        let request = tokio::spawn(raw_get(proxy.port, "/api/work"));
        let health = server.next().await;
        assert_eq!(health.path, "/api/health");
        health.reply.send(TestReply::Health).unwrap();
        let pending = server.next().await;
        assert_eq!(pending.path, "/api/work");
        // 首次健康成功但业务首部超时；再次真实 TLS 探测断开才允许重建。
        let failed_probe = server.next().await;
        assert_eq!(failed_probe.path, "/api/health");
        assert_eq!(pool.wait_ready().await.unwrap(), 0);
        drop(failed_probe);
        let recovered_probe = server.next().await;
        assert_eq!(recovered_probe.path, "/api/health");
        assert_eq!(pool.wait_ready().await.unwrap(), 1);
        tokio::time::timeout(Duration::ZERO, old.closed())
            .await
            .unwrap();
        recovered_probe.reply.send(TestReply::Health).unwrap();
        let retry = server.next().await;
        assert_eq!(retry.path, "/api/work");
        retry.reply.send(TestReply::Health).unwrap();
        assert!(request.await.unwrap().0.contains("200"));
        assert_eq!(pool.endpoint_for_test().id(), old.id());
        drop(pending);
        pool.endpoint_for_test().close().await;
        tunnel.close().await;
    }

    #[tokio::test]
    async fn failed_iroh_post_is_not_replayed_and_the_next_request_recovers() {
        let (mut server, tunnel, pool, proxy) = spawn_iroh_proxy(RESPONSE_HEADERS_TIMEOUT).await;
        let port = proxy.port;
        let post = tokio::spawn(async move {
            reqwest::Client::new()
                .post(format!("http://127.0.0.1:{port}/api/post-once"))
                .body("{}")
                .send()
                .await
                .unwrap()
                .status()
        });
        let health = server.next().await;
        health.reply.send(TestReply::Health).unwrap();
        let submitted = server.next().await;
        assert_eq!(submitted.path, "/api/post-once");
        let old = pool.endpoint_for_test();
        old.close().await;
        assert!(post.await.unwrap().is_server_error());
        drop(submitted);
        let next = tokio::spawn(raw_get(port, "/api/after-post"));
        let probe = server.next().await;
        assert_eq!(probe.path, "/api/health");
        probe.reply.send(TestReply::Health).unwrap();
        let request = server.next().await;
        assert_eq!(request.path, "/api/after-post", "不能重放已发送的 POST");
        request.reply.send(TestReply::Health).unwrap();
        assert!(next.await.unwrap().0.contains("200"));
        assert_eq!(pool.wait_ready().await.unwrap(), 1);
        assert_eq!(pool.endpoint_for_test().id(), old.id());
        pool.endpoint_for_test().close().await;
        tunnel.close().await;
    }

    #[tokio::test]
    async fn healthy_iroh_http_errors_and_rejected_identity_never_rebuild_the_endpoint() {
        let (mut server, tunnel, pool, proxy) = spawn_iroh_proxy(RESPONSE_HEADERS_TIMEOUT).await;
        let old = pool.endpoint_for_test();
        for (reply, expected) in [
            (TestReply::Health, "200"),
            (TestReply::ServerError, "500"),
            (TestReply::Unauthorized, "401"),
        ] {
            let request = tokio::spawn(raw_get(proxy.port, "/api/business"));
            let mut exchange = server.next().await;
            if exchange.path == "/api/health" {
                exchange.reply.send(TestReply::Health).unwrap();
                exchange = server.next().await;
            }
            assert_eq!(exchange.path, "/api/business");
            exchange.reply.send(reply).unwrap();
            assert!(request.await.unwrap().0.contains(expected));
            assert_eq!(pool.wait_ready().await.unwrap(), 0);
        }
        for reply in [TestReply::ServerError, TestReply::Unauthorized] {
            proxy.invalidate_remote_upstream();
            let request = tokio::spawn(raw_get(proxy.port, "/api/business"));
            let health = server.next().await;
            assert_eq!(health.path, "/api/health");
            health.reply.send(reply).unwrap();
            assert!(request.await.unwrap().0.contains("502"));
            assert_eq!(pool.wait_ready().await.unwrap(), 0);
        }
        let mut profile = proxy.active_profile().unwrap();
        profile.fingerprint = "A".repeat(64);
        assert!(proxy.resolve_upstream(&profile).await.is_err());
        assert_eq!(pool.wait_ready().await.unwrap(), 0);
        profile.fingerprint = server.fingerprint.clone();
        profile.endpoints[0].node_id = Some("invalid-node-id".into());
        assert!(proxy.resolve_upstream(&profile).await.is_err());
        assert_eq!(pool.wait_ready().await.unwrap(), 0);
        assert_eq!(pool.endpoint_for_test().id(), old.id());
        old.close().await;
        tunnel.close().await;
    }

    #[tokio::test]
    async fn transport_classification_uses_real_reqwest_tls_and_io_error_chains() {
        let mut server = spawn_controlled_upstream().await;
        let bad_pin = pinned_client(&"A".repeat(64))
            .unwrap()
            .get(&server.url)
            .send()
            .await
            .unwrap_err();
        assert!(bad_pin.is_connect(), "指纹拒绝也会被包装为 connect 错误");
        assert!(!is_transport_error(&bad_pin));
        let invalid_url = pinned_client(&server.fingerprint)
            .unwrap()
            .get("invalid-url")
            .send()
            .await
            .unwrap_err();
        assert!(!is_transport_error(&invalid_url));
        let (silent, _) = spawn_silent_endpoint().await;
        let timeout = pinned_client(&server.fingerprint)
            .unwrap()
            .get(silent)
            .timeout(Duration::from_millis(30))
            .send()
            .await
            .unwrap_err();
        assert!(is_transport_error(&timeout));
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let refused = pinned_client(&server.fingerprint)
            .unwrap()
            .get(format!("https://{address}"))
            .send()
            .await
            .unwrap_err();
        assert!(is_transport_error(&refused));
        for reply in [
            None,
            Some(TestReply::ServerError),
            Some(TestReply::Unauthorized),
        ] {
            let outgoing = pinned_client(&server.fingerprint)
                .unwrap()
                .get(&server.url)
                .bearer_auth("pst_test");
            let request = tokio::spawn(async move { outgoing.send().await });
            let exchange = server.next().await;
            let transport = reply.is_none();
            if let Some(reply) = reply {
                exchange.reply.send(reply).unwrap();
            } else {
                drop(exchange);
            }
            let error = request
                .await
                .unwrap()
                .and_then(reqwest::Response::error_for_status)
                .unwrap_err();
            assert_eq!(is_transport_error(&error), transport, "{error:?}");
        }
    }

    #[tokio::test]
    async fn healthy_lan_is_independent_of_recovering_or_unavailable_iroh() {
        for recovering in [true, false] {
            let (mut server, tunnel, pool, proxy) =
                spawn_iroh_proxy(RESPONSE_HEADERS_TIMEOUT).await;
            let mut profile = proxy.active_profile().unwrap();
            profile
                .endpoints
                .push(ServerEndpoint::lan(server.url.clone()));
            proxy.store.lock().unwrap().upsert(profile).unwrap();
            let initial = tokio::spawn(raw_get(proxy.port, "/api/lan"));
            server.next().await.reply.send(TestReply::Health).unwrap();
            server.next().await.reply.send(TestReply::Health).unwrap();
            assert!(initial.await.unwrap().0.contains("200"));
            assert_eq!(proxy.active_transport().as_deref(), Some("lan"));

            pool.endpoint_for_test().close().await;
            // 固定在恢复等待或启动失败状态，避免 Iroh 恰好快速完成掩盖 LAN 被阻塞。
            pool.set_unavailable_for_test(recovering);
            for cached in [true, false] {
                if !cached {
                    proxy.invalidate_remote_upstream();
                }
                tokio::time::timeout(Duration::from_secs(1), async {
                    let request = tokio::spawn(raw_get(proxy.port, "/api/lan"));
                    if !cached {
                        let probe = server.next().await;
                        assert_eq!(probe.path, "/api/health");
                        probe.reply.send(TestReply::Health).unwrap();
                    }
                    let business = server.next().await;
                    assert_eq!(business.path, "/api/lan");
                    business.reply.send(TestReply::Health).unwrap();
                    assert!(request.await.unwrap().0.contains("200"));
                })
                .await
                .expect("健康 LAN 的缓存命中与新探测都不能等待 Iroh");
                assert_eq!(proxy.active_transport().as_deref(), Some("lan"));
                assert_eq!(pool.generation(), 1);
            }
            tunnel.close().await;
        }
    }

    #[tokio::test]
    async fn obsolete_proxy_generation_cannot_recover_the_current_tunnel() {
        let (_server, tunnel, pool, proxy) = spawn_iroh_proxy(RESPONSE_HEADERS_TIMEOUT).await;
        let old = proxy.remote_generation();
        proxy.invalidate_remote_upstream();
        assert!(!proxy.recover_tunnel_generation(old, 0));
        assert_eq!(pool.wait_ready().await.unwrap(), 0);
        assert!(proxy.recover_tunnel_generation(proxy.remote_generation(), 0));
        assert_eq!(pool.wait_ready().await.unwrap(), 1);
        assert!(!proxy.recover_tunnel_generation(old, 1));
        assert_eq!(pool.wait_ready().await.unwrap(), 1);
        pool.endpoint_for_test().close().await;
        tunnel.close().await;
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
                    b"HTTP/1.1 200 OK\r\ncontent-type: text/javascript\r\ncontent-length: 128\r\nconnection: close\r\n\r\nexport const broken =",
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
    async fn healthy_lan_after_many_silent_endpoints_is_probed_concurrently() {
        let (url, fingerprint) = spawn_upstream("health").await;
        let mut profile = profile_for(&url, &fingerprint);
        let mut silent = Vec::new();
        profile.endpoints.clear();
        for _ in 0..12 {
            let (url, connections) = spawn_silent_endpoint().await;
            profile.endpoints.push(ServerEndpoint::lan(url));
            silent.push(connections);
        }
        profile.endpoints.push(ServerEndpoint::lan(url));
        let port = spawn_proxy(Some(profile)).await;
        let (status, _) = tokio::time::timeout(Duration::from_secs(2), raw_get(port, "/api/test"))
            .await
            .expect("可用 LAN 不能排在失效端点的超时后面");
        assert!(status.contains("200"), "{status}");
        assert!(silent.iter().all(|count| count.load(Ordering::SeqCst) > 0));
    }

    #[tokio::test]
    async fn recovery_replaces_the_pool_while_healthy_requests_reuse_tls() {
        let mut server = spawn_controlled_upstream().await;
        let profile = profile_for(&server.url, &server.fingerprint);
        let proxy =
            spawn_proxy_with_timeouts(profile, Duration::from_secs(2), Duration::from_millis(250))
                .await;
        let first = tokio::spawn(raw_get(proxy.port, "/api/first"));
        let probe = server.next().await;
        let original_connection = probe.connection;
        assert_eq!(probe.path, "/api/health");
        probe.reply.send(TestReply::Health).unwrap();
        let request = server.next().await;
        assert_eq!(request.connection, original_connection);
        assert_eq!(request.path, "/api/first");
        request.reply.send(TestReply::Health).unwrap();
        assert!(first.await.unwrap().0.contains("200"));

        // 先证明缓存命中会复用连接，再让这条连接只收请求、不返回首部。
        let pending = tokio::spawn(raw_get(proxy.port, "/api/pending"));
        let old_request = server.next().await;
        assert_eq!(old_request.connection, original_connection);
        assert_eq!(old_request.path, "/api/pending");
        let retry_probe = server.next().await;
        assert_ne!(retry_probe.connection, original_connection);
        let retry_connection = retry_probe.connection;
        assert_eq!(retry_probe.path, "/api/health");
        retry_probe.reply.send(TestReply::Health).unwrap();
        let retry = server.next().await;
        assert_eq!(retry.connection, retry_connection);
        assert_eq!(retry.path, "/api/pending");
        retry.reply.send(TestReply::Health).unwrap();
        assert!(pending.await.unwrap().0.contains("200"));
        drop(old_request);

        proxy.resume_remote_network().await;
        let recovered = tokio::spawn(raw_get(proxy.port, "/api/recovered"));
        let fresh_probe = server.next().await;
        assert_ne!(fresh_probe.connection, retry_connection);
        let fresh_connection = fresh_probe.connection;
        assert_eq!(fresh_probe.path, "/api/health");
        fresh_probe.reply.send(TestReply::Health).unwrap();
        let fresh = server.next().await;
        assert_eq!(fresh.connection, fresh_connection);
        assert_eq!(fresh.path, "/api/recovered");
        fresh.reply.send(TestReply::Health).unwrap();
        assert!(recovered.await.unwrap().0.contains("200"));
    }

    #[tokio::test]
    async fn post_retries_a_superseded_probe_without_publishing_stale_cache_or_replaying() {
        let mut server = spawn_controlled_upstream().await;
        let proxy = spawn_proxy_with_timeouts(
            profile_for(&server.url, &server.fingerprint),
            Duration::from_secs(2),
            Duration::from_secs(1),
        )
        .await;
        let url = format!("http://127.0.0.1:{}/api/post", proxy.port);
        let request = tokio::spawn(async move { reqwest::Client::new().post(url).send().await });
        let old_probe = server.next().await;
        assert_eq!(old_probe.path, "/api/health");
        proxy.invalidate_remote_upstream();
        let generation = proxy.remote_generation();
        let old_connection = old_probe.connection;
        old_probe.reply.send(TestReply::Health).unwrap();

        let fresh_probe = server.next().await;
        assert_eq!(fresh_probe.path, "/api/health");
        assert_ne!(fresh_probe.connection, old_connection);
        assert!(proxy.active_transport().is_none());
        fresh_probe.reply.send(TestReply::Health).unwrap();
        let business = server.next().await;
        assert_eq!(business.path, "/api/post");
        business.reply.send(TestReply::Health).unwrap();
        assert_eq!(request.await.unwrap().unwrap().status(), StatusCode::OK);
        assert_eq!(proxy.remote_generation(), generation);
        assert_eq!(proxy.active_transport().as_deref(), Some("lan"));
        assert!(server.exchanges.try_recv().is_err());
    }

    #[tokio::test]
    async fn concurrent_startup_requests_share_one_health_probe() {
        let mut server = spawn_controlled_upstream().await;
        let profile = profile_for(&server.url, &server.fingerprint);
        let proxy = spawn_proxy_with_timeouts(
            profile.clone(),
            Duration::from_secs(2),
            Duration::from_secs(1),
        )
        .await;
        let owner = start_resolution(&proxy, &profile);
        let probe = server.next().await;
        let generation = proxy.remote_generation();
        let requests = (0..8)
            .map(|_| tokio::spawn(raw_get(proxy.port, "/api/parallel")))
            .collect::<Vec<_>>();
        assert!(
            tokio::time::timeout(Duration::from_millis(100), server.exchanges.recv())
                .await
                .is_err(),
            "等待者不应重复发送健康探测"
        );
        probe.reply.send(TestReply::Health).unwrap();
        assert_eq!(owner.await.unwrap().unwrap().generation, generation);
        for _ in 0..requests.len() {
            let business = server.next().await;
            assert_eq!(business.path, "/api/parallel");
            business.reply.send(TestReply::Health).unwrap();
        }
        for request in requests {
            assert!(request.await.unwrap().0.contains("200"));
        }
        assert_eq!(proxy.remote_generation(), generation);
    }

    #[tokio::test]
    async fn resolution_waiter_timeout_does_not_invalidate_the_probe_owner() {
        let mut server = spawn_controlled_upstream().await;
        let profile = profile_for(&server.url, &server.fingerprint);
        let proxy = spawn_proxy_with_timeouts(
            profile.clone(),
            Duration::from_millis(150),
            Duration::from_millis(100),
        )
        .await;
        let owner = start_resolution(&proxy, &profile);
        let probe = server.next().await;
        let generation = proxy.remote_generation();
        let (status, _) = raw_get(proxy.port, "/api/waiter").await;
        assert!(status.contains("504"));
        assert_eq!(proxy.remote_generation(), generation);
        probe.reply.send(TestReply::Health).unwrap();
        assert_eq!(owner.await.unwrap().unwrap().generation, generation);
        assert!(server.exchanges.try_recv().is_err());
    }

    #[tokio::test]
    async fn business_errors_leave_concurrent_healthy_requests_and_the_pool_intact() {
        for (status, reply) in [
            (500, TestReply::ServerError),
            (408, TestReply::RequestTimeout),
        ] {
            let mut server = spawn_controlled_upstream().await;
            let proxy = spawn_proxy_with_timeouts(
                profile_for(&server.url, &server.fingerprint),
                Duration::from_secs(2),
                Duration::from_secs(1),
            )
            .await;
            let failing = tokio::spawn(raw_get(proxy.port, "/api/failing"));
            server.next().await.reply.send(TestReply::Health).unwrap();
            let failure = server.next().await;
            assert_eq!(failure.path, "/api/failing");
            let healthy = tokio::spawn(raw_get(proxy.port, "/api/healthy"));
            let pending = server.next().await;
            assert_eq!(pending.path, "/api/healthy");
            let generation = proxy.remote_generation();
            failure.reply.send(reply).unwrap();
            assert!(failing.await.unwrap().0.contains(&status.to_string()));
            assert_eq!(proxy.remote_generation(), generation);
            assert_eq!(proxy.active_transport().as_deref(), Some("lan"));
            pending.reply.send(TestReply::Health).unwrap();
            assert!(healthy.await.unwrap().0.contains("200"));
            let next = tokio::spawn(raw_get(proxy.port, "/api/next"));
            let reused = server.next().await;
            assert_eq!(reused.path, "/api/next");
            reused.reply.send(TestReply::Health).unwrap();
            assert!(next.await.unwrap().0.contains("200"));
        }
    }

    #[tokio::test]
    async fn old_json_failure_cannot_invalidate_the_recovered_connection() {
        let mut server = spawn_controlled_upstream().await;
        let proxy = spawn_proxy_with_timeouts(
            profile_for(&server.url, &server.fingerprint),
            Duration::from_secs(2),
            Duration::from_secs(1),
        )
        .await;
        let old = tokio::spawn(raw_get(proxy.port, "/api/old"));
        server.next().await.reply.send(TestReply::Health).unwrap();
        let old_request = server.next().await;
        assert_eq!(old_request.path, "/api/old");

        proxy.invalidate_remote_upstream();
        let recovered = tokio::spawn(raw_get(proxy.port, "/api/new"));
        let fresh_probe = server.next().await;
        let fresh_connection = fresh_probe.connection;
        assert_ne!(fresh_connection, old_request.connection);
        assert_eq!(fresh_probe.path, "/api/health");
        fresh_probe.reply.send(TestReply::Health).unwrap();
        let fresh = server.next().await;
        assert_eq!(fresh.path, "/api/new");
        fresh.reply.send(TestReply::Health).unwrap();
        assert!(recovered.await.unwrap().0.contains("200"));
        let generation = proxy.remote_generation();

        old_request.reply.send(TestReply::TruncatedJson).unwrap();
        assert!(old.await.unwrap().0.contains("502"));
        assert_eq!(proxy.remote_generation(), generation);
        assert_eq!(proxy.active_transport().as_deref(), Some("lan"));
        let reused = tokio::spawn(raw_get(proxy.port, "/api/reused"));
        let reused_request = server.next().await;
        assert_eq!(reused_request.path, "/api/reused");
        assert_eq!(reused_request.connection, fresh_connection);
        reused_request.reply.send(TestReply::Health).unwrap();
        assert!(reused.await.unwrap().0.contains("200"));
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
    async fn silent_lan_group_falls_back_to_iroh_and_recovery_prefers_lan() {
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
        let mut silent = Vec::new();
        for _ in 0..12 {
            let (url, connections) = spawn_silent_endpoint().await;
            profile.endpoints.push(ServerEndpoint::lan(url));
            silent.push(connections);
        }
        profile.endpoints.push(ServerEndpoint::iroh(remote));
        let path = std::env::temp_dir().join(format!("pisper-proxy-test-{}.json", fast_id()));
        let mut store = crate::mobile::store::ProfileStore::load(&path);
        store.upsert(profile.clone()).unwrap();
        store.set_last_mode("remote").unwrap();
        let proxy = start_proxy(Arc::new(Mutex::new(store)), Some(tunnels))
            .await
            .unwrap();

        let (status, raw) = tokio::time::timeout(
            PROBE_TIMEOUT + Duration::from_secs(4),
            raw_get(proxy.port, "/api/health"),
        )
        .await
        .expect("无响应 LAN 的数量不能耗尽 Iroh 的连接机会");
        assert!(status.contains("200"), "unexpected status: {status}");
        assert!(String::from_utf8_lossy(&raw).contains("{\"ok\":true}"));
        assert_eq!(proxy.active_transport().as_deref(), Some("iroh"));
        assert!(silent.iter().all(|count| count.load(Ordering::SeqCst) > 0));

        // 档案即使把 Iroh 放在前面，恢复到可用 LAN 后也应优先局域网。
        profile.endpoints.retain(|endpoint| endpoint.kind == "iroh");
        profile.endpoints.push(ServerEndpoint::lan(url));
        proxy.store.lock().unwrap().upsert(profile).unwrap();
        proxy.invalidate_remote_upstream();
        let (status, _) =
            tokio::time::timeout(Duration::from_secs(2), raw_get(proxy.port, "/api/health"))
                .await
                .unwrap();
        assert!(status.contains("200"), "{status}");
        assert_eq!(proxy.active_transport().as_deref(), Some("lan"));
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
    async fn remote_json_is_complete_and_preserves_headers() {
        for behavior in ["health", "json_suffix"] {
            let (url, fingerprint) = spawn_upstream(behavior).await;
            let port = spawn_proxy(Some(profile_for(&url, &fingerprint))).await;
            let response = reqwest::Client::new()
                .get(format!("http://127.0.0.1:{port}/api/test"))
                .timeout(Duration::from_secs(2))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()["x-upstream"], "preserved");
            assert_eq!(response.content_length(), Some(11));
            assert!(!response.headers().contains_key("transfer-encoding"));
            assert_eq!(response.bytes().await.unwrap(), b"{\"ok\":true}"[..]);
        }
    }

    #[tokio::test]
    async fn remote_json_truncation_returns_502_without_partial_json() {
        for behavior in ["truncated_json", "truncated_chunked_json"] {
            let (url, fingerprint) = spawn_upstream(behavior).await;
            let proxy = spawn_proxy_with_timeouts(
                profile_for(&url, &fingerprint),
                Duration::from_secs(1),
                Duration::from_millis(500),
            )
            .await;
            let (status, raw) =
                tokio::time::timeout(Duration::from_secs(2), raw_get(proxy.port, "/api/test"))
                    .await
                    .unwrap();
            assert!(status.contains("502"), "{status}");
            let text = String::from_utf8_lossy(&raw);
            assert!(text.contains("读取桌面端 JSON 响应失败"));
            assert!(!text.contains("{\"ok\":"));
            assert!(proxy.active_transport().is_none());
        }
    }

    #[tokio::test]
    async fn stalled_remote_json_returns_504_within_shared_budget() {
        let (url, fingerprint) = spawn_upstream("stalled_json").await;
        let proxy = spawn_proxy_with_timeouts(
            profile_for(&url, &fingerprint),
            Duration::from_millis(200),
            Duration::from_millis(150),
        )
        .await;
        let (status, raw) =
            tokio::time::timeout(Duration::from_secs(1), raw_get(proxy.port, "/api/test"))
                .await
                .expect("停滞 JSON 必须在前端超时前返回");
        assert!(status.contains("504"), "{status}");
        assert!(!String::from_utf8_lossy(&raw).contains("{\"ok\":"));
        assert!(proxy.active_transport().is_none());
    }

    #[tokio::test]
    async fn endpoint_resolution_uses_the_request_budget() {
        let requests = Arc::new(AtomicU64::new(0));
        let (url, fingerprint) = spawn_scripted_upstream(
            "health",
            Duration::from_secs(2),
            Duration::ZERO,
            requests.clone(),
        )
        .await;
        let proxy = spawn_proxy_with_timeouts(
            profile_for(&url, &fingerprint),
            Duration::from_millis(150),
            Duration::from_millis(100),
        )
        .await;
        let (status, _) =
            tokio::time::timeout(Duration::from_secs(1), raw_get(proxy.port, "/api/test"))
                .await
                .expect("探测不能绕过统一预算");
        assert!(status.contains("504"), "{status}");
        assert_eq!(requests.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn safe_retries_and_probes_share_one_deadline() {
        let requests = Arc::new(AtomicU64::new(0));
        let (url, fingerprint) = spawn_scripted_upstream(
            "health",
            Duration::from_millis(100),
            Duration::from_secs(2),
            requests.clone(),
        )
        .await;
        let proxy = spawn_proxy_with_timeouts(
            profile_for(&url, &fingerprint),
            Duration::from_millis(700),
            Duration::from_millis(400),
        )
        .await;
        // 独立预算将耗时至少 1000ms；统一预算应在第二次首部等待中结束。
        let (status, _) =
            tokio::time::timeout(Duration::from_millis(900), raw_get(proxy.port, "/api/test"))
                .await
                .expect("重试不能重新获得完整预算");
        assert!(status.contains("504"), "{status}");
        assert_eq!(requests.load(Ordering::SeqCst), 2);
        assert!(proxy.active_transport().is_none());
    }

    #[tokio::test]
    async fn post_is_not_replayed_after_headers_timeout_or_disconnect() {
        for behavior in ["health", "disconnect"] {
            let requests = Arc::new(AtomicU64::new(0));
            let delay = if behavior == "health" {
                Duration::from_secs(2)
            } else {
                Duration::ZERO
            };
            let (url, fingerprint) =
                spawn_scripted_upstream(behavior, Duration::ZERO, delay, requests.clone()).await;
            let proxy = spawn_proxy_with_timeouts(
                profile_for(&url, &fingerprint),
                Duration::from_millis(600),
                Duration::from_millis(150),
            )
            .await;
            let response = reqwest::Client::new()
                .post(format!("http://127.0.0.1:{}/api/chat", proxy.port))
                .body("{\"message\":\"test\"}")
                .timeout(Duration::from_secs(1))
                .send()
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                if behavior == "health" {
                    StatusCode::GATEWAY_TIMEOUT
                } else {
                    StatusCode::BAD_GATEWAY
                }
            );
            assert_eq!(requests.load(Ordering::SeqCst), 1);
            assert!(proxy.active_transport().is_none());
        }
    }

    #[tokio::test]
    async fn downloads_stream_past_the_response_budget() {
        let (url, fingerprint) = spawn_upstream("download").await;
        let proxy = spawn_proxy_with_timeouts(
            profile_for(&url, &fingerprint),
            Duration::from_millis(200),
            Duration::from_millis(150),
        )
        .await;
        let mut response = reqwest::Client::new()
            .get(format!("http://127.0.0.1:{}/api/file", proxy.port))
            .timeout(Duration::from_secs(2))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let first = tokio::time::timeout(Duration::from_millis(200), response.chunk())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(first, b"one"[..]);
        assert_eq!(response.bytes().await.unwrap(), b"two"[..]);
    }

    #[tokio::test]
    async fn sse_streams_incrementally() {
        SSE_GATE.set(tokio::sync::Semaphore::new(0)).ok();
        let (url, fingerprint) = spawn_upstream("sse").await;
        let proxy = spawn_proxy_with_timeouts(
            profile_for(&url, &fingerprint),
            Duration::from_millis(200),
            Duration::from_millis(150),
        )
        .await;
        let port = proxy.port;

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
        // 超过响应构造预算后才放行第二帧，证明长连接不会被总预算切断。
        tokio::time::sleep(Duration::from_millis(250)).await;
        SSE_GATE.get().unwrap().add_permits(1);
        let mut rest = Vec::new();
        let read_rest = async { stream.read_to_end(&mut rest).await.unwrap() };
        tokio::time::timeout(std::time::Duration::from_secs(10), read_rest)
            .await
            .expect("读取剩余帧超时");
        assert!(String::from_utf8_lossy(&rest).contains("event: done"));
    }
}
