//! 配对流程：用二维码 payload（或手动输入）里的配对码向桌面端换取长期设备令牌。
//! 配对请求本身就走指纹锁定的 HTTPS——指纹来自带外渠道（二维码/人工核对），
//! 因此配对链路自始抵御中间人。
use serde::{Deserialize, Serialize};

use crate::iroh_tunnel::TunnelBridgePool;

use super::pinning::pinned_client;
use super::store::{normalize_fingerprint, ServerEndpoint, ServerProfile};

/// 二维码 payload（契约见 docs/mobile.md §5.2）。
#[derive(Debug, Deserialize)]
pub struct QrPayload {
    pub v: u32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub endpoints: Vec<ServerEndpoint>,
    pub fp: String,
    pub code: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairOutcome {
    pub server_id: String,
    pub server_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PairResponse {
    device_id: String,
    token: String,
    server_name: String,
    #[serde(default)]
    endpoints: Vec<ServerEndpoint>,
}

#[derive(Debug, Deserialize)]
struct PairError {
    error: Option<String>,
}

fn now_iso8601() -> String {
    // 避免引入时间库：秒级时间戳足够标识配对时间。
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    format!("{seconds}")
}

async fn endpoint_url(
    endpoint: &ServerEndpoint,
    tunnels: Option<&TunnelBridgePool>,
) -> Result<String, String> {
    if endpoint.kind == "iroh" {
        let tunnels = tunnels.ok_or_else(|| "Iroh 桥接尚未启动。".to_string())?;
        return tunnels.bridge_url(endpoint.tunnel_endpoint()?).await;
    }
    if !endpoint.url.starts_with("https://") {
        return Err("远程端点必须使用 HTTPS。".into());
    }
    Ok(endpoint.url.trim_end_matches('/').to_string())
}

/// 依次尝试 payload 里的 endpoint：第一个指纹匹配且配对码被接受的成功。
pub async fn pair(
    payload: &QrPayload,
    device_name: &str,
    tunnels: Option<&TunnelBridgePool>,
) -> Result<ServerProfile, String> {
    if payload.v != 1 {
        return Err(format!("不支持的二维码版本 v{}，请升级 App。", payload.v));
    }
    let fingerprint = normalize_fingerprint(&payload.fp);
    if fingerprint.len() < 16 {
        return Err("二维码中的证书指纹不完整。".into());
    }
    if payload.endpoints.is_empty() {
        return Err("二维码中没有可连接的地址。".into());
    }

    let client = pinned_client(&fingerprint)?;
    let mut last_error = String::new();
    for endpoint in &payload.endpoints {
        let base_url = match endpoint_url(endpoint, tunnels).await {
            Ok(url) => url,
            Err(error) => {
                last_error = format!("{}: {error}", endpoint.display_address());
                continue;
            }
        };
        match try_pair_endpoint(&client, &base_url, &payload.code, device_name).await {
            Ok(response) => {
                let endpoints = if response.endpoints.is_empty() {
                    payload.endpoints.clone()
                } else {
                    response.endpoints.clone()
                };
                return Ok(ServerProfile {
                    id: format!("srv_{}", fast_id()),
                    name: if response.server_name.is_empty() {
                        payload.name.clone()
                    } else {
                        response.server_name
                    },
                    endpoints,
                    fingerprint,
                    device_id: response.device_id,
                    token: response.token,
                    paired_at: now_iso8601(),
                });
            }
            Err(error) => last_error = format!("{}: {error}", endpoint.display_address()),
        }
    }
    Err(if last_error.is_empty() {
        "无法连接到桌面端。".into()
    } else {
        last_error
    })
}

async fn try_pair_endpoint(
    client: &reqwest::Client,
    base_url: &str,
    code: &str,
    device_name: &str,
) -> Result<PairResponse, String> {
    let response = client
        .post(format!(
            "{}/api/remote/pair",
            base_url.trim_end_matches('/')
        ))
        .header("Content-Type", "application/json")
        .body(
            serde_json::to_string(&serde_json::json!({
                "code": code,
                "deviceName": device_name,
            }))
            .map_err(|error| error.to_string())?,
        )
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let status = response.status();
    let body = response.text().await.map_err(|error| error.to_string())?;
    if !status.is_success() {
        let parsed = serde_json::from_str::<PairError>(&body).ok();
        return Err(parsed
            .and_then(|parsed| parsed.error)
            .unwrap_or_else(|| format!("HTTP {status}")));
    }
    let parsed: PairResponse = serde_json::from_str(&body).map_err(|error| error.to_string())?;
    Ok(parsed)
}

fn fast_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("{nanos:x}")
}

#[cfg(test)]
mod tests {
    //! 配对链路集成测试：自建自签 TLS 上游模拟桌面端 /api/remote/pair，
    //! 验证指纹锁定的客户端能完成配对并拿到设备令牌。
    use super::*;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn sha256_hex(bytes: &[u8]) -> String {
        use sha2::Digest;
        sha2::Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect()
    }

    // rustls 的 with_single_cert 会走进程级默认 provider 加载私钥；
    // reqwest 同时拉入了 ring 与 aws-lc-rs，必须显式安装一个。
    fn ensure_crypto_provider() {
        static ONCE: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        ONCE.get_or_init(|| {
            let _ = rustls::crypto::ring::default_provider().install_default();
        });
    }

    async fn spawn_pair_upstream() -> (String, String) {
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
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
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
                    let mut head = Vec::new();
                    let mut buf = [0u8; 4096];
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
                    // 头部之后的字节已经是请求体的一部分，不能丢。
                    let header_end = head.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
                    let text = String::from_utf8_lossy(&head);
                    let content_length: usize = text
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length: "))
                        .and_then(|v| v.trim().parse().ok())
                        .unwrap_or(0);
                    let mut body = head.split_off(header_end);
                    while body.len() < content_length {
                        let n = stream.read(&mut buf).await.unwrap();
                        if n == 0 {
                            return;
                        }
                        body.extend_from_slice(&buf[..n]);
                    }
                    let payload = "{\"deviceId\":\"dev_test\",\"token\":\"pst_test\",\"serverName\":\"测试桌面\",\"apiVersion\":1}".as_bytes();
                    stream
                        .write_all(
                            format!(
                                "HTTP/1.1 201 Created\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n",
                                payload.len()
                            )
                            .as_bytes(),
                        )
                        .await
                        .unwrap();
                    stream.write_all(payload).await.unwrap();
                });
            }
        });
        (format!("https://{addr}"), fingerprint)
    }

    #[tokio::test]
    async fn pair_roundtrip_against_tls_upstream() {
        let (url, fingerprint) = spawn_pair_upstream().await;
        let payload = QrPayload {
            v: 1,
            name: "测试".into(),
            endpoints: vec![ServerEndpoint::lan(url)],
            fp: format!("SHA256:{fingerprint}"),
            code: "ABCD-EFGH".into(),
        };
        let profile = pair(&payload, "测试手机", None).await.expect("配对应成功");
        assert_eq!(profile.device_id, "dev_test");
        assert_eq!(profile.token, "pst_test");
        assert_eq!(profile.name, "测试桌面");
    }
}
