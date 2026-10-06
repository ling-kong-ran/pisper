//! Native command dispatch from upstream `modes/rpc/rpc-mode.ts`.
//!
//! Uses the real runtime and AgentSession, never a response table. Prompt
//! preflight emits its own acknowledgement while the turn continues, allowing
//! later commands (including abort and queue updates) to run concurrently.
//! This typed entrypoint does not claim permissive JS input parsing, stdin
//! ownership, signal handling, or extension UI transport; the mode owns those.

use super::types::{
    RpcCommand, RpcCommandKind, RpcResponse, RpcSessionState, RpcSlashCommand,
    RpcSlashCommandSource,
};
use crate::coding_agent::agent_session::bash_executor::BashOperationsHandle;
use crate::coding_agent::agent_session::{CycleDirection, ExecuteBashOptions, PromptOptions};
use crate::coding_agent::core::agent_session_runtime::{
    AgentSessionRuntime, ForkOptions, ForkPosition, NewSessionOptionsRuntime, SwitchSessionOptions,
};
use crate::coding_agent::extensions::types::{InputSource, UserBashEventResult};
use crate::coding_agent::utils::text::trim_js_whitespace;
use anyhow::Result;
use futures::future::BoxFuture;
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

pub type RpcResponseSink = Arc<dyn Fn(RpcResponse) + Send + Sync>;
pub type RpcRebind = Arc<dyn Fn() -> BoxFuture<'static, Result<()>> + Send + Sync>;

#[derive(Default)]
struct PendingPrompts {
    handles: Vec<tokio::task::JoinHandle<()>>,
    closed: bool,
}

#[derive(Clone)]
pub struct RpcDispatcher {
    runtime: Arc<AgentSessionRuntime>,
    output: RpcResponseSink,
    rebind: Option<RpcRebind>,
    prompts: Arc<Mutex<PendingPrompts>>,
}
impl RpcDispatcher {
    pub fn new(runtime: Arc<AgentSessionRuntime>, output: RpcResponseSink) -> Self {
        Self {
            runtime,
            output,
            rebind: None,
            prompts: Arc::new(Mutex::new(PendingPrompts::default())),
        }
    }
    /// The mode passes the same rebind routine it installs on the runtime.
    /// Upstream also calls it explicitly after successful replacements.
    pub fn with_rebind(mut self, rebind: RpcRebind) -> Self {
        self.rebind = Some(rebind);
        self
    }
    /// Cancel detached prompt preflight/turn futures when their RPC host ends.
    /// The closed bit also covers a prompt racing with mode teardown.
    pub fn abort_pending_prompts(&self) {
        let mut prompts = self.prompts.lock().expect("rpc prompt tasks");
        prompts.closed = true;
        for handle in prompts.handles.drain(..) {
            handle.abort();
        }
    }
    fn spawn_prompt(&self, pending: impl std::future::Future<Output = ()> + Send + 'static) {
        let mut prompts = self.prompts.lock().expect("rpc prompt tasks");
        if prompts.closed {
            return;
        }
        prompts.handles.retain(|handle| !handle.is_finished());
        prompts.handles.push(tokio::spawn(pending));
    }
    async fn rebind(&self) -> Result<()> {
        if let Some(rebind) = &self.rebind {
            rebind().await?;
        }
        Ok(())
    }
    /// Non-prompt results are returned to the mode; prompts use the sink only
    /// after acceptance (or once for a failure before acceptance).
    pub async fn handle(&self, command: RpcCommand) -> Option<RpcResponse> {
        let name = command.kind.as_str();
        let id = command.id.clone();
        match self.execute(command).await {
            Ok(response) => response,
            Err(error) => Some(RpcResponse::error(id, name, error.to_string())),
        }
    }
    async fn execute(&self, command: RpcCommand) -> Result<Option<RpcResponse>> {
        let RpcCommand { id, kind } = command;
        let name = kind.as_str();
        // Always read the runtime's live session at command start. A completed
        // replacement must not leave the dispatcher talking to the old one.
        let session = self.runtime.session();
        let data = match kind {
            RpcCommandKind::Prompt {
                message,
                images,
                streaming_behavior,
            } => {
                let output = self.output.clone();
                let accepted = Arc::new(AtomicBool::new(false));
                let accept = accepted.clone();
                let callback_output = output.clone();
                let callback_id = id.clone();
                let mut pending = Box::pin(async move {
                    let result = session
                        .prompt(
                            message,
                            Some(PromptOptions {
                                images: images.map(|images| {
                                    images
                                        .into_iter()
                                        .map(|image| image.into_content())
                                        .collect()
                                }),
                                streaming_behavior,
                                source: Some(InputSource::Rpc),
                                preflight_result: Some(Arc::new(move |disposition| {
                                    accept.store(true, Ordering::SeqCst);
                                    callback_output(RpcResponse::success(
                                        callback_id.clone(),
                                        "prompt",
                                        Some(json!({
                                            "disposition": disposition.as_str(),
                                        })),
                                    ));
                                })),
                                ..Default::default()
                            }),
                        )
                        .await;
                    if let Err(error) = result {
                        if !accepted.load(Ordering::SeqCst) {
                            output(RpcResponse::error(id, "prompt", error.to_string()));
                        }
                    }
                });
                // JS async functions begin synchronously through their first
                // suspension. Poll once before detaching rather than merely
                // enqueuing prompt startup after subsequent state commands.
                if futures::poll!(pending.as_mut()).is_pending() {
                    self.spawn_prompt(pending);
                }
                return Ok(None);
            }
            RpcCommandKind::Steer { message, images } => {
                let disposition = session
                    .steer(
                        message,
                        images.map(|items| {
                            items
                                .into_iter()
                                .map(|image| image.into_content())
                                .collect()
                        }),
                        Some(InputSource::Rpc),
                    )
                    .await?;
                Some(json!({ "disposition": disposition.as_str() }))
            }
            RpcCommandKind::FollowUp { message, images } => {
                let disposition = session
                    .follow_up(
                        message,
                        images.map(|items| {
                            items
                                .into_iter()
                                .map(|image| image.into_content())
                                .collect()
                        }),
                        Some(InputSource::Rpc),
                    )
                    .await?;
                Some(json!({ "disposition": disposition.as_str() }))
            }
            RpcCommandKind::Abort => {
                session.abort().await;
                None
            }
            RpcCommandKind::ClearQueue => {
                let (steering, follow_up) = session.clear_queue();
                Some(json!({"steering": steering, "followUp": follow_up}))
            }
            RpcCommandKind::NewSession { parent_session } => {
                let result = self
                    .runtime
                    .new_session(NewSessionOptionsRuntime {
                        parent_session: parent_session.filter(|value| !value.is_empty()),
                        ..Default::default()
                    })
                    .await?;
                if !result.cancelled {
                    self.rebind().await?;
                }
                Some(serde_json::to_value(result)?)
            }
            RpcCommandKind::GetState => Some(serde_json::to_value(RpcSessionState {
                model: session.model(),
                thinking_level: session.thinking_level(),
                is_streaming: session.is_streaming(),
                is_compacting: session.is_compacting(),
                steering_mode: session.steering_mode(),
                follow_up_mode: session.follow_up_mode(),
                session_file: session.session_file(),
                session_id: session.session_id(),
                session_name: session.session_name(),
                auto_compaction_enabled: session.auto_compaction_enabled()?,
                message_count: session.messages().len(),
                pending_message_count: session.pending_message_count(),
            })?),
            RpcCommandKind::SetModel { provider, model_id } => {
                let model = session
                    .model_runtime()
                    .get_available_snapshot()
                    .into_iter()
                    .find(|m| m.provider == provider && m.id == model_id);
                let Some(model) = model else {
                    return Ok(Some(RpcResponse::error(
                        id,
                        name,
                        format!("Model not found: {provider}/{model_id}"),
                    )));
                };
                session.set_model(model.clone(), None).await?;
                Some(serde_json::to_value(model)?)
            }
            RpcCommandKind::CycleModel => Some(
                match session.cycle_model(CycleDirection::Forward, None).await? {
                    Some(result) => {
                        json!({"model":result.model, "thinkingLevel":result.thinking_level, "isScoped":result.is_scoped})
                    }
                    None => Value::Null,
                },
            ),
            RpcCommandKind::GetAvailableModels => {
                Some(json!({"models":session.model_runtime().get_available_snapshot()}))
            }
            RpcCommandKind::SetThinkingLevel { level } => {
                session.set_thinking_level(level, None);
                None
            }
            RpcCommandKind::CycleThinkingLevel => Some(match session.cycle_thinking_level(None) {
                Some(level) => json!({"level":level}),
                None => Value::Null,
            }),
            RpcCommandKind::GetAvailableThinkingLevels => {
                Some(json!({"levels":session.get_available_thinking_levels()}))
            }
            RpcCommandKind::SetSteeringMode { mode } => {
                session.set_steering_mode(mode);
                None
            }
            RpcCommandKind::SetFollowUpMode { mode } => {
                session.set_follow_up_mode(mode);
                None
            }
            RpcCommandKind::Compact {
                custom_instructions,
            } => Some(serde_json::to_value(
                session.compact(custom_instructions).await?,
            )?),
            RpcCommandKind::SetAutoCompaction { enabled } => {
                session.set_auto_compaction_enabled(enabled)?;
                None
            }
            RpcCommandKind::SetAutoRetry { enabled } => {
                session.set_auto_retry_enabled(enabled);
                None
            }
            RpcCommandKind::AbortRetry => {
                session.abort_retry();
                None
            }
            RpcCommandKind::Bash {
                command,
                exclude_from_context,
            } => {
                let cwd = session
                    .session_manager
                    .lock()
                    .expect("session manager")
                    .get_cwd()
                    .to_owned();
                let event = json!({"type":"user_bash", "command":command, "excludeFromContext":exclude_from_context.unwrap_or(false), "cwd":cwd});
                let event_result = session
                    .extension_runner()
                    .emit_user_bash(&event)
                    .await
                    .map_err(anyhow::Error::msg)?;
                let result = match event_result {
                    Some(UserBashEventResult::Result(result)) => {
                        session.record_bash_result(&command, &result, exclude_from_context);
                        result
                    }
                    other => {
                        let operations = match other {
                            Some(UserBashEventResult::Operations(operations)) => {
                                Some(BashOperationsHandle {
                                    exec: operations.exec,
                                })
                            }
                            _ => None,
                        };
                        session
                            .execute_bash(
                                &command,
                                None,
                                Some(ExecuteBashOptions {
                                    exclude_from_context,
                                    id: id.clone(),
                                    operations,
                                }),
                            )
                            .await?
                    }
                };
                Some(serde_json::to_value(result)?)
            }
            RpcCommandKind::AbortBash => {
                session.abort_bash();
                None
            }
            RpcCommandKind::GetSessionStats => {
                Some(serde_json::to_value(session.get_session_stats()?)?)
            }
            RpcCommandKind::ExportHtml { output_path } => {
                Some(json!({"path":session.export_to_html(output_path, None).await?}))
            }
            RpcCommandKind::SwitchSession { session_path } => {
                let result = self
                    .runtime
                    .switch_session(&session_path, SwitchSessionOptions::default())
                    .await?;
                if !result.cancelled {
                    self.rebind().await?;
                }
                Some(serde_json::to_value(result)?)
            }
            RpcCommandKind::Fork { entry_id } => {
                let result = self.runtime.fork(&entry_id, ForkOptions::default()).await?;
                if !result.cancelled {
                    self.rebind().await?;
                }
                let mut data = serde_json::Map::new();
                if let Some(text) = result.selected_text {
                    data.insert("text".into(), text.into());
                }
                data.insert("cancelled".into(), result.cancelled.into());
                Some(Value::Object(data))
            }
            RpcCommandKind::Clone => {
                let leaf_id = session
                    .session_manager
                    .lock()
                    .expect("session manager")
                    .get_leaf_id()
                    .map(str::to_owned);
                let Some(leaf_id) = leaf_id.filter(|id| !id.is_empty()) else {
                    return Ok(Some(RpcResponse::error(
                        id,
                        name,
                        "Cannot clone session: no current entry selected",
                    )));
                };
                let result = self
                    .runtime
                    .fork(
                        &leaf_id,
                        ForkOptions {
                            position: ForkPosition::At,
                            ..Default::default()
                        },
                    )
                    .await?;
                if !result.cancelled {
                    self.rebind().await?;
                }
                Some(json!({"cancelled":result.cancelled}))
            }
            RpcCommandKind::GetForkMessages => Some(
                json!({"messages":session.get_user_messages_for_forking()?.into_iter().map(|(entry_id,text)|json!({"entryId":entry_id,"text":text})).collect::<Vec<_>>()}),
            ),
            RpcCommandKind::GetEntries { since } => {
                let manager = session.session_manager.lock().expect("session manager");
                let entries = manager.get_entries();
                let start = if let Some(since) = since {
                    let Some(index) = entries
                        .iter()
                        .position(|entry| entry.id() == Some(since.as_str()))
                    else {
                        return Ok(Some(RpcResponse::error(
                            id,
                            name,
                            format!("Entry not found: {since}"),
                        )));
                    };
                    index + 1
                } else {
                    0
                };
                Some(json!({"entries":&entries[start..], "leafId":manager.get_leaf_id()}))
            }
            RpcCommandKind::GetTree => {
                let manager = session.session_manager.lock().expect("session manager");
                Some(json!({"tree":manager.get_tree(), "leafId":manager.get_leaf_id()}))
            }
            RpcCommandKind::GetLastAssistantText => {
                Some(match session.get_last_assistant_text()? {
                    Some(text) => json!({"text":text}),
                    None => json!({}),
                })
            }
            RpcCommandKind::SetSessionName { name: raw_name } => {
                let session_name = trim_js_whitespace(&raw_name);
                if session_name.is_empty() {
                    return Ok(Some(RpcResponse::error(
                        id,
                        name,
                        "Session name cannot be empty",
                    )));
                }
                session.set_session_name(session_name);
                None
            }
            RpcCommandKind::GetMessages => Some(json!({"messages":session.messages()})),
            RpcCommandKind::GetCommands => {
                let mut commands: Vec<RpcSlashCommand> = session
                    .extension_runner()
                    .get_registered_commands()
                    .into_iter()
                    .map(|command| RpcSlashCommand {
                        name: command.invocation_name,
                        description: command.command.description,
                        source: RpcSlashCommandSource::Extension,
                        source_info: command.command.source_info,
                    })
                    .collect();
                commands.extend(session.prompt_templates().into_iter().map(|template| {
                    RpcSlashCommand {
                        name: template.name,
                        description: Some(template.description),
                        source: RpcSlashCommandSource::Prompt,
                        source_info: template.source_info,
                    }
                }));
                let skills = self
                    .runtime
                    .services()
                    .resource_loader
                    .lock()
                    .expect("resource loader")
                    .get_skills()
                    .skills;
                commands.extend(skills.into_iter().map(|skill| RpcSlashCommand {
                    name: format!("skill:{}", skill.name),
                    description: Some(skill.description),
                    source: RpcSlashCommandSource::Skill,
                    source_info: skill.source_info,
                }));
                Some(json!({"commands":commands}))
            }
        };
        Ok(Some(RpcResponse::success(id, name, data)))
    }
}

#[cfg(test)]
#[path = "dispatch_tests.rs"]
mod tests;
