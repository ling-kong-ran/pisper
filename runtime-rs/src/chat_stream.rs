use std::{
    collections::VecDeque,
    convert::Infallible,
    sync::{Arc, Mutex},
};

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive},
        IntoResponse, Sse,
    },
    Json,
};
use pi_rust::coding_agent::{
    agent_session::{AgentSessionEvent, PromptOptions},
    modes::json_event::to_json_event_string,
};
use serde_json::{json, Value};

use crate::{product, security, ApiError, AppState};

#[derive(Default)]
pub(crate) struct Projection {
    thinking: String,
    text: String,
    tools: Vec<Value>,
    failure: Option<String>,
    aborted: bool,
}

fn text_content(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|item| {
                (item.get("type").and_then(Value::as_str) == Some("text"))
                    .then(|| item.get("text").and_then(Value::as_str))
                    .flatten()
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn record(state: &AppState, run_id: &str, event: &str, mut data: Value) {
    if let Some(data) = data.as_object_mut() {
        data.insert(
            "eventAt".into(),
            json!(
                pi_rust::agent_core::harness::session::jsonl::iso8601::format_iso8601_utc(
                    product::now_ms() as i64
                )
            ),
        );
    }
    let cursor = state
        .frame_cursor
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        + 1;
    if let Some(run) = state.chat_runs.lock().expect("chat runs lock").get(run_id) {
        run.record(cursor, event, data);
    }
}

pub(crate) fn project(
    raw: &Value,
    session_id: &str,
    projection: &mut Projection,
) -> Option<(String, Value)> {
    let kind = raw.get("type").and_then(Value::as_str)?;
    if kind == "agent_end" {
        if let Some(message) = raw
            .get("messages")
            .and_then(Value::as_array)
            .and_then(|messages| {
                messages.iter().rev().find(|message| {
                    message.get("role").and_then(Value::as_str) == Some("assistant")
                })
            })
        {
            let text = text_content(&message["content"]);
            if !text.is_empty() {
                projection.text = text;
            }
            if message.get("stopReason").and_then(Value::as_str) == Some("error") {
                projection.failure = Some(
                    message
                        .get("errorMessage")
                        .and_then(Value::as_str)
                        .unwrap_or("Model request failed")
                        .to_string(),
                );
            }
            projection.aborted = message["stopReason"] == "aborted";
        }
        // agent_end 可以先于 prompt 返回和存盘；终态只在后台任务真正收尾后发送。
        return None;
    }
    let (name, mut data) = product::map_pi_event(raw, session_id, &mut projection.thinking)?;
    if name == "text_delta" {
        projection.text.push_str(
            data.get("delta")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        );
    } else if name == "tool_start" {
        let mut tool = data.clone();
        tool["type"] = json!("tool");
        tool["status"] = json!("running");
        projection.tools.push(tool);
    } else if name == "tool_update" || name == "tool_end" {
        let id = data.get("id").cloned().unwrap_or(Value::Null);
        let result = raw
            .get("result")
            .or_else(|| raw.get("partialResult"))
            .unwrap_or(&Value::Null);
        data["output"] = json!(text_content(&result["content"]));
        if name == "tool_end" {
            data["error"] = json!(raw.get("isError").and_then(Value::as_bool).unwrap_or(false));
            data["finishedAt"] = json!(product::now_ms());
            data["status"] = if data["error"] == true {
                json!("error")
            } else {
                json!("done")
            };
        }
        if let Some(tool) = projection
            .tools
            .iter_mut()
            .find(|tool| tool.get("id") == Some(&id))
        {
            if let Some(object) = data.as_object() {
                for (key, value) in object {
                    tool[key] = value.clone();
                }
            }
        }
    }
    Some((name, data))
}

fn event(frame: product::Frame) -> Result<Event, Infallible> {
    Ok(Event::default()
        .event(frame.event)
        .id(frame.cursor.to_string())
        .data(frame.data.to_string()))
}

fn response(
    frames: Vec<product::Frame>,
    rx: tokio::sync::broadcast::Receiver<product::Frame>,
    after: u64,
    closed: bool,
) -> axum::response::Response {
    let pending: VecDeque<_> = frames
        .into_iter()
        .filter(|frame| frame.cursor > after)
        .collect();
    let stream = futures::stream::unfold(
        (pending, rx, after, false),
        move |(mut pending, mut rx, mut cursor, terminal)| async move {
            if terminal {
                return None;
            }
            loop {
                let frame = if let Some(frame) = pending.pop_front() {
                    frame
                } else {
                    if closed {
                        return None;
                    }
                    match rx.recv().await {
                        Ok(frame) => frame,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            let frame = product::Frame {
                                cursor: cursor + 1,
                                event: "resync_required".into(),
                                data: json!({"reason": "stream_buffer_overflow"}),
                            };
                            return Some((event(frame), (pending, rx, cursor, true)));
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
                    }
                };
                if frame.cursor <= cursor {
                    continue;
                }
                cursor = frame.cursor;
                let terminal = frame.event == "done" || frame.event == "error";
                return Some((event(frame), (pending, rx, cursor, terminal)));
            }
        },
    );
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

pub(crate) async fn chat(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<axum::response::Response, ApiError> {
    chat_owned(state, body, None).await
}

pub(crate) async fn chat_owned(
    state: Arc<AppState>,
    body: Value,
    owner_cancel: Option<tokio_util::sync::CancellationToken>,
) -> Result<axum::response::Response, ApiError> {
    let owner_cancel = owner_cancel.unwrap_or_default();
    if owner_cancel.is_cancelled() {
        return Err(ApiError::bad_request("任务已停止。"));
    }
    let requested_mode = if body["teamMode"].as_bool().unwrap_or(false) {
        Some("team")
    } else if body["goalMode"].as_bool().unwrap_or(false) {
        Some("goal")
    } else {
        None
    };
    let session_id = body
        .get("sessionId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| ApiError::bad_request("missing sessionId"))?
        .to_string();
    let mut message = body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let raw_user_message = message.clone();
    let internal = raw_user_message.starts_with(crate::goal_api::CONTINUATION_MARKER)
        || raw_user_message.starts_with(crate::multi_agent_api::COMPLETION_MARKER);
    let isolated_context = body["isolatedContext"].as_bool().unwrap_or(false);
    let attachments = body["attachments"].as_array().cloned().unwrap_or_default();
    if message.trim().is_empty() && attachments.is_empty() {
        return Err(ApiError::bad_request("消息不能为空。"));
    }
    let hosted = crate::session_runtime::hosted(&state, &session_id).await?;
    let run_guard = hosted.run.clone().try_lock_owned().map_err(|_| {
        ApiError::new(
            StatusCode::CONFLICT,
            "session_busy",
            "当前会话正在运行，请等待结束或停止运行。",
        )
    })?;
    let guard = hosted.mutation.clone().try_lock_owned().map_err(|_| {
        ApiError::new(
            StatusCode::CONFLICT,
            "session_busy",
            "当前会话正在运行，请等待结束或停止运行。",
        )
    })?;
    let configuration = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        state.engine_mutation.clone().read_owned(),
    )
    .await
    .map_err(|_| {
        ApiError::new(
            StatusCode::CONFLICT,
            "configuration_busy",
            "模型配置正在更新，请稍后重试。",
        )
    })?;
    let session = hosted.session();
    hosted.ensure_present()?;
    let prepared = crate::asset_api::attachments::prepare(
        state.assets.clone(),
        session_id.clone(),
        String::new(),
        attachments,
        owner_cancel.clone(),
    )
    .await
    .map_err(|error| ApiError::bad_request(security::redact_secret_text(&error.to_string())))?;
    if owner_cancel.is_cancelled() {
        return Err(ApiError::bad_request("任务已停止。"));
    }
    let images = prepared.images;
    if !prepared.contexts.is_empty() {
        message.push_str(&format!(
            "{}{}",
            crate::session_api::ATTACHMENT_MARKER,
            prepared.contexts.join("\n\n")
        ));
    }
    let memory_context = state
        .memory_tasks
        .context_for(
            &raw_user_message,
            std::path::Path::new(&hosted.runtime.cwd()),
            &session.get_active_tool_names(),
            isolated_context || internal,
        )
        .map_err(|error| ApiError::internal(error.to_string()))?;
    if !memory_context.is_empty() {
        message = format!("{memory_context}\n\n{message}");
    }
    if !isolated_context && !internal {
        state
            .memory_tasks
            .set_user_message(&session_id, &raw_user_message);
    }
    let selected = session.model().ok_or_else(|| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "model_not_configured",
            "请先连接模型并选择默认模型。",
        )
    })?;
    if !crate::provider_config::available_models(&state)
        .await?
        .iter()
        .any(|model| model.provider == selected.provider && model.id == selected.id)
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "model_not_available",
            "当前模型已禁用或没有配置凭据，请重新选择可用模型。",
        ));
    }
    let mut goal = state.goals.goals.get(&session_id);
    if let Some(mode) = requested_mode {
        let budget = &body["goalTokenBudget"];
        let next = if goal.as_ref().is_some_and(|goal| goal.status == "paused") {
            let mut options = json!({"mode":mode});
            if !budget.is_null() {
                options["tokenBudget"] = budget.clone();
            }
            state.goals.goals.resume(&session_id, &options)
        } else if !goal
            .as_ref()
            .is_some_and(|goal| goal.status == "active" && goal.mode == mode)
        {
            state
                .goals
                .goals
                .start(&session_id, &raw_user_message, budget, mode)
        } else {
            Ok(goal.clone().expect("active goal"))
        };
        goal = Some(next.map_err(|error| ApiError::bad_request(error.to_string()))?);
    } else if !internal && goal.as_ref().is_some_and(|goal| goal.status == "active") {
        goal = state
            .goals
            .pause(&session_id)
            .await
            .map_err(|error| ApiError::internal(error.to_string()))?;
    }
    let driven_goal = goal.as_ref().is_some_and(|goal| goal.status == "active");
    if driven_goal && goal.as_ref().is_some_and(|goal| goal.mode == "team") {
        state
            .team
            .ensure(&session_id)
            .map_err(|error| ApiError::bad_request(error.to_string()))?;
    } else if state
        .team
        .get(&session_id)
        .is_some_and(|team| team["status"] == "active")
    {
        state
            .team
            .stop_state(&session_id, "Team mode was paused.")
            .map_err(|error| ApiError::internal(error.to_string()))?;
        state
            .agents
            .abort_parent(&session_id, "Team mode was paused.")
            .await
            .map_err(|error| ApiError::internal(error.to_string()))?;
    }
    if !driven_goal && !internal {
        let plan = state
            .plans
            .replace(&session_id, &json!([]), "replace")
            .map_err(|error| ApiError::internal(error.to_string()))?;
        (state.executor.event_sink())("plan_update", &json!({"sessionId":session_id,"plan":plan}));
    }
    state.agents.resume_notifications(&session_id);
    if owner_cancel.is_cancelled() {
        return Err(ApiError::bad_request("任务已停止。"));
    }
    let run_id = product::new_id();
    let (tx, _) = tokio::sync::broadcast::channel::<product::Frame>(4096);
    state.chat_runs.lock().expect("chat runs lock").insert(
        run_id.clone(),
        product::ChatRun {
            frames: Mutex::new(Vec::new()),
            tx,
            closed: std::sync::atomic::AtomicBool::new(false),
            cancel: owner_cancel,
            finished: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            settled: Arc::new(tokio::sync::Notify::new()),
        },
    );
    record(
        &state,
        &run_id,
        "run",
        json!({"runId":run_id,"kind":"chat","sessionId":session_id}),
    );
    let mut meta = crate::session_api::snapshot(&state, &session_id)?;
    meta["streaming"] = json!(true);
    meta["configurationBusy"] = json!(true);
    meta["startedAt"] = json!(
        pi_rust::agent_core::harness::session::jsonl::iso8601::format_iso8601_utc(
            product::now_ms() as i64
        )
    );
    record(&state, &run_id, "meta", meta);
    let projection = Arc::new(Mutex::new(Projection::default()));
    let mapper_state = state.clone();
    let mapper_run = run_id.clone();
    let mapper_session = session_id.clone();
    let mapper_projection = projection.clone();
    let listener = Arc::new(move |event: &AgentSessionEvent| {
        if let Ok(raw) = to_json_event_string(event).and_then(|text| {
            serde_json::from_str::<Value>(&text).map_err(|error| error.to_string())
        }) {
            let mapped = project(
                &raw,
                &mapper_session,
                &mut mapper_projection.lock().expect("projection lock"),
            );
            if let Some((name, data)) = mapped {
                record(&mapper_state, &mapper_run, &name, data);
            }
        }
    });
    let unsub = session.subscribe(listener);
    let approval_state = state.clone();
    let approval_run = run_id.clone();
    let approval_unsub = state.approvals.subscribe(
        Some(&session_id),
        Arc::new(move |_, event, data| {
            record(&approval_state, &approval_run, event, data.clone());
        }),
    );
    let (frames, rx) = {
        let runs = state.chat_runs.lock().expect("chat runs lock");
        let run = &runs[&run_id];
        let frames = run.frames.lock().expect("frames lock").clone();
        (frames, run.tx.subscribe())
    };
    let task_state = state.clone();
    let (run_cancel, run_finished, run_settled) = {
        let runs = state.chat_runs.lock().expect("chat runs lock");
        let run = &runs[&run_id];
        (
            run.cancel.clone(),
            run.finished.clone(),
            run.settled.clone(),
        )
    };
    let response_run_id = run_id.clone();
    let mut asset_events = state.events.subscribe();
    tokio::spawn(async move {
        struct Settled(
            Arc<std::sync::atomic::AtomicBool>,
            Arc<tokio::sync::Notify>,
            Option<tokio::sync::OwnedMutexGuard<()>>,
        );
        impl Drop for Settled {
            fn drop(&mut self) {
                drop(self.2.take());
                self.0.store(true, std::sync::atomic::Ordering::Release);
                self.1.notify_waiters();
            }
        }
        let _settled = Settled(run_finished, run_settled, Some(run_guard));
        let mut guard = Some(guard);
        let mut configuration = Some(configuration);
        if driven_goal {
            drop(guard.take());
            drop(configuration.take());
        }
        let prompt = async {
            if run_cancel.is_cancelled() {
                return Err(anyhow::anyhow!("This operation was aborted"));
            }
            if driven_goal {
                let outcome = task_state
                    .goals
                    .run_prepared(
                        session_id.clone(),
                        crate::session_workers::PromptRequest {
                            session_id: session_id.clone(),
                            text: message,
                            images,
                            internal,
                            isolated: isolated_context,
                            context_prepared: true,
                        },
                        Arc::new(|_, _| {}),
                    )
                    .await?;
                if outcome.aborted {
                    projection.lock().expect("projection lock").aborted = true;
                }
                if let Some(error) = outcome.error {
                    return Err(anyhow::anyhow!(error));
                }
                Ok(())
            } else {
                session
                    .prompt(
                        message,
                        Some(PromptOptions {
                            images: (!images.is_empty()).then_some(images),
                            start_cancellation: Some(run_cancel.clone()),
                            ..Default::default()
                        }),
                    )
                    .await
                    .map_err(|error| anyhow::anyhow!(error.to_string()))
            }
        };
        tokio::pin!(prompt);
        let result = loop {
            tokio::select! {
                _=run_cancel.cancelled()=>{
                    // abort 等待 prompt 到达 idle；必须同时继续轮询原 prompt，
                    // 不能先 await abort 再丢弃仍持有收尾状态的 Future。
                    if driven_goal {
                        let _ = tokio::join!(task_state.goals.cancel(&session_id), &mut prompt);
                    } else {
                        let _ = tokio::join!(session.abort(), &mut prompt);
                    }
                    projection.lock().expect("projection lock").aborted = true;
                    break Err(anyhow::anyhow!("This operation was aborted"));
                },
                result = &mut prompt => break result,
                event = asset_events.recv() => {
                    if let Ok(event) = event {
                        record_asset_event(&task_state, &run_id, &session_id, &event);
                    }
                }
            }
        };
        while let Ok(event) = asset_events.try_recv() {
            record_asset_event(&task_state, &run_id, &session_id, &event);
        }
        match task_state
            .asset_tracker
            .flush(&session_id, &session.session_name().unwrap_or_default())
            .await
        {
            Ok(assets) => {
                for asset in assets {
                    let data = crate::asset_api::projection::attachment(&asset);
                    record(&task_state, &run_id, "generated_asset", data.clone());
                    let _ = task_state.events.send(
                        json!({"pisperEvent":"generated_asset","sessionId":session_id,"data":data})
                            .to_string(),
                    );
                }
            }
            Err(error) => {
                tracing::warn!(error=%security::redact_secret_text(&error.to_string()), "Workspace assets remain pending")
            }
        }
        task_state.approvals.cancel_session(&session_id);
        drop(approval_unsub);
        unsub.unsubscribe();
        let mut terminal = crate::session_api::snapshot(&task_state, &session_id)
            .unwrap_or_else(|_| json!({"sessionId":session_id}));
        let (text, tools, projected_failure, aborted) = {
            let projection = projection.lock().expect("projection lock");
            (
                projection.text.clone(),
                projection.tools.clone(),
                projection.failure.clone(),
                projection.aborted,
            )
        };
        let failure = result
            .err()
            .map(|error| error.to_string())
            .or(projected_failure);
        terminal["text"] = json!(text);
        terminal["tools"] = json!(tools);
        terminal["streaming"] = json!(false);
        terminal["aborted"] = json!(aborted);
        if let Some(error) = failure {
            terminal["message"] = json!(security::redact_secret_text(&error));
            let _ = task_state.events.send(
                json!({"pisperEvent":"error","sessionId":session_id,"data":terminal}).to_string(),
            );
            record(&task_state, &run_id, "error", terminal);
        } else {
            if !isolated_context
                && !internal
                && !aborted
                && session
                    .get_active_tool_names()
                    .iter()
                    .any(|tool| tool == "memory_remember")
            {
                if let Some(model) = session.model() {
                    task_state.memory_tasks.capture(session.model_runtime().clone(), crate::memory_store::runtime::CaptureInput {
                        session_id: session_id.clone(), cwd: std::path::PathBuf::from(hosted.runtime.cwd()),
                        model, user: raw_user_message, assistant: text.clone(),
                        source_timestamp: pi_rust::agent_core::harness::session::jsonl::iso8601::format_iso8601_utc(product::now_ms() as i64),
                    });
                }
            }
            record(&task_state, &run_id, "text_end", json!({"text":text}));
            let _ = task_state.events.send(
                json!({"pisperEvent":"done","sessionId":session_id,"data":terminal}).to_string(),
            );
            record(&task_state, &run_id, "done", terminal);
        }
        hosted.touch();
        drop(guard);
        drop(configuration);
        drop(hosted);
        if let Err(error) = task_state.sessions.sweep(&task_state, "").await {
            tracing::warn!(code = error.code, "Resident session cleanup failed");
        }
    });
    let mut result = response(frames, rx, 0, false);
    result.headers_mut().insert(
        "x-pisper-run-id",
        response_run_id
            .parse()
            .map_err(|_| ApiError::internal("Invalid owned run identity"))?,
    );
    Ok(result)
}

fn record_asset_event(state: &AppState, run_id: &str, session_id: &str, event: &str) {
    if let Ok(event) = serde_json::from_str::<Value>(event) {
        if event["sessionId"] == session_id {
            if let Some(name) = event["pisperEvent"].as_str().filter(|name| {
                matches!(
                    *name,
                    "generated_asset"
                        | "goal_update"
                        | "plan_update"
                        | "team_update"
                        | "agents_update"
                        | "agent_update"
                        | "agent_status"
                        | "agent_notification_error"
                )
            }) {
                record(state, run_id, name, event["data"].clone());
            }
        }
    }
}

pub(crate) async fn run_events(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<axum::response::Response, ApiError> {
    let after = params
        .get("after")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let (frames, rx, closed) = {
        let runs = state.chat_runs.lock().expect("chat runs lock");
        let run = runs.get(&id).ok_or_else(|| {
            ApiError::new(StatusCode::NOT_FOUND, "run_not_found", "对话运行不存在。")
        })?;
        let frames = run.frames.lock().expect("frames lock").clone();
        (
            frames,
            run.tx.subscribe(),
            run.closed.load(std::sync::atomic::Ordering::Relaxed),
        )
    };
    Ok(response(frames, rx, after, closed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[test]
    fn model_failure_is_not_a_successful_empty_answer() {
        let mut projection = Projection::default();
        assert!(project(&json!({"type":"agent_end","messages":[{"role":"assistant","content":[],"stopReason":"error","errorMessage":"invalid model"}]}), "s", &mut projection).is_none());
        assert_eq!(projection.failure.as_deref(), Some("invalid model"));
    }

    #[tokio::test]
    async fn replay_stream_ends_after_terminal_frame() {
        let (tx, rx) = tokio::sync::broadcast::channel(8);
        let frames = vec![product::Frame {
            cursor: 1,
            event: "done".into(),
            data: json!({"text":"ok"}),
        }];
        let response = response(frames, rx, 0, false);
        let mut body = response.into_body().into_data_stream();
        assert!(body.next().await.is_some());
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), body.next())
                .await
                .unwrap()
                .is_none()
        );
        drop(tx);
    }

    #[test]
    fn tool_result_preserves_output_and_error_status() {
        let mut projection = Projection::default();
        project(
            &json!({"type":"tool_execution_start","toolCallId":"t","toolName":"read","args":{}}),
            "s",
            &mut projection,
        );
        let (_, data) = project(&json!({"type":"tool_execution_end","toolCallId":"t","isError":true,"result":{"content":[{"type":"text","text":"missing file"}]}}), "s", &mut projection).unwrap();
        assert_eq!(data["output"], "missing file");
        assert_eq!(projection.tools[0]["status"], "error");
    }
}
