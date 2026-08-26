//! Iroh 透明字节隧道：QUIC 仅承载 TCP 字节，不终止或改写上层 TLS、HTTP 与 SSE。
use std::{
    collections::HashMap,
    fs, io,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::Path,
    str::FromStr,
    sync::Arc,
    time::Duration,
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

pub struct TunnelClient {
    endpoint: Endpoint,
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
        let mut builder = Endpoint::builder(iroh::endpoint::presets::Minimal)
            .secret_key(secret_key)
            .relay_mode(relay_mode);
        if !enable_ip_transports {
            builder = builder.clear_ip_transports();
        }
        let endpoint = builder
            .bind()
            .await
            .map_err(|error| format!("Iroh 客户端启动失败：{error}"))?;
        Ok(Self { endpoint })
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
        let accept_task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let endpoint = endpoint.clone();
                let remote = remote.clone();
                tokio::spawn(async move {
                    let Ok(connection) = endpoint.connect(remote, PISPER_TUNNEL_ALPN).await else {
                        return;
                    };
                    let Ok((send, recv)) = connection.open_bi().await else {
                        return;
                    };
                    let _ = copy_tcp_and_quic(stream, send, recv).await;
                });
            }
        });
        Ok(TunnelBridge { port, accept_task })
    }

    pub async fn close(self) {
        self.endpoint.close().await;
    }
}

pub struct TunnelBridgePool {
    client: TunnelClient,
    bridges: tokio::sync::Mutex<HashMap<String, Arc<TunnelBridge>>>,
}

impl TunnelBridgePool {
    pub async fn start(secret_key: SecretKey, relay_mode: RelayMode) -> Result<Self, String> {
        Ok(Self {
            client: TunnelClient::start(secret_key, relay_mode).await?,
            bridges: tokio::sync::Mutex::new(HashMap::new()),
        })
    }

    pub async fn bridge_url(&self, remote: TunnelEndpoint) -> Result<String, String> {
        let key = serde_json::to_string(&remote).map_err(|error| error.to_string())?;
        let mut bridges = self.bridges.lock().await;
        if let Some(bridge) = bridges.get(&key) {
            return Ok(bridge.url());
        }
        let bridge = Arc::new(self.client.open_bridge(remote).await?);
        let url = bridge.url();
        bridges.insert(key, bridge);
        Ok(url)
    }

    pub async fn invalidate(&self, remote: &TunnelEndpoint) {
        if let Ok(key) = serde_json::to_string(remote) {
            self.bridges.lock().await.remove(&key);
        }
    }
}

pub struct TunnelBridge {
    pub port: u16,
    accept_task: JoinHandle<()>,
}

impl TunnelBridge {
    pub fn url(&self) -> String {
        format!("https://127.0.0.1:{}", self.port)
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
    let mut builder = Endpoint::builder(iroh::endpoint::presets::Minimal)
        .secret_key(secret_key)
        .alpns(vec![PISPER_TUNNEL_ALPN.to_vec()])
        .relay_mode(relay_mode);
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
        });
        (address, fingerprint, gate)
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
