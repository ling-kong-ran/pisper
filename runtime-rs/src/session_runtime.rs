//! Resident Pi runtimes: every session has its own agent, queue and mutation lock.
//! Configuration uses a separate runtime; it never replaces a running conversation.
use crate::{ApiError, AppState};
use axum::http::StatusCode;
use pi_rust::coding_agent::{
    agent_session::{AgentSession, AgentSessionUnsubscribe},
    core::agent_session_runtime::{
        create_agent_session_runtime, AgentSessionRuntime, CreateAgentSessionRuntimeFactory,
        CreateAgentSessionRuntimeOptions,
    },
    modes::json_event::to_json_event_string,
    session_manager::SessionManager,
};
use serde_json::json;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, Weak,
    },
    time::Duration,
};

const MAX_RESIDENT: usize = 3;
const IDLE_TTL: Duration = Duration::from_secs(5 * 60);

fn execution_mode(state: &AppState, id: &str) -> String {
    let owner = state.executor.tool_owner(id);
    let configured = state
        .session_meta
        .lock()
        .expect("session metadata")
        .get(&owner)
        .and_then(|meta| meta.execution_mode.clone())
        .unwrap_or_default();
    crate::execution_modes::normalize(&configured)
        .unwrap_or(crate::execution_modes::DEFAULT_EXECUTION_MODE)
        .to_owned()
}

pub(crate) struct HostedSession {
    pub(crate) runtime: Arc<AgentSessionRuntime>,
    pub(crate) mutation: Arc<tokio::sync::Mutex<()>>,
    /// Owns a complete public run, including gaps between Goal continuation rounds.
    pub(crate) run: Arc<tokio::sync::Mutex<()>>,
    last_accessed: AtomicU64,
    listener: Mutex<Option<AgentSessionUnsubscribe>>,
    invalid: AtomicBool,
    removed: AtomicBool,
    tool_revision: AtomicU64,
    retired: AtomicBool,
    mode_snapshot: String,
}
impl HostedSession {
    pub(crate) fn ensure_present(&self) -> Result<(), ApiError> {
        if self.removed.load(Ordering::Acquire) {
            return Err(ApiError::new(
                StatusCode::NOT_FOUND,
                "session_not_found",
                "会话已删除。",
            ));
        }
        if self.retired.load(Ordering::Acquire) {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "session_runtime_replaced",
                "会话工具配置已更新，请重新请求。",
            ));
        }
        Ok(())
    }
    pub(crate) fn session(&self) -> Arc<AgentSession> {
        self.runtime.session()
    }
    pub(crate) fn busy(&self) -> bool {
        let session = self.session();
        session.is_streaming()
            || session.is_compacting()
            || self.mutation.try_lock().is_err()
            || self.run.try_lock().is_err()
    }
    pub(crate) fn rebind(&self, state: &AppState) {
        let session = self.session();
        if let Err(error) = state.plugin_integration.apply_loadout(session.clone()) {
            tracing::warn!(error=%crate::security::redact_secret_text(&error.message), "Tool configuration could not be restored");
        }
        crate::tool_policy::install(
            session.clone(),
            state.session_meta.clone(),
            state.approvals.clone(),
        );
        if let Err(error) = crate::asset_api::tracker::install_context_with_file_changes(
            session.clone(),
            state.asset_tracker.clone(),
            state.file_changes.clone(),
            session.session_id(),
            Some(crate::session_runtime::asset_event_sink(state)),
        ) {
            tracing::warn!(error=%crate::security::redact_secret_text(&error.to_string()), "Workspace asset capture could not be attached");
        }
        let id = session.session_id();
        let tx = state.events.clone();
        let listener = session.subscribe(Arc::new(move |event| {
            if let Ok(raw) = to_json_event_string(event).and_then(|raw| {
                serde_json::from_str::<serde_json::Value>(&raw).map_err(|error| error.to_string())
            }) {
                let _ =
                    tx.send(json!({"pisperEvent":"engine","sessionId":id,"data":raw}).to_string());
            }
        }));
        if let Some(previous) = self
            .listener
            .lock()
            .expect("resident listener")
            .replace(listener)
        {
            previous.unsubscribe();
        }
    }
    pub(crate) fn touch(&self) {
        self.last_accessed
            .store(crate::product::now_ms(), Ordering::Relaxed);
    }
    pub(crate) async fn abort(&self) -> Result<(), ApiError> {
        if tokio::time::timeout(Duration::from_secs(10), self.session().abort())
            .await
            .is_err()
        {
            self.invalid.store(true, Ordering::Release);
            self.session().dispose();
            return Err(ApiError::new(
                StatusCode::REQUEST_TIMEOUT,
                "abort_timeout",
                "停止已超时，原生会话已强制释放。",
            ));
        }
        Ok(())
    }
}
impl Drop for HostedSession {
    fn drop(&mut self) {
        if let Some(listener) = self.listener.lock().expect("resident listener").take() {
            listener.unsubscribe();
        }
    }
}

pub(crate) struct SessionRuntimeRegistry {
    factory: CreateAgentSessionRuntimeFactory,
    entries: Mutex<HashMap<String, Arc<HostedSession>>>,
    creating: tokio::sync::Mutex<()>,
    closed: AtomicBool,
    tool_revision: AtomicU64,
    built_revisions: Mutex<HashMap<String, (Weak<AgentSessionRuntime>, u64, String)>>,
}
impl SessionRuntimeRegistry {
    pub(crate) fn new(factory: CreateAgentSessionRuntimeFactory) -> Self {
        Self {
            factory,
            entries: Mutex::new(HashMap::new()),
            creating: tokio::sync::Mutex::new(()),
            closed: AtomicBool::new(false),
            tool_revision: AtomicU64::new(0),
            built_revisions: Mutex::new(HashMap::new()),
        }
    }
    /// Observation doesn't extend the idle TTL; listing tabs must not retain every agent.
    pub(crate) fn get(&self, id: &str) -> Option<Arc<HostedSession>> {
        self.entries
            .lock()
            .expect("resident sessions")
            .get(id)
            .cloned()
    }
    pub(crate) fn all(&self) -> Vec<Arc<HostedSession>> {
        self.entries
            .lock()
            .expect("resident sessions")
            .values()
            .cloned()
            .collect()
    }
    /// 工具上下文可能持有底层 ID；浏览器和截图按客户端公共会话 ID 隔离。
    pub(crate) fn public_id_for_native(&self, native_id: &str) -> Option<String> {
        let sessions = self
            .entries
            .lock()
            .expect("resident sessions")
            .iter()
            .map(|(id, host)| (id.clone(), host.clone()))
            .collect::<Vec<_>>();
        sessions
            .into_iter()
            .find(|(_, host)| host.session().session_id() == native_id)
            .map(|(id, _)| id)
    }
    pub(crate) fn busy(&self, id: &str) -> bool {
        self.get(id).is_some_and(|host| host.busy())
    }
    pub(crate) fn any_busy(&self) -> bool {
        self.all().iter().any(|host| host.busy())
    }
    pub(crate) fn refresh_tools(&self) {
        self.tool_revision.fetch_add(1, Ordering::AcqRel);
    }
    pub(crate) async fn build_runtime(
        &self,
        state: &AppState,
        manager: SessionManager,
    ) -> Result<Arc<AgentSessionRuntime>, ApiError> {
        let tool_revision = self.tool_revision.load(Ordering::Acquire);
        let mode_snapshot = execution_mode(state, manager.get_session_id());
        let cwd = manager.get_cwd().to_string();
        let runtime = create_agent_session_runtime(
            self.factory.clone(),
            CreateAgentSessionRuntimeOptions {
                cwd,
                agent_dir: state.agent_dir.clone(),
                session_manager: Arc::new(Mutex::new(manager)),
                session_start_event: None,
                project_trust_context: None,
            },
        )
        .await
        .map_err(|error| ApiError::internal(error.to_string()))?;
        let runtime = Arc::new(runtime);
        if let Err(error) = state.providers.restore(&runtime).await {
            let _ = runtime.dispose().await;
            return Err(ApiError::internal(error));
        }
        if let Err(error) = runtime
            .session()
            .bind_extensions(crate::extension_bindings())
            .await
        {
            let _ = runtime.dispose().await;
            return Err(ApiError::internal(error.to_string()));
        }
        if let Err(error) = state.plugin_integration.apply_loadout(runtime.session()) {
            let _ = runtime.dispose().await;
            return Err(ApiError::internal(error.message));
        }
        let mut built = self.built_revisions.lock().expect("built tool revisions");
        built.retain(|_, (runtime, _, _)| runtime.strong_count() > 0);
        built.insert(
            runtime.session().session_id(),
            (Arc::downgrade(&runtime), tool_revision, mode_snapshot),
        );
        drop(built);
        Ok(runtime)
    }
    pub(crate) fn insert(
        &self,
        state: &AppState,
        runtime: Arc<AgentSessionRuntime>,
    ) -> Result<Arc<HostedSession>, ApiError> {
        let id = runtime.session().session_id();
        let (revision, mode_snapshot) = self
            .built_revisions
            .lock()
            .expect("built tool revisions")
            .remove(&id)
            .filter(|(built, _, _)| {
                built
                    .upgrade()
                    .is_some_and(|built| Arc::ptr_eq(&built, &runtime))
            })
            .map(|(_, revision, mode)| (revision, mode))
            .unwrap_or_else(|| (0, execution_mode(state, &id)));
        let hosted = Arc::new(HostedSession {
            runtime,
            mutation: Arc::new(tokio::sync::Mutex::new(())),
            run: Arc::new(tokio::sync::Mutex::new(())),
            last_accessed: AtomicU64::new(crate::product::now_ms()),
            listener: Mutex::new(None),
            invalid: AtomicBool::new(false),
            removed: AtomicBool::new(false),
            tool_revision: AtomicU64::new(revision),
            retired: AtomicBool::new(false),
            mode_snapshot,
        });
        hosted.rebind(state);
        let mut entries = self.entries.lock().expect("resident sessions");
        if self.closed.load(Ordering::Acquire) {
            return Err(ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "runtime_shutdown",
                "运行时正在关闭。",
            ));
        }
        if entries.contains_key(&id) {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "session_hosted",
                "会话已在运行。",
            ));
        }
        entries.insert(id, hosted.clone());
        Ok(hosted)
    }
    pub(crate) async fn host(
        &self,
        state: &AppState,
        id: &str,
    ) -> Result<Arc<HostedSession>, ApiError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "runtime_shutdown",
                "运行时正在关闭。",
            ));
        }
        if let Some(hosted) = self.get(id) {
            if !hosted.invalid.load(Ordering::Acquire)
                && ((hosted.tool_revision.load(Ordering::Acquire)
                    == self.tool_revision.load(Ordering::Acquire)
                    && hosted.mode_snapshot == execution_mode(state, id))
                    || hosted.busy())
            {
                hosted.touch();
                return Ok(hosted);
            }
            if hosted.busy() {
                return Err(ApiError::new(
                    StatusCode::CONFLICT,
                    "session_busy",
                    "会话正在结束，请稍后重试。",
                ));
            }
        }
        let _creating = self.creating.lock().await;
        if let Some(hosted) = self.get(id) {
            if !hosted.invalid.load(Ordering::Acquire)
                && ((hosted.tool_revision.load(Ordering::Acquire)
                    == self.tool_revision.load(Ordering::Acquire)
                    && hosted.mode_snapshot == execution_mode(state, id))
                    || hosted.busy())
            {
                hosted.touch();
                return Ok(hosted);
            }
            if hosted.busy() {
                return Err(ApiError::new(
                    StatusCode::CONFLICT,
                    "session_busy",
                    "会话正在结束，请稍后重试。",
                ));
            }
            // 持有两条实际准入租约后才能退休旧注册表，封住 busy() 瞬时检查的竞态。
            let _run = match hosted.run.clone().try_lock_owned() {
                Ok(guard) => guard,
                Err(_) if !hosted.invalid.load(Ordering::Acquire) => return Ok(hosted),
                Err(_) => {
                    return Err(ApiError::new(
                        StatusCode::CONFLICT,
                        "session_busy",
                        "会话正在结束，请稍后重试。",
                    ))
                }
            };
            let _mutation = match hosted.mutation.clone().try_lock_owned() {
                Ok(guard) => guard,
                Err(_) if !hosted.invalid.load(Ordering::Acquire) => return Ok(hosted),
                Err(_) => {
                    return Err(ApiError::new(
                        StatusCode::CONFLICT,
                        "session_busy",
                        "会话正在结束，请稍后重试。",
                    ))
                }
            };
            if hosted.session().is_streaming() || hosted.session().is_compacting() {
                return Err(ApiError::new(
                    StatusCode::CONFLICT,
                    "session_busy",
                    "会话正在结束，请稍后重试。",
                ));
            }
            hosted.retired.store(true, Ordering::Release);
            self.entries.lock().expect("resident sessions").remove(id);
            hosted
                .runtime
                .dispose()
                .await
                .map_err(|error| ApiError::internal(error.to_string()))?;
        }
        let _configuration = state.engine_mutation.read().await;
        let manager = SessionManager::open(
            &crate::session_api::find_session_path(state, id)?,
            None,
            None,
        )
        .map_err(|error| ApiError::internal(error.to_string()))?;
        let runtime = self.build_runtime(state, manager).await?;
        let hosted = self.insert(state, runtime.clone());
        if hosted.is_err() {
            let _ = runtime.dispose().await;
        }
        hosted
    }
    pub(crate) async fn remove(&self, state: &AppState, id: &str) -> Result<(), ApiError> {
        self.remove_inner(state, id, None).await
    }
    pub(crate) async fn delete(
        &self,
        state: &AppState,
        id: &str,
        journal: &str,
    ) -> Result<(), ApiError> {
        self.remove_inner(state, id, Some(journal)).await
    }
    async fn remove_inner(
        &self,
        state: &AppState,
        id: &str,
        journal: Option<&str>,
    ) -> Result<(), ApiError> {
        let _creating = self.creating.lock().await;
        let hosted = self.get(id);
        // Keep stale HTTP references from reserving a run after the busy check.
        let _run = hosted
            .as_ref()
            .map(|host| host.run.clone().try_lock_owned())
            .transpose()
            .map_err(|_| {
                ApiError::new(StatusCode::CONFLICT, "session_busy", "请先停止当前会话。")
            })?;
        let _mutation = hosted
            .as_ref()
            .map(|host| host.mutation.clone().try_lock_owned())
            .transpose()
            .map_err(|_| {
                ApiError::new(StatusCode::CONFLICT, "session_busy", "请先停止当前会话。")
            })?;
        if hosted
            .as_ref()
            .is_some_and(|host| host.session().is_streaming() || host.session().is_compacting())
        {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "session_busy",
                "请先停止当前会话。",
            ));
        }
        // The creation lease spans deleting the public journal. A host cannot
        // reopen it between eviction and the caller's domain cleanup.
        if journal.is_some() {
            let _ = state.browser.close_session(id).await;
        }
        if let Some(journal) = journal {
            std::fs::remove_file(journal).map_err(|error| ApiError::internal(error.to_string()))?;
        }
        if let Some(host) = &hosted {
            host.removed.store(true, Ordering::Release);
        }
        self.entries.lock().expect("resident sessions").remove(id);
        if let Some(hosted) = hosted {
            state.approvals.cancel_session(id);
            state.asset_tracker.cancel_session(id);
            state.asset_tracker.wait_session(id).await;
            state.memory_tasks.forget_session(id);
            hosted
                .runtime
                .dispose()
                .await
                .map_err(|error| ApiError::internal(error.to_string()))?;
        }
        Ok(())
    }
    /// Protected sessions can exceed the cache ceiling, matching release's LRU policy.
    pub(crate) async fn sweep(&self, state: &AppState, except: &str) -> Result<usize, ApiError> {
        let _creating = self.creating.lock().await;
        let now = crate::product::now_ms();
        let mut removed = Vec::new();
        {
            let mut entries = self.entries.lock().expect("resident sessions");
            let mut candidates: Vec<_> = entries
                .iter()
                .filter(|(id, host)| {
                    id.as_str() != except
                        && !host.busy()
                        && !state.goals.busy(id)
                        && Arc::strong_count(host) == 1
                })
                .map(|(id, host)| (id.clone(), host.last_accessed.load(Ordering::Relaxed)))
                .collect();
            candidates.sort_by_key(|(_, accessed)| *accessed);
            for (id, accessed) in candidates {
                if now.saturating_sub(accessed) < IDLE_TTL.as_millis() as u64
                    && entries.len() <= MAX_RESIDENT
                {
                    continue;
                }
                if let Some(hosted) = entries.remove(&id) {
                    removed.push((id, hosted));
                }
            }
        }
        let count = removed.len();
        let mut first_error = None;
        for (id, hosted) in removed {
            state
                .approvals
                .resolve_session(&id, false, "会话运行时已从内存释放，请重新发送消息。");
            state.memory_tasks.forget_session(&id);
            state.asset_tracker.wait_session(&id).await;
            if let Err(error) = hosted.runtime.dispose().await {
                first_error.get_or_insert_with(|| ApiError::internal(error.to_string()));
            }
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(count)
    }
    pub(crate) async fn shutdown(&self, state: &AppState) -> Result<(), ApiError> {
        self.closed.store(true, Ordering::Release);
        let _creating = self.creating.lock().await;
        let sessions = std::mem::take(&mut *self.entries.lock().expect("resident sessions"));
        let results =
            futures::future::join_all(sessions.into_iter().map(|(id, hosted)| async move {
                state.approvals.cancel_session(&id);
                state.asset_tracker.cancel_session(&id);
                state.memory_tasks.forget_session(&id);
                let aborted = hosted.abort().await;
                state.asset_tracker.wait_session(&id).await;
                let disposed =
                    tokio::time::timeout(Duration::from_secs(20), hosted.runtime.dispose())
                        .await
                        .map_err(|_| ApiError::internal("Session cleanup timed out"))?
                        .map_err(|error| ApiError::internal(error.to_string()));
                aborted.and(disposed)
            }))
            .await;
        for result in results {
            result?;
        }
        Ok(())
    }
}

pub(crate) fn asset_event_sink(state: &AppState) -> crate::asset_api::tracker::AssetEventSink {
    let events = state.events.clone();
    Arc::new(move |session_id, assets| {
        let events = events.clone();
        Box::pin(async move {
            for asset in assets {
                let _ = events.send(json!({"pisperEvent":"generated_asset","sessionId":session_id,"data":crate::asset_api::projection::attachment(&asset)}).to_string());
            }
        })
    })
}

pub(crate) async fn hosted(state: &AppState, id: &str) -> Result<Arc<HostedSession>, ApiError> {
    state.sessions.host(state, id).await
}

pub(crate) struct SessionMutation {
    pub(crate) hosted: Arc<HostedSession>,
    _session: tokio::sync::OwnedMutexGuard<()>,
    _configuration: tokio::sync::OwnedRwLockReadGuard<()>,
    _run: Option<tokio::sync::OwnedMutexGuard<()>>,
}
pub(crate) async fn mutation(state: &AppState, id: &str) -> Result<SessionMutation, ApiError> {
    acquire_mutation(state, id, true).await
}
/// GoalRunner already holds the whole-run lease; individual Pi rounds take only
/// the native mutation and configuration locks.
pub(crate) async fn prompt_mutation(
    state: &AppState,
    id: &str,
) -> Result<SessionMutation, ApiError> {
    acquire_mutation(state, id, false).await
}
async fn acquire_mutation(
    state: &AppState,
    id: &str,
    reserve_run: bool,
) -> Result<SessionMutation, ApiError> {
    let hosted = hosted(state, id).await?;
    let run = if reserve_run {
        Some(hosted.run.clone().try_lock_owned().map_err(|_| {
            ApiError::new(StatusCode::CONFLICT, "session_busy", "当前会话正在运行。")
        })?)
    } else {
        None
    };
    let session =
        hosted.mutation.clone().try_lock_owned().map_err(|_| {
            ApiError::new(StatusCode::CONFLICT, "session_busy", "当前会话正在运行。")
        })?;
    let configuration = state
        .engine_mutation
        .clone()
        .try_read_owned()
        .map_err(|_| {
            ApiError::new(
                StatusCode::CONFLICT,
                "configuration_busy",
                "模型配置正在更新，请稍后重试。",
            )
        })?;
    hosted.ensure_present()?;
    Ok(SessionMutation {
        hosted,
        _session: session,
        _configuration: configuration,
        _run: run,
    })
}
