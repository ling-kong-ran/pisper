//! 工作流领域与真实 Pi 会话、图片和通知服务之间的组合边界。
use crate::{
    native_workflow::{
        image_nodes::{GeneratedImage, ImageGenerationRequest, ImageGenerator, ImageNodeService},
        media::MediaService,
        Result, WorkflowError,
    },
    session_workers::PromptRequest,
    workflow_engine::{
        AgentRequest, AgentResult, ImageNodeRequest, ImageNodeResult, MediaInputs, RunCancellation,
        WorkflowExecutor,
    },
    AppState,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use futures::future::BoxFuture;
use pi_rust::ai::types::ImageContent;
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    sync::{Arc, OnceLock, Weak},
};

struct Bindings {
    state: Weak<AppState>,
    images: Weak<ImageNodeService>,
}
pub(crate) struct WorkflowRuntimeExecutor {
    bindings: OnceLock<Bindings>,
    media: Arc<MediaService>,
}
fn api_error(error: crate::ApiError) -> WorkflowError {
    WorkflowError {
        status: error.status,
        code: error.code.into(),
        message: crate::security::redact_secret_text(&error.message),
        partial_output: None,
    }
}
impl WorkflowRuntimeExecutor {
    pub(crate) fn new(media: Arc<MediaService>) -> Arc<Self> {
        Arc::new(Self {
            bindings: OnceLock::new(),
            media,
        })
    }
    pub(crate) fn attach(
        &self,
        state: &Arc<AppState>,
        images: &Arc<ImageNodeService>,
    ) -> Result<()> {
        self.bindings
            .set(Bindings {
                state: Arc::downgrade(state),
                images: Arc::downgrade(images),
            })
            .map_err(|_| WorkflowError::busy("工作流执行器已连接。"))
    }
    fn state(&self) -> Result<Arc<AppState>> {
        self.bindings
            .get()
            .and_then(|binding| binding.state.upgrade())
            .filter(|state| !state.shutdown.is_cancelled())
            .ok_or_else(|| {
                let mut error = WorkflowError::coded(
                    "workflow_executor_closed",
                    "工作流执行器尚未就绪或正在关闭。",
                );
                error.status = StatusCode::SERVICE_UNAVAILABLE;
                error
            })
    }
}

/// 即使来源是内部媒体服务，也重新限制附件大小，防止直接执行调用绕过边界。
fn image_attachments(attachments: &[Value]) -> Result<Vec<ImageContent>> {
    if attachments.len() > 8 {
        return Err(WorkflowError::invalid("图片附件最多 8 张。"));
    }
    let mut total = 0_usize;
    attachments
        .iter()
        .map(|attachment| {
            let mime = attachment["mimeType"].as_str().unwrap_or("");
            let encoded = attachment["data"].as_str().unwrap_or("");
            if attachment["kind"] != "image"
                || !["image/png", "image/jpeg", "image/webp"].contains(&mime)
                || encoded.len() > 12 * 1024 * 1024
            {
                return Err(WorkflowError::coded(
                    "workflow_media_invalid",
                    "图片附件无效。",
                ));
            }
            let bytes = STANDARD
                .decode(encoded)
                .map_err(|_| WorkflowError::coded("workflow_media_invalid", "图片附件无效。"))?;
            total += bytes.len();
            if bytes.is_empty()
                || bytes.len() > 8 * 1024 * 1024
                || total > 20 * 1024 * 1024
                || attachment["size"].as_u64() != Some(bytes.len() as u64)
            {
                return Err(WorkflowError::coded(
                    "workflow_media_invalid",
                    "图片附件无效。",
                ));
            }
            let (width, height, actual_mime) =
                crate::native_workflow::media::raster_dimensions(&bytes).ok_or_else(|| {
                    WorkflowError::coded("workflow_media_invalid", "图片附件无效。")
                })?;
            if actual_mime != mime
                || width == 0
                || height == 0
                || width > 4096
                || height > 4096
                || u64::from(width) * u64::from(height) > 16_000_000
            {
                return Err(WorkflowError::coded(
                    "workflow_media_invalid",
                    "图片附件无效。",
                ));
            }
            Ok(ImageContent {
                data: encoded.into(),
                mime_type: mime.into(),
            })
        })
        .collect()
}

struct ActiveTools {
    session: Arc<pi_rust::coding_agent::agent_session::AgentSession>,
    previous: Vec<String>,
}
impl Drop for ActiveTools {
    fn drop(&mut self) {
        self.session.set_active_tools_by_name(self.previous.clone());
    }
}

impl WorkflowExecutor for WorkflowRuntimeExecutor {
    fn catalog(&self) -> BoxFuture<'_, Result<Value>> {
        Box::pin(async move {
            let state = self.state()?;
            let _configuration = state.engine_mutation.read().await;
            let models = crate::provider_config::available_models(&state).await.map_err(api_error)?
                .into_iter().map(|model| json!({"provider":model.provider,"model":model.id,"label":format!("{} / {}",model.provider,model.name),"id":model.id,"name":model.name})).collect::<Vec<_>>();
            let Json(skills) = crate::get_skills(State(state.clone())).await;
            let tools = state
                .runtime
                .session()
                .get_all_tools()
                .into_iter()
                .map(|tool| json!({"name":tool.name,"description":tool.description}))
                .collect::<Vec<_>>();
            let browser = state.providers.app_preferences().map_err(api_error)?["notifications"]
                ["browser"]["enabled"]
                == true;
            let image_models = crate::native_image_runtime::visual::models(&state).await?;
            Ok(
                json!({"cwd":state.cwd,"models":models,"skills":skills["skills"],"tools":tools,
                "notificationTargets":{"browser":{"enabled":browser}},"imageModels":image_models}),
            )
        })
    }
    fn prompt(&self, request: AgentRequest) -> BoxFuture<'_, Result<AgentResult>> {
        Box::pin(async move {
            let active = || {
                if request.cancellation.is_cancelled() {
                    Err(WorkflowError::cancelled())
                } else {
                    Ok(())
                }
            };
            active()?;
            let state = self.state()?;
            let images = image_attachments(&request.attachments)?;
            let session_id = if request.session_id.is_empty() {
                let (_, Json(session)) = crate::session_api::create_session(
                    State(state.clone()),
                    Some(Json(json!({"cwd":request.cwd,"name":request.title}))),
                )
                .await
                .map_err(api_error)?;
                session["id"]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| WorkflowError::io("新会话未返回 ID。"))?
                    .to_owned()
            } else {
                request.session_id.clone()
            };
            // 在第一次模型请求前公布 ID，使取消/重启日志都有确定的会话所有者。
            (request.on_session)(session_id.clone()).await;
            active()?;
            let _mode = crate::set_session_execution_mode(
                State(state.clone()),
                Path(session_id.clone()),
                Json(json!({"mode":request.execution_mode})),
            )
            .await
            .map_err(api_error)?;
            active()?;
            let lease = crate::session_runtime::mutation(&state, &session_id)
                .await
                .map_err(api_error)?;
            active()?;
            let session = lease.hosted.session();
            let available = crate::provider_config::available_models(&state)
                .await
                .map_err(api_error)?;
            active()?;
            if !request.model.is_null() {
                let provider = request.model["provider"].as_str().unwrap_or("");
                let selected = request.model["model"].as_str().unwrap_or("");
                let model = available
                    .iter()
                    .find(|model| model.provider == provider && model.id == selected)
                    .ok_or_else(|| {
                        WorkflowError::coded(
                            "model_not_available",
                            "所选工作流模型不可用或已禁用。",
                        )
                    })?;
                session.set_model(model.clone(), None).await.map_err(|_| {
                    WorkflowError::coded("model_not_available", "所选工作流模型无法启用。")
                })?;
                active()?;
            }
            if !session.model().is_some_and(|selected| {
                available
                    .iter()
                    .any(|model| model.provider == selected.provider && model.id == selected.id)
            }) {
                return Err(WorkflowError::coded(
                    "model_not_available",
                    "工作流没有已配置并启用的对话模型。",
                ));
            }
            let previous = session.get_active_tool_names();
            let mut tools = previous.clone();
            let known = session
                .get_all_tools()
                .into_iter()
                .map(|tool| tool.name)
                .collect::<HashSet<_>>();
            for name in &request.requested_tool_names {
                if !known.contains(name) {
                    return Err(WorkflowError::coded(
                        "workflow_tool_missing",
                        "工作流请求的工具尚未安装。",
                    ));
                }
                if !tools.contains(name) {
                    tools.push(name.clone());
                }
            }
            session.set_active_tools_by_name(tools);
            let _tools = ActiveTools {
                session: session.clone(),
                previous,
            };
            active()?;
            let outcome = crate::execution_adapter::run_prompt_cancellable(
                &state,
                lease.hosted.runtime.clone(),
                PromptRequest {
                    session_id: session_id.clone(),
                    text: request.message,
                    internal: false,
                    isolated: request.isolated_context,
                    ..Default::default()
                },
                state.executor.event_sink(),
                images,
                request.isolated_context,
                request.cancellation.token(),
            )
            .await;
            state.approvals.cancel_session(&session_id);
            // Even an aborted/error turn can have written real files. Capture
            // them and finish native ownership before publishing any outcome.
            let assets = state
                .asset_tracker
                .flush(&session_id, &session.session_name().unwrap_or_default())
                .await;
            lease.hosted.touch();
            let outcome = outcome.map_err(|error| {
                WorkflowError::coded(
                    "workflow_agent_error",
                    crate::security::redact_secret_text(&error.to_string()),
                )
            })?;
            let assets = assets
                .map_err(|_| WorkflowError::io("生成资产登记失败。"))?
                .iter()
                .map(crate::asset_api::projection::attachment)
                .collect();
            if outcome.aborted || request.cancellation.is_cancelled() {
                return Err(WorkflowError::cancelled());
            }
            if let Some(error) = outcome.error {
                return Err(WorkflowError::coded("workflow_agent_error", error));
            }
            Ok(AgentResult {
                text: outcome.output,
                session_id,
                assets,
            })
        })
    }
    fn abort(&self, session_id: String) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let state = self.state()?;
            crate::session_abort(State(state), Path(session_id))
                .await
                .map(|_| ())
                .map_err(api_error)
        })
    }
    fn notify(&self, event: String, data: Value, options: Value) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let state = self.state()?;
            let app = state.providers.app_preferences().map_err(api_error)?;
            state
                .notifications
                .notify(&app, &event, &data, &options)
                .await
                .map_err(api_error)?;
            Ok(())
        })
    }
    fn media_inputs(&self, inputs: Value) -> BoxFuture<'_, Result<MediaInputs>> {
        Box::pin(async move { self.media.resolve_inputs(&inputs).await })
    }
    fn image_node(&self, request: ImageNodeRequest) -> BoxFuture<'_, Result<ImageNodeResult>> {
        Box::pin(async move {
            let _state = self.state()?;
            let images = self
                .bindings
                .get()
                .and_then(|bindings| bindings.images.upgrade())
                .ok_or_else(|| WorkflowError::io("原生图片服务正在关闭。"))?;
            images.execute(request).await
        })
    }
}
impl ImageGenerator for WorkflowRuntimeExecutor {
    fn generate(
        &self,
        request: ImageGenerationRequest,
        cancellation: Arc<RunCancellation>,
    ) -> BoxFuture<'_, Result<GeneratedImage>> {
        Box::pin(async move {
            crate::native_image_runtime::visual::generate(&self.state()?, request, cancellation)
                .await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_attachments_reject_size_forgery_and_non_images() {
        assert!(image_attachments(&[
            json!({"kind":"image","mimeType":"image/png","data":"AA==","size":3})
        ])
        .is_err());
        assert!(image_attachments(&[
            json!({"kind":"file","mimeType":"text/plain","data":"AA==","size":1})
        ])
        .is_err());
        assert!(image_attachments(&vec![Value::Null; 9]).is_err());
    }
}
