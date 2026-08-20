//! 配对流程：用二维码 payload（或手动输入）里的配对码向桌面端换取长期设备令牌。
//! 配对请求本身就走指纹锁定的 HTTPS——指纹来自带外渠道（二维码/人工核对），
//! 因此配对链路自始抵御中间人。
use serde::{Deserialize, Serialize};

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

/// 依次尝试 payload 里的 endpoint：第一个指纹匹配且配对码被接受的成功。
pub async fn pair(payload: &QrPayload, device_name: &str) -> Result<ServerProfile, String> {
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
        match try_pair_endpoint(&client, &endpoint.url, &payload.code, device_name).await {
            Ok(response) => {
                return Ok(ServerProfile {
                    id: format!("srv_{}", fast_id()),
                    name: if response.server_name.is_empty() {
                        payload.name.clone()
                    } else {
                        response.server_name
                    },
                    endpoints: payload.endpoints.clone(),
                    fingerprint,
                    device_id: response.device_id,
                    token: response.token,
                    paired_at: now_iso8601(),
                });
            }
            Err(error) => last_error = format!("{}: {}", endpoint.url, error),
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
