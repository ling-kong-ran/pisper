//! Thin HTTP contract for native notification storage and actual gateway dispatch.
use crate::native_notifications::{dispatch, templates};
use crate::{ApiError, AppState};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post, put},
    Json, Router,
};
use serde_json::Value;
use std::{collections::HashMap, sync::Arc};

pub(crate) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/settings/notifications", get(settings))
        .route(
            "/api/settings/notifications/browser",
            get(settings).patch(browser),
        )
        .route("/api/settings/notifications/browser/events", get(events))
        .route(
            "/api/settings/notifications/templates/{event}/{platform}",
            put(update_template),
        )
        .route(
            "/api/settings/notifications/templates/{event}/{platform}/test",
            post(test_template),
        )
        .route(
            "/api/settings/notifications/chat-completed",
            post(chat_completed),
        )
        .route(
            "/api/settings/notifications/chat-waiting",
            post(chat_waiting),
        )
}
async fn settings(State(state): State<Arc<AppState>>) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        state
            .notifications
            .get_state(&state.providers.app_preferences()?)?,
    ))
}
async fn browser(
    State(state): State<Arc<AppState>>,
    Json(input): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let enabled = input.get("enabled").is_some_and(templates::truthy);
    let app = state
        .providers
        .update_browser_notifications_enabled(enabled)
        .await?;
    Ok(Json(state.notifications.get_state(&app)?))
}
async fn events(
    State(state): State<Arc<AppState>>,
    Query(parameters): Query<HashMap<String, String>>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(state.notifications.poll(
        parameters.get("after").map(String::as_str).unwrap_or(""),
    )?))
}
async fn update_template(
    State(state): State<Arc<AppState>>,
    Path((event, platform)): Path<(String, String)>,
    Json(input): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(state.notifications.update_template(
        &event,
        &platform,
        &input,
        &state.providers.app_preferences()?,
    )?))
}
async fn test_template(
    State(state): State<Arc<AppState>>,
    Path((event, platform)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        state
            .notifications
            .test_template(&event, &platform, &state.providers.app_preferences()?)
            .await?,
    ))
}
pub(crate) async fn chat_report_response(
    service: &crate::native_notifications::NotificationService,
    app: &Value,
    waiting: bool,
    input: &Value,
) -> (StatusCode, Json<Value>) {
    (
        StatusCode::ACCEPTED,
        Json(dispatch::chat_report(service, app, waiting, input).await),
    )
}
async fn chat_completed(
    State(state): State<Arc<AppState>>,
    Json(input): Json<Value>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let app = state.providers.app_preferences()?;
    Ok(chat_report_response(&state.notifications, &app, false, &input).await)
}
async fn chat_waiting(
    State(state): State<Arc<AppState>>,
    Json(input): Json<Value>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let app = state.providers.app_preferences()?;
    Ok(chat_report_response(&state.notifications, &app, true, &input).await)
}
