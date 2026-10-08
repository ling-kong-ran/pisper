//! Cookie 鉴权后的组件管理，及只授权单个组件静态资源的前置凭证入口。
use crate::native_custom_ui::{AssetResponse, CustomUiError, CustomUiService};
use axum::{
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Path, Request, State},
    http::{Extensions, HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct RequestIdentity {
    pub owner: String,
    pub remote: bool,
}
impl RequestIdentity {
    pub fn local() -> Self {
        Self {
            owner: "local".into(),
            remote: false,
        }
    }
}
// 必须由已鉴权宿主提供；请求 body/header 不能自行声明设备身份。
pub type IdentityResolver =
    Arc<dyn Fn(&HeaderMap, &Extensions) -> Result<RequestIdentity, CustomUiError> + Send + Sync>;
pub type OwnerActive = Arc<dyn Fn(&str) -> bool + Send + Sync>;
struct HttpState {
    service: Arc<CustomUiService>,
    identity: IdentityResolver,
}

pub fn router<S: Clone + Send + Sync + 'static>(
    service: Arc<CustomUiService>,
    identity: Option<IdentityResolver>,
) -> Router<S> {
    Router::new()
        .route("/api/custom-ui/import", post(import))
        .route("/api/custom-ui/components", get(components))
        .route("/api/custom-ui/components/{id}/views", post(create_view))
        .route("/api/custom-ui/views/{id}", put(renew).delete(revoke))
        .route("/api/custom-ui/bridge.js", get(bridge))
        .route(
            "/api/custom-ui/components/{id}/assets/{*path}",
            get(legacy_asset),
        )
        .layer(DefaultBodyLimit::max(16 * 1024 * 1024))
        .with_state(Arc::new(HttpState {
            service,
            identity: identity.unwrap_or_else(|| Arc::new(|_, _| Ok(RequestIdentity::local()))),
        }))
}
impl IntoResponse for CustomUiError {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (status, Json(json!({"code":self.code,"error":self.message}))).into_response()
    }
}
fn response(asset: AssetResponse) -> Response {
    let length = asset.body.len();
    let mut response = Body::from(asset.body).into_response();
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static(asset.content_type),
    );
    if let Ok(length) = axum::http::HeaderValue::from_str(&length.to_string()) {
        response
            .headers_mut()
            .insert(axum::http::header::CONTENT_LENGTH, length);
    }
    for (name, value) in asset.headers {
        if let Ok(value) = axum::http::HeaderValue::from_str(&value) {
            response
                .headers_mut()
                .insert(axum::http::HeaderName::from_static(name), value);
        }
    }
    response
}
async fn blocking<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T, CustomUiError> + Send + 'static,
) -> Result<T, CustomUiError> {
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|_| CustomUiError::new(500, "component_storage_failed", "组件文件操作失败。"))?
}
async fn components(State(state): State<Arc<HttpState>>) -> Result<Json<Value>, CustomUiError> {
    let service = state.service.clone();
    blocking(move || service.list_components()).await.map(Json)
}
async fn import(
    State(state): State<Arc<HttpState>>,
    bytes: Bytes,
) -> Result<(StatusCode, Json<Value>), CustomUiError> {
    let service = state.service.clone();
    Ok((
        StatusCode::CREATED,
        Json(blocking(move || service.import_bundle(&bytes)).await?),
    ))
}
async fn create_view(
    State(state): State<Arc<HttpState>>,
    Path(id): Path<String>,
    request: Request,
) -> Result<Json<crate::native_custom_ui::ViewGrant>, CustomUiError> {
    let identity = (state.identity)(request.headers(), request.extensions())?;
    validate_identity(&identity)?;
    let bytes = axum::body::to_bytes(request.into_body(), 64 * 1024)
        .await
        .map_err(|_| invalid_origin())?;
    let input: Value = serde_json::from_slice(&bytes).map_err(|_| invalid_origin())?;
    let object = input.as_object().ok_or_else(invalid_origin)?;
    if object.keys().any(|key| key != "origin") {
        return Err(invalid_origin());
    }
    let origin = object
        .get("origin")
        .and_then(Value::as_str)
        .ok_or_else(invalid_origin)?
        .to_owned();
    let service = state.service.clone();
    blocking(move || service.create_view(&id, &identity.owner, &origin))
        .await
        .map(Json)
}
fn invalid_origin() -> CustomUiError {
    CustomUiError::new(400, "component_origin_invalid", "组件来源无效。")
}
fn validate_identity(identity: &RequestIdentity) -> Result<(), CustomUiError> {
    if identity.owner.is_empty() || identity.remote && identity.owner == "local" {
        Err(CustomUiError::new(
            403,
            "component_owner_invalid",
            "组件设备身份无效。",
        ))
    } else {
        Ok(())
    }
}
async fn renew(
    State(state): State<Arc<HttpState>>,
    Path(id): Path<String>,
    request: Request,
) -> Result<Json<Value>, CustomUiError> {
    let identity = (state.identity)(request.headers(), request.extensions())?;
    validate_identity(&identity)?;
    state.service.renew_view(&id, &identity.owner)?;
    Ok(Json(json!({"ok":true})))
}
async fn revoke(
    State(state): State<Arc<HttpState>>,
    Path(id): Path<String>,
    request: Request,
) -> Result<Json<Value>, CustomUiError> {
    let identity = (state.identity)(request.headers(), request.extensions())?;
    validate_identity(&identity)?;
    state.service.revoke_view(&id, &identity.owner);
    Ok(Json(json!({"ok":true})))
}
async fn bridge(State(state): State<Arc<HttpState>>) -> Response {
    response(state.service.bridge(None))
}
async fn legacy_asset(
    State(state): State<Arc<HttpState>>,
    Path((id, path)): Path<(String, String)>,
) -> Result<Response, CustomUiError> {
    // 保留 release 旧 URL 的三级兼容范围，深层路径使用短期凭证入口。
    if path.split('/').count() > 3 {
        return Err(CustomUiError::new(
            404,
            "component_asset_missing",
            "组件资源不存在。",
        ));
    }
    let service = state.service.clone();
    blocking(move || service.asset(&id, &path, None))
        .await
        .map(response)
}

// 在 desktop Cookie auth 前调用；Some 表示已处理 render 子路径（包括404）。
// 令牌从未返回一般 API 身份；remote=true 会拒绝 local view，非 local owner 默认拒绝。
pub async fn render_request(
    service: Arc<CustomUiService>,
    method: Method,
    uri: axum::http::Uri,
    remote: bool,
    owner_active: Option<OwnerActive>,
) -> Option<Response> {
    let rest = uri.path().strip_prefix("/api/custom-ui/render/")?;
    if method != Method::GET {
        return Some(
            CustomUiError::new(404, "component_view_expired", "组件预览不存在或已过期。")
                .into_response(),
        );
    }
    let Some((token, resource)) = rest.split_once('/') else {
        return Some(expired());
    };
    let Some(view) = service.get_view(token) else {
        return Some(expired());
    };
    if remote && view.owner == "local"
        || view.owner != "local"
            && !owner_active
                .as_ref()
                .is_some_and(|active| active(&view.owner))
    {
        return Some(expired());
    }
    let resource_base = format!("{}/api/custom-ui/render/{token}/", view.origin);
    if resource == "bridge.js" {
        return Some(response(service.bridge(Some(&resource_base))));
    }
    let Some(encoded_path) = resource.strip_prefix("assets/") else {
        return Some(expired());
    };
    let Some(path) = decode_uri_component(encoded_path) else {
        return Some(
            CustomUiError::new(404, "component_asset_missing", "组件资源不存在。").into_response(),
        );
    };
    let result =
        blocking(move || service.asset(&view.component_id, &path, Some(&resource_base))).await;
    Some(match result {
        Ok(asset) => response(asset),
        Err(error) => error.into_response(),
    })
}
fn expired() -> Response {
    CustomUiError::new(404, "component_view_expired", "组件预览不存在或已过期。").into_response()
}
fn decode_uri_component(input: &str) -> Option<String> {
    let mut output = Vec::new();
    let bytes = input.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let pair = std::str::from_utf8(bytes.get(index + 1..index + 3)?).ok()?;
            output.push(u8::from_str_radix(pair, 16).ok()?);
            index += 3;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(output).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn percent_paths_are_decoded_once_and_invalid_utf8_is_rejected() {
        assert_eq!(
            decode_uri_component("a%20b/+x.js").as_deref(),
            Some("a b/+x.js")
        );
        assert_eq!(
            decode_uri_component("%252e%252e/x").as_deref(),
            Some("%2e%2e/x")
        );
        for path in ["%", "%G0", "%ff"] {
            assert!(decode_uri_component(path).is_none());
        }
    }
}
