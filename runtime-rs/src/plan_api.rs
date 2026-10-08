//! 主会话计划工具与 HTTP 适配。子 Agent 仅能读取父计划，不能变更状态。
#[path = "native_plans/mod.rs"]
mod domain;
use crate::session_workers::{native_tool, EventSink, SessionExecutor};
use axum::{
    extract::{Path, State},
    routing::get,
    Json, Router,
};
pub(crate) use domain::PlanService;
use serde_json::{json, Value};
use std::sync::Arc;

pub(crate) fn tools(
    service: Arc<PlanService>,
    session: String,
    can_write: bool,
    events: EventSink,
) -> Vec<pi_rust::agent_core::types::AgentTool> {
    let parameters = json!({"type":"object","required":["items"],"properties":{"mode":{"type":"string","enum":["auto","replace"]},"items":{"type":"array","maxItems":50,"items":{"type":"object","required":["title","status"],"properties":{"id":{"type":"string","minLength":1,"maxLength":80},"title":{"type":"string","minLength":1,"maxLength":300},"status":{"type":"string","enum":["pending","in_progress","completed","blocked"]},"note":{"type":"string","maxLength":1000},"assignee":{"type":"string","maxLength":80},"dependsOn":{"type":"array","maxItems":20,"items":{"type":"string","minLength":1,"maxLength":80}}}}}}});
    ["get_plan","get_task_list","update_plan","update_task_list"].into_iter().filter(|name|can_write||name.starts_with("get_")).map(|name|{
        let service=service.clone();let session=session.clone();let events=events.clone();let update=name.starts_with("update_");
        native_tool(name,if update{"Maintain the complete primary session plan with stable ids. A disjoint temporary plan suspends unfinished work and restores it when completed. An empty array cancels every plan; use replace only for explicit permanent redirection."}else{"Read the primary session plan for coordination."},if update{parameters.clone()}else{json!({"type":"object","properties":{}})},Arc::new(move|args|{
            let service=service.clone();let session=session.clone();let events=events.clone();Box::pin(async move{
                let plan=if update{let plan=service.replace(&session,&args["items"],args["mode"].as_str().unwrap_or("auto"))?;events("plan_update",&json!({"sessionId":session,"plan":plan}));plan}else{service.get(&session)};Ok(json!({"plan":plan}))
            })
        }))
    }).collect()
}
#[derive(Clone)]
struct Api {
    service: Arc<PlanService>,
    executor: Arc<dyn SessionExecutor>,
    events: EventSink,
}
pub(crate) fn routes<S: Clone + Send + Sync + 'static>(
    service: Arc<PlanService>,
    executor: Arc<dyn SessionExecutor>,
    events: EventSink,
) -> Router<S> {
    Router::new()
        .route(
            "/api/sessions/{id}/plan",
            get(read_plan).put(update_plan).delete(clear_plan),
        )
        .with_state(Api {
            service,
            executor,
            events,
        })
}
async fn scope(api: &Api, id: &str) -> Result<(), crate::ApiError> {
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
async fn read_plan(
    State(api): State<Api>,
    Path(id): Path<String>,
) -> Result<Json<Value>, crate::ApiError> {
    scope(&api, &id).await?;
    Ok(Json(json!({"plan":api.service.get(&id)})))
}
async fn update_plan(
    State(api): State<Api>,
    Path(id): Path<String>,
    Json(input): Json<Value>,
) -> Result<Json<Value>, crate::ApiError> {
    scope(&api, &id).await?;
    let plan = api
        .service
        .replace(
            &id,
            &input["items"],
            input["mode"].as_str().unwrap_or("auto"),
        )
        .map_err(|e| crate::ApiError::bad_request(e.to_string()))?;
    (api.events)("plan_update", &json!({"sessionId":id,"plan":plan}));
    Ok(Json(json!({"plan":plan})))
}
async fn clear_plan(
    State(api): State<Api>,
    Path(id): Path<String>,
) -> Result<Json<Value>, crate::ApiError> {
    scope(&api, &id).await?;
    let plan = api
        .service
        .remove(&id)
        .map_err(|e| crate::ApiError::internal(e.to_string()))?;
    (api.events)("plan_update", &json!({"sessionId":id,"plan":plan}));
    Ok(Json(json!({"plan":plan})))
}
