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

use iroh::{Endpoint, NodeAddr, NodeId, RelayMode, RelayUrl, SecretKey, Watcher};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{copy, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};

pub const PISPER_TUNNEL_ALPN: &[u8] = b"pisper/remote-tcp/1";

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
    fn from_node_addr(address: NodeAddr) -> Self {
        Self {
            node_id: address.node_id.to_string(),
            relay_url: address.relay_url.map(|value| value.to_string()),
            direct_addresses: address
                .direct_addresses
                .into_iter()
                .map(|value| value.to_string())
                .collect(),
        }
    }

    fn to_node_addr(&self) -> Result<NodeAddr, String> {
        let node_id = NodeId::from_str(&self.node_id)
            .map_err(|error| format!("Iroh 节点 ID 无效：{error}"))?;
        let relay_url = self
            .relay_url
            .as_deref()
            .map(RelayUrl::from_str)
            .transpose()
            .map_err(|error| format!("Iroh relay 地址无效：{error}"))?;
        let direct_addresses = self
            .direct_addresses
            .iter()
            .map(|value| {
                value
                    .parse::<SocketAddr>()
                    .map_err(|error| format!("Iroh 直连地址无效：{error}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(NodeAddr::from_parts(node_id, relay_url, direct_addresses))
    }
}

pub struct TunnelServer {
    endpoint: Endpoint,
    accept_task: JoinHandle<()>,
}

impl TunnelServer {
    pub fn node_id(&self) -> String {
        self.endpoint.node_id().to_string()
    }

    pub fn local_port(&self) -> Option<u16> {
        self.endpoint.bound_sockets().first().map(SocketAddr::port)
    }

    pub async fn endpoint(&self, timeout: Duration) -> TunnelEndpoint {
        let direct_addresses = tokio::time::timeout(
            timeout.min(Duration::from_secs(2)),
            self.endpoint.direct_addresses().initialized(),
        )
        .await
        .map(|addresses| {
            addresses
                .into_iter()
                .map(|address| address.addr)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
        let relay_url = tokio::time::timeout(timeout, self.endpoint.home_relay().initialized())
            .await
            .ok();
        TunnelEndpoint::from_node_addr(NodeAddr::from_parts(
            self.endpoint.node_id(),
            relay_url,
            direct_addresses,
        ))
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
        let endpoint = Endpoint::builder()
            .secret_key(secret_key)
            .relay_mode(relay_mode)
            .bind()
            .await
            .map_err(|error| format!("Iroh 客户端启动失败：{error}"))?;
        Ok(Self { endpoint })
    }

    pub async fn open_bridge(&self, remote: TunnelEndpoint) -> Result<TunnelBridge, String> {
        let remote = remote.to_node_addr()?;
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
    let endpoint = Endpoint::builder()
        .secret_key(secret_key)
        .alpns(vec![PISPER_TUNNEL_ALPN.to_vec()])
        .relay_mode(relay_mode)
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
    let secret = SecretKey::generate(rand::rngs::OsRng);
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
        let server_secret = SecretKey::generate(rand::rngs::OsRng);
        let server = start_server(target, server_secret, RelayMode::Disabled)
            .await
            .unwrap();
        let remote = loopback_endpoint(server.node_id(), server.local_port().unwrap());
        let client =
            TunnelClient::start(SecretKey::generate(rand::rngs::OsRng), RelayMode::Disabled)
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
