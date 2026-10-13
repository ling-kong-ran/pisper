//! 远程 HTTPS 的权限边界；只有此监听器可以标记请求为已验证远端。
use crate::{remote_access, ApiError, AppState};
use axum::{
    body::Body,
    extract::{Request, State},
    http::{header, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use futures::StreamExt;
use std::sync::Arc;
use subtle::ConstantTimeEq;

#[derive(Clone)]
pub(crate) struct RemoteRequest;

pub(crate) async fn start(state: &Arc<AppState>) -> Result<(), ApiError> {
    if state.runtime_profile != "desktop" {
        return Err(ApiError::new(
            StatusCode::NOT_IMPLEMENTED,
            "remote_unavailable",
            "移动本地运行时不提供远程服务器。",
        ));
    }
    let router = crate::app_router(state.clone())
        .map_err(|_| ApiError::internal("无法创建远程路由。"))?
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            authenticate,
        ));
    let main_port = state
        .self_base
        .lock()
        .expect("self base")
        .as_ref()
        .and_then(|base| base.parse::<axum::http::Uri>().ok())
        .and_then(|uri| uri.port_u16());
    state
        .remote
        .start(router, main_port)
        .await
        .map_err(|message| {
            state.remote.error(&message);
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "remote_start_failed",
                message,
            )
        })
}

fn rejected(status: StatusCode, code: &'static str, message: &str) -> Response {
    ApiError::new(status, code, message).into_response()
}

async fn authenticate(
    State(state): State<Arc<AppState>>,
    mut request: Request,
    next: Next,
) -> Response {
    if !state
        .remote_enabled
        .load(std::sync::atomic::Ordering::Acquire)
    {
        return rejected(
            StatusCode::SERVICE_UNAVAILABLE,
            "remote_disabled",
            "远程访问已关闭。",
        );
    }
    let path = request.uri().path();
    let method = request.method();
    let request_path = path
        .strip_prefix("/api/remote/pairing-requests/")
        .is_some_and(|id| !id.is_empty() && !id.contains('/'));
    let public = (path == "/api/remote/pair" && method == Method::POST)
        || (path == "/api/remote/pairing-requests" && method == Method::POST)
        || (request_path && matches!(*method, Method::GET | Method::DELETE));
    // 配对凭据不能开启新的管理权限；本机二维码、开关、防火墙、审批和设备管理仅由桌面操作。
    if !path.starts_with("/api/")
        || (path.starts_with("/api/remote/") && !public && path != "/api/remote/status")
    {
        return rejected(
            StatusCode::FORBIDDEN,
            "local_management_required",
            "此操作只能在服务器本机执行。",
        );
    }
    if path.starts_with("/api/desktop/") || path.starts_with("/api/terminal/") {
        return rejected(
            StatusCode::FORBIDDEN,
            "local_management_required",
            "此操作需要本机桌面连接。",
        );
    }
    // 手机原生代理不会发送 Origin；拒绝网页直接跨站兑换一次性凭据。
    if request.headers().contains_key(header::ORIGIN) {
        return rejected(
            StatusCode::FORBIDDEN,
            "forbidden_origin",
            "远程连接不接受浏览器跨站请求。",
        );
    }
    let cancellation = if public {
        None
    } else {
        let token = request
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .filter(|value| !value.is_empty() && value.len() <= 1024);
        let Some(token) = token else {
            return rejected(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "需要已配对设备凭据。",
            );
        };
        let hash = remote_access::hash(token);
        let mut devices = state.pairing.devices.lock().expect("paired devices");
        let Some(device) = devices.iter_mut().find(|device| {
            device.revoked_at.is_none()
                && bool::from(device.token_hash.as_bytes().ct_eq(hash.as_bytes()))
        }) else {
            return rejected(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "设备凭据无效或已吊销。",
            );
        };
        let id = device.id.clone();
        let now = crate::product::now_ms();
        if now.saturating_sub(
            crate::session_ops::parse_timestamp_ms(&device.last_seen_at).unwrap_or(0),
        ) >= 60_000
        {
            device.last_seen_at = crate::session_ops::iso_timestamp(now);
            if crate::security::save_devices(&state.data_dir, &devices).is_err() {
                return rejected(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "remote_storage_failed",
                    "无法保存设备状态。",
                );
            }
        }
        // 持有设备锁时注册取消句柄，避免吊销或重启在鉴权与流注册之间穿过。
        Some(state.remote.device_stream(&id))
    };
    if cancellation
        .as_ref()
        .is_some_and(|token| token.is_cancelled())
    {
        return rejected(StatusCode::UNAUTHORIZED, "unauthorized", "设备凭据已吊销。");
    }
    request.headers_mut().remove(header::COOKIE);
    request.extensions_mut().insert(RemoteRequest);
    let response = if public {
        // 未配对设备不能以慢速请求体长期占据有限连接；已认证任务仍可正常长时间运行。
        match tokio::time::timeout(std::time::Duration::from_secs(15), next.run(request)).await {
            Ok(response) => response,
            Err(_) => {
                return rejected(
                    StatusCode::REQUEST_TIMEOUT,
                    "remote_request_timeout",
                    "配对请求超时。",
                )
            }
        }
    } else {
        next.run(request).await
    };
    let (mut parts, body) = response.into_parts();
    parts.headers.remove(header::SET_COOKIE);
    parts.headers.insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    let body = match cancellation {
        Some(token) => {
            Body::from_stream(body.into_data_stream().take_until(token.cancelled_owned()))
        }
        None => body,
    };
    Response::from_parts(parts, body)
}
