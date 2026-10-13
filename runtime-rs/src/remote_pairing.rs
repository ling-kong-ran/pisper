//! 一次性二维码兑换；文件格式仍为原 remote-access.json，只保存设备令牌哈希。
use crate::{remote_access, ApiError, AppState};
use axum::{
    extract::{ConnectInfo, State},
    http::StatusCode,
    Json,
};
use base64::Engine;
use serde_json::{json, Value};
use std::{net::SocketAddr, sync::Arc};
use subtle::ConstantTimeEq;

pub(crate) fn device_name(body: &Value) -> Result<String, ApiError> {
    let name = match body.get("deviceName") {
        None => "device",
        Some(value) => value.as_str().ok_or_else(|| {
            ApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid_device_name",
                "设备名称无效。",
            )
        })?,
    }
    .trim();
    if name.is_empty() || name.len() > 256 || name.chars().any(char::is_control) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_device_name",
            "设备名称无效。",
        ));
    }
    Ok(name.to_string())
}

fn normalize_code(code: &str) -> String {
    code.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

fn verify_code(pending: &Option<(String, u64)>, hash: &str, now: u64) -> Result<(), ApiError> {
    match pending {
        Some((_, expires)) if *expires <= now => Err(ApiError::new(
            StatusCode::GONE,
            "pairing_code_expired",
            "配对码已过期。",
        )),
        Some((expected, _)) if bool::from(expected.as_bytes().ct_eq(hash.as_bytes())) => Ok(()),
        _ => Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "pairing_code_invalid",
            "配对码无效。",
        )),
    }
}

pub(crate) async fn create_code(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, ApiError> {
    let _configuration = state.remote.configure().await;
    let info = state.remote.refresh_endpoints().await;
    if info["listening"] != true {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "remote_disabled",
            "请先开启远程访问。",
        ));
    }
    if !info["endpoints"]
        .as_array()
        .is_some_and(|items| items.iter().any(|item| item["t"] == "iroh"))
    {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "remote_endpoint_unavailable",
            "P2P 地址正在准备，请稍后重试。",
        ));
    }
    let mut bytes = [0u8; 8];
    getrandom::getrandom(&mut bytes).map_err(|_| ApiError::internal("安全随机数不可用。"))?;
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let raw: String = bytes
        .iter()
        .map(|b| ALPHABET[(*b & 31) as usize] as char)
        .collect();
    let code = format!("{}-{}", &raw[..4], &raw[4..]);
    let expires = crate::product::now_ms() + 300_000;
    let payload = json!({"v":1,"name":"Pisper","endpoints":info["endpoints"],"fp":info["fingerprint"],"code":code});
    let qr = qrcode::QrCode::new(payload.to_string().as_bytes())
        .map_err(|_| ApiError::internal("无法生成配对二维码。"))?;
    let side = qr.width();
    let scale = (400 / (side + 8)).max(1);
    let image = image::GrayImage::from_fn(
        ((side + 8) * scale) as u32,
        ((side + 8) * scale) as u32,
        |x, y| {
            let (x, y) = (x as usize / scale, y as usize / scale);
            let black = x >= 4
                && y >= 4
                && x < side + 4
                && y < side + 4
                && qr[(x - 4, y - 4)] == qrcode::Color::Dark;
            image::Luma([if black { 0 } else { 255 }])
        },
    );
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageLuma8(image)
        .write_to(&mut png, image::ImageFormat::Png)
        .map_err(|_| ApiError::internal("无法渲染配对二维码。"))?;
    // 只有成功生成的二维码才替换未使用配对码，渲染失败不使用户手中的凭据失效。
    let hash = remote_access::hash(&raw);
    crate::security::save_pairing_code(&state.data_dir, &hash, expires)
        .map_err(ApiError::internal)?;
    *state.pairing.pending.lock().expect("pairing code") = Some((hash, expires));
    Ok(Json(
        json!({"code":code,"expiresAt":crate::session_ops::iso_timestamp(expires),
        "qrDataUrl":format!("data:image/png;base64,{}",base64::engine::general_purpose::STANDARD.encode(png.into_inner()))}),
    ))
}

pub(crate) async fn pair(
    State(state): State<Arc<AppState>>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let _configuration = state.remote.configure().await;
    if !state
        .remote_enabled
        .load(std::sync::atomic::Ordering::Acquire)
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "remote_disabled",
            "远程访问已关闭。",
        ));
    }
    let name = device_name(&body)?;
    let code = body
        .get("code")
        .and_then(Value::as_str)
        .filter(|value| value.len() <= 32)
        .ok_or_else(|| ApiError::bad_request("配对码格式无效。"))?;
    if !state.remote.pairing_attempt(&address.ip().to_string()) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "pairing_rate_limited",
            "配对尝试过多，请一分钟后重试。",
        ));
    }
    let hash = remote_access::hash(&normalize_code(code));
    let now = crate::product::now_ms();
    let mut pending = state.pairing.pending.lock().expect("pairing code");
    verify_code(&pending, &hash, now)?;
    let token = format!(
        "pst_{}",
        remote_access::random_secret().map_err(ApiError::internal)?
    );
    let device = crate::security::PairedDevice::new(crate::product::new_id(), name, &token, now);
    let mut devices = state.pairing.devices.lock().expect("paired devices");
    let mut updated = devices.clone();
    updated.push(device.clone());
    crate::security::redeem_pairing_code(&state.data_dir, &updated).map_err(ApiError::internal)?;
    *devices = updated;
    *pending = None;
    state.remote.paired(&address.ip().to_string());
    Ok((
        StatusCode::CREATED,
        Json(
            json!({"deviceId":device.id,"token":token,"name":device.name,
        "serverName":"Pisper","endpoints":state.remote.info()["endpoints"],"apiVersion":1}),
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_rejects_expired_consumed_and_incorrect_codes() {
        let hash = remote_access::hash(&normalize_code("abcd-1234"));
        let pending = Some((hash.clone(), 100));
        assert!(verify_code(&pending, &remote_access::hash("ABCD1234"), 99).is_ok());
        assert!(verify_code(&pending, &hash, 100).is_err());
        assert!(verify_code(&pending, &remote_access::hash("WRONG123"), 99).is_err());
        assert!(verify_code(&None, &hash, 99).is_err());
    }
}
