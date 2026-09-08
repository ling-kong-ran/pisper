//! Iroh 透明字节隧道：QUIC 仅承载 TCP 字节，不终止或改写上层 TLS、HTTP 与 SSE。
use std::{
    collections::HashMap,
    fs, io,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::Path,
    str::FromStr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayMode, RelayUrl, SecretKey, TransportAddr, Watcher,
};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{copy, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};

pub const PISPER_TUNNEL_ALPN: &[u8] = b"pisper/remote-tcp/1";

const PRODUCTION_RELAY_HOSTS: [&str; 3] = [
    "use1-1.relay.n0.iroh.link",
    "euc1-1.relay.n0.iroh.link",
    "aps1-1.relay.n0.iroh.link",
];

/// 返回应用使用的生产 relay 配置，避免旧版 Iroh 默认地址与公网证书不匹配。
pub fn production_relay_mode() -> RelayMode {
    RelayMode::custom(PRODUCTION_RELAY_HOSTS.iter().map(|host| {
        format!("https://{host}")
            .parse::<RelayUrl>()
            .expect("生产 relay 地址必须有效")
    }))
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelEndpoint {
    pub node_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay_url: Option<String>,
    #[serde(default)]
    pub direct_addresses: Vec<String>,
}

impl TunnelEndpoint {
    fn from_endpoint_addr(address: EndpointAddr) -> Self {
        let mut relay_url = None;
        let mut direct_addresses = Vec::new();
        for address in address.addrs {
            match address {
                TransportAddr::Relay(value) => relay_url = Some(value.to_string()),
                TransportAddr::Ip(value) => direct_addresses.push(value.to_string()),
                TransportAddr::Custom(_) => {}
                _ => {}
            }
        }
        Self {
            node_id: address.id.to_string(),
            relay_url,
            direct_addresses,
        }
    }

    fn to_endpoint_addr(&self) -> Result<EndpointAddr, String> {
        let node_id = EndpointId::from_str(&self.node_id)
            .map_err(|error| format!("Iroh 节点 ID 无效：{error}"))?;
        let relay_url = self
            .relay_url
            .as_deref()
            .map(RelayUrl::from_str)
            .transpose()
            .map_err(|error| format!("Iroh relay 地址无效：{error}"))?;
        let relay = relay_url.map(TransportAddr::Relay);
        let direct_addresses = self
            .direct_addresses
            .iter()
            .map(|value| {
                value
                    .parse::<SocketAddr>()
                    .map(TransportAddr::Ip)
                    .map_err(|error| format!("Iroh 直连地址无效：{error}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(EndpointAddr::from_parts(
            node_id,
            relay.into_iter().chain(direct_addresses),
        ))
    }
}

pub struct TunnelServer {
    endpoint: Endpoint,
    accept_task: JoinHandle<()>,
}

impl TunnelServer {
    pub fn node_id(&self) -> String {
        self.endpoint.id().to_string()
    }

    pub fn local_port(&self) -> Option<u16> {
        self.endpoint.bound_sockets().first().map(SocketAddr::port)
    }

    pub async fn endpoint(&self, timeout: Duration) -> TunnelEndpoint {
        let mut statuses = self.endpoint.home_relay_status();
        let relay_url = tokio::time::timeout(timeout, async {
            loop {
                let value = statuses.get();
                if let Some(status) = value.iter().find(|status| status.is_connected()) {
                    break Some(status.url().clone());
                }
                statuses.updated().await.ok()?;
            }
        })
        .await
        .ok()
        .flatten();
        let address = self.endpoint.watch_addr().get();
        let mut published = TunnelEndpoint::from_endpoint_addr(address);
        published.node_id = self.endpoint.id().to_string();
        if relay_url.is_some() {
            published.relay_url = relay_url.map(|value| value.to_string());
        }
        published
    }

    pub async fn shutdown(&self) {
        self.accept_task.abort();
        self.endpoint.close().await;
    }

    pub async fn close(self) {
        self.shutdown().await;
    }
}

impl Drop for TunnelServer {
    fn drop(&mut self) {
        self.accept_task.abort();
    }
}

#[derive(Clone)]
struct TunnelClientConfig {
    secret_key: SecretKey,
    relay_mode: RelayMode,
    enable_ip_transports: bool,
}

#[derive(Clone)]
pub struct TunnelClient {
    endpoint: Endpoint,
    config: TunnelClientConfig,
}

impl TunnelClient {
    pub async fn start(secret_key: SecretKey, relay_mode: RelayMode) -> Result<Self, String> {
        Self::start_with_ip_transports(secret_key, relay_mode, true).await
    }

    #[cfg(test)]
    async fn start_relay_only(
        secret_key: SecretKey,
        relay_mode: RelayMode,
    ) -> Result<Self, String> {
        Self::start_with_ip_transports(secret_key, relay_mode, false).await
    }

    async fn start_with_ip_transports(
        secret_key: SecretKey,
        relay_mode: RelayMode,
        enable_ip_transports: bool,
    ) -> Result<Self, String> {
        let config = TunnelClientConfig {
            secret_key: secret_key.clone(),
            relay_mode: relay_mode.clone(),
            enable_ip_transports,
        };
        let use_address_lookup = !matches!(&relay_mode, RelayMode::Disabled);
        let mut builder = Endpoint::builder(iroh::endpoint::presets::Minimal)
            .secret_key(secret_key)
            .relay_mode(relay_mode);
        if use_address_lookup {
            // 二维码仅保存地址快照；桌面换 relay 后必须按经过身份校验的 NodeId 查找新地址。
            builder = builder
                .address_lookup(iroh::address_lookup::PkarrResolver::n0_dns())
                .address_lookup(iroh::address_lookup::DnsAddressLookup::n0_dns());
        }
        if !enable_ip_transports {
            builder = builder.clear_ip_transports();
        }
        let endpoint = builder
            .bind()
            .await
            .map_err(|error| format!("Iroh 客户端启动失败：{error}"))?;
        Ok(Self { endpoint, config })
    }

    pub async fn open_bridge(&self, remote: TunnelEndpoint) -> Result<TunnelBridge, String> {
        let remote = remote.to_endpoint_addr()?;
        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
            .await
            .map_err(|error| format!("Iroh 回环桥接监听失败：{error}"))?;
        let port = listener
            .local_addr()
            .map_err(|error| error.to_string())?
            .port();
        let endpoint = self.endpoint.clone();
        let (stop, mut stopping) = tokio::sync::watch::channel(false);
        let (completed, finished) = tokio::sync::watch::channel(false);
        let accept_task = tokio::spawn(async move {
            // 子任务必须随桥一起退休，避免冻结期间的连接继续持有旧 Endpoint。
            let mut connections = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    biased;
                    _ = stopping.changed() => break,
                    _ = connections.join_next(), if !connections.is_empty() => {},
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else { break };
                        let endpoint = endpoint.clone();
                        let remote = remote.clone();
                        connections.spawn(async move {
                            let Ok(connection) = endpoint.connect(remote, PISPER_TUNNEL_ALPN).await else {
                                return;
                            };
                            let Ok((send, recv)) = connection.open_bi().await else {
                                return;
                            };
                            let _ = copy_tcp_and_quic(stream, send, recv).await;
                        });
                    }
                }
            }
            connections.shutdown().await;
            completed.send_replace(true);
        });
        Ok(TunnelBridge {
            port,
            accept_task,
            stop,
            finished,
        })
    }

    pub async fn close(self) {
        self.endpoint.close().await;
        // 其他持有者可能已经开始 close；必须等待真正关闭后才能注册同身份的新实例。
        self.endpoint.closed().await;
    }
}

/// 同一故障窗口最多重建一次，避免离线时并发请求反复注册同一个节点身份。
const TUNNEL_RECOVERY_COOLDOWN: Duration = Duration::from_secs(30);

struct TunnelPoolState {
    client: Option<TunnelClient>,
    bridges: HashMap<String, Arc<TunnelBridge>>,
    generation: u64,
    recovering: bool,
    last_recovery: Option<Instant>,
    error: Option<String>,
}

struct TunnelPoolInner {
    config: TunnelClientConfig,
    state: Mutex<TunnelPoolState>,
    changed: tokio::sync::watch::Sender<u64>,
}

pub struct TunnelBridgePool {
    inner: Arc<TunnelPoolInner>,
}

fn recovery_trace(stage: &'static str, generation: u64) {
    if std::env::var("PISPER_MOBILE_NETWORK_TRACE").is_ok_and(|value| value == "1") {
        // 只记录固定阶段与计数，不把连接错误链中的地址或身份材料写入设备日志。
        eprintln!("[pisper-iroh] stage={stage} generation={generation}");
    }
}

impl TunnelBridgePool {
    pub async fn start(secret_key: SecretKey, relay_mode: RelayMode) -> Result<Self, String> {
        let client = TunnelClient::start(secret_key, relay_mode).await?;
        Ok(Self {
            inner: Arc::new(TunnelPoolInner {
                config: client.config.clone(),
                state: Mutex::new(TunnelPoolState {
                    client: Some(client),
                    bridges: HashMap::new(),
                    generation: 0,
                    recovering: false,
                    last_recovery: None,
                    error: None,
                }),
                changed: tokio::sync::watch::channel(0).0,
            }),
        })
    }

    /// LAN 只需比较世代，不能等待 Iroh 回收或依赖其启动结果。
    pub fn generation(&self) -> u64 {
        self.inner
            .state
            .lock()
            .map(|state| state.generation)
            .unwrap_or(0)
    }

    pub async fn network_change(&self) {
        let endpoint = self
            .inner
            .state
            .lock()
            .ok()
            .and_then(|state| state.client.as_ref().map(|client| client.endpoint.clone()));
        if let Some(endpoint) = endpoint {
            endpoint.network_change().await;
        }
    }

    /// 订阅先于检查，完成通知不会在检查与等待之间丢失。
    pub async fn wait_ready(&self) -> Result<u64, String> {
        let mut changed = self.inner.changed.subscribe();
        let mut joined = false;
        loop {
            let unavailable = {
                let state = self.inner.state.lock().map_err(|_| "Iroh pool poisoned")?;
                if !state.recovering {
                    if state.client.is_some() {
                        return Ok(state.generation);
                    }
                    Some((
                        state.generation,
                        state
                            .error
                            .clone()
                            .unwrap_or_else(|| "Iroh 尚未就绪。".into()),
                    ))
                } else {
                    if !joined {
                        recovery_trace("recovery_join", state.generation);
                        joined = true;
                    }
                    None
                }
            };
            if let Some((generation, error)) = unavailable {
                if !self.recover_if_current(generation) {
                    return Err(error);
                }
                continue;
            }
            changed
                .changed()
                .await
                .map_err(|_| "Iroh 恢复任务已结束。")?;
        }
    }

    /// 同步认领恢复权，调用者可在检查代理世代的同一临界区内提交失败证据。
    pub fn recover_if_current(&self, generation: u64) -> bool {
        let Ok(mut state) = self.inner.state.lock() else {
            return false;
        };
        if state.recovering {
            let joined = generation.wrapping_add(1) == state.generation;
            if joined {
                recovery_trace("recovery_join", state.generation);
            }
            return joined;
        }
        if state.generation != generation {
            return false;
        }
        if state
            .last_recovery
            .is_some_and(|last| last.elapsed() < TUNNEL_RECOVERY_COOLDOWN)
        {
            recovery_trace("recovery_cooldown", generation);
            return false;
        }
        state.recovering = true;
        state.generation = state.generation.wrapping_add(1);
        state.last_recovery = Some(Instant::now());
        let generation = state.generation;
        let old = state.client.take();
        let bridges = std::mem::take(&mut state.bridges);
        let inner = self.inner.clone();
        recovery_trace("recovery_start", generation);
        // close 一旦开始便不能取消；独立任务持有全部资源，HTTP 超时只取消等待者。
        tokio::spawn(async move {
            for bridge in bridges.values() {
                bridge.stop.send_replace(true);
            }
            for bridge in bridges.values() {
                bridge.retire().await;
            }
            if let Some(old) = old {
                old.close().await;
            }
            recovery_trace("recovery_closed", generation);
            let config = &inner.config;
            let result = TunnelClient::start_with_ip_transports(
                config.secret_key.clone(),
                config.relay_mode.clone(),
                config.enable_ip_transports,
            )
            .await;
            let success = result.is_ok();
            {
                let mut state = inner.state.lock().expect("Iroh pool poisoned");
                match result {
                    Ok(client) => {
                        state.client = Some(client);
                        state.error = None;
                    }
                    Err(error) => {
                        state.error = Some(error);
                    }
                }
                state.recovering = false;
            }
            recovery_trace(
                if success {
                    "recovery_end"
                } else {
                    "recovery_failed"
                },
                generation,
            );
            inner.changed.send_replace(generation);
        });
        true
    }

    pub async fn bridge_url(&self, remote: TunnelEndpoint) -> Result<String, String> {
        self.bridge_url_with_generation(remote)
            .await
            .map(|(url, _)| url)
    }

    pub async fn bridge_url_with_generation(
        &self,
        remote: TunnelEndpoint,
    ) -> Result<(String, u64), String> {
        let key = serde_json::to_string(&remote).map_err(|error| error.to_string())?;
        loop {
            self.wait_ready().await?;
            let snapshot = {
                let state = self.inner.state.lock().map_err(|_| "Iroh pool poisoned")?;
                if let Some(bridge) = state.bridges.get(&key) {
                    if !bridge.accept_task.is_finished() {
                        return Ok((bridge.url(), state.generation));
                    }
                }
                state
                    .client
                    .as_ref()
                    .map(|client| (client.clone(), state.generation))
            };
            let Some((client, generation)) = snapshot else {
                continue;
            };
            let bridge = Arc::new(client.open_bridge(remote.clone()).await?);
            let mut state = self.inner.state.lock().map_err(|_| "Iroh pool poisoned")?;
            if state.recovering || state.generation != generation {
                continue;
            }
            let current = state
                .bridges
                .entry(key.clone())
                .or_insert_with(|| bridge.clone());
            if current.accept_task.is_finished() {
                *current = bridge;
            }
            return Ok((current.url(), generation));
        }
    }

    pub async fn invalidate(&self, remote: &TunnelEndpoint) {
        if let Ok(key) = serde_json::to_string(remote) {
            if let Ok(mut state) = self.inner.state.lock() {
                state.bridges.remove(&key);
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn set_unavailable_for_test(&self, recovering: bool) {
        let mut state = self.inner.state.lock().unwrap();
        state.client = None;
        state.bridges.clear();
        state.generation += 1;
        state.recovering = recovering;
        state.last_recovery = Some(Instant::now());
        state.error = Some("测试模拟 Endpoint 启动失败。".into());
    }

    #[cfg(test)]
    pub(crate) fn endpoint_for_test(&self) -> Endpoint {
        self.inner
            .state
            .lock()
            .unwrap()
            .client
            .as_ref()
            .unwrap()
            .endpoint
            .clone()
    }
}

pub struct TunnelBridge {
    pub port: u16,
    accept_task: JoinHandle<()>,
    stop: tokio::sync::watch::Sender<bool>,
    finished: tokio::sync::watch::Receiver<bool>,
}

impl TunnelBridge {
    pub fn url(&self) -> String {
        format!("https://127.0.0.1:{}", self.port)
    }

    async fn retire(&self) {
        self.stop.send_replace(true);
        let mut finished = self.finished.clone();
        let _ = finished.wait_for(|finished| *finished).await;
    }
}

impl Drop for TunnelBridge {
    fn drop(&mut self) {
        self.accept_task.abort();
    }
}

pub async fn start_server(
    target: SocketAddr,
    secret_key: SecretKey,
    relay_mode: RelayMode,
) -> Result<TunnelServer, String> {
    start_server_with_ip_transports(target, secret_key, relay_mode, true).await
}

#[cfg(test)]
async fn start_server_relay_only(
    target: SocketAddr,
    secret_key: SecretKey,
    relay_mode: RelayMode,
) -> Result<TunnelServer, String> {
    start_server_with_ip_transports(target, secret_key, relay_mode, false).await
}

async fn start_server_with_ip_transports(
    target: SocketAddr,
    secret_key: SecretKey,
    relay_mode: RelayMode,
    enable_ip_transports: bool,
) -> Result<TunnelServer, String> {
    let publish_address = !matches!(&relay_mode, RelayMode::Disabled);
    let mut builder = Endpoint::builder(iroh::endpoint::presets::Minimal)
        .secret_key(secret_key)
        .alpns(vec![PISPER_TUNNEL_ALPN.to_vec()])
        .relay_mode(relay_mode);
    if publish_address {
        // 只发布签名 relay 记录，不公开局域网 IP；网络变化时由 Iroh 更新 NodeId 对应地址。
        builder = builder.address_lookup(
            iroh::address_lookup::PkarrPublisher::n0_dns()
                .addr_filter(iroh::address_lookup::AddrFilter::relay_only()),
        );
    }
    if !enable_ip_transports {
        builder = builder.clear_ip_transports();
    }
    let endpoint = builder
        .bind()
        .await
        .map_err(|error| format!("Iroh 服务端启动失败：{error}"))?;
    let accept_endpoint = endpoint.clone();
    let accept_task = tokio::spawn(async move {
        while let Some(incoming) = accept_endpoint.accept().await {
            let Ok(connecting) = incoming.accept() else {
                continue;
            };
            tokio::spawn(async move {
                let Ok(connection) = connecting.await else {
                    return;
                };
                loop {
                    let Ok((send, recv)) = connection.accept_bi().await else {
                        break;
                    };
                    tokio::spawn(async move {
                        let Ok(stream) = TcpStream::connect(target).await else {
                            return;
                        };
                        let _ = copy_tcp_and_quic(stream, send, recv).await;
                    });
                }
            });
        }
    });
    Ok(TunnelServer {
        endpoint,
        accept_task,
    })
}

async fn copy_tcp_and_quic(
    tcp: TcpStream,
    mut send: iroh::endpoint::SendStream,
    mut recv: iroh::endpoint::RecvStream,
) -> io::Result<()> {
    let (mut tcp_read, mut tcp_write) = tcp.into_split();
    let upload = async {
        copy(&mut tcp_read, &mut send).await?;
        send.finish().map_err(io::Error::other)
    };
    let download = async {
        copy(&mut recv, &mut tcp_write).await?;
        tcp_write.shutdown().await
    };
    tokio::try_join!(upload, download)?;
    Ok(())
}

pub fn load_or_create_secret(path: &Path) -> Result<SecretKey, String> {
    if let Ok(bytes) = fs::read(path) {
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| "Iroh 私钥文件长度无效。".to_string())?;
        return Ok(SecretKey::from_bytes(&bytes));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let secret = SecretKey::generate();
    let temporary = path.with_extension("key.tmp");
    let mut options = fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    use std::io::Write;
    let mut file = options
        .open(&temporary)
        .map_err(|error| error.to_string())?;
    file.write_all(&secret.to_bytes())
        .map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    fs::rename(temporary, path).map_err(|error| error.to_string())?;
    Ok(secret)
}

pub fn loopback_endpoint(node_id: String, port: u16) -> TunnelEndpoint {
    TunnelEndpoint {
        node_id,
        relay_url: None,
        direct_addresses: vec![SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port).to_string()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BytesMut;
    use futures_util::StreamExt;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn ensure_crypto_provider() {
        static ONCE: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        ONCE.get_or_init(|| {
            let _ = rustls::crypto::ring::default_provider().install_default();
        });
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        use sha2::Digest;
        sha2::Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect()
    }

    async fn spawn_tls_sse_server() -> (SocketAddr, String, Arc<tokio::sync::Semaphore>) {
        spawn_tls_sse_server_with_connections(1).await
    }

    async fn spawn_tls_sse_server_with_connections(
        connections: usize,
    ) -> (SocketAddr, String, Arc<tokio::sync::Semaphore>) {
        ensure_crypto_provider();
        let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let cert_der = certified.cert.der().clone();
        let fingerprint = sha256_hex(cert_der.as_ref());
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let tls_config = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![cert_der],
                rustls::pki_types::PrivatePkcs8KeyDer::from(certified.key_pair.serialize_der())
                    .into(),
            )
            .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls_config));
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let server_gate = gate.clone();
        tokio::spawn(async move {
            for _ in 0..connections {
                let (stream, _) = listener.accept().await.unwrap();
                let mut stream = acceptor.accept(stream).await.unwrap();
                let mut request = Vec::new();
                let mut buffer = [0u8; 1024];
                while !request.windows(4).any(|value| value == b"\r\n\r\n") {
                    let read = stream.read(&mut buffer).await.unwrap();
                    if read == 0 {
                        return;
                    }
                    request.extend_from_slice(&buffer[..read]);
                }
                stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n",
                )
                .await
                .unwrap();
                let first = b"event: run\ndata: {\"runId\":\"r1\"}\n\n";
                stream
                    .write_all(format!("{:x}\r\n", first.len()).as_bytes())
                    .await
                    .unwrap();
                stream.write_all(first).await.unwrap();
                stream.write_all(b"\r\n").await.unwrap();
                stream.flush().await.unwrap();
                let permit = server_gate.acquire().await.unwrap();
                permit.forget();
                let second = b"event: done\ndata: {}\n\n";
                stream
                    .write_all(format!("{:x}\r\n", second.len()).as_bytes())
                    .await
                    .unwrap();
                stream.write_all(second).await.unwrap();
                stream.write_all(b"\r\n0\r\n\r\n").await.unwrap();
            }
        });
        (address, fingerprint, gate)
    }

    #[tokio::test]
    async fn repeated_network_notifications_preserve_identity_and_existing_tls_bridge() {
        let (target, fingerprint, gate) = spawn_tls_sse_server_with_connections(2).await;
        let server = start_server(target, SecretKey::generate(), RelayMode::Disabled)
            .await
            .unwrap();
        let remote = loopback_endpoint(server.node_id(), server.local_port().unwrap());
        let pool = TunnelBridgePool::start(SecretKey::generate(), RelayMode::Disabled)
            .await
            .unwrap();
        let original_id = pool.endpoint_for_test().id();
        let original_url = pool.bridge_url(remote.clone()).await.unwrap();
        let key = serde_json::to_string(&remote).unwrap();
        let original_bridge = pool
            .inner
            .state
            .lock()
            .unwrap()
            .bridges
            .get(&key)
            .unwrap()
            .clone();
        gate.add_permits(2);
        for attempt in 0..2 {
            if attempt == 1 {
                for _ in 0..3 {
                    tokio::time::timeout(Duration::from_secs(1), pool.network_change())
                        .await
                        .expect("网络通知不应阻塞恢复");
                }
                // netwatch 有 250ms 合并窗口；等它处理通知后再验证原桥仍可建立新连接。
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
            let http = crate::mobile::pinning::pinned_client(&fingerprint).unwrap();
            let response = http
                .get(&original_url)
                .timeout(Duration::from_secs(5))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), reqwest::StatusCode::OK);
            let body = response.text().await.unwrap();
            assert!(body.contains("event: run") && body.contains("event: done"));
            assert_eq!(pool.endpoint_for_test().id(), original_id);
            assert_eq!(pool.bridge_url(remote.clone()).await.unwrap(), original_url);
            let current_bridge = pool
                .inner
                .state
                .lock()
                .unwrap()
                .bridges
                .get(&key)
                .unwrap()
                .clone();
            assert!(Arc::ptr_eq(&original_bridge, &current_bridge));
        }
        pool.endpoint_for_test().close().await;
        server.close().await;
    }

    #[tokio::test]
    async fn cached_bridge_rebuilds_a_finished_listener_before_the_first_request() {
        let (target, fingerprint, gate) = spawn_tls_sse_server().await;
        let server = start_server(target, SecretKey::generate(), RelayMode::Disabled)
            .await
            .unwrap();
        let remote = loopback_endpoint(server.node_id(), server.local_port().unwrap());
        let pool = TunnelBridgePool::start(SecretKey::generate(), RelayMode::Disabled)
            .await
            .unwrap();
        let original_url = pool.bridge_url(remote.clone()).await.unwrap();
        let key = serde_json::to_string(&remote).unwrap();
        let original = pool
            .inner
            .state
            .lock()
            .unwrap()
            .bridges
            .get(&key)
            .unwrap()
            .clone();
        original.accept_task.abort();
        tokio::time::timeout(Duration::from_secs(1), async {
            while !original.accept_task.is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("旧桥监听器必须已停止");
        // 占住旧端口，保证错误复用死桥 URL 时不会碰巧命中新监听器。
        let old_address: SocketAddr = original_url.trim_start_matches("https://").parse().unwrap();
        let _old_port = TcpListener::bind(old_address).await.unwrap();
        let recovered_url = pool.bridge_url(remote.clone()).await.unwrap();
        assert_ne!(original_url, recovered_url);
        assert_eq!(pool.bridge_url(remote).await.unwrap(), recovered_url);
        let current = pool
            .inner
            .state
            .lock()
            .unwrap()
            .bridges
            .get(&key)
            .unwrap()
            .clone();
        assert!(!Arc::ptr_eq(&original, &current));
        assert!(!current.accept_task.is_finished());

        gate.add_permits(1);
        let http = crate::mobile::pinning::pinned_client(&fingerprint).unwrap();
        let response = http
            .get(recovered_url)
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let body = response.text().await.unwrap();
        assert!(body.contains("event: run") && body.contains("event: done"));
        pool.endpoint_for_test().close().await;
        server.close().await;
    }

    #[tokio::test]
    async fn shared_recovery_survives_caller_cancellation_and_retires_existing_tls_bridge() {
        let (target, fingerprint, gate) = spawn_tls_sse_server_with_connections(2).await;
        let server = start_server(target, SecretKey::generate(), RelayMode::Disabled)
            .await
            .unwrap();
        let remote = loopback_endpoint(server.node_id(), server.local_port().unwrap());
        let pool = Arc::new(
            TunnelBridgePool::start(SecretKey::generate(), RelayMode::Disabled)
                .await
                .unwrap(),
        );
        let old = pool.endpoint_for_test();
        let (url, generation) = pool
            .bridge_url_with_generation(remote.clone())
            .await
            .unwrap();
        let key = serde_json::to_string(&remote).unwrap();
        let bridge = pool.inner.state.lock().unwrap().bridges[&key].clone();
        let http = crate::mobile::pinning::pinned_client(&fingerprint).unwrap();
        let response = http
            .get(&url)
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .unwrap();
        let mut stream = response.bytes_stream();
        assert!(stream
            .next()
            .await
            .unwrap()
            .unwrap()
            .starts_with(b"event: run"));
        pool.network_change().await;
        assert_eq!(pool.wait_ready().await.unwrap(), generation);
        gate.add_permits(1);
        while let Some(chunk) = stream.next().await {
            chunk.unwrap();
        }

        let mut caller = Box::pin(async {
            assert!(pool.recover_if_current(generation));
            pool.wait_ready().await
        });
        assert!(futures_util::poll!(caller.as_mut()).is_pending());
        // 超时会丢弃调用者 future；必须与后台 close/build 的资源所有权分离。
        drop(caller);
        let mut waiters = tokio::task::JoinSet::new();
        for _ in 0..8 {
            let pool = pool.clone();
            waiters.spawn(async move {
                pool.recover_if_current(generation);
                pool.wait_ready().await.unwrap()
            });
        }
        tokio::time::timeout(Duration::from_secs(10), async {
            while let Some(result) = waiters.join_next().await {
                assert_eq!(result.unwrap(), generation + 1);
            }
        })
        .await
        .unwrap();
        assert_eq!(pool.endpoint_for_test().id(), old.id());
        tokio::time::timeout(Duration::ZERO, old.closed())
            .await
            .expect("新 Endpoint 发布前必须完整关闭旧实例");
        assert!(bridge.accept_task.is_finished());
        assert!(*bridge.finished.borrow());
        assert!(
            !pool.recover_if_current(generation + 1),
            "冷却内不能重建第二次"
        );
        pool.inner.state.lock().unwrap().last_recovery = None;
        assert!(
            !pool.recover_if_current(generation),
            "旧实例的失败不能回收新实例"
        );

        drop(stream);
        gate.add_permits(2);
        let recovered = pool.bridge_url(remote).await.unwrap();
        let body = crate::mobile::pinning::pinned_client(&fingerprint)
            .unwrap()
            .get(recovered)
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(body.contains("event: run") && body.contains("event: done"));
        pool.endpoint_for_test().close().await;
        server.close().await;
    }

    #[tokio::test]
    async fn preserves_pinned_tls_http_and_incremental_sse() {
        let (target, fingerprint, gate) = spawn_tls_sse_server().await;
        let server_secret = SecretKey::generate();
        let server = start_server(target, server_secret, RelayMode::Disabled)
            .await
            .unwrap();
        let remote = loopback_endpoint(server.node_id(), server.local_port().unwrap());
        let client = TunnelClient::start(SecretKey::generate(), RelayMode::Disabled)
            .await
            .unwrap();
        let bridge = client.open_bridge(remote).await.unwrap();
        let http = crate::mobile::pinning::pinned_client(&fingerprint).unwrap();
        let response = http.get(bridge.url()).send().await.unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let mut stream = response.bytes_stream();
        let first = tokio::time::timeout(Duration::from_secs(10), stream.next())
            .await
            .expect("第一帧不应被缓冲")
            .expect("SSE 流不应提前结束")
            .unwrap();
        let first = String::from_utf8(first.to_vec()).unwrap();
        assert!(first.contains("event: run"));
        assert!(!first.contains("event: done"));

        gate.add_permits(1);
        let mut rest = BytesMut::new();
        while let Some(chunk) = stream.next().await {
            rest.extend_from_slice(&chunk.unwrap());
        }
        assert!(String::from_utf8(rest.to_vec())
            .unwrap()
            .contains("event: done"));

        drop(bridge);
        client.close().await;
        server.close().await;
    }

    #[tokio::test]
    #[ignore = "需要访问公网 Iroh relay；普通 CI 不执行"]
    async fn relays_without_direct_addresses() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter("iroh=debug,iroh_relay=debug")
            .try_init();
        let (target, fingerprint, _gate) = spawn_tls_sse_server().await;
        let relay_mode = production_relay_mode();
        if let RelayMode::Custom(relay_map) = &relay_mode {
            let urls = relay_map.urls::<Vec<RelayUrl>>();
            println!("应用 relay map: {urls:?}");
            assert!(urls.iter().all(|url| !url.as_str().contains("iroh.link./")));
        }
        let server = start_server_relay_only(target, SecretKey::generate(), relay_mode)
            .await
            .unwrap();
        let published = server.endpoint(Duration::from_secs(30)).await;
        assert!(
            published.relay_url.is_some(),
            "桌面端没有拿到公网 relay 地址：{published:?}"
        );
        let remote = TunnelEndpoint {
            node_id: published.node_id,
            relay_url: published.relay_url,
            // 刻意移除所有局域网/直连候选，模拟手机只使用蜂窝网络。
            direct_addresses: Vec::new(),
        };
        let client = TunnelClient::start_relay_only(SecretKey::generate(), production_relay_mode())
            .await
            .unwrap();
        let bridge = client.open_bridge(remote).await.unwrap();
        let http = crate::mobile::pinning::pinned_client(&fingerprint).unwrap();
        let response = tokio::time::timeout(Duration::from_secs(30), http.get(bridge.url()).send())
            .await
            .expect("公网 relay 建连超时")
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        drop(bridge);
        client.close().await;
        server.close().await;
    }

    #[tokio::test]
    #[ignore = "需要公网签名地址发现与 relay；验证陈旧配对地址恢复"]
    async fn discovers_current_relay_when_paired_address_is_stale() {
        let (target, fingerprint, gate) = spawn_tls_sse_server().await;
        let server =
            start_server_relay_only(target, SecretKey::generate(), production_relay_mode())
                .await
                .unwrap();
        let published = server.endpoint(Duration::from_secs(30)).await;
        let current_relay = published.relay_url.as_ref().expect("必须连接公网 relay");
        let current_host = RelayUrl::from_str(current_relay).unwrap();
        let stale_relay = PRODUCTION_RELAY_HOSTS
            .iter()
            .map(|host| format!("https://{host}/"))
            .find(|url| RelayUrl::from_str(url).unwrap() != current_host)
            .unwrap();
        let client = TunnelClient::start_relay_only(SecretKey::generate(), production_relay_mode())
            .await
            .unwrap();
        // 等待新的签名记录真正可查询，排除首次发布传播延迟对恢复断言的干扰。
        tokio::time::timeout(Duration::from_secs(40), async {
            loop {
                let mut lookup = Box::pin(
                    client
                        .endpoint
                        .address_lookup()
                        .unwrap()
                        .resolve(server.endpoint.id()),
                );
                while let Some(result) = lookup.next().await {
                    if matches!(result, Ok(Ok(_))) {
                        return;
                    }
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        })
        .await
        .expect("桌面必须发布可按 NodeId 验证的当前 relay 记录");
        let bridge = client
            .open_bridge(TunnelEndpoint {
                node_id: published.node_id,
                relay_url: Some(stale_relay),
                direct_addresses: Vec::new(),
            })
            .await
            .unwrap();
        let http = crate::mobile::pinning::pinned_client(&fingerprint).unwrap();
        let response = tokio::time::timeout(Duration::from_secs(25), http.get(bridge.url()).send())
            .await
            .expect("陈旧地址不能阻止发现当前 relay")
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        gate.add_permits(1);
        let body = tokio::time::timeout(Duration::from_secs(10), response.text())
            .await
            .unwrap()
            .unwrap();
        assert!(body.contains("event: run") && body.contains("event: done"));
        drop(bridge);
        client.close().await;
        server.close().await;
    }

    #[tokio::test]
    #[ignore = "由移动端 UI 验收脚本显式启动，停止文件控制生命周期"]
    async fn serves_relay_only_mobile_ui_fixture() {
        let target: SocketAddr = std::env::var("PISPER_TEST_TUNNEL_TARGET")
            .expect("必须提供隔离测试 Runtime 的回环地址")
            .parse()
            .unwrap();
        assert!(target.ip().is_loopback(), "测试隧道不能转发到外部服务");
        let output = std::path::PathBuf::from(
            std::env::var("PISPER_TEST_TUNNEL_OUTPUT").expect("必须提供测试元数据路径"),
        );
        let stop = output.with_extension("stop");
        assert!(!stop.exists(), "旧停止文件必须由验收脚本先处理");
        ensure_crypto_provider();
        let server =
            start_server_relay_only(target, SecretKey::generate(), production_relay_mode())
                .await
                .unwrap();
        let published = server.endpoint(Duration::from_secs(30)).await;
        assert!(published.relay_url.is_some(), "未连接到公网 relay");
        assert!(
            published.direct_addresses.is_empty(),
            "必须禁用所有 IP 传输"
        );
        fs::write(&output, serde_json::to_vec(&published).unwrap()).unwrap();
        println!("PISPER_RELAY_FIXTURE_READY");
        // 常驻时间有上限，避免验收中断后遗留公网测试入口。
        let deadline = tokio::time::Instant::now() + Duration::from_secs(1800);
        while !stop.exists() && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        server.close().await;
    }

    #[test]
    fn persists_secret_key() {
        let path = std::env::temp_dir().join(format!(
            "pisper-iroh-secret-{}-{}.key",
            std::process::id(),
            rand::random::<u64>()
        ));
        let first = load_or_create_secret(&path).unwrap();
        let second = load_or_create_secret(&path).unwrap();
        assert_eq!(first.to_bytes(), second.to_bytes());
        assert_eq!(fs::read(&path).unwrap().len(), 32);
        let _ = fs::remove_file(path);
    }
}
