//! 配对流程：用二维码 payload（或手动输入）里的配对码向桌面端换取长期设备令牌。
//! 配对请求本身就走指纹锁定的 HTTPS——指纹来自带外渠道（二维码/人工核对），
//! 因此配对链路自始抵御中间人。
use std::sync::atomic::{AtomicBool, Ordering};

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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApprovalRequestResponse {
    request_id: String,
    request_secret: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApprovalStatusResponse {
    status: String,
    device_id: Option<String>,
    token: Option<String>,
    #[serde(default)]
    server_name: String,
    #[serde(default)]
    endpoints: Vec<ServerEndpoint>,
}

impl ApprovalStatusResponse {
    fn into_pair_response(self) -> Result<PairResponse, String> {
        Ok(PairResponse {
            device_id: self
                .device_id
                .ok_or_else(|| "桌面端批准结果缺少设备标识。".to_string())?,
            token: self
                .token
                .ok_or_else(|| "桌面端批准结果缺少设备令牌。".to_string())?,
            server_name: self.server_name,
            endpoints: self.endpoints,
        })
    }
}

fn now_iso8601() -> String {
    // 避免引入时间库：秒级时间戳足够标识配对时间。
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    format!("{seconds}")
}

fn profile_from_response(
    response: PairResponse,
    fallback_name: &str,
    fallback_endpoints: &[ServerEndpoint],
    fingerprint: String,
) -> ServerProfile {
    let endpoints = if response.endpoints.is_empty() {
        fallback_endpoints.to_vec()
    } else {
        response.endpoints
    };
    ServerProfile {
        id: format!("srv_{}", fast_id()),
        name: if response.server_name.is_empty() {
            fallback_name.to_string()
        } else {
            response.server_name
        },
        endpoints,
        fingerprint,
        device_id: response.device_id,
        token: response.token,
        paired_at: now_iso8601(),
    }
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
                return Ok(profile_from_response(
                    response,
                    &payload.name,
                    &payload.endpoints,
                    fingerprint,
                ));
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

/// 通过 mDNS 发现的 LAN 端点提交连接申请，等待桌面批准后领取设备令牌。
pub async fn pair_with_approval(
    name: &str,
    url: &str,
    fingerprint: &str,
    device_name: &str,
    cancelled: &AtomicBool,
) -> Result<ServerProfile, String> {
    let fingerprint = normalize_fingerprint(fingerprint);
    if fingerprint.len() < 64 {
        return Err("发现记录中的证书指纹不完整。".into());
    }
    let url = url.trim().trim_end_matches('/');
    if !url.starts_with("https://") {
        return Err("发现记录中的远程端点不是 HTTPS。".into());
    }
    let client = pinned_client(&fingerprint)?;
    let response = client
        .post(format!("{url}/api/remote/pairing-requests"))
        .timeout(std::time::Duration::from_secs(10))
        .json(&serde_json::json!({ "deviceName": device_name }))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let status = response.status();
    let body = response.text().await.map_err(|error| error.to_string())?;
    if status != reqwest::StatusCode::ACCEPTED {
        return Err(pair_error_message(status, &body));
    }
    let request: ApprovalRequestResponse =
        serde_json::from_str(&body).map_err(|error| error.to_string())?;
    let fallback_endpoints = vec![ServerEndpoint::lan(url.to_string())];
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(130);

    loop {
        if cancelled.load(Ordering::Acquire) {
            cancel_approval_request(&client, url, &request).await;
            return Err("连接申请已取消。".into());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("连接申请已过期，请重新申请。".into());
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let response = client
            .get(format!(
                "{url}/api/remote/pairing-requests/{}",
                request.request_id
            ))
            .timeout(std::time::Duration::from_secs(10))
            .header("x-pisper-pairing-secret", &request.request_secret)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        let status = response.status();
        let body = response.text().await.map_err(|error| error.to_string())?;
        if !status.is_success() {
            return Err(pair_error_message(status, &body));
        }
        let result: ApprovalStatusResponse =
            serde_json::from_str(&body).map_err(|error| error.to_string())?;
        if cancelled.load(Ordering::Acquire) {
            cancel_approval_request(&client, url, &request).await;
            return Err("连接申请已取消。".into());
        }
        let approval_status = result.status.clone();
        match approval_status.as_str() {
            "pending" => continue,
            "rejected" => return Err("桌面端已拒绝连接申请。".into()),
            "expired" => return Err("连接申请已过期，请重新申请。".into()),
            "approved" => {
                let response = result.into_pair_response()?;
                return Ok(profile_from_response(
                    response,
                    name,
                    &fallback_endpoints,
                    fingerprint,
                ));
            }
            _ => return Err("桌面端返回了未知的连接申请状态。".into()),
        }
    }
}

pub async fn revoke_pairing_result(
    base_url: &str,
    fingerprint: &str,
    profile: &ServerProfile,
) -> Result<(), String> {
    let client = pinned_client(fingerprint)?;
    let response = client
        .post(format!(
            "{}/api/remote/devices/{}/revoke",
            base_url.trim_end_matches('/'),
            profile.device_id
        ))
        .timeout(std::time::Duration::from_secs(10))
        .bearer_auth(&profile.token)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!("HTTP {}", response.status()))
    }
}

async fn cancel_approval_request(
    client: &reqwest::Client,
    base_url: &str,
    request: &ApprovalRequestResponse,
) {
    let _ = client
        .delete(format!(
            "{base_url}/api/remote/pairing-requests/{}",
            request.request_id
        ))
        .timeout(std::time::Duration::from_secs(10))
        .header("x-pisper-pairing-secret", &request.request_secret)
        .send()
        .await;
}

fn pair_error_message(status: reqwest::StatusCode, body: &str) -> String {
    serde_json::from_str::<PairError>(body)
        .ok()
        .and_then(|parsed| parsed.error)
        .unwrap_or_else(|| format!("HTTP {status}"))
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
        return Err(pair_error_message(status, &body));
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

    #[test]
    fn approval_status_contract_accepts_pending_and_requires_approved_credentials() {
        let pending: ApprovalStatusResponse =
            serde_json::from_str(r#"{"status":"pending","expiresAt":"later"}"#).unwrap();
        assert_eq!(pending.status, "pending");

        let approved: ApprovalStatusResponse = serde_json::from_str(
            r#"{"status":"approved","deviceId":"dev_test","token":"pst_test","serverName":"测试桌面","endpoints":[]}"#,
        )
        .unwrap();
        let pair = approved.into_pair_response().unwrap();
        assert_eq!(pair.device_id, "dev_test");
        assert_eq!(pair.token, "pst_test");

        let incomplete: ApprovalStatusResponse =
            serde_json::from_str(r#"{"status":"approved"}"#).unwrap();
        assert!(incomplete.into_pair_response().is_err());
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
