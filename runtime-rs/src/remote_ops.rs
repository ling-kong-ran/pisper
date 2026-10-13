//! release `http/routes/remote.mjs` 中剩余的远程访问路由：
//! 防火墙状态/重试、配对审批流（pairing-requests）与 revoke 别名。
//! 契约对齐 release（状态码 + 机读 code + 响应字段）。
//!
//! 传输及其权限边界由 remote_access / remote_http 持有。

use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::{Extension, Json};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;
use subtle::ConstantTimeEq;

use crate::{product, security, ApiError, AppState};

type ArcAppState = Arc<AppState>;

const PAIRING_REQUEST_TTL_MS: u64 = 120_000;

async fn remote_endpoints(state: &AppState) -> Value {
    state.remote.refresh_endpoints().await["endpoints"].clone()
}
fn server_name() -> &'static str {
    "Pisper"
}
fn paired_token() -> Result<String, ApiError> {
    Ok(format!(
        "pst_{}",
        crate::remote_access::random_secret().map_err(ApiError::internal)?
    ))
}

// ------------------------------------------------------ pairing requests

/// release POST /api/remote/pairing-requests：申请方创建配对审批（202）。
pub(crate) async fn create_request(
    State(state): State<ArcAppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    remote: Option<Extension<crate::remote_http::RemoteRequest>>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
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
    let private = match addr.ip() {
        std::net::IpAddr::V4(ip) => ip.is_private() || ip.is_loopback() || ip.is_link_local(),
        std::net::IpAddr::V6(ip) => {
            ip.is_loopback() || ip.is_unique_local() || ip.is_unicast_link_local()
        }
    };
    // Iroh 转发在 TLS 端表现为回环地址，不能被当作同一局域网的审批请求。
    if !private || (remote.is_some() && addr.ip().is_loopback()) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "pairing_lan_required",
            "连接申请需要局域网地址。",
        ));
    }
    let device_name = crate::remote_pairing::device_name(&body)?;
    let ip = addr.ip().to_string();
    let now = product::now_ms();
    let secret = crate::remote_access::random_secret().map_err(ApiError::internal)?;
    let approval = security::PairingApproval {
        request_id: product::new_id(),
        secret: secret.clone(),
        device_name: device_name.clone(),
        ip,
        created_at: now,
        expires_at: now + PAIRING_REQUEST_TTL_MS,
        status: "pending".to_string(),
        device_id: None,
        token: None,
    };
    {
        let mut requests = state
            .pairing_requests
            .lock()
            .expect("pairing requests lock");
        requests.retain(|request| request.expires_at > now);
        if requests.len() >= 32 {
            return Err(ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "pairing_request_limit",
                "等待配对的设备过多。",
            ));
        }
        requests.push(approval.clone());
    }
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "requestId": approval.request_id,
            "requestSecret": approval.secret,
            "expiresAt": crate::session_ops::iso_timestamp(approval.expires_at),
            "serverName": server_name(),
            "endpoints": remote_endpoints(&state).await,
            "fingerprint": state.remote.info()["fingerprint"],
            "apiVersion": 1,
        })),
    ))
}

fn find_request(state: &AppState, request_id: &str) -> Result<security::PairingApproval, ApiError> {
    let now = product::now_ms();
    let requests = state
        .pairing_requests
        .lock()
        .expect("pairing requests lock");
    let Some(request) = requests.iter().find(|r| r.request_id == request_id) else {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "pairing_request_not_found",
            "配对申请不存在。",
        ));
    };
    if request.expires_at <= now {
        return Err(ApiError::new(
            StatusCode::GONE,
            "pairing_request_expired",
            "配对申请已过期。",
        ));
    }
    Ok(request.clone())
}

fn require_pairing_secret(
    request: &security::PairingApproval,
    headers: &HeaderMap,
) -> Result<(), ApiError> {
    let presented = headers
        .get("x-pisper-pairing-secret")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if !bool::from(presented.as_bytes().ct_eq(request.secret.as_bytes())) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "pairing_secret_mismatch",
            "配对校验失败。",
        ));
    }
    Ok(())
}

/// release GET /api/remote/pairing-requests/:requestId：
/// approved 返回 pairedResponse（含 token），其余返回状态视图。
pub(crate) async fn request_status(
    State(state): State<ArcAppState>,
    Path(request_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let request = find_request(&state, &request_id)?;
    require_pairing_secret(&request, &headers)?;
    match request.status.as_str() {
        "approved" => Ok(Json(json!({
            "deviceId": request.device_id,
            "token": request.token,
            "serverName": server_name(),
            "endpoints": remote_endpoints(&state).await,
            "apiVersion": 1,
            "status": "approved",
        }))),
        "denied" => Ok(Json(json!({"status": "denied"}))),
        _ => Ok(Json(json!({
            "status": "pending",
            "expiresAt": crate::session_ops::iso_timestamp(request.expires_at),
        }))),
    }
}

/// release DELETE /api/remote/pairing-requests/:requestId → 204。
pub(crate) async fn cancel_request(
    State(state): State<ArcAppState>,
    Path(request_id): Path<String>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let request = find_request(&state, &request_id)?;
    require_pairing_secret(&request, &headers)?;
    {
        let mut requests = state
            .pairing_requests
            .lock()
            .expect("pairing requests lock");
        requests.retain(|r| r.request_id != request_id);
    }
    Ok(StatusCode::NO_CONTENT)
}

/// release GET /api/remote/pairing-requests：桌面端审批列表（不含 secret/token）。
pub(crate) async fn list_requests(State(state): State<ArcAppState>) -> Json<Value> {
    let now = product::now_ms();
    let requests = state
        .pairing_requests
        .lock()
        .expect("pairing requests lock");
    let out: Vec<Value> = requests
        .iter()
        .filter(|request| request.expires_at > now && request.status == "pending")
        .map(|request| {
            json!({
                "requestId": request.request_id,
                "id": request.request_id,
                "requestedAt": crate::session_ops::iso_timestamp(request.created_at),
                "deviceName": request.device_name,
                "ip": request.ip,
                "status": request.status,
                "createdAt": request.created_at,
                "expiresAt": crate::session_ops::iso_timestamp(request.expires_at),
            })
        })
        .collect();
    Json(json!({ "requests": out }))
}

/// release POST /api/remote/pairing-requests/:requestId/decision：
/// 批准时签发设备凭据并登记到 remote-access.json。
pub(crate) async fn decide_request(
    State(state): State<ArcAppState>,
    Path(request_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let approved = body
        .get("approved")
        .and_then(Value::as_bool)
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid_pairing_decision",
                "approved 必须是布尔值。",
            )
        })?;
    let mut requests = state.pairing_requests.lock().expect("pairing requests");
    let request = requests
        .iter_mut()
        .find(|request| request.request_id == request_id)
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "pairing_request_not_found",
                "配对申请不存在。",
            )
        })?;
    if request.expires_at <= product::now_ms() {
        return Err(ApiError::new(
            StatusCode::GONE,
            "pairing_request_expired",
            "配对申请已过期。",
        ));
    }
    if request.status != "pending" {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "pairing_request_resolved",
            "配对申请已处理。",
        ));
    }
    if approved {
        let token = paired_token()?;
        let device = security::PairedDevice::new(
            product::new_id(),
            request.device_name.clone(),
            &token,
            product::now_ms(),
        );
        let mut devices = state.pairing.devices.lock().expect("devices lock");
        let mut updated = devices.clone();
        updated.push(device.clone());
        security::save_devices(&state.data_dir, &updated).map_err(ApiError::internal)?;
        *devices = updated;
        request.device_id = Some(device.id.clone());
        request.token = Some(token);
        request.status = "approved".to_string();
    } else {
        request.status = "denied".to_string();
    }
    Ok(Json(json!({
        "requestId": request.request_id,
        "status": request.status,
        "deviceId": request.device_id,
        "deviceName": request.device_name,
    })))
}

// --------------------------------------------------------- revoke alias

/// release POST /api/remote/devices/:deviceId/revoke → 204。
pub(crate) async fn revoke_device_post(
    State(state): State<ArcAppState>,
    Path(device_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    {
        let mut devices = state.pairing.devices.lock().expect("devices lock");
        security::revoke_device(&state.data_dir, &mut devices, &device_id)?;
        state.remote.revoke(&device_id);
    }
    Ok(StatusCode::NO_CONTENT)
}
