//! Full lifecycle port of upstream `agent-session-runtime.ts`.
//!
//! Owns a replaceable, real AgentSession and cwd-bound services. The factory is
//! supplied by the caller; this is not an implementation of SDK initialization.
//! Callbacks/factories are native poll-driven futures, not JS eager promises.
//! No state/manager mutex guard crosses an await or a user callback. Reads after
//! yields intentionally consult the live slot, matching upstream reentrancy.
//! Filesystem failures retain native OS errors (not Node error/stack formatting).
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, bail, Result};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub use super::agent_session_services::{AgentSessionRuntimeDiagnostic, AgentSessionServices};
use super::session_cwd::assert_session_cwd_exists;
use crate::coding_agent::agent_session::AgentSession;
use crate::coding_agent::extensions::runner::emit_session_shutdown_event;
use crate::coding_agent::extensions::types::{
    ExtensionCommandContext, LoadExtensionsResult, ProjectTrustContext, SessionStartEvent,
    SessionStartReason,
};
use crate::coding_agent::session_manager::{NewSessionOptions, SessionManager};
use crate::coding_agent::utils::paths::resolve_path_auto_base;

pub type SessionManagerRef = Arc<Mutex<SessionManager>>;
pub type RuntimeFuture<T> = BoxFuture<'static, Result<T>>;
pub type CreateAgentSessionRuntimeFactory = Arc<
    dyn Fn(CreateAgentSessionRuntimeOptions) -> RuntimeFuture<CreateAgentSessionRuntimeResult>
        + Send
        + Sync,
>;
pub type RebindSession = Arc<dyn Fn(Arc<AgentSession>) -> RuntimeFuture<()> + Send + Sync>;
pub type BeforeSessionInvalidate = Arc<dyn Fn() -> Result<()> + Send + Sync>;
pub type WithSession = Arc<dyn Fn(ExtensionCommandContext) -> RuntimeFuture<()> + Send + Sync>;
pub type SetupSession = Arc<dyn Fn(SessionManagerRef) -> RuntimeFuture<()> + Send + Sync>;
pub type ProjectTrustContextFactory =
    Arc<dyn Fn(&str) -> Result<ProjectTrustContext> + Send + Sync>;

#[derive(Clone)]
pub struct CreateAgentSessionRuntimeOptions {
    pub cwd: String,
    pub agent_dir: String,
    pub session_manager: SessionManagerRef,
    pub session_start_event: Option<SessionStartEvent>,
    pub project_trust_context: Option<ProjectTrustContext>,
}

pub struct CreateAgentSessionRuntimeResult {
    pub session: Arc<AgentSession>,
    pub extensions_result: LoadExtensionsResult,
    pub model_fallback_message: Option<String>,
    pub services: Arc<AgentSessionServices>,
    pub diagnostics: Vec<AgentSessionRuntimeDiagnostic>,
}

#[derive(Clone, Default)]
pub struct SwitchSessionOptions {
    pub cwd_override: Option<String>,
    pub with_session: Option<WithSession>,
    pub project_trust_context_factory: Option<ProjectTrustContextFactory>,
}
#[derive(Clone, Default)]
pub struct NewSessionOptionsRuntime {
    pub parent_session: Option<String>,
    pub setup: Option<SetupSession>,
    pub with_session: Option<WithSession>,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ForkPosition {
    #[default]
    Before,
    At,
}
#[derive(Clone, Default)]
pub struct ForkOptions {
    pub position: ForkPosition,
    pub with_session: Option<WithSession>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionReplacementResult {
    pub cancelled: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForkResult {
    pub cancelled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionImportFileNotFoundError {
    pub file_path: String,
}
impl fmt::Display for SessionImportFileNotFoundError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "File not found: {}", self.file_path)
    }
}
impl std::error::Error for SessionImportFileNotFoundError {}

struct CurrentRuntime {
    session: Arc<AgentSession>,
    services: Arc<AgentSessionServices>,
    diagnostics: Vec<AgentSessionRuntimeDiagnostic>,
    model_fallback_message: Option<String>,
    rebind_session: Option<RebindSession>,
    before_session_invalidate: Option<BeforeSessionInvalidate>,
}

pub struct AgentSessionRuntime {
    current: Mutex<CurrentRuntime>,
    create_runtime: CreateAgentSessionRuntimeFactory,
}
impl AgentSessionRuntime {
    pub fn new(
        session: Arc<AgentSession>,
        services: Arc<AgentSessionServices>,
        create_runtime: CreateAgentSessionRuntimeFactory,
        diagnostics: Vec<AgentSessionRuntimeDiagnostic>,
        model_fallback_message: Option<String>,
    ) -> Self {
        Self {
            current: Mutex::new(CurrentRuntime {
                session,
                services,
                diagnostics,
                model_fallback_message,
                rebind_session: None,
                before_session_invalidate: None,
            }),
            create_runtime,
        }
    }
    pub fn session(&self) -> Arc<AgentSession> {
        self.current.lock().expect("runtime slot").session.clone()
    }
    pub fn services(&self) -> Arc<AgentSessionServices> {
        self.current.lock().expect("runtime slot").services.clone()
    }
    pub fn cwd(&self) -> String {
        self.services().cwd.clone()
    }
    pub fn diagnostics(&self) -> Vec<AgentSessionRuntimeDiagnostic> {
        self.current
            .lock()
            .expect("runtime slot")
            .diagnostics
            .clone()
    }
    pub fn model_fallback_message(&self) -> Option<String> {
        self.current
            .lock()
            .expect("runtime slot")
            .model_fallback_message
            .clone()
    }
    pub fn set_rebind_session(&self, rebind: Option<RebindSession>) {
        self.current.lock().expect("runtime slot").rebind_session = rebind;
    }
    pub fn set_before_session_invalidate(&self, before: Option<BeforeSessionInvalidate>) {
        self.current
            .lock()
            .expect("runtime slot")
            .before_session_invalidate = before;
    }

    async fn emit_before_switch(
        &self,
        reason: &str,
        target: Option<&str>,
    ) -> SessionReplacementResult {
        let runner = self.session().extension_runner();
        if !runner.has_handlers("session_before_switch") {
            return SessionReplacementResult { cancelled: false };
        }
        let mut event = json!({"type":"session_before_switch", "reason":reason});
        if let Some(target) = target {
            event["targetSessionFile"] = target.into();
        }
        let result = runner.emit(&mut event).await;
        // Distinct from the generic runner's JS-truthy short circuit.
        SessionReplacementResult {
            cancelled: result.as_ref().and_then(|v| v.get("cancel")) == Some(&Value::Bool(true)),
        }
    }
    async fn emit_before_fork(
        &self,
        entry_id: &str,
        position: ForkPosition,
    ) -> SessionReplacementResult {
        let runner = self.session().extension_runner();
        if !runner.has_handlers("session_before_fork") {
            return SessionReplacementResult { cancelled: false };
        }
        let result = runner
            .emit(
                &mut json!({"type":"session_before_fork", "entryId":entry_id, "position":position}),
            )
            .await;
        SessionReplacementResult {
            cancelled: result.as_ref().and_then(|v| v.get("cancel")) == Some(&Value::Bool(true)),
        }
    }
    fn invalidate_current(&self) -> Result<()> {
        let before = self
            .current
            .lock()
            .expect("runtime slot")
            .before_session_invalidate
            .clone();
        if let Some(before) = before {
            before()?;
        }
        self.session().dispose();
        Ok(())
    }
    async fn teardown_current(&self, reason: &str, target: Option<String>) -> Result<()> {
        self.session().abort().await;
        let mut event = json!({"type":"session_shutdown", "reason":reason});
        if let Some(target) = target {
            event["targetSessionFile"] = target.into();
        }
        emit_session_shutdown_event(&self.session().extension_runner(), &event).await;
        self.invalidate_current()
    }
    fn apply(&self, result: CreateAgentSessionRuntimeResult) {
        let mut current = self.current.lock().expect("runtime slot");
        current.session = result.session;
        current.services = result.services;
        current.diagnostics = result.diagnostics;
        current.model_fallback_message = result.model_fallback_message;
    }
    async fn finish_session_replacement(&self, with_session: Option<WithSession>) -> Result<()> {
        let rebind = self
            .current
            .lock()
            .expect("runtime slot")
            .rebind_session
            .clone();
        if let Some(rebind) = rebind {
            rebind(self.session()).await?;
        }
        if let Some(with_session) = with_session {
            with_session(self.session().create_replaced_session_context()?).await?;
        }
        Ok(())
    }
    fn create_options(
        &self,
        cwd: String,
        session_manager: SessionManagerRef,
        reason: SessionStartReason,
        previous_session_file: Option<String>,
    ) -> CreateAgentSessionRuntimeOptions {
        CreateAgentSessionRuntimeOptions {
            cwd,
            agent_dir: self.services().agent_dir.clone(),
            session_manager,
            session_start_event: Some(SessionStartEvent {
                event_type: "session_start".into(),
                reason,
                previous_session_file,
            }),
            project_trust_context: None,
        }
    }

    pub async fn switch_session(
        &self,
        session_path: &str,
        options: SwitchSessionOptions,
    ) -> Result<SessionReplacementResult> {
        let before = self.emit_before_switch("resume", Some(session_path)).await;
        if before.cancelled {
            return Ok(before);
        }
        let previous = self.session().session_file();
        let manager = SessionManager::open(session_path, None, options.cwd_override.as_deref())?;
        assert_session_cwd_exists(&manager, &self.cwd())?;
        self.teardown_current("resume", manager.get_session_file().map(str::to_owned))
            .await?;
        let cwd = manager.get_cwd().to_owned();
        let mut create = self.create_options(
            cwd.clone(),
            Arc::new(Mutex::new(manager)),
            SessionStartReason::Resume,
            previous,
        );
        // Called after teardown, not as an eager preflight before invalidation.
        create.project_trust_context = options
            .project_trust_context_factory
            .map(|factory| factory(&cwd))
            .transpose()?;
        self.apply((self.create_runtime)(create).await?);
        self.finish_session_replacement(options.with_session)
            .await?;
        Ok(SessionReplacementResult { cancelled: false })
    }

    pub async fn new_session(
        &self,
        options: NewSessionOptionsRuntime,
    ) -> Result<SessionReplacementResult> {
        let before = self.emit_before_switch("new", None).await;
        if before.cancelled {
            return Ok(before);
        }
        let previous = self.session().session_file();
        let current = self.session().session_manager.clone();
        let (dir, persisted) = {
            let current = current.lock().expect("session manager");
            (current.get_session_dir().to_owned(), current.is_persisted())
        };
        let mut manager = if persisted {
            SessionManager::create(&self.cwd(), Some(&dir), None)?
        } else {
            SessionManager::in_memory(&self.cwd(), None, None)?
        };
        if let Some(parent) = options.parent_session.filter(|s| !s.is_empty()) {
            manager.new_session(Some(&NewSessionOptions {
                id: None,
                parent_session: Some(parent),
            }))?;
        }
        self.teardown_current("new", manager.get_session_file().map(str::to_owned))
            .await?;
        self.apply(
            (self.create_runtime)(self.create_options(
                self.cwd(),
                Arc::new(Mutex::new(manager)),
                SessionStartReason::New,
                previous,
            ))
            .await?,
        );
        if let Some(setup) = options.setup {
            setup(self.session().session_manager.clone()).await?;
            // Upstream evaluates the assignment LHS after setup, then its RHS.
            let agent = self.session().agent.clone();
            let messages = self
                .session()
                .session_manager
                .lock()
                .expect("session manager")
                .build_session_context()
                .messages;
            agent.state().messages = messages;
        }
        self.finish_session_replacement(options.with_session)
            .await?;
        Ok(SessionReplacementResult { cancelled: false })
    }

    pub async fn fork(&self, entry_id: &str, options: ForkOptions) -> Result<ForkResult> {
        if self
            .emit_before_fork(entry_id, options.position)
            .await
            .cancelled
        {
            return Ok(ForkResult {
                cancelled: true,
                selected_text: None,
            });
        }
        let entry = self
            .session()
            .session_manager
            .lock()
            .expect("session manager")
            .get_entry(entry_id)
            .cloned()
            .ok_or_else(|| anyhow!("Invalid entry ID for forking"))?;
        let (target, selected_text) = if options.position == ForkPosition::At {
            (entry.id().map(str::to_owned), None)
        } else {
            // The stored message wire projection also preserves custom/loose
            // JSON string/text-block representations inherited by SessionManager.
            let value = serde_json::to_value(&entry)?;
            if value["type"] != "message" || value["message"]["role"] != "user" {
                bail!("Invalid entry ID for forking");
            }
            (
                entry.parent_id().map(str::to_owned),
                Some(extract_user_message_text(&value["message"]["content"])),
            )
        };
        let previous = self.session().session_file();
        let persisted = self
            .session()
            .session_manager
            .lock()
            .expect("session manager")
            .is_persisted();
        if persisted {
            let file = self
                .session()
                .session_file()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| anyhow!("Persisted session is missing a session file"))?;
            let dir = self
                .session()
                .session_manager
                .lock()
                .expect("session manager")
                .get_session_dir()
                .to_owned();
            if target.as_deref().is_none_or(str::is_empty) {
                let mut manager = SessionManager::create(&self.cwd(), Some(&dir), None)?;
                manager.new_session(Some(&NewSessionOptions {
                    id: None,
                    parent_session: Some(file),
                }))?;
                self.teardown_current("fork", manager.get_session_file().map(str::to_owned))
                    .await?;
                self.apply(
                    (self.create_runtime)(self.create_options(
                        self.cwd(),
                        Arc::new(Mutex::new(manager)),
                        SessionStartReason::Fork,
                        previous,
                    ))
                    .await?,
                );
            } else {
                if !Path::new(&file).exists() {
                    bail!("This session has not been saved yet. Wait for the first assistant response before cloning or forking it.");
                }
                let mut manager = SessionManager::open(&file, Some(&dir), None)?;
                let branched =
                    manager.create_branched_session(target.as_deref().expect("nonempty target"))?;
                if branched.as_deref().is_none_or(str::is_empty) {
                    bail!("Failed to create forked session");
                }
                self.teardown_current("fork", manager.get_session_file().map(str::to_owned))
                    .await?;
                let cwd = manager.get_cwd().to_owned();
                self.apply(
                    (self.create_runtime)(self.create_options(
                        cwd,
                        Arc::new(Mutex::new(manager)),
                        SessionStartReason::Fork,
                        previous,
                    ))
                    .await?,
                );
            }
        } else {
            let manager = self.session().session_manager.clone();
            let target_file = manager
                .lock()
                .expect("session manager")
                .get_session_file()
                .map(str::to_owned);
            self.teardown_current("fork", target_file).await?;
            {
                let mut manager = manager.lock().expect("session manager");
                if let Some(target) = target.as_deref().filter(|s| !s.is_empty()) {
                    manager.create_branched_session(target)?;
                } else {
                    manager.new_session(Some(&NewSessionOptions {
                        id: None,
                        parent_session: previous.clone(),
                    }))?;
                }
            }
            self.apply(
                (self.create_runtime)(self.create_options(
                    self.cwd(),
                    manager,
                    SessionStartReason::Fork,
                    previous,
                ))
                .await?,
            );
        }
        self.finish_session_replacement(options.with_session)
            .await?;
        Ok(ForkResult {
            cancelled: false,
            selected_text,
        })
    }

    pub async fn import_from_jsonl(
        &self,
        input_path: &str,
        cwd_override: Option<&str>,
    ) -> Result<SessionReplacementResult> {
        let resolved = resolve_path_auto_base(input_path)?;
        if !Path::new(&resolved).exists() {
            return Err(SessionImportFileNotFoundError {
                file_path: resolved,
            }
            .into());
        }
        let dir = self
            .session()
            .session_manager
            .lock()
            .expect("session manager")
            .get_session_dir()
            .to_owned();
        if !Path::new(&dir).exists() {
            fs::create_dir_all(&dir)?;
        }
        let file_name = Path::new(&resolved)
            .file_name()
            .ok_or_else(|| anyhow!("Import path has no file name"))?;
        let mut destination = Path::new(&dir).join(file_name);
        let source_already_stored =
            resolve_path_auto_base(&destination.to_string_lossy())? == resolved;
        if !source_already_stored {
            let name = destination
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let ext = destination
                .extension()
                .map(|s| format!(".{}", s.to_string_lossy()))
                .unwrap_or_default();
            let mut suffix = 1;
            while destination.exists() {
                destination = Path::new(&dir).join(format!("{name}-{suffix}{ext}"));
                suffix += 1;
            }
        }
        let destination = destination.to_string_lossy().into_owned();
        let before = self.emit_before_switch("resume", Some(&destination)).await;
        if before.cancelled {
            return Ok(before);
        }
        let previous = self.session().session_file();
        if !source_already_stored {
            copy_exclusive(Path::new(&resolved), Path::new(&destination))?;
        }
        let manager = SessionManager::open(&destination, Some(&dir), cwd_override)?;
        assert_session_cwd_exists(&manager, &self.cwd())?;
        self.teardown_current("resume", manager.get_session_file().map(str::to_owned))
            .await?;
        let cwd = manager.get_cwd().to_owned();
        self.apply(
            (self.create_runtime)(self.create_options(
                cwd,
                Arc::new(Mutex::new(manager)),
                SessionStartReason::Resume,
                previous,
            ))
            .await?,
        );
        self.finish_session_replacement(None).await?;
        Ok(SessionReplacementResult { cancelled: false })
    }

    /// Not idempotent; no pre-shutdown abort. Print mode supplies its own guard.
    pub async fn dispose(&self) -> Result<()> {
        emit_session_shutdown_event(
            &self.session().extension_runner(),
            &json!({"type":"session_shutdown", "reason":"quit"}),
        )
        .await;
        self.invalidate_current()
    }
}

fn extract_user_message_text(content: &Value) -> String {
    if let Some(text) = content.as_str() {
        return text.to_owned();
    }
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter(|part| part["type"] == "text")
        .filter_map(|part| part["text"].as_str())
        .collect()
}

/// COPYFILE_EXCL: never truncate a destination that appears during an awaited
/// hook. Only a file successfully created by this call may be removed on error.
fn copy_exclusive(source: &Path, destination: &Path) -> std::io::Result<()> {
    let mut input = File::open(source)?;
    let metadata = input.metadata()?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    let result = std::io::copy(&mut input, &mut output)
        .and_then(|_| output.set_permissions(metadata.permissions()));
    if result.is_err() {
        drop(output);
        let _ = fs::remove_file(destination);
    }
    result
}

/// Validate the stored cwd before invoking even the first factory.
pub async fn create_agent_session_runtime(
    create_runtime: CreateAgentSessionRuntimeFactory,
    options: CreateAgentSessionRuntimeOptions,
) -> Result<AgentSessionRuntime> {
    {
        let manager = options.session_manager.lock().expect("session manager");
        assert_session_cwd_exists(&*manager, &options.cwd)?;
    }
    let result = create_runtime(options).await?;
    Ok(AgentSessionRuntime::new(
        result.session,
        result.services,
        create_runtime,
        result.diagnostics,
        result.model_fallback_message,
    ))
}

#[cfg(test)]
#[path = "agent_session_runtime_tests.rs"]
mod tests;
