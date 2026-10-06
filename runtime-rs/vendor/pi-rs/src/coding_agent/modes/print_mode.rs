//! Upstream `modes/print-mode.ts`, on the real AgentSessionRuntime.
//!
//! Output uses the shared raw-stdout queue, including awaited Agent listeners
//! for backpressure. Session replacement rebinds the live session; no prompt or
//! command uses a session cached before an awaited replacement.
//! OS signals/stderr/exit are isolated in PrintProcess, not the session engine.
//! Unix registers SIGTERM/SIGHUP through Tokio. Windows has no OS SIGTERM
//! delivery API; a native embedder can deliver it through PrintProcess. This
//! does not install a Ctrl-C handler (upstream print mode does not do so).

use super::json_event::to_json_event_string;
use crate::agent_core::{agent::Unsubscribe, types::AgentMessage};
use crate::ai::types::{content::ImageContent, message::AssistantBlock, primitives::StopReason};
use crate::coding_agent::agent_session::{
    AgentSession, AgentSessionUnsubscribe, ExtensionBindings, NavigateTreeOptions, PromptOptions,
};
use crate::coding_agent::core::agent_session_runtime::{
    AgentSessionRuntime, ForkOptions, ForkPosition, NewSessionOptionsRuntime, SetupSession,
    SwitchSessionOptions, WithSession,
};
use crate::coding_agent::core::output_guard::OutputGuard;
use crate::coding_agent::extensions::types::{
    self as extension, Cancelled, CommandFuture, ExtensionCommandContextActions, ExtensionMode,
    SessionManagerHandle,
};
use crate::coding_agent::session_manager::FileEntry;
use anyhow::{anyhow, Result};
use futures::future::BoxFuture;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, Weak,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PrintMode {
    #[default]
    Text,
    Json,
}
#[derive(Clone, Default)]
pub struct PrintModeOptions {
    pub mode: PrintMode,
    pub messages: Vec<String>,
    pub initial_message: Option<String>,
    pub initial_images: Option<Vec<ImageContent>>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrintSignal {
    Terminate,
    Hangup,
}
impl PrintSignal {
    pub fn exit_code(self) -> i32 {
        match self {
            Self::Terminate => 143,
            Self::Hangup => 129,
        }
    }
}
pub type SignalHandler = Arc<dyn Fn() + Send + Sync>;
pub struct SignalRegistration(Option<Box<dyn FnOnce() + Send>>);
impl SignalRegistration {
    pub fn new(cleanup: impl FnOnce() + Send + 'static) -> Self {
        Self(Some(Box::new(cleanup)))
    }
}
impl Drop for SignalRegistration {
    fn drop(&mut self) {
        if let Some(cleanup) = self.0.take() {
            cleanup();
        }
    }
}
/// The process boundary. Tests replace only these effects, not sessions,
/// RuntimeHost replacement, extension binding, or Agent event subscriptions.
pub trait PrintProcess: Send + Sync {
    fn signals(&self) -> Vec<PrintSignal>;
    fn on_signal(&self, signal: PrintSignal, handler: SignalHandler) -> Result<SignalRegistration>;
    fn error(&self, message: &str);
    fn kill_tracked_children(&self);
    fn exit(&self, code: i32);
}
pub(crate) struct NativePrintProcess;
impl PrintProcess for NativePrintProcess {
    fn signals(&self) -> Vec<PrintSignal> {
        #[cfg(unix)]
        {
            vec![PrintSignal::Terminate, PrintSignal::Hangup]
        }
        #[cfg(not(unix))]
        {
            vec![]
        }
    }
    fn on_signal(&self, signal: PrintSignal, handler: SignalHandler) -> Result<SignalRegistration> {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal as subscribe, SignalKind};
            let mut stream = subscribe(match signal {
                PrintSignal::Terminate => SignalKind::terminate(),
                PrintSignal::Hangup => SignalKind::hangup(),
            })?;
            let task = tokio::spawn(async move {
                while stream.recv().await.is_some() {
                    handler();
                }
            });
            // Dropping the listener unregisters this mode's callbacks. Tokio's
            // process-global signal trampoline remains installed, as documented
            // by Tokio; restoring a foreign process handler is not attempted.
            Ok(SignalRegistration::new(move || task.abort()))
        }
        #[cfg(not(unix))]
        {
            let _ = (signal, handler);
            Err(anyhow!(
                "This platform has no native POSIX signal subscription"
            ))
        }
    }
    fn error(&self, message: &str) {
        eprintln!("{message}");
    }
    fn kill_tracked_children(&self) {
        crate::coding_agent::utils::shell::kill_tracked_detached_children();
    }
    fn exit(&self, code: i32) {
        std::process::exit(code);
    }
}
#[derive(Default)]
struct Subscriptions {
    session: Option<AgentSessionUnsubscribe>,
    pressure: Option<Unsubscribe>,
}
impl Subscriptions {
    fn remove(&mut self) {
        if let Some(subscription) = self.session.take() {
            subscription.unsubscribe();
        }
        if let Some(subscription) = self.pressure.take() {
            subscription.unsubscribe();
        }
    }
}
struct PrintState {
    runtime: Arc<AgentSessionRuntime>,
    session: Mutex<Arc<AgentSession>>,
    mode: PrintMode,
    output: OutputGuard,
    process: Arc<dyn PrintProcess>,
    subscriptions: Mutex<Subscriptions>,
    disposed: AtomicBool,
    wire_error: Mutex<Option<String>>,
}
impl PrintState {
    fn session(&self) -> Arc<AgentSession> {
        self.session.lock().expect("print session").clone()
    }
    /// The guard and unsubscribe prefix are synchronous. A second caller does
    /// not await an already-running dispose (exactly upstream's boolean guard).
    fn dispose(&self) -> BoxFuture<'static, Result<()>> {
        if self.disposed.swap(true, Ordering::SeqCst) {
            return Box::pin(async { Ok(()) });
        }
        self.subscriptions
            .lock()
            .expect("print subscriptions")
            .remove();
        let runtime = self.runtime.clone();
        Box::pin(async move { runtime.dispose().await })
    }
    fn take_wire_error(&self) -> Result<()> {
        match self.wire_error.lock().expect("print wire error").take() {
            Some(error) => Err(anyhow!(error)),
            None => Ok(()),
        }
    }
    async fn rebind(self: &Arc<Self>) -> Result<()> {
        *self.session.lock().expect("print session") = self.runtime.session();
        let process = self.process.clone();
        self.session()
            .bind_extensions(ExtensionBindings {
                mode: Some(if self.mode == PrintMode::Json {
                    ExtensionMode::Json
                } else {
                    ExtensionMode::Print
                }),
                command_context_actions: Some(command_actions(Arc::downgrade(self))),
                on_error: Some(Arc::new(move |error| {
                    process.error(&format!(
                        "Extension error ({}): {}",
                        error.extension_path, error.error
                    ))
                })),
                ..Default::default()
            })
            .await?;
        // bindExtensions may itself replace the runtime. Read the live slot
        // after it settles, not the session captured when binding started.
        let session = self.session();
        let mut subscriptions = self.subscriptions.lock().expect("print subscriptions");
        subscriptions.remove();
        let weak = Arc::downgrade(self);
        subscriptions.session = Some(session.subscribe(Arc::new(move |event| {
            if let Some(state) = weak.upgrade() {
                if state.mode == PrintMode::Json {
                    match to_json_event_string(event) {
                        Ok(wire) => state.output.write_raw_stdout(&(wire + "\n")),
                        Err(error) => {
                            state
                                .wire_error
                                .lock()
                                .expect("print wire error")
                                .get_or_insert(error);
                        }
                    }
                }
            }
        })));
        if self.mode == PrintMode::Json {
            let weak = Arc::downgrade(self);
            subscriptions.pressure = Some(session.agent.subscribe(move |_, _| {
                let weak = weak.clone();
                Box::pin(async move {
                    if let Some(state) = weak.upgrade() {
                        if let Err(error) = state.output.wait_for_raw_stdout_backpressure().await {
                            state
                                .wire_error
                                .lock()
                                .expect("print wire error")
                                .get_or_insert_with(|| error.to_string());
                        }
                    }
                })
            }));
        }
        Ok(())
    }
    async fn run(self: &Arc<Self>, options: PrintModeOptions) -> Result<i32> {
        if self.mode == PrintMode::Json {
            let header = self
                .session()
                .session_manager
                .lock()
                .expect("session manager")
                .get_header();
            if let Some(header) = header {
                let wire = crate::serde_support::to_json_string_with_js_numbers(
                    &FileEntry::Session(header),
                )?;
                self.output.write_raw_stdout(&(wire + "\n"));
            }
        }
        self.rebind().await?;
        if let Some(initial) = options.initial_message.filter(|text| !text.is_empty()) {
            self.session()
                .prompt(
                    initial,
                    Some(PromptOptions {
                        images: options.initial_images,
                        ..Default::default()
                    }),
                )
                .await?;
            self.take_wire_error()?;
        }
        for message in options.messages {
            self.session().prompt(message, None).await?;
            self.take_wire_error()?;
        }
        if self.mode == PrintMode::Text {
            let last = self.session().state().messages.last().cloned();
            if let Some(AgentMessage::Assistant(assistant)) = last {
                if matches!(
                    assistant.stop_reason,
                    StopReason::Error | StopReason::Aborted
                ) {
                    let fallback = if assistant.stop_reason == StopReason::Aborted {
                        "Request aborted"
                    } else {
                        "Request error"
                    };
                    self.process.error(
                        assistant
                            .error_message
                            .as_deref()
                            .filter(|message| !message.is_empty())
                            .unwrap_or(fallback),
                    );
                    return Ok(1);
                }
                for block in assistant.content {
                    if let AssistantBlock::Text(text) = block {
                        self.output.write_raw_stdout(&(text.text + "\n"));
                    }
                }
            }
        }
        Ok(0)
    }
}
fn live(state: &Weak<PrintState>) -> std::result::Result<Arc<PrintState>, String> {
    state
        .upgrade()
        .ok_or_else(|| "Print mode has ended".to_string())
}
fn with_session(handler: Option<extension::WithSessionHandler>) -> Option<WithSession> {
    handler.map(|handler| {
        Arc::new(move |context| {
            let pending = handler(&context);
            Box::pin(async move {
                pending
                    .map_err(anyhow::Error::msg)?
                    .await
                    .map_err(anyhow::Error::msg)
            }) as BoxFuture<'static, Result<()>>
        }) as WithSession
    })
}
fn setup_session(handler: Option<extension::SessionSetupHandler>) -> Option<SetupSession> {
    handler.map(|handler| {
        Arc::new(
            move |manager: crate::coding_agent::core::agent_session_runtime::SessionManagerRef| {
                let handle: SessionManagerHandle = manager;
                let pending = handler(&handle);
                Box::pin(async move {
                    pending
                        .map_err(anyhow::Error::msg)?
                        .await
                        .map_err(anyhow::Error::msg)
                }) as BoxFuture<'static, Result<()>>
            },
        ) as SetupSession
    })
}
fn command_actions(weak: Weak<PrintState>) -> ExtensionCommandContextActions {
    let idle = weak.clone();
    let new = weak.clone();
    let fork = weak.clone();
    let navigate = weak.clone();
    let switch = weak.clone();
    ExtensionCommandContextActions {
        wait_for_idle: Arc::new(move || {
            let session = live(&idle)?.session();
            CommandFuture::spawn(async move {
                session.wait_for_idle().await;
                Ok(())
            })
        }),
        new_session: Arc::new(move |options| {
            let runtime = live(&new)?.runtime.clone();
            let options = options.unwrap_or_default();
            let options = NewSessionOptionsRuntime {
                parent_session: options.parent_session,
                setup: setup_session(options.setup),
                with_session: with_session(options.with_session),
            };
            CommandFuture::spawn(async move {
                runtime
                    .new_session(options)
                    .await
                    .map(|result| Cancelled {
                        cancelled: result.cancelled,
                    })
                    .map_err(|error| error.to_string())
            })
        }),
        fork: Arc::new(move |id, options| {
            let runtime = live(&fork)?.runtime.clone();
            let id = id.to_owned();
            let options = options.unwrap_or_default();
            let options = ForkOptions {
                position: if options.position == Some(extension::TreePosition::At) {
                    ForkPosition::At
                } else {
                    ForkPosition::Before
                },
                with_session: with_session(options.with_session),
            };
            CommandFuture::spawn(async move {
                runtime
                    .fork(&id, options)
                    .await
                    .map(|result| Cancelled {
                        cancelled: result.cancelled,
                    })
                    .map_err(|error| error.to_string())
            })
        }),
        navigate_tree: Arc::new(move |id, options| {
            let session = live(&navigate)?.session();
            let id = id.to_owned();
            let options = options.unwrap_or_default();
            let options = NavigateTreeOptions {
                summarize: options.summarize,
                custom_instructions: options.custom_instructions,
                replace_instructions: options.replace_instructions,
                label: options.label,
            };
            CommandFuture::spawn(async move {
                session
                    .navigate_tree(&id, options)
                    .await
                    .map(|result| Cancelled {
                        cancelled: result.cancelled,
                    })
                    .map_err(|error| error.to_string())
            })
        }),
        switch_session: Arc::new(move |path, options| {
            let runtime = live(&switch)?.runtime.clone();
            let path = path.to_owned();
            let options = SwitchSessionOptions {
                with_session: with_session(options.unwrap_or_default().with_session),
                ..Default::default()
            };
            CommandFuture::spawn(async move {
                runtime
                    .switch_session(&path, options)
                    .await
                    .map(|result| Cancelled {
                        cancelled: result.cancelled,
                    })
                    .map_err(|error| error.to_string())
            })
        }),
        reload: Arc::new(move || {
            let session = live(&weak)?.session();
            CommandFuture::spawn(async move {
                session
                    .reload(None)
                    .await
                    .map_err(|error| error.to_string())
            })
        }),
    }
}
/// Run against the process-wide output queue. Call from within a Tokio runtime.
pub async fn run_print_mode(
    runtime: Arc<AgentSessionRuntime>,
    options: PrintModeOptions,
) -> Result<i32> {
    run_print_mode_with_io(
        runtime,
        options,
        crate::coding_agent::core::output_guard::process_guard().clone(),
        Arc::new(NativePrintProcess),
    )
    .await
}
/// Injectable process effects, with the same real runtime/session implementation.
pub async fn run_print_mode_with_io(
    runtime: Arc<AgentSessionRuntime>,
    options: PrintModeOptions,
    output: OutputGuard,
    process: Arc<dyn PrintProcess>,
) -> Result<i32> {
    let state = Arc::new(PrintState {
        session: Mutex::new(runtime.session()),
        runtime,
        mode: options.mode,
        output,
        process,
        subscriptions: Mutex::new(Subscriptions::default()),
        disposed: AtomicBool::new(false),
        wire_error: Mutex::new(None),
    });
    let mut signals = Vec::new();
    for signal in state.process.signals() {
        let weak = Arc::downgrade(&state);
        signals.push(state.process.on_signal(
            signal,
            Arc::new(move || {
                if let Some(state) = weak.upgrade() {
                    state.process.kill_tracked_children();
                    let pending = state.dispose();
                    let process = state.process.clone();
                    tokio::spawn(async move {
                        let _ = pending.await;
                        process.exit(signal.exit_code());
                    });
                }
            }),
        )?);
    }
    let weak = Arc::downgrade(&state);
    state.runtime.set_rebind_session(Some(Arc::new(move |_| {
        let state = weak.upgrade();
        Box::pin(async move {
            state
                .ok_or_else(|| anyhow!("Print mode has ended"))?
                .rebind()
                .await
        })
    })));
    let code = match state.run(options).await {
        Ok(code) => code,
        Err(error) => {
            state.process.error(&error.to_string());
            1
        }
    };
    // Upstream finally order is observable; failed disposal skips flush and
    // overrides a successful return or a caught prompt failure.
    for signal in signals {
        drop(signal);
    }
    state.dispose().await?;
    state
        .output
        .flush_raw_stdout()
        .await
        .map_err(|error| anyhow!(error.to_string()))?;
    Ok(code)
}

#[cfg(test)]
#[path = "print_mode_tests.rs"]
mod tests;
