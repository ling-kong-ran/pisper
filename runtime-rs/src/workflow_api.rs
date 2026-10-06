//! 工作流 HTTP 响应沿用 release facade 的封装，不让客户端读取内部执行器。
use crate::native_workflow::{
    bundle::{self, EngineBundleStore},
    image_nodes::ImageNodeService,
    inputs,
    media::MediaService,
};
use crate::native_workflow::{Result, WorkflowError};
use crate::workflow_engine::WorkflowService;
use axum::{
    body::{to_bytes, Body},
    extract::{Path, Query, Request, State},
    http::StatusCode,
    response::Response,
    routing::{get, patch, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Clone)]
struct MediaApi {
    workflows: Arc<WorkflowService>,
    media: Arc<MediaService>,
    images: Arc<ImageNodeService>,
    engines: Option<Arc<dyn EngineBundleStore>>,
}
pub(crate) fn media_routes<S: Clone + Send + Sync + 'static>(
    workflows: Arc<WorkflowService>,
    media: Arc<MediaService>,
    images: Arc<ImageNodeService>,
    engines: Option<Arc<dyn EngineBundleStore>>,
) -> Router<S> {
    Router::new()
        .route("/api/workflow-media", post(upload_media))
        .route("/api/workflow-media/{id}/content", get(media_content))
        .route("/api/workflow-image-process", post(process_image))
        .route("/api/workflow-image-models", get(image_models))
        .route("/api/workflows/{id}/bundle", get(export_bundle))
        .route("/api/workflows/import-bundle", post(import_bundle))
        .with_state(MediaApi {
            workflows,
            media,
            images,
            engines,
        })
}
fn media_failure(mut failure: WorkflowError) -> WorkflowError {
    if !failure.code.starts_with("workflow_media_") && !failure.code.starts_with("workflow_input_")
    {
        failure.status = StatusCode::BAD_REQUEST;
        failure.code = "workflow_media_invalid".into();
    }
    failure.message = "工作流素材上传或读取失败。".into();
    failure
}
async fn upload_media(
    State(api): State<MediaApi>,
    Query(query): Query<std::collections::HashMap<String, String>>,
    request: Request,
) -> Result<(StatusCode, Json<Value>)> {
    let mime = request
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_lowercase();
    let maximum = if mime.starts_with("image/") { 8 } else { 64 } * 1024 * 1024;
    let buffer = to_bytes(request.into_body(), maximum).await.map_err(|_| {
        let mut error =
            WorkflowError::coded("workflow_media_too_large", "工作流素材上传或读取失败。");
        error.status = StatusCode::PAYLOAD_TOO_LARGE;
        error
    })?;
    let media = api
        .media
        .upload(
            query.get("name").map(String::as_str).unwrap_or("media"),
            &mime,
            &buffer,
        )
        .await
        .map_err(media_failure)?;
    Ok((StatusCode::CREATED, Json(media)))
}
async fn media_content(State(api): State<MediaApi>, Path(id): Path<String>) -> Result<Response> {
    let media = api.media.read(&id).await.map_err(media_failure)?;
    Response::builder()
        .status(200)
        .header(
            "Content-Type",
            media.metadata["media"]["mimeType"]
                .as_str()
                .unwrap_or("application/octet-stream"),
        )
        .header("Content-Length", media.buffer.len())
        .header("Cache-Control", "private, max-age=60")
        .header("X-Content-Type-Options", "nosniff")
        .body(Body::from(media.buffer))
        .map_err(WorkflowError::io)
}
async fn image_models(State(api): State<MediaApi>) -> Result<Json<Value>> {
    let catalog = api.workflows.executor().catalog().await?;
    Ok(Json(
        json!({"models":catalog.get("imageModels").cloned().unwrap_or(json!([]))}),
    ))
}
struct CancelOnDrop(Arc<crate::workflow_engine::RunCancellation>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
async fn process_image(
    State(api): State<MediaApi>,
    Json(input): Json<Value>,
) -> Result<Json<Value>> {
    let operation = input["operation"]
        .as_str()
        .filter(|operation| ["background", "inpaint"].contains(operation))
        .ok_or_else(crate::native_workflow::image_protocol::invalid)?;
    let reference = inputs::media(&input["reference"])?;
    let image = crate::native_workflow::image_protocol::settings(input.get("image"))?;
    let cancellation = Arc::new(crate::workflow_engine::RunCancellation::default());
    let _owner = CancelOnDrop(cancellation.clone());
    let run_id = crate::native_workflow::id()?;
    let request = |node, inputs, predecessors| crate::workflow_engine::ImageNodeRequest {
        node,
        inputs,
        predecessors,
        workflow_id: run_id.clone(),
        run_id: run_id.clone(),
        cwd: String::new(),
        cancellation: cancellation.clone(),
        resume_output: None,
    };
    let source = api
        .images
        .execute(request(
            json!({"id":"input","kind":"media-input","image":{"inputName":"reference"}}),
            json!({"reference":reference}),
            vec![],
        ))
        .await?;
    let output = api
        .images
        .execute(request(
            json!({"id":"process","kind":format!("media-{operation}"),"image":image}),
            json!({"reference":reference}),
            vec![json!({"output":source.output})],
        ))
        .await?;
    Ok(Json(output.output["frames"][0]["media"].clone()))
}
async fn export_bundle(State(api): State<MediaApi>, Path(id): Path<String>) -> Result<Response> {
    let workflow = api
        .workflows
        .export(&id)
        .await
        .ok_or_else(|| missing("工作流不存在。"))?;
    let bytes = bundle::export(workflow, &api.media, api.engines.as_deref()).await?;
    Response::builder()
        .status(200)
        .header("Content-Type", "application/zip")
        .header("Content-Length", bytes.len())
        .header("Cache-Control", "no-store")
        .body(Body::from(bytes))
        .map_err(WorkflowError::io)
}
async fn import_bundle(
    State(api): State<MediaApi>,
    request: Request,
) -> Result<(StatusCode, Json<Value>)> {
    let bytes = to_bytes(request.into_body(), bundle::MAX_BUNDLE_BYTES)
        .await
        .map_err(|_| bundle::invalid())?;
    let imported = bundle::import(&bytes, &api.media, api.engines.as_deref()).await?;
    let workflow = match api.workflows.import(imported.definition).await {
        Ok(workflow) => workflow,
        Err(error) => {
            api.media.discard_imported(imported.media).await?;
            return Err(error);
        }
    };
    api.media.commit_imported(imported.media).await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"workflow":workflow,"requirements":imported.requirements})),
    ))
}

pub(crate) fn routes<S: Clone + Send + Sync + 'static>(service: Arc<WorkflowService>) -> Router<S> {
    Router::new()
        .route("/api/workflows", get(get_workflows).post(create_workflow))
        .route(
            "/api/workflows/{id}",
            patch(update_workflow).delete(delete_workflow),
        )
        .route("/api/workflows/{id}/duplicate", post(duplicate_workflow))
        .route("/api/workflows/{id}/run", post(run_workflow))
        .route("/api/workflows/{id}/nodes/{node}/run", post(run_node))
        .route("/api/workflow-runs/{id}", get(get_run))
        .route("/api/workflow-runs/{id}/retry", post(retry_run))
        .route("/api/workflow-runs/{id}/stop", post(stop_run))
        .route("/api/sessions/{id}/workflow-runs", get(session_runs))
        .route(
            "/api/workflow-runs/{id}/approvals/{node}",
            post(resolve_approval),
        )
        .with_state(service)
}
async fn session_runs(
    State(service): State<Arc<WorkflowService>>,
    Path(id): Path<String>,
) -> Json<Value> {
    Json(json!({"runs":service.state(Some(&id)).await["runs"]}))
}
pub(crate) async fn dashboard(service: &WorkflowService) -> Result<Value> {
    let mut state = service.state(None).await;
    let catalog = service.executor().catalog().await?;
    for key in ["skills", "models", "notificationTargets"] {
        state[key] = catalog[key].clone();
    }
    state["cwd"] = catalog.get("cwd").cloned().unwrap_or(Value::Null);
    Ok(state)
}
fn missing(message: &str) -> WorkflowError {
    WorkflowError {
        status: StatusCode::NOT_FOUND,
        code: "workflow_not_found".into(),
        message: message.into(),
        partial_output: None,
    }
}
async fn filter_targets(service: &WorkflowService, mut input: Value) -> Result<Value> {
    let catalog = service.executor().catalog().await?;
    let enabled = |target: &Value| {
        target
            .as_str()
            .is_some_and(|target| catalog["notificationTargets"][target]["enabled"] == true)
    };
    if let Some(targets) = input.get_mut("notifications").and_then(Value::as_array_mut) {
        targets.retain(enabled);
    }
    if let Some(nodes) = input.get_mut("nodes").and_then(Value::as_array_mut) {
        for node in nodes {
            if let Some(targets) = node
                .get_mut("notificationTargets")
                .and_then(Value::as_array_mut)
            {
                targets.retain(enabled);
            }
        }
    }
    Ok(input)
}
async fn get_workflows(State(service): State<Arc<WorkflowService>>) -> Result<Json<Value>> {
    Ok(Json(dashboard(&service).await?))
}
async fn create_workflow(
    State(service): State<Arc<WorkflowService>>,
    Json(input): Json<Value>,
) -> Result<(StatusCode, Json<Value>)> {
    let workflow = service
        .create(filter_targets(&service, input).await?)
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"workflow":workflow,"state":dashboard(&service).await?})),
    ))
}
async fn update_workflow(
    State(service): State<Arc<WorkflowService>>,
    Path(id): Path<String>,
    Json(input): Json<Value>,
) -> Result<Json<Value>> {
    let workflow = service
        .update(&id, filter_targets(&service, input).await?)
        .await?
        .ok_or_else(|| missing("工作流不存在。"))?;
    Ok(Json(
        json!({"workflow":workflow,"state":dashboard(&service).await?}),
    ))
}
async fn delete_workflow(
    State(service): State<Arc<WorkflowService>>,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    if !service.remove(&id).await? {
        return Err(missing("工作流不存在。"));
    }
    Ok(Json(json!({"deleted":true})))
}
async fn duplicate_workflow(
    State(service): State<Arc<WorkflowService>>,
    Path(id): Path<String>,
    Json(input): Json<Value>,
) -> Result<(StatusCode, Json<Value>)> {
    let workflow = service
        .duplicate(&id, input)
        .await?
        .ok_or_else(|| missing("工作流不存在。"))?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"workflow":workflow,"state":dashboard(&service).await?})),
    ))
}
async fn run_workflow(
    State(service): State<Arc<WorkflowService>>,
    Path(id): Path<String>,
    input: Option<Json<Value>>,
) -> Result<(StatusCode, Json<Value>)> {
    let run = service
        .run(&id, input.map(|v| v.0).unwrap_or(json!({})))
        .await?
        .ok_or_else(|| missing("工作流不存在。"))?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"started":true,"run":run})),
    ))
}
async fn run_node(
    State(service): State<Arc<WorkflowService>>,
    Path((id, node)): Path<(String, String)>,
    Json(input): Json<Value>,
) -> Result<(StatusCode, Json<Value>)> {
    let source = input["sourceRunId"]
        .as_str()
        .filter(|s| s.len() <= 80)
        .ok_or_else(|| {
            WorkflowError::coded("workflow_image_source_stale", "workflow_image_source_stale")
        })?;
    let run = service
        .run(&id, json!({"nodeId":node,"sourceRunId":source}))
        .await?
        .ok_or_else(|| missing("工作流不存在。"))?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"started":true,"run":run})),
    ))
}
async fn get_run(
    State(service): State<Arc<WorkflowService>>,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    Ok(Json(
        service
            .get_run(&id)
            .await
            .ok_or_else(|| missing("工作流运行不存在。"))?,
    ))
}
async fn retry_run(
    State(service): State<Arc<WorkflowService>>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<Value>)> {
    let run = service
        .retry(&id)
        .await?
        .ok_or_else(|| missing("工作流运行不存在或不能重试。"))?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"started":true,"run":run})),
    ))
}
async fn stop_run(
    State(service): State<Arc<WorkflowService>>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<Value>)> {
    let run = service
        .stop(&id)
        .await?
        .ok_or_else(|| missing("工作流运行不存在或已经结束。"))?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"stopping":true,"run":run})),
    ))
}
async fn resolve_approval(
    State(service): State<Arc<WorkflowService>>,
    Path((id, node)): Path<(String, String)>,
    Json(input): Json<Value>,
) -> Result<Json<Value>> {
    let approved = input["approved"].as_bool().unwrap_or(false);
    let comment = input["comment"].as_str().unwrap_or("").to_owned();
    let run = service
        .approval(&id, &node, approved, comment)
        .await?
        .ok_or_else(|| missing("待审批节点不存在或已经处理。"))?;
    Ok(Json(json!({"resolved":true,"run":run})))
}
