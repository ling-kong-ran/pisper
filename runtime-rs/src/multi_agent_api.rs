//! 多 Agent 的原生 Pi 工具、父域隔离 HTTP 和真实 Team 成员通讯。
pub(crate) use crate::session_workers::agents::{
    completion_prompt, AgentService, TaskGraph, COMPLETION_MARKER,
};
pub(crate) use crate::session_workers::agents::{CompletionBatch, CompletionDispatcher};
pub(crate) use crate::session_workers::team::{Coordinator as TeamCoordinatorAdapter, TeamService};
pub(crate) use crate::session_workers::team_workflow::TeamWorkflowService;
use crate::session_workers::{native_tool, SessionExecutor};
use axum::{
    extract::{Path, State},
    routing::get,
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::Arc;

/// 仅用于 extension 注册声明，真实 execute 会由 root adapter 按当前 ctx 重取工具。
pub(crate) fn templates(service: Arc<AgentService>) -> Vec<pi_rust::agent_core::types::AgentTool> {
    let mut definitions = tools(service.clone(), "catalog".into());
    definitions.extend(member_tools(service, "catalog".into(), "catalog".into()));
    definitions
}

pub(crate) fn tools(
    service: Arc<AgentService>,
    parent: String,
) -> Vec<pi_rust::agent_core::types::AgentTool> {
    ["spawn_agent","list_agents","send_message","followup_task","wait_agent","interrupt_agent","update_team_task","run_team_workflow"].into_iter().map(|name|{
        let parameters=match name{
            "spawn_agent"=>json!({"type":"object","required":["taskName","message"],"properties":{"taskName":{"type":"string","minLength":1,"maxLength":48},"message":{"type":"string","minLength":1,"maxLength":12000},"role":{"type":"string","maxLength":80},"files":{"type":"array","maxItems":96,"items":{"type":"string","maxLength":240}},"dependsOn":{"type":"array","maxItems":32,"items":{"type":"string","maxLength":48}}}}),
            "list_agents"=>json!({"type":"object","properties":{}}),
            "run_team_workflow"=>json!({"type":"object","required":["path"],"properties":{"path":{"type":"string","minLength":1,"maxLength":240},"args":{"type":"object"}}}),
            "wait_agent"=>json!({"type":"object","properties":{"target":{"type":"string"},"timeoutMs":{"type":"number","minimum":250,"maximum":30000}}}),
            "update_team_task"=>json!({"type":"object","required":["target"],"properties":{"target":{"type":"string"},"taskName":{"type":"string","maxLength":48},"role":{"type":"string","maxLength":80},"message":{"type":"string","maxLength":12000},"files":{"type":"array","maxItems":96,"items":{"type":"string","maxLength":240}},"dependsOn":{"type":"array","maxItems":32,"items":{"type":"string","maxLength":48}}}}),
            _=>json!({"type":"object","required":if name=="interrupt_agent"{vec!["target"]}else{vec!["target","message"]},"properties":{"target":{"type":"string","minLength":1},"message":{"type":"string","minLength":1,"maxLength":12000}}}),
        };
        let service=service.clone();let parent=parent.clone();
        native_tool(name,match name{"spawn_agent"=>"Delegate a concrete independent task to an isolated background Agent. Supply full context, constraints, files and dependencies. Continue useful non-overlapping work; members cannot recursively spawn agents.","wait_agent"=>"Wait briefly for a terminal result only when a decision requires it. Do not busy-poll. Timeout leaves the background task running.","followup_task"=>"Give an existing Agent another task while preserving its context.","send_message"=>"Send context to an active Agent without starting another run.","interrupt_agent"=>"Interrupt the current owned Agent run; inspect partial workspace changes before taking over.","update_team_task"=>"Update a queued Team task after new evidence; running tasks must be interrupted first.","run_team_workflow"=>"Run a workspace JavaScript Team workflow with restricted agent, parallel, pipeline and phase APIs. Dependencies, file ownership, real member results and verified task reuse are enforced by the runtime.",_=>"Inspect Agents owned by this primary session."},parameters,Arc::new(move|args|{let service=service.clone();let parent=parent.clone();Box::pin(async move{
            let target=args["target"].as_str().unwrap_or_default();let message=args["message"].as_str().unwrap_or_default();
            match name{
                "spawn_agent"=>service.spawn(parent,args).await,
                "list_agents"=>Ok(json!({"agents":service.list(&parent)})),
                "send_message"=>service.send_message(&parent,target,message).await,
                "followup_task"=>service.followup(&parent,target,message).await,
                "interrupt_agent"=>service.interrupt(&parent,target,"Agent was interrupted.").await,
                "update_team_task"=>service.update_task(&parent,target,&args).await,
                "run_team_workflow"=>service.run_workflow(parent,args["path"].as_str().unwrap_or_default().into(),args["args"].clone()).await,
                _=>{let result=service.wait(&parent,target,args["timeoutMs"].as_u64().unwrap_or(15000)).await?;if let Some(agent)=result.get("agent").filter(|v|v.is_object()){service.acknowledge(&parent,&[agent.clone()])?;}Ok(result)},
            }
        })}))
    }).collect()
}
#[derive(Clone)]
struct TeamApi {
    team: Arc<TeamService>,
    executor: Arc<dyn SessionExecutor>,
}
/// Release 的 Team HTTP 协议为这一条 GET；图修改和编排执行通过真实 Pi 工具完成。
pub(crate) fn team_routes<S: Clone + Send + Sync + 'static>(
    team: Arc<TeamService>,
    executor: Arc<dyn SessionExecutor>,
) -> Router<S> {
    Router::new()
        .route("/api/sessions/{id}/team", get(team_get))
        .with_state(TeamApi { team, executor })
}
async fn team_get(
    State(api): State<TeamApi>,
    Path(id): Path<String>,
) -> Result<Json<Value>, crate::ApiError> {
    api.executor
        .validate_session(id.clone())
        .await
        .map_err(|_| {
            crate::ApiError::new(
                axum::http::StatusCode::NOT_FOUND,
                "session_not_found",
                "会话不存在。",
            )
        })?;
    let team = api.team.projection(&id).ok_or_else(|| {
        crate::ApiError::new(
            axum::http::StatusCode::NOT_FOUND,
            "team_not_found",
            "当前会话没有 Team。",
        )
    })?;
    Ok(Json(json!({"team":team})))
}
pub(crate) fn member_tools(
    service: Arc<AgentService>,
    parent: String,
    sender: String,
) -> Vec<pi_rust::agent_core::types::AgentTool> {
    ["list_team_members","send_team_message"].into_iter().map(|name|{let service=service.clone();let parent=parent.clone();let sender=sender.clone();
        native_tool(name,"Restricted Team member coordination within the same parent session. Send evidence and handoffs, and do not spawn or alter the parent plan.",if name=="send_team_message"{json!({"type":"object","required":["target","message"],"properties":{"target":{"type":"string","minLength":1},"message":{"type":"string","minLength":1,"maxLength":12000}}})}else{json!({"type":"object","properties":{}})},Arc::new(move|args|{let service=service.clone();let parent=parent.clone();let sender=sender.clone();Box::pin(async move{
            if name=="list_team_members"{service.validate_member(&parent,&sender)?;Ok(json!({"agents":service.summaries(&parent)}))}else{service.send_from_agent(&parent,&sender,args["target"].as_str().unwrap_or_default(),args["message"].as_str().unwrap_or_default()).await}
        })}))
    }).collect()
}
#[derive(Clone)]
struct Api {
    service: Arc<AgentService>,
    executor: Arc<dyn SessionExecutor>,
}
pub(crate) fn routes<S: Clone + Send + Sync + 'static>(
    service: Arc<AgentService>,
    executor: Arc<dyn SessionExecutor>,
) -> Router<S> {
    Router::new()
        .route("/api/sessions/{id}/agents", get(list).post(spawn))
        .route(
            "/api/sessions/{id}/agents/{target}/{action}",
            axum::routing::post(action),
        )
        .with_state(Api { service, executor })
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
fn fail(error: anyhow::Error) -> crate::ApiError {
    crate::ApiError::bad_request(crate::security::redact_secret_text(&error.to_string()))
}
async fn list(
    State(api): State<Api>,
    Path(id): Path<String>,
) -> Result<Json<Value>, crate::ApiError> {
    validate(&api, &id).await?;
    Ok(Json(json!({"agents":api.service.list(&id)})))
}
async fn spawn(
    State(api): State<Api>,
    Path(id): Path<String>,
    Json(input): Json<Value>,
) -> Result<Json<Value>, crate::ApiError> {
    validate(&api, &id).await?;
    Ok(Json(
        json!({"agent":api.service.spawn(id,input).await.map_err(fail)?}),
    ))
}
async fn action(
    State(api): State<Api>,
    Path((id, target, action)): Path<(String, String, String)>,
    Json(input): Json<Value>,
) -> Result<Json<Value>, crate::ApiError> {
    validate(&api, &id).await?;
    let result = match action.as_str() {
        "interrupt" => {
            api.service
                .interrupt(&id, &target, "Agent was interrupted.")
                .await
        }
        "message" => {
            api.service
                .send_message(&id, &target, input["message"].as_str().unwrap_or_default())
                .await
        }
        "followup" => {
            api.service
                .followup(&id, &target, input["message"].as_str().unwrap_or_default())
                .await
        }
        "wait" => {
            api.service
                .wait(&id, &target, input["timeoutMs"].as_u64().unwrap_or(15000))
                .await
        }
        _ => Err(anyhow::anyhow!("Unknown Agent action.")),
    };
    Ok(Json(result.map_err(fail)?))
}
