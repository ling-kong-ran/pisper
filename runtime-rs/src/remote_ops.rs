//! release `http/routes/remote.mjs` 中剩余的远程访问路由：
//! 防火墙状态/重试、配对审批流（pairing-requests）与 revoke 别名。
//! 契约对齐 release（状态码 + 机读 code + 响应字段）。
//!
//! 诚实接缝：系统防火墙规则管理与远程 HTTPS 监听（Iroh/LAN 端点）
//! 尚未在本后端实现，相应状态如实报告而不是伪造 allowed/listening。

use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde_json::{json, Value};
use sha2::Digest;
use std::net::SocketAddr;
use std::sync::Arc;

use crate::{product, security, ApiError, AppState};

type ArcAppState = Arc<AppState>;

const PAIRING_REQUEST_TTL_MS: u64 = 300_000;

fn remote_endpoints(state: &AppState) -> Value {
    if state.remote_enabled.load(std::sync::atomic::Ordering::Relaxed) {
        json!([{ "t": "lan" }])
    } else {
        json!([])
    }
}

fn server_name() -> &'static str {
    "Pisper"
}

fn paired_token(state: &AppState, device_name: &str) -> String {
    let digest = sha2::Sha256::digest(
        format!(
            "{}:{}:{}:{}",
            product::now_ms(),
            device_name,
            std::process::id(),
            state.fingerprint
        )
        .as_bytes(),
    );
    digest
        .iter()
        .take(32)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

// -------------------------------------------------------------- firewall

/// release RemoteFirewallService.status 的原生视图。
async fn remote_firewall_view(state: &AppState) -> Value {
    let enabled = state.remote_enabled.load(std::sync::atomic::Ordering::Relaxed);
    json!({
        "state": if enabled { "failed" } else { "disabled" },
        "reason": if enabled { json!("firewall_management_unavailable") } else { Value::Null },
        "port": Value::Null,
        "scope": "program_port",
        "checkedAt": Value::Null,
        "busy": false,
        "lanReachability": "unverified",
    })
}

/// release GET /api/remote/firewall。
pub(crate) async fn firewall_status(
    State(state): State<ArcAppState>,
) -> Json<Value> {
    Json(remote_firewall_view(&state).await)
}

/// release POST /api/remote/firewall/retry：非 JSON 请求按契约返回 415。
pub(crate) async fn firewall_retry(
    State(state): State<ArcAppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if !content_type.starts_with("application/json") {
        return Err(ApiError::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "json_required",
            "此操作需要 JSON 请求。",
        ));
    }
    Ok(Json(remote_firewall_view(&state).await))
}

// ------------------------------------------------------ pairing requests

/// release POST /api/remote/pairing-requests：申请方创建配对审批（202）。
pub(crate) async fn create_request(
    State(state): State<ArcAppState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let device_name = body
        .get("deviceName")
        .and_then(Value::as_str)
        .unwrap_or("device")
        .to_string();
    let ip = headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .unwrap_or_else(|| addr.ip().to_string());
    let now = product::now_ms();
    let secret = {
        let digest = sha2::Sha256::digest(
            format!("{}:{}:{}", now, std::process::id(), state.fingerprint).as_bytes(),
        );
        digest
            .iter()
            .take(16)
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
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
        let mut requests = state.pairing_requests.lock().expect("pairing requests lock");
        requests.retain(|request| request.expires_at > now);
        requests.push(approval.clone());
        security::save_pairing_requests(&state.data_dir, &requests)
            .map_err(ApiError::internal)?;
    }
    Ok(Json(json!({
        "requestId": approval.request_id,
        "requestSecret": approval.secret,
        "expiresAt": approval.expires_at,
        "serverName": server_name(),
        "endpoints": remote_endpoints(&state),
        "fingerprint": state.fingerprint,
        "apiVersion": 1,
    })))
}

fn find_request(state: &AppState, request_id: &str) -> Result<security::PairingApproval, ApiError> {
    let now = product::now_ms();
    let requests = state.pairing_requests.lock().expect("pairing requests lock");
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
    if presented != request.secret {
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
            "endpoints": remote_endpoints(&state),
            "apiVersion": 1,
            "status": "approved",
        }))),
        "denied" => Ok(Json(json!({"status": "denied"}))),
        _ => Ok(Json(json!({
            "status": "pending",
            "expiresAt": request.expires_at,
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
        let mut requests = state.pairing_requests.lock().expect("pairing requests lock");
        requests.retain(|r| r.request_id != request_id);
        security::save_pairing_requests(&state.data_dir, &requests)
            .map_err(ApiError::internal)?;
    }
    Ok(StatusCode::NO_CONTENT)
}

/// release GET /api/remote/pairing-requests：桌面端审批列表（不含 secret/token）。
pub(crate) async fn list_requests(State(state): State<ArcAppState>) -> Json<Value> {
    let now = product::now_ms();
    let requests = state.pairing_requests.lock().expect("pairing requests lock");
    let out: Vec<Value> = requests
        .iter()
        .filter(|request| request.expires_at > now)
        .map(|request| {
            json!({
                "requestId": request.request_id,
                "deviceName": request.device_name,
                "ip": request.ip,
                "status": request.status,
                "createdAt": request.created_at,
                "expiresAt": request.expires_at,
            })
        })
        .collect();
    Json(json!({ "requests": out }))
}

/// release POST /api/remote/pairing-requests/:requestId/decision：
/// 批准时签发设备凭据并登记到 devices.json。
pub(crate) async fn decide_request(
    State(state): State<ArcAppState>,
    Path(request_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let approved = body.get("approved").and_then(Value::as_bool).ok_or_else(|| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_pairing_decision",
            "approved 必须是布尔值。",
        )
    })?;
    let mut request = find_request(&state, &request_id)?;
    if request.status != "pending" {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "pairing_request_resolved",
            "配对申请已处理。",
        ));
    }
    if approved {
        let token = paired_token(&state, &request.device_name);
        let device = security::PairedDevice {
            id: product::new_id(),
            name: request.device_name.clone(),
            token: token.clone(),
            paired_at: product::now_ms(),
        };
        request.device_id = Some(device.id.clone());
        request.token = Some(token);
        request.status = "approved".to_string();
        let mut devices = state.pairing.devices.lock().expect("devices lock");
        devices.push(device);
        security::save_devices(&state.data_dir, &devices).map_err(ApiError::internal)?;
    } else {
        request.status = "denied".to_string();
    }
    {
        let mut requests = state.pairing_requests.lock().expect("pairing requests lock");
        if let Some(slot) = requests.iter_mut().find(|r| r.request_id == request_id) {
            slot.status = request.status.clone();
            slot.device_id = request.device_id.clone();
            slot.token = request.token.clone();
        }
        security::save_pairing_requests(&state.data_dir, &requests)
            .map_err(ApiError::internal)?;
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
        let before = devices.len();
        devices.retain(|d| d.id != device_id);
        if devices.len() == before {
            return Err(ApiError::new(
                StatusCode::NOT_FOUND,
                "device_not_found",
                "设备不存在。",
            ));
        }
        security::save_devices(&state.data_dir, &devices).map_err(ApiError::internal)?;
    }
    Ok(StatusCode::NO_CONTENT)
}
