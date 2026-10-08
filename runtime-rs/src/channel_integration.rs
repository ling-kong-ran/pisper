//! 渠道的 Agent 组合根：复用正式聊天/会话/审批入口，领域不持有 AppState。
use crate::{
    native_channels::{AgentPort, ChannelError, PromptRequest, PromptResult, Result},
    ApiError, AppState,
};
use axum::{
    extract::{Path, State},
    Json,
};
use futures::StreamExt;
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    sync::{Arc, OnceLock, Weak},
};

pub(crate) struct ChannelIntegration {
    state: OnceLock<Weak<AppState>>,
}
struct InputOwner(tokio_util::sync::CancellationToken);
impl Drop for InputOwner {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
struct OwnedRun {
    cancel: tokio_util::sync::CancellationToken,
    finished: Arc<std::sync::atomic::AtomicBool>,
    settled: Arc<tokio::sync::Notify>,
}
impl OwnedRun {
    async fn wait(&self) {
        loop {
            let settled = self.settled.notified();
            if self.finished.load(std::sync::atomic::Ordering::Acquire) {
                break;
            }
            settled.await;
        }
    }
}
impl Drop for OwnedRun {
    fn drop(&mut self) {
        if !self.finished.load(std::sync::atomic::Ordering::Acquire) {
            self.cancel.cancel();
        }
    }
}
impl ChannelIntegration {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: OnceLock::new(),
        })
    }
    pub(crate) fn attach(&self, state: &Arc<AppState>) -> Result<()> {
        self.state
            .set(Arc::downgrade(state))
            .map_err(|_| ChannelError::new("渠道 Agent 已连接。"))
    }
    fn state(&self) -> Result<Arc<AppState>> {
        self.state
            .get()
            .and_then(Weak::upgrade)
            .filter(|state| !state.shutdown.is_cancelled())
            .ok_or_else(|| ChannelError::new("渠道 Agent 尚未连接或正在关闭。"))
    }
    pub(crate) fn agent_port(self: &Arc<Self>) -> AgentPort {
        let prompt = self.clone();
        let directory = self.clone();
        let cwd = self.clone();
        let execution = self.clone();
        let run = self.clone();
        let approval = self.clone();
        let abort = self.clone();
        AgentPort {
            prompt: Arc::new(move |request| {
                let integration = prompt.clone();
                Box::pin(async move { integration.prompt(request).await })
            }),
            validate_directory: Arc::new(move |input| {
                let integration = directory.clone();
                Box::pin(async move {
                    let state = integration.state()?;
                    let path = if std::path::Path::new(&input).is_absolute() {
                        std::path::PathBuf::from(input)
                    } else {
                        std::path::Path::new(&state.cwd).join(input)
                    };
                    let path = std::fs::canonicalize(path)
                        .map_err(|_| ChannelError::new("工作目录不存在。"))?;
                    if !path.is_dir() {
                        return Err(ChannelError::new("工作目录必须是文件夹。"));
                    }
                    Ok(path
                        .to_string_lossy()
                        .trim_start_matches(r"\\?\")
                        .to_owned())
                })
            }),
            set_cwd: Arc::new(move |id, input| {
                let integration = cwd.clone();
                Box::pin(async move {
                    crate::set_session_cwd(
                        State(integration.state()?),
                        Path(id),
                        Json(json!({"cwd":input})),
                    )
                    .await
                    .map_err(api)?;
                    Ok(())
                })
            }),
            set_execution_mode: Arc::new(move |id, mode| {
                let integration = execution.clone();
                Box::pin(async move {
                    crate::set_session_execution_mode(
                        State(integration.state()?),
                        Path(id),
                        Json(json!({"mode":mode})),
                    )
                    .await
                    .map_err(api)?;
                    Ok(())
                })
            }),
            set_run_mode: Arc::new(move |id, mode| {
                let integration = run.clone();
                Box::pin(async move {
                    crate::set_session_run_mode(
                        State(integration.state()?),
                        Path(id),
                        Json(json!({"mode":mode})),
                    )
                    .await
                    .map_err(api)?;
                    Ok(())
                })
            }),
            resolve_approval: Arc::new(move |id, approval_id, approved| {
                let integration = approval.clone();
                Box::pin(async move {
                    Ok(integration
                        .state()?
                        .approvals
                        .resolve(&id, &approval_id, approved))
                })
            }),
            abort: Arc::new(move |id| {
                let integration = abort.clone();
                Box::pin(async move {
                    let state = integration.state()?;
                    if crate::session_api::find_session_path(&state, &id).is_err() {
                        return Ok(false);
                    }
                    let known = state.sessions.get(&id).is_some()
                        || state.goals.goals.get(&id).is_some()
                        || state.team.get(&id).is_some();
                    crate::session_abort(State(state), Path(id))
                        .await
                        .map_err(api)?;
                    Ok(known)
                })
            }),
        }
    }
    async fn prompt(&self, request: PromptRequest) -> Result<PromptResult> {
        let _input_owner = InputOwner(request.cancellation.clone());
        if request.cancellation.is_cancelled() {
            return Err(ChannelError::new("任务已停止。"));
        }
        let state = self.state()?;
        let id = if request.session_id.is_empty()
            || crate::session_api::find_session_path(&state, &request.session_id).is_err()
        {
            let (_, Json(session)) = crate::session_api::create_session(
                State(state.clone()),
                Some(Json(json!({"cwd":request.cwd,"name":request.title}))),
            )
            .await
            .map_err(api)?;
            session["id"]
                .as_str()
                .ok_or_else(|| ChannelError::new("新会话未返回 ID。"))?
                .to_owned()
        } else {
            request.session_id.clone()
        };
        if !request.execution_mode.is_empty() {
            let _ = crate::set_session_execution_mode(
                State(state.clone()),
                Path(id.clone()),
                Json(json!({"mode":request.execution_mode})),
            )
            .await
            .map_err(api)?;
        }
        if request.model["provider"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
            && request.model["model"]
                .as_str()
                .is_some_and(|value| !value.is_empty())
        {
            let current = crate::session_api::model_state(&state, &id).map_err(api)?;
            if current["provider"] != request.model["provider"]
                || current["id"] != request.model["model"]
            {
                crate::post_session_model(
                    State(state.clone()),
                    Path(id.clone()),
                    Json(request.model.clone()),
                )
                .await
                .map_err(api)?;
            }
        }
        if request.cancellation.is_cancelled() {
            return Err(ChannelError::new("任务已停止。"));
        }
        let response=crate::chat_stream::chat_owned(state.clone(),json!({"sessionId":id,"message":request.message,
            "attachments":request.attachments,"goalMode":request.goal_mode,"teamMode":request.team_mode}),Some(request.cancellation.clone())).await.map_err(api)?;
        let run_id = response
            .headers()
            .get("x-pisper-run-id")
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| ChannelError::new("渠道会话未返回运行标识。"))?;
        let owned = {
            let runs = state
                .chat_runs
                .lock()
                .map_err(|_| ChannelError::new("会话运行锁无效。"))?;
            let run = runs
                .get(run_id)
                .ok_or_else(|| ChannelError::new("渠道会话运行不存在。"))?;
            OwnedRun {
                cancel: run.cancel.clone(),
                finished: run.finished.clone(),
                settled: run.settled.clone(),
            }
        };
        let mut bytes = response.into_body().into_data_stream();
        let mut pending = Vec::new();
        let mut text = String::new();
        let mut asset_seen = HashSet::new();
        let mut assets = Vec::new();
        let mut terminal_seen = false;
        let mut actual_id = id.clone();
        let collected: Result<()> = async {
            while let Some(chunk) = tokio::select! {biased;
                _=request.cancellation.cancelled()=>{
                    return Err(ChannelError::new("任务已停止。"));
                },value=bytes.next()=>value
            } {
                pending.extend_from_slice(
                    &chunk.map_err(|error| ChannelError::new(error.to_string()))?,
                );
                while let Some(end) = pending.windows(2).position(|value| value == b"\n\n") {
                    let frame = pending.drain(..end + 2).collect::<Vec<_>>();
                    if let Some((event, data)) = decode_frame(&frame)? {
                        if matches!(event.as_str(), "meta" | "done") {
                            if let Some(id) = data["sessionId"].as_str() {
                                actual_id = id.to_owned();
                            }
                        }
                        if event == "text_delta" {
                            text.push_str(data["delta"].as_str().unwrap_or(""));
                        }
                        if event == "generated_asset" {
                            if let Some(id) = data["id"].as_str() {
                                if asset_seen.insert(id.to_owned()) {
                                    assets.push(id.to_owned());
                                }
                            }
                        }
                        if event == "resync_required" {
                            return Err(ChannelError::new(
                                "渠道会话事件流需要重新同步，回复未作为完成结果发送。",
                            ));
                        }
                        if matches!(event.as_str(), "done" | "error") {
                            terminal_seen = true;
                        }
                        (request.on_event)(event, data);
                    }
                }
            }
            if !terminal_seen {
                return Err(ChannelError::new(
                    "渠道会话响应提前结束，回复未作为完成结果发送。",
                ));
            }
            Ok(())
        }
        .await;
        if collected.is_err() {
            owned.cancel.cancel();
        }
        owned.wait().await;
        collected?;
        if request.cancellation.is_cancelled() {
            return Err(ChannelError::new("任务已停止。"));
        }
        if text.trim().is_empty() {
            let Json(messages) = crate::session_api::get_messages(
                State(state.clone()),
                Path(actual_id.clone()),
                axum::extract::Query(Default::default()),
            )
            .await
            .map_err(api)?;
            if let Some(messages) = messages
                .as_array()
                .or_else(|| messages["messages"].as_array())
            {
                text = messages
                    .iter()
                    .rev()
                    .find(|message| message["role"] == "agent")
                    .and_then(|message| message["text"].as_str())
                    .unwrap_or("")
                    .to_owned();
            }
        }
        let output_assets = {
            let store = state
                .assets
                .lock()
                .map_err(|error| ChannelError::new(error.to_string()))?;
            assets
                .into_iter()
                .filter_map(|id| store.find(&id))
                .filter_map(reply_asset)
                .collect()
        };
        let hosted = state.sessions.get(&actual_id);
        let cwd = hosted
            .as_ref()
            .map(|host| host.runtime.cwd())
            .unwrap_or_else(|| state.cwd.clone());
        let model = hosted
            .as_ref()
            .and_then(|host| host.session().model())
            .map(|model| format!("{}/{}", model.provider, model.id))
            .unwrap_or_default();
        Ok(PromptResult {
            session_id: actual_id,
            cwd,
            model,
            text: text.trim().to_owned(),
            assets: output_assets,
        })
    }
}
fn api(error: ApiError) -> ChannelError {
    ChannelError {
        message: error.message,
        status: Some(error.status.as_u16()),
    }
}
fn reply_asset(asset: &Value) -> Option<Value> {
    // 生成资产保留原工作区 filePath，导入资产使用归档 storagePath；两者都必须发送。
    let path = asset["storagePath"]
        .as_str()
        .filter(|path| !path.is_empty())
        .or_else(|| asset["filePath"].as_str().filter(|path| !path.is_empty()))?;
    Some(json!({"id":asset["id"],"name":asset["name"],"path":path,"mimeType":asset["mimeType"]}))
}
fn decode_frame(bytes: &[u8]) -> Result<Option<(String, Value)>> {
    let text =
        std::str::from_utf8(bytes).map_err(|_| ChannelError::new("渠道会话响应不是 UTF-8。"))?;
    let mut name = None;
    let mut data = Vec::new();
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("event:") {
            name = Some(value.trim().to_owned());
        } else if let Some(value) = line.strip_prefix("data:") {
            data.push(value.trim_start());
        }
    }
    if let Some(name) = name {
        if !data.is_empty() {
            return Ok(Some((
                name,
                serde_json::from_str(&data.join("\n"))
                    .map_err(|error| ChannelError::new(error.to_string()))?,
            )));
        }
    }
    Ok(None)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn channel_assets_include_archived_files_and_generated_originals() {
        assert_eq!(reply_asset(&json!({"id":"generated","name":"sprite.png","filePath":"/owned/generated/visuals/sprite.png","mimeType":"image/png"})).unwrap()["path"],"/owned/generated/visuals/sprite.png");
        assert_eq!(
            reply_asset(
                &json!({"storagePath":"/owned/assets/copied.png","filePath":"/owned/original.png"})
            )
            .unwrap()["path"],
            "/owned/assets/copied.png"
        );
        assert!(reply_asset(&json!({"storagePath":"","filePath":""})).is_none());
    }
    #[test]
    fn actual_sse_fields_decode_without_losing_unicode_or_multiline_data() {
        let frame = b"id: 4\nevent: text_delta\ndata: {\"delta\":\"\\u4e2d\"}\n\n";
        assert_eq!(
            decode_frame(frame).unwrap(),
            Some(("text_delta".into(), json!({"delta":"中"})))
        );
        assert!(decode_frame(b": keep-alive\n\n").unwrap().is_none());
        assert!(decode_frame(b"event: done\ndata: invalid\n\n").is_err());
    }
}
