//! 将领域执行接口接入独立 Pi 会话，避免子 Agent 或工作流借用主会话引擎。
use crate::{
    session_runtime::HostedSession,
    session_workers::{
        ChildRequest, EventSink, InputKind, PromptRequest, RunOutcome, SessionExecutor,
        SessionScope,
    },
    AppState,
};
use anyhow::{anyhow, bail, Result};
use futures::future::BoxFuture;
use pi_rust::{
    agent_core::types::AgentTool,
    coding_agent::{
        agent_session::{AgentSessionEvent, PromptOptions},
        core::{agent_session_runtime::AgentSessionRuntime, resource_loader::InlineExtension},
        extensions::{
            loader::ExtensionFactory,
            types::{sync_handler, HandlerResult, ToolDefinition},
        },
        modes::json_event::to_json_event_string,
        session_manager::SessionManager,
    },
};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock, Weak,
    },
    time::{Duration, Instant},
};

type ToolProvider = Arc<dyn Fn(String, Option<String>, EventSink) -> Vec<AgentTool> + Send + Sync>;
#[derive(Clone)]
struct ChildConfig {
    alias: String,
    parent: String,
    tools: Vec<String>,
    owned_files: Vec<String>,
    system_prompt: String,
}
struct Child {
    runtime: Arc<AgentSessionRuntime>,
    mutation: Arc<tokio::sync::Mutex<()>>,
    scope: SessionScope,
}
pub(crate) struct PiExecutor {
    state: Arc<OnceLock<Weak<AppState>>>,
    children: Mutex<HashMap<String, Arc<Child>>>,
    configurations: Mutex<HashMap<String, ChildConfig>>,
    tools: Mutex<Vec<ToolProvider>>,
    creating: tokio::sync::RwLock<()>,
    reservations: Mutex<HashSet<String>>,
    closed: AtomicBool,
}
struct ChildReservation {
    executor: Arc<PiExecutor>,
    alias: String,
    native_id: Option<String>,
    committed: bool,
}
impl Drop for ChildReservation {
    fn drop(&mut self) {
        self.executor
            .reservations
            .lock()
            .expect("child reservations")
            .remove(&self.alias);
        if !self.committed {
            if let Some(id) = &self.native_id {
                self.executor
                    .configurations
                    .lock()
                    .expect("child configurations")
                    .remove(id);
            }
        }
    }
}
impl PiExecutor {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Arc::new(OnceLock::new()),
            children: Mutex::new(HashMap::new()),
            configurations: Mutex::new(HashMap::new()),
            tools: Mutex::new(Vec::new()),
            creating: tokio::sync::RwLock::new(()),
            reservations: Mutex::new(HashSet::new()),
            closed: AtomicBool::new(false),
        })
    }
    pub(crate) fn attach(&self, state: &Arc<AppState>) {
        if self.state.set(Arc::downgrade(state)).is_ok() {
            let dispatcher_state = Arc::downgrade(state);
            let callback: crate::multi_agent_api::CompletionDispatcher = Arc::new(move |batch| {
                let state = dispatcher_state.clone();
                Box::pin(async move {
                    let Some(state) = state
                        .upgrade()
                        .filter(|state| !state.shutdown.is_cancelled())
                    else {
                        return Ok(false);
                    };
                    dispatch_completion(state, batch).await
                })
            });
            state.agents.set_completion_dispatcher(callback);
        }
    }
    fn state(&self) -> Result<Arc<AppState>> {
        self.state
            .get()
            .and_then(Weak::upgrade)
            .ok_or_else(|| anyhow!("Session executor is not initialized or has shut down."))
    }
    pub(crate) fn register_tools(&self, provider: ToolProvider) {
        self.tools.lock().expect("execution tools").push(provider);
    }
    /// 工具上下文只读解析亲子权限归属，不创建或切换任何会话。
    pub(crate) fn tool_owner(&self, native_id: &str) -> String {
        self.configurations
            .lock()
            .expect("child configurations")
            .get(native_id)
            .map(|config| config.parent.clone())
            .unwrap_or_else(|| native_id.to_owned())
    }
    pub(crate) fn tool_owned_files(&self, native_id: &str) -> Vec<String> {
        self.configurations
            .lock()
            .expect("child configurations")
            .get(native_id)
            .map(|config| config.owned_files.clone())
            .unwrap_or_default()
    }
    pub(crate) fn tool_limit(&self, native_id: &str) -> Option<Vec<String>> {
        self.configurations
            .lock()
            .expect("child configurations")
            .get(native_id)
            .map(|config| config.tools.clone())
    }
    pub(crate) fn tool_session(
        &self,
        native_id: &str,
    ) -> Option<Arc<pi_rust::coding_agent::agent_session::AgentSession>> {
        self.children
            .lock()
            .expect("child sessions")
            .values()
            .find(|child| child.runtime.session().session_id() == native_id)
            .map(|child| child.runtime.session())
    }
    pub(crate) fn event_sink(&self) -> EventSink {
        let state = self.state.clone();
        Arc::new(move |event, data| {
            if let Some(state) = state.get().and_then(Weak::upgrade) {
                let _ = state.events.send(
                    json!({"pisperEvent":event,"sessionId":data["sessionId"],"data":data})
                        .to_string(),
                );
            }
        })
    }
    fn tool_list(&self, native_id: &str) -> Vec<AgentTool> {
        let config = self
            .configurations
            .lock()
            .expect("child configurations")
            .get(native_id)
            .cloned();
        let (id, parent) = config
            .map(|config| (config.alias, Some(config.parent)))
            .unwrap_or_else(|| (native_id.to_owned(), None));
        self.tools
            .lock()
            .expect("execution tools")
            .iter()
            .flat_map(|provider| provider(id.clone(), parent.clone(), self.event_sink()))
            .collect()
    }
    pub(crate) fn extension(self: &Arc<Self>) -> InlineExtension {
        let weak = Arc::downgrade(self);
        let factory: ExtensionFactory = Arc::new(move |api| {
            let executor = weak.upgrade().ok_or("Execution services have shut down")?;
            for template in executor.tool_list("catalog") {
                let name = template.name.clone();
                let mut definition = ToolDefinition::new(
                    &name,
                    &template.label,
                    &template.description,
                    template.parameters,
                );
                let weak = weak.clone();
                definition.execute_async = Some(Arc::new(move |call, args, signal, _, ctx| {
                    let weak = weak.clone();
                    let name = name.clone();
                    Box::pin(async move {
                        if signal.as_ref().is_some_and(|signal| signal.is_aborted()) {
                            return Err("Tool execution was cancelled".into());
                        }
                        let native_id =
                            crate::memory_store::runtime::session_id(ctx.session_manager()?)?;
                        let executor = weak.upgrade().ok_or("Execution services have shut down")?;
                        let tool = executor
                            .tool_list(&native_id)
                            .into_iter()
                            .find(|tool| tool.name == name)
                            .ok_or("This session cannot use the requested execution tool")?;
                        let token = tokio_util::sync::CancellationToken::new();
                        let cancelled = token.clone();
                        let _subscription = signal
                            .as_ref()
                            .map(|signal| signal.on_abort(Arc::new(move || cancelled.cancel())));
                        let result = tokio::select! {
                            result = (tool.execute)(call, args, Some(token.clone()), None) => result.map_err(|error| error.to_string())?,
                            _ = token.cancelled() => return Err("Tool execution was cancelled".into()),
                        };
                        serde_json::to_value(result).map_err(|error| error.to_string())
                    })
                }));
                api.register_tool(definition)?;
            }
            let weak = weak.clone();
            api.on(
                "before_agent_start",
                sync_handler(move |_, ctx| {
                    let native_id =
                        crate::memory_store::runtime::session_id(ctx.session_manager()?)?;
                    let Some(executor) = weak.upgrade() else {
                        return Err("Execution services have shut down".into());
                    };
                    let config = executor
                        .configurations
                        .lock()
                        .map_err(|_| "Child configuration lock failed")?
                        .get(&native_id)
                        .cloned();
                    Ok(config
                        .filter(|config| !config.system_prompt.is_empty())
                        .map(|config| {
                            HandlerResult::Json(json!({"systemPrompt":config.system_prompt}))
                        }))
                }),
            )?;
            Ok(())
        });
        InlineExtension::Named {
            factory,
            name: "builtin:pisper-execution".into(),
            hidden: false,
        }
    }
    fn child(&self, id: &str) -> Option<Arc<Child>> {
        self.children
            .lock()
            .expect("child sessions")
            .get(id)
            .cloned()
    }
    fn public_scope(state: &AppState, host: &HostedSession) -> SessionScope {
        let session = host.session();
        let id = session.session_id();
        let meta = state
            .session_meta
            .lock()
            .expect("session metadata")
            .get(&id)
            .cloned()
            .unwrap_or_default();
        let execution = meta
            .execution_mode
            .unwrap_or_else(|| crate::execution_modes::DEFAULT_EXECUTION_MODE.into());
        SessionScope {
            session_id: id,
            cwd: host.runtime.cwd(),
            model: session
                .model()
                .map(|model| format!("{}/{}", model.provider, model.id))
                .unwrap_or_default(),
            thinking_level: serde_json::to_value(session.thinking_level())
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_else(|| "off".into()),
            permission_mode: meta
                .permission_mode
                .unwrap_or_else(|| crate::execution_modes::permission_mode(&execution).into()),
            execution_mode: execution,
            tool_names: session.get_active_tool_names(),
            ..Default::default()
        }
    }
    pub(crate) async fn shutdown(&self) -> Result<()> {
        self.closed.store(true, Ordering::Release);
        let _creating = self.creating.write().await;
        let children = std::mem::take(&mut *self.children.lock().expect("child sessions"));
        let results = futures::future::join_all(children.into_values().map(|child| async move {
            if tokio::time::timeout(Duration::from_secs(10), child.runtime.session().abort())
                .await
                .is_err()
            {
                child.runtime.session().dispose();
            }
            tokio::time::timeout(Duration::from_secs(20), child.runtime.dispose())
                .await
                .map_err(|_| anyhow!("Child session cleanup timed out"))??;
            Ok::<_, anyhow::Error>(())
        }))
        .await;
        self.configurations
            .lock()
            .expect("child configurations")
            .clear();
        for result in results {
            result?;
        }
        Ok(())
    }
}
impl SessionExecutor for Arc<PiExecutor> {
    fn validate_session(&self, id: String) -> BoxFuture<'static, Result<()>> {
        let executor = self.clone();
        Box::pin(async move {
            if executor.child(&id).is_some() {
                return Ok(());
            }
            let state = executor.state()?;
            crate::session_api::find_session_path(&state, &id)
                .map(|_| ())
                .map_err(|error| anyhow!(error.message))
        })
    }
    fn scope(&self, id: String) -> BoxFuture<'static, Result<SessionScope>> {
        let executor = self.clone();
        Box::pin(async move {
            if let Some(child) = executor.child(&id) {
                return Ok(child.scope.clone());
            }
            let state = executor.state()?;
            let host = state
                .sessions
                .host(&state, &id)
                .await
                .map_err(|error| anyhow!(error.message))?;
            Ok(PiExecutor::public_scope(&state, &host))
        })
    }
    fn create_child(&self, request: ChildRequest) -> BoxFuture<'static, Result<SessionScope>> {
        let executor = self.clone();
        Box::pin(async move {
            let state = executor.state()?;
            let _creating = executor.creating.read().await;
            if executor.closed.load(Ordering::Acquire) || state.shutdown.is_cancelled() {
                bail!("Session executor is shutting down");
            }
            {
                let mut reservations = executor.reservations.lock().expect("child reservations");
                if executor.child(&request.id).is_some() || !reservations.insert(request.id.clone())
                {
                    bail!("Child session already exists");
                }
            }
            let mut reservation = ChildReservation {
                executor: executor.clone(),
                alias: request.id.clone(),
                native_id: None,
                committed: false,
            };
            let mut manager = SessionManager::in_memory(&request.parent.cwd, None, None)?;
            let native_id = manager.get_session_id().to_owned();
            reservation.native_id = Some(native_id.clone());
            let (provider, model) = request
                .parent
                .model
                .split_once('/')
                .ok_or_else(|| anyhow!("Parent session has no model"))?;
            manager.append_model_change(provider, model)?;
            manager.append_thinking_level_change(&request.parent.thinking_level)?;
            let mut tools = request.tools.clone();
            tools.retain(|name| {
                !crate::session_workers::agents::CHILD_FORBIDDEN.contains(&name.as_str())
                    && !matches!(name.as_str(), "run_team" | "memory_remember")
            });
            for tool in ["get_plan", "get_task_list"] {
                if !tools.iter().any(|name| name == tool) {
                    tools.push(tool.into());
                }
            }
            executor
                .configurations
                .lock()
                .expect("child configurations")
                .insert(
                    native_id.clone(),
                    ChildConfig {
                        alias: request.id.clone(),
                        parent: request.parent.session_id.clone(),
                        tools: tools.clone(),
                        owned_files: request.owned_files.clone(),
                        system_prompt: request.system_prompt,
                    },
                );
            let configuration = state.engine_mutation.read().await;
            let runtime = match state.sessions.build_runtime(&state, manager).await {
                Ok(runtime) => runtime,
                Err(error) => {
                    executor
                        .configurations
                        .lock()
                        .expect("child configurations")
                        .remove(&native_id);
                    return Err(anyhow!(error.message));
                }
            };
            let session = runtime.session();
            session.set_active_tools_by_name(tools);
            crate::tool_policy::install_context(
                session.clone(),
                state.session_meta.clone(),
                state.approvals.clone(),
                request.parent.session_id.clone(),
                request.owned_files.clone(),
            );
            if let Err(error) = crate::asset_api::tracker::install_context_with_file_changes(
                session.clone(),
                state.asset_tracker.clone(),
                state.file_changes.clone(),
                request.parent.session_id.clone(),
                Some(crate::session_runtime::asset_event_sink(&state)),
            ) {
                executor
                    .configurations
                    .lock()
                    .expect("child configurations")
                    .remove(&native_id);
                let _ = runtime.dispose().await;
                return Err(error);
            }
            let allowed = session.get_active_tool_names();
            let mut hooks = session.agent.runtime();
            let previous = hooks.before_tool_call.clone();
            hooks.before_tool_call = Some(Arc::new(move |context| {
                let allowed = allowed.clone();
                let previous = previous.clone();
                Box::pin(async move {
                    if !allowed.iter().any(|name| name == &context.tool_call.name) {
                        return pi_rust::agent_core::agent_loop::BeforeToolCallOutcome {
                            args: None,
                            result: Some(pi_rust::agent_core::types::BeforeToolCallResult {
                                block: Some(true),
                                reason: Some(
                                    "Tool is outside this child Agent's allowed tool set.".into(),
                                ),
                                terminate: None,
                            }),
                        };
                    }
                    match previous {
                        Some(previous) => previous(context).await,
                        None => Default::default(),
                    }
                })
            }));
            drop(hooks);
            let mut scope = request.parent;
            scope.parent_session_id = Some(scope.session_id.clone());
            scope.session_id = request.id.clone();
            scope.owned_files = request.owned_files;
            scope.tool_names = session.get_active_tool_names();
            let child = Arc::new(Child {
                runtime,
                mutation: Arc::new(tokio::sync::Mutex::new(())),
                scope: scope.clone(),
            });
            executor
                .children
                .lock()
                .expect("child sessions")
                .insert(request.id, child);
            reservation.committed = true;
            drop(configuration);
            Ok(scope)
        })
    }
    fn prompt(
        &self,
        mut request: PromptRequest,
        events: EventSink,
    ) -> BoxFuture<'static, Result<RunOutcome>> {
        let executor = self.clone();
        Box::pin(async move {
            let state = executor.state()?;
            let images = std::mem::take(&mut request.images);
            let isolated = request.isolated;
            if let Some(child) = executor.child(&request.session_id) {
                let _guard = child
                    .mutation
                    .clone()
                    .try_lock_owned()
                    .map_err(|_| anyhow!("Child session is busy"))?;
                let _configuration = state.engine_mutation.read().await;
                return run_prompt(&state, child.runtime.clone(), request, events, images, true)
                    .await;
            }
            let lease = crate::session_runtime::prompt_mutation(&state, &request.session_id)
                .await
                .map_err(|error| anyhow!(error.message))?;
            let outcome = run_prompt(
                &state,
                lease.hosted.runtime.clone(),
                request,
                events,
                images,
                isolated,
            )
            .await;
            lease.hosted.touch();
            outcome
        })
    }
    fn enqueue(&self, id: String, text: String, kind: InputKind) -> BoxFuture<'static, Result<()>> {
        let executor = self.clone();
        Box::pin(async move {
            let state = executor.state()?;
            let child = executor.child(&id);
            let owned_child = child.is_some();
            let mut public_reserved = false;
            let session = if let Some(child) = child {
                child.runtime.session()
            } else {
                let host = state
                    .sessions
                    .host(&state, &id)
                    .await
                    .map_err(|error| anyhow!(error.message))?;
                public_reserved = host.run.try_lock().is_err();
                host.session()
            };
            if !owned_child && !public_reserved && !session.is_streaming() {
                bail!("Session is idle; enqueue requires an active run");
            }
            match kind {
                InputKind::Steer => {
                    session.steer(text, None, None).await?;
                }
                InputKind::FollowUp | InputKind::Notification => {
                    session.follow_up(text, None, None).await?;
                }
            }
            Ok(())
        })
    }
    fn abort(&self, id: String) -> BoxFuture<'static, Result<()>> {
        let executor = self.clone();
        Box::pin(async move {
            let state = executor.state()?;
            if let Some(child) = executor.child(&id) {
                if tokio::time::timeout(Duration::from_secs(10), child.runtime.session().abort())
                    .await
                    .is_err()
                {
                    child.runtime.session().dispose();
                    bail!("Child session abort timed out");
                }
            } else if let Some(host) = state.sessions.get(&id) {
                state.approvals.cancel_session(&id);
                state.asset_tracker.cancel_session(&id);
                host.abort().await.map_err(|error| anyhow!(error.message))?;
                state.asset_tracker.wait_session(&id).await;
            }
            Ok(())
        })
    }
    fn dispose(&self, id: String) -> BoxFuture<'static, Result<()>> {
        let executor = self.clone();
        Box::pin(async move {
            let state = executor.state()?;
            if let Some(child) = executor.child(&id) {
                let _guard = child
                    .mutation
                    .clone()
                    .try_lock_owned()
                    .map_err(|_| anyhow!("Child session is busy"))?;
                executor
                    .children
                    .lock()
                    .expect("child sessions")
                    .remove(&id);
                executor
                    .configurations
                    .lock()
                    .expect("child configurations")
                    .remove(&child.runtime.session().session_id());
                child.runtime.dispose().await?;
            } else {
                state
                    .sessions
                    .remove(&state, &id)
                    .await
                    .map_err(|error| anyhow!(error.message))?;
            }
            Ok(())
        })
    }
}

async fn dispatch_completion(
    state: Arc<AppState>,
    batch: crate::multi_agent_api::CompletionBatch,
) -> Result<bool> {
    use sha2::Digest;
    if batch.cancellation.is_cancelled() || state.shutdown.is_cancelled() {
        return Ok(false);
    }
    let id = &batch.session_id;
    let receipts: Vec<_> = batch
        .entries
        .iter()
        .map(|entry| {
            let key = json!([entry["mailboxId"], entry["id"], entry["resultVersion"]]);
            let hash: String = sha2::Sha256::digest(key.to_string().as_bytes())
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            format!("[Pisper delivery {hash}]")
        })
        .collect();
    let host = state
        .sessions
        .host(&state, id)
        .await
        .map_err(|error| anyhow!(error.message))?;
    if batch.cancellation.is_cancelled() || state.shutdown.is_cancelled() {
        return Ok(false);
    }
    if deliveries_recorded(&host.session(), &receipts)? {
        return Ok(true);
    }
    let message = fresh_delivery_message(&host.session(), &batch.entries, &receipts)?;
    if host.session().is_streaming()
        || (state.goals.busy(id)
            && state
                .goals
                .goals
                .get(id)
                .is_some_and(|goal| goal.status == "active"))
    {
        if !message.is_empty() {
            if batch.cancellation.is_cancelled() || state.shutdown.is_cancelled() {
                return Ok(false);
            }
            SessionExecutor::enqueue(
                &state.executor,
                id.clone(),
                message,
                InputKind::Notification,
            )
            .await?;
        }
        // The durable mailbox remains authoritative until Pi persists a user
        // message. Merely appending to its in-memory queue is not delivery.
        return Ok(false);
    }
    if host.busy() {
        return Ok(false);
    }
    let lease = match crate::session_runtime::mutation(&state, id).await {
        Ok(lease) => lease,
        Err(error) if error.status == axum::http::StatusCode::CONFLICT => return Ok(false),
        Err(error) => return Err(anyhow!(error.message)),
    };
    let session = lease.hosted.session();
    if batch.cancellation.is_cancelled() || state.shutdown.is_cancelled() {
        return Ok(false);
    }
    if deliveries_recorded(&session, &receipts)? {
        return Ok(true);
    }
    let message = fresh_delivery_message(&session, &batch.entries, &receipts)?;
    let queued = session
        .get_follow_up_messages()
        .iter()
        .any(|text| receipts.iter().any(|receipt| text.contains(receipt)));
    let model = session
        .model()
        .ok_or_else(|| anyhow!("Parent session has no model"))?;
    if !crate::provider_config::available_models(&state)
        .await
        .map_err(|error| anyhow!(error.message))?
        .iter()
        .any(|available| available.provider == model.provider && available.id == model.id)
    {
        return Ok(false);
    }
    if batch.cancellation.is_cancelled() || state.shutdown.is_cancelled() {
        return Ok(false);
    }
    if !queued && message.is_empty() {
        // A consumed queue message is still awaiting its durable MessageEnd.
        return Ok(false);
    }
    if queued && !message.is_empty() {
        session.follow_up(message.clone(), None, None).await?;
    }
    let result = execute_prompt(
        &state,
        lease.hosted.runtime.clone(),
        PromptRequest {
            session_id: id.clone(),
            text: message,
            internal: true,
            ..Default::default()
        },
        Arc::new(|_, _| {}),
        Vec::new(),
        false,
        queued,
        Some(batch.cancellation.clone()),
    )
    .await;
    if let Err(error) = state
        .asset_tracker
        .flush(id, &session.session_name().unwrap_or_default())
        .await
    {
        tracing::warn!(error=%crate::security::redact_secret_text(&error.to_string()), "Delivered Agent notification assets remain pending");
    }
    let mut terminal =
        crate::session_api::snapshot(&state, id).unwrap_or_else(|_| json!({"sessionId":id}));
    terminal["streaming"] = json!(false);
    terminal["configurationBusy"] = json!(false);
    let event = match result {
        Ok(outcome) => {
            terminal["text"] = json!(outcome.output);
            terminal["aborted"] = json!(outcome.aborted);
            if let Some(error) = outcome.error {
                terminal["message"] = json!(error);
                "error"
            } else {
                "done"
            }
        }
        Err(error) => {
            terminal["message"] = json!(crate::security::redact_secret_text(&error.to_string()));
            "error"
        }
    };
    let _ = state
        .events
        .send(json!({"pisperEvent":event,"sessionId":id,"data":terminal}).to_string());
    lease.hosted.touch();
    // The notification was delivered to the real engine. A model failure is
    // published once and must not trigger endless duplicate paid attempts.
    deliveries_recorded(&session, &receipts)
}

fn fresh_delivery_message(
    session: &pi_rust::coding_agent::agent_session::AgentSession,
    entries: &[Value],
    receipts: &[String],
) -> Result<String> {
    let mut queued = session.get_follow_up_messages();
    queued.extend(session.get_in_flight_user_messages());
    let mut messages = Vec::new();
    for (entry, receipt) in entries.iter().zip(receipts) {
        if !delivery_recorded(session, receipt)?
            && !queued.iter().any(|text| text.contains(receipt))
        {
            messages.push(format!(
                "{}\n\n{receipt}",
                crate::multi_agent_api::completion_prompt(entry)
            ));
        }
    }
    Ok(messages.join("\n\n"))
}
fn deliveries_recorded(
    session: &pi_rust::coding_agent::agent_session::AgentSession,
    receipts: &[String],
) -> Result<bool> {
    for receipt in receipts {
        if !delivery_recorded(session, receipt)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn delivery_recorded(
    session: &pi_rust::coding_agent::agent_session::AgentSession,
    receipt: &str,
) -> Result<bool> {
    if let Some(error) = session.message_persistence_error() {
        bail!("Parent transcript persistence failed: {error}");
    }
    let manager = session
        .session_manager
        .lock()
        .map_err(|_| anyhow!("Parent session manager lock failed"))?;
    let Some(path) = manager.get_session_file() else {
        bail!("Completion delivery requires a persistent parent session");
    };
    // SessionManager mutates memory before append; only the actual JSONL is a
    // receipt. Hold its writer lock while reading, including all saved branches
    // so a rewind does not cause the same delivered result to be paid for again.
    let transcript = match std::fs::read_to_string(path) {
        Ok(transcript) => transcript,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    persisted_delivery_recorded(&transcript, receipt)
}

fn persisted_delivery_recorded(transcript: &str, receipt: &str) -> Result<bool> {
    for line in transcript.lines().filter(|line| !line.trim().is_empty()) {
        let entry: Value = serde_json::from_str(line)?;
        let message = &entry["message"];
        if entry["type"] == "message" && message["role"] == "user" {
            if message["content"]
                .as_str()
                .is_some_and(|text| text.contains(receipt))
                || message["content"].as_array().is_some_and(|parts| {
                    parts.iter().any(|part| {
                        part["type"] == "text"
                            && part["text"]
                                .as_str()
                                .is_some_and(|text| text.contains(receipt))
                    })
                })
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

#[derive(Default)]
struct Outcome {
    text: String,
    usage: Value,
    error: Option<String>,
    aborted: bool,
    turn_started: Option<Instant>,
}
pub(crate) async fn run_prompt(
    state: &Arc<AppState>,
    runtime: Arc<AgentSessionRuntime>,
    request: PromptRequest,
    events: EventSink,
    images: Vec<pi_rust::ai::types::ImageContent>,
    isolated: bool,
) -> Result<RunOutcome> {
    execute_prompt(
        state, runtime, request, events, images, isolated, false, None,
    )
    .await
}
pub(crate) async fn run_prompt_cancellable(
    state: &Arc<AppState>,
    runtime: Arc<AgentSessionRuntime>,
    request: PromptRequest,
    events: EventSink,
    images: Vec<pi_rust::ai::types::ImageContent>,
    isolated: bool,
    cancellation: tokio_util::sync::CancellationToken,
) -> Result<RunOutcome> {
    execute_prompt(
        state,
        runtime,
        request,
        events,
        images,
        isolated,
        false,
        Some(cancellation),
    )
    .await
}
async fn execute_prompt(
    state: &Arc<AppState>,
    runtime: Arc<AgentSessionRuntime>,
    request: PromptRequest,
    events: EventSink,
    images: Vec<pi_rust::ai::types::ImageContent>,
    isolated: bool,
    continue_queued: bool,
    cancellation: Option<tokio_util::sync::CancellationToken>,
) -> Result<RunOutcome> {
    let session = runtime.session();
    let raw_user = request.text.clone();
    let context = if request.internal || request.context_prepared {
        String::new()
    } else {
        state.memory_tasks.context_for(
            &raw_user,
            std::path::Path::new(&runtime.cwd()),
            &session.get_active_tool_names(),
            isolated,
        )?
    };
    if !request.internal && !request.context_prepared && !isolated {
        state
            .memory_tasks
            .set_user_message(&request.session_id, &raw_user);
    }
    let text = if context.is_empty() {
        raw_user.clone()
    } else {
        format!("{context}\n\n{raw_user}")
    };
    let outcome = Arc::new(Mutex::new(Outcome::default()));
    let observed = outcome.clone();
    let id = request.session_id.clone();
    // 私有子会话没有 JSONL 可供今日用量扫描；按真实 turn_end 写入同一账本。
    let private_usage = session
        .session_manager
        .lock()
        .map_err(|_| anyhow!("Session manager lock failed"))?
        .get_session_file()
        .is_none();
    let usage_ledger = state.usage.clone();
    let listener = session.subscribe(Arc::new(move |event: &AgentSessionEvent| {
        let Ok(raw) = to_json_event_string(event) else {
            return;
        };
        let Ok(mut raw) = serde_json::from_str::<Value>(&raw) else {
            return;
        };
        let kind = raw["type"].as_str().unwrap_or_default().to_owned();
        let mut observed = observed.lock().expect("prompt outcome");
        if kind == "turn_start" {
            observed.turn_started = Some(Instant::now());
        }
        if kind == "turn_end" {
            raw["elapsedSeconds"] = json!(observed
                .turn_started
                .take()
                .map(|started| started.elapsed().as_secs_f64())
                .unwrap_or(0.0));
            observed.usage = add_usage(&observed.usage, &raw["message"]["usage"]);
            if private_usage {
                if let Err(error) = usage_ledger.record(
                    &chrono::Local::now().format("%Y-%m-%d").to_string(),
                    &format!("agent:{id}:{}", uuid::Uuid::new_v4()),
                    &raw["message"]["usage"],
                ) {
                    tracing::warn!(error=%crate::security::redact_secret_text(&error.to_string()), "Child Agent usage could not be recorded");
                }
            }
        }
        if kind == "agent_end" {
            if let Some(message) = raw["messages"].as_array().and_then(|messages| {
                messages
                    .iter()
                    .rev()
                    .find(|message| message["role"] == "assistant")
            }) {
                observed.text = message["content"]
                    .as_array()
                    .map(|blocks| {
                        blocks
                            .iter()
                            .filter(|block| block["type"] == "text")
                            .filter_map(|block| block["text"].as_str())
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default();
                observed.aborted = message["stopReason"] == "aborted";
                if message["stopReason"] == "error" {
                    observed.error = Some(
                        message["errorMessage"]
                            .as_str()
                            .unwrap_or("Model request failed")
                            .into(),
                    );
                }
            }
        }
        drop(observed);
        raw["sessionId"] = json!(id);
        events(&kind, &raw);
    }));
    let mut listener = PromptActivity {
        listener: Some(listener),
        session: session.clone(),
        finished: false,
    };
    let prompt = async {
        if continue_queued {
            session
                .continue_queued_cancellable(cancellation.clone())
                .await
        } else {
            session
                .prompt(
                    text,
                    Some(PromptOptions {
                        images: (!images.is_empty()).then_some(images),
                        start_cancellation: cancellation.clone(),
                        ..Default::default()
                    }),
                )
                .await
        }
    };
    tokio::pin!(prompt);
    let result = if let Some(cancellation) = &cancellation {
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                // Abort preflight work too, while continuing to poll native
                // finally. The start token prevents an idle abort from being
                // erased by a prompt that finishes preflight afterwards.
                let (_, result) = tokio::join!(session.abort(), &mut prompt);
                result
            }
            result = &mut prompt => result,
        }
    } else {
        prompt.await
    };
    listener.finished = true;
    if let Some(listener) = listener.listener.take() {
        listener.unsubscribe();
    }
    let outcome = {
        let mut outcome = outcome.lock().expect("prompt outcome");
        if let Err(error) = result {
            outcome.error = Some(error.to_string());
        }
        RunOutcome {
            output: outcome.text.clone(),
            usage: outcome.usage.clone(),
            error: outcome
                .error
                .clone()
                .map(|error| crate::security::redact_secret_text(&error)),
            aborted: outcome.aborted,
        }
    };
    if outcome.error.is_none()
        && !outcome.aborted
        && !isolated
        && !request.internal
        && !request.context_prepared
        && session
            .get_active_tool_names()
            .iter()
            .any(|tool| tool == "memory_remember")
    {
        if let Some(model) = session.model() {
            state.memory_tasks.capture(
                session.model_runtime().clone(),
                crate::memory_store::runtime::CaptureInput {
                    session_id: request.session_id,
                    cwd: runtime.cwd().into(),
                    model,
                    user: raw_user,
                    assistant: outcome.output.clone(),
                    source_timestamp: chrono::Utc::now().to_rfc3339(),
                },
            );
        }
    }
    Ok(outcome)
}
struct PromptActivity {
    listener: Option<pi_rust::coding_agent::agent_session::AgentSessionUnsubscribe>,
    session: Arc<pi_rust::coding_agent::agent_session::AgentSession>,
    finished: bool,
}
impl Drop for PromptActivity {
    fn drop(&mut self) {
        if let Some(listener) = self.listener.take() {
            listener.unsubscribe();
        }
        if !self.finished {
            self.session.agent.abort();
        }
    }
}
fn add_usage(before: &Value, usage: &Value) -> Value {
    let mut total = json!({});
    for field in [
        "input",
        "output",
        "cacheRead",
        "cacheWrite",
        "reasoning",
        "totalTokens",
    ] {
        let before = before[field].as_f64().unwrap_or(0.0).max(0.0).round() as u64;
        let current = usage[field].as_f64().unwrap_or(0.0).max(0.0).round() as u64;
        total[field] = json!(before.saturating_add(current));
    }
    total
}

#[cfg(test)]
mod completion_regressions {
    use super::*;
    use pi_rust::{
        agent_core::{Agent, AgentInitialState, AgentOptions},
        ai::{
            auth::{
                credential_store::{CredentialStore, InMemoryCredentialStore},
                types::{ApiKeyCredential, AuthOperationOptions, Credential},
            },
            models::{
                create_models,
                faux::{FauxModelDefinition, FauxProviderOptions},
                faux_assistant_message, faux_provider, CreateModelsOptions, FauxProviderHandle,
            },
        },
        coding_agent::{
            agent_session::{AgentSession, AgentSessionConfig, ExtensionBindings},
            core::{
                model_runtime::{CreateModelRuntimeOptions, ModelRuntime},
                models_store::InMemoryCodingAgentModelsStore,
                resource_loader::{DefaultResourceLoader, DefaultResourceLoaderOptions},
                settings_manager::SettingsManager,
            },
            extensions::types::HandlerFn,
        },
    };

    struct NativeFixture {
        session: Arc<AgentSession>,
        faux: FauxProviderHandle,
        dir: std::path::PathBuf,
    }
    impl Drop for NativeFixture {
        fn drop(&mut self) {
            self.session.dispose();
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
    async fn native_fixture(factories: Vec<ExtensionFactory>) -> NativeFixture {
        let dir = std::env::temp_dir().join(format!("pisper-completion-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let cwd = dir.to_string_lossy().into_owned();
        let faux = faux_provider(FauxProviderOptions {
            provider: Some("anthropic".into()),
            api: Some("anthropic-messages".into()),
            models: vec![FauxModelDefinition {
                id: "claude-4-5".into(),
                ..Default::default()
            }],
            ..Default::default()
        });
        faux.set_responses(vec![
            faux_assistant_message("first proof", Default::default()).into(),
            faux_assistant_message("completion proof", Default::default()).into(),
        ]);
        let mut models = create_models(CreateModelsOptions::default());
        models.set_provider(faux.provider.clone());
        let agent = Agent::new(
            AgentOptions {
                initial_state: AgentInitialState {
                    model: Some(faux.get_model(None).unwrap()),
                    system_prompt: Some("offline proof".into()),
                    ..Default::default()
                },
                ..Default::default()
            },
            Arc::new(models),
        );
        let credentials = Arc::new(InMemoryCredentialStore::default());
        credentials
            .modify(
                "anthropic",
                Box::new(|_| {
                    Box::pin(async {
                        Ok(Some(Credential::ApiKey(ApiKeyCredential {
                            key: Some("isolated-fixture".into()),
                            ..Default::default()
                        })))
                    })
                }),
                &AuthOperationOptions::default(),
            )
            .await
            .unwrap();
        let model_runtime = ModelRuntime::create(CreateModelRuntimeOptions {
            credentials: Some(credentials),
            models_path: Some(None),
            models_store: Some(Arc::new(InMemoryCodingAgentModelsStore::default())),
            allow_model_network: false,
            refresh_on_create: Some(false),
            ..Default::default()
        })
        .await
        .unwrap();
        let settings_manager =
            SettingsManager::create_with(&cwd, &cwd, Default::default()).unwrap();
        let mut loader = DefaultResourceLoader::new(DefaultResourceLoaderOptions {
            cwd: cwd.clone(),
            agent_dir: cwd.clone(),
            extension_factories: factories
                .into_iter()
                .map(InlineExtension::Factory)
                .collect(),
            no_skills: true,
            no_prompt_templates: true,
            no_themes: true,
            no_context_files: true,
            ..Default::default()
        });
        loader.reload_without_trust().unwrap();
        let session_manager = Arc::new(Mutex::new(
            SessionManager::create(&cwd, Some(&dir.join("sessions").to_string_lossy()), None)
                .unwrap(),
        ));
        let session = AgentSession::new(AgentSessionConfig {
            agent: Arc::new(agent),
            session_manager,
            settings_manager,
            cwd,
            scoped_models: Vec::new(),
            resource_loader: Arc::new(Mutex::new(loader)),
            custom_tools: Vec::new(),
            model_runtime,
            cache_warmer: None,
            initial_active_tool_names: None,
            uses_default_tools: None,
            allowed_tool_names: None,
            excluded_tool_names: None,
            base_tools_override: Vec::new(),
            session_start_event: None,
            html_exporter: None,
        })
        .unwrap();
        session
            .bind_extensions(ExtensionBindings::default())
            .await
            .unwrap();
        NativeFixture { session, faux, dir }
    }
    fn gate(
        kind: &'static str,
        receipt: Option<&'static str>,
    ) -> (
        ExtensionFactory,
        Arc<tokio::sync::Notify>,
        Arc<tokio::sync::Notify>,
    ) {
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let start = entered.clone();
        let finish = release.clone();
        let factory: ExtensionFactory = Arc::new(move |api| {
            let entered = start.clone();
            let release = finish.clone();
            let handler: HandlerFn = Arc::new(move |event, _| {
                let entered = entered.clone();
                let release = release.clone();
                Box::pin(async move {
                    if receipt.is_none_or(|receipt| {
                        event["message"]["role"] == "user"
                            && event["message"]["content"].to_string().contains(receipt)
                    }) {
                        entered.notify_one();
                        release.notified().await;
                    }
                    Ok(None)
                })
            });
            api.on(kind, handler).map(|_| ())
        });
        (factory, entered, release)
    }
    #[tokio::test]
    async fn consumed_completion_remains_in_flight_until_real_jsonl_append() {
        const RECEIPT: &str = "[Pisper delivery isolated-real-queue]";
        let (factory, entered, release) = gate("message_start", Some(RECEIPT));
        let fixture = native_fixture(vec![factory]).await;
        fixture
            .session
            .follow_up(RECEIPT, None, None)
            .await
            .unwrap();
        let session = fixture.session.clone();
        let running = tokio::spawn(async move { session.prompt("begin", None).await });
        tokio::time::timeout(Duration::from_secs(5), entered.notified())
            .await
            .unwrap();
        assert!(fixture.session.get_follow_up_messages().is_empty());
        assert!(fixture
            .session
            .get_in_flight_user_messages()
            .iter()
            .any(|text| text.contains(RECEIPT)));
        assert!(!delivery_recorded(&fixture.session, RECEIPT).unwrap());
        assert!(fresh_delivery_message(
            &fixture.session,
            &[json!({"id":"proof"})],
            &[RECEIPT.into()]
        )
        .unwrap()
        .is_empty());
        release.notify_one();
        tokio::time::timeout(Duration::from_secs(5), running)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(delivery_recorded(&fixture.session, RECEIPT).unwrap());
        assert!(fixture.session.get_in_flight_user_messages().is_empty());
        assert_eq!(fixture.faux.state().lock().unwrap().call_count, 2);
    }
    #[tokio::test]
    async fn failed_real_jsonl_append_never_acknowledges_memory_receipt() {
        let fixture = native_fixture(Vec::new()).await;
        let path = fixture.session.session_file().unwrap();
        let parent = std::path::Path::new(&path).parent().unwrap();
        assert!(parent.starts_with(&fixture.dir));
        std::fs::remove_dir_all(parent).unwrap();
        std::fs::write(parent, b"block the owned session directory").unwrap();
        let receipt = "[Pisper delivery failed-disk-write]";
        assert!(fixture.session.prompt(receipt, None).await.is_err());
        assert!(fixture.session.message_persistence_error().is_some());
        assert!(fixture.session.session_manager.lock().unwrap().get_branch(None).iter().any(|entry| {
            matches!(entry, pi_rust::coding_agent::session_manager::SessionEntry::Message(entry) if entry.message.role() == "user")
        }));
        assert!(delivery_recorded(&fixture.session, receipt).is_err());
        assert!(fixture
            .session
            .prompt("must not pay again", None)
            .await
            .is_err());
        assert_eq!(fixture.faux.state().lock().unwrap().call_count, 0);
    }
    #[tokio::test]
    async fn suspended_preflight_never_starts_a_native_model_call() {
        let (factory, entered, release) = gate("before_agent_start", None);
        let fixture = native_fixture(vec![factory]).await;
        let cancellation = tokio_util::sync::CancellationToken::new();
        let session = fixture.session.clone();
        let token = cancellation.clone();
        let running = tokio::spawn(async move {
            session
                .prompt(
                    "pending completion",
                    Some(PromptOptions {
                        start_cancellation: Some(token),
                        ..Default::default()
                    }),
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), entered.notified())
            .await
            .unwrap();
        cancellation.cancel();
        fixture.session.abort().await;
        release.notify_one();
        assert!(tokio::time::timeout(Duration::from_secs(5), running)
            .await
            .unwrap()
            .unwrap()
            .is_err());
        assert_eq!(fixture.faux.state().lock().unwrap().call_count, 0);
        assert!(!fixture.session.is_streaming());
        assert!(fixture.session.get_in_flight_user_messages().is_empty());
    }
}
