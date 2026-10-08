//! Authenticated HTTP adapter. Canonical configuration and session reload are injected
//! by the host; the plugin domain never holds the whole application state.
use crate::native_plugins::{PluginError, Result, ToolPluginService};
use axum::{
    extract::{DefaultBodyLimit, Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, patch, post},
    Json, Router,
};
use futures::future::BoxFuture;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

pub type ProjectState =
    Arc<dyn Fn(Value, Option<String>) -> BoxFuture<'static, Result<Value>> + Send + Sync>;
pub type Reload = Arc<dyn Fn() -> BoxFuture<'static, Result<()>> + Send + Sync>;
#[derive(Clone)]
pub struct PluginHooks {
    pub project: Option<ProjectState>,
    pub reload: Reload,
}
struct HttpState {
    service: Arc<ToolPluginService>,
    hooks: PluginHooks,
}
pub fn router<S: Clone + Send + Sync + 'static>(
    service: Arc<ToolPluginService>,
    hooks: PluginHooks,
) -> Router<S> {
    Router::new()
        .route("/api/plugins", get(list).put(save))
        .route("/api/plugins/inspect", post(inspect))
        .route("/api/plugins/install", post(install))
        .route("/api/plugins/{id}", patch(toggle).delete(uninstall))
        .route(
            "/api/plugins/{id}/capabilities/{name}",
            patch(toggle_capability),
        )
        .layer(DefaultBodyLimit::max(24 * 1024 * 1024))
        .with_state(Arc::new(HttpState { service, hooks }))
}
impl IntoResponse for PluginError {
    fn into_response(self) -> Response {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":self.message})),
        )
            .into_response()
    }
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListQuery {
    session_id: Option<String>,
}
async fn project(state: &HttpState, value: Value, session: Option<String>) -> Result<Value> {
    if let Some(project) = &state.hooks.project {
        project(value, session.filter(|v| !v.is_empty())).await
    } else {
        Ok(value)
    }
}
async fn list(
    State(state): State<Arc<HttpState>>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Value>> {
    Ok(Json(
        project(&state, state.service.get_state().await?, query.session_id).await?,
    ))
}
async fn inspect(
    State(state): State<Arc<HttpState>>,
    Json(input): Json<Value>,
) -> Result<Json<Value>> {
    Ok(Json(
        state
            .service
            .inspect(input["path"].as_str().unwrap_or(""))
            .await?,
    ))
}
async fn install(
    State(state): State<Arc<HttpState>>,
    Json(input): Json<Value>,
) -> Result<(StatusCode, Json<Value>)> {
    let result = state
        .service
        .install(input["inspectionId"].as_str().unwrap_or(""))
        .await?;
    (state.hooks.reload)().await?;
    Ok((StatusCode::CREATED, Json(result)))
}
async fn save(
    State(state): State<Arc<HttpState>>,
    Json(mut input): Json<Value>,
) -> Result<Json<Value>> {
    let current = state.service.get_state().await?;
    let visible = project(&state, current.clone(), None).await?;
    let visible_ids: Vec<&str> = visible["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v["id"].as_str())
        .collect();
    let mut requested: Vec<Value> = input["enabledTools"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for id in current["enabledTools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        if !visible_ids.contains(&id) && !requested.iter().any(|v| v == id) {
            requested.push(json!(id));
        }
    }
    if !input.is_object() {
        input = json!({});
    }
    input["enabledTools"] = json!(requested);
    let saved = state.service.save_state(input).await?;
    (state.hooks.reload)().await?;
    Ok(Json(project(&state, saved, None).await?))
}
async fn toggle(
    State(state): State<Arc<HttpState>>,
    Path(id): Path<String>,
    Json(input): Json<Value>,
) -> Result<Json<Value>> {
    let enabled = input["enabled"]
        .as_bool()
        .ok_or_else(|| PluginError::new("插件启用状态无效。"))?;
    let result = state.service.set_plugin_enabled(&id, enabled).await?;
    (state.hooks.reload)().await?;
    Ok(Json(result))
}
async fn toggle_capability(
    State(state): State<Arc<HttpState>>,
    Path((id, name)): Path<(String, String)>,
    Json(input): Json<Value>,
) -> Result<Json<Value>> {
    let enabled = input["enabled"]
        .as_bool()
        .ok_or_else(|| PluginError::new("插件能力启用状态无效。"))?;
    let result = state
        .service
        .set_capability_enabled(&id, &name, enabled)
        .await?;
    (state.hooks.reload)().await?;
    Ok(Json(result))
}
async fn uninstall(
    State(state): State<Arc<HttpState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    let result = state.service.uninstall(&id).await?;
    (state.hooks.reload)().await?;
    Ok(Json(result))
}
