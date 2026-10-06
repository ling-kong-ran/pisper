//! Goal HTTP 和 Pi 工具只调用真实目标控制器，执行链由 root 的 Pi adapter 提供。
#[path = "native_goals/mod.rs"]
mod domain;
use crate::session_workers::{native_tool, SessionExecutor};
use axum::{
    extract::{Path, State},
    routing::get,
    Json, Router,
};
pub(crate) use domain::runner::{GoalRunner, TeamCoordinator};
pub(crate) use domain::{usage_tokens, Goal, GoalService, CONTINUATION_MARKER};
use serde_json::{json, Value};
use std::sync::Arc;
pub(crate) fn tools(
    runner: Arc<GoalRunner>,
    session: String,
) -> Vec<pi_rust::agent_core::types::AgentTool> {
    ["get_goal","update_goal"].into_iter().map(|name|{let runner=runner.clone();let session=session.clone();let update=name=="update_goal";
        native_tool(name,if update{"Mark the active Goal complete only after every explicit requirement is complete and verified with concrete evidence. Do not complete merely because progress is substantial or budget is low."}else{"Read the current goal objective, status and token budget."},if update{json!({"type":"object","required":["status"],"properties":{"status":{"const":"complete","type":"string"}}})}else{json!({"type":"object","properties":{}})},Arc::new(move|args|{let runner=runner.clone();let session=session.clone();Box::pin(async move{let goal=if update{if args["status"]!="complete"{anyhow::bail!("update_goal only accepts status=complete.")}Some(runner.complete(&session).await?)}else{runner.goals.get(&session)};Ok(json!({"goal":goal}))})}))
    }).collect()
}
#[derive(Clone)]
struct Api {
    runner: Arc<GoalRunner>,
    executor: Arc<dyn SessionExecutor>,
}
pub(crate) fn routes<S: Clone + Send + Sync + 'static>(
    runner: Arc<GoalRunner>,
    executor: Arc<dyn SessionExecutor>,
) -> Router<S> {
    Router::new()
        .route("/api/sessions/{id}/goal", get(get_goal).patch(update_goal))
        .with_state(Api { runner, executor })
}
async fn validate(api: &Api, id: &str) -> Result<(), crate::ApiError> {
    api.executor
        .validate_session(id.into())
        .await
        .map(|_| ())
        .map_err(|e| {
            crate::ApiError::new(
                axum::http::StatusCode::NOT_FOUND,
                "session_not_found",
                e.to_string(),
            )
        })
}
async fn get_goal(
    State(api): State<Api>,
    Path(id): Path<String>,
) -> Result<Json<Value>, crate::ApiError> {
    validate(&api, &id).await?;
    let goal = api.runner.goals.get(&id).ok_or_else(|| {
        crate::ApiError::new(
            axum::http::StatusCode::NOT_FOUND,
            "goal_not_found",
            "当前会话没有 Goal。",
        )
    })?;
    Ok(Json(json!({"goal":goal})))
}
async fn update_goal(
    State(api): State<Api>,
    Path(id): Path<String>,
    Json(input): Json<Value>,
) -> Result<Json<Value>, crate::ApiError> {
    validate(&api, &id).await?;
    let result = match input["action"].as_str() {
        Some("pause") => api.runner.pause(&id).await,
        Some("set-budget") => api
            .runner
            .set_budget(&id, &input["tokenBudget"])
            .await
            .map(Some),
        _ => Err(anyhow::anyhow!("Goal 操作无效。")),
    };
    let goal = result
        .map_err(|e| crate::ApiError::bad_request(e.to_string()))?
        .ok_or_else(|| {
            crate::ApiError::new(
                axum::http::StatusCode::NOT_FOUND,
                "goal_not_found",
                "当前会话没有进行中的 Goal。",
            )
        })?;
    Ok(Json(json!({"goal":goal})))
}
