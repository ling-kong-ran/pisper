//! Native RPC mode on the real runtime: concurrent JSONL commands, extension
//! UI replies, event streaming, session replacement, stdout backpressure and
//! process shutdown. CLI argument routing is a separate caller.
//!
//! Well-typed command behavior is ported. Malformed known-command diagnostics
//! still come from serde (not V8), and lone-surrogate JSON strings cannot yet
//! pass through the typed native command model. Those are compatibility gaps,
//! not silently accepted/rewritten inputs.
use super::dispatch::RpcDispatcher;
use super::jsonl::{serialize_json_line, JsonlLineReader};
use super::types::{RpcCommand, RpcResponse};
use super::ui::RpcExtensionUi;
use crate::agent_core::agent::Unsubscribe;
use crate::coding_agent::agent_session::{
    AgentSessionEvent, AgentSessionUnsubscribe, ExtensionBindings, NavigateTreeOptions,
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
use crate::coding_agent::modes::json_event::to_json_event_string;
use crate::coding_agent::modes::print_mode::{
    NativePrintProcess, PrintProcess, PrintSignal, SignalRegistration,
};
use anyhow::{anyhow, Result};
use futures::future::BoxFuture;
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, Weak,
};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::Notify;
use tokio::task::JoinSet;

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
#[derive(Clone, Copy, Default)]
struct Shutdown {
    code: i32,
    signal: Option<PrintSignal>,
}
#[derive(Default)]
struct ShutdownRequests {
    first: Option<Shutdown>,
    repeated: Option<Shutdown>,
}
struct RpcState {
    runtime: Arc<AgentSessionRuntime>,
    output: OutputGuard,
    ui: RpcExtensionUi,
    subscriptions: Mutex<Subscriptions>,
    shutdown_requested: AtomicBool,
    exit: Mutex<ShutdownRequests>,
    wake: Notify,
    ended: AtomicBool,
    wire_error: Mutex<Option<String>>,
}
impl RpcState {
    fn output(&self, value: &Value) {
        if self.ended.load(Ordering::SeqCst) {
            return;
        }
        match serialize_json_line(value) {
            Ok(wire) => self.output.write_raw_stdout(&wire),
            Err(error) => self.fail(error.to_string()),
        }
    }
    fn fail(&self, error: String) {
        self.wire_error
            .lock()
            .expect("rpc wire error")
            .get_or_insert(error);
        self.exit(Shutdown {
            code: 1,
            signal: None,
        });
    }
    fn exit(&self, cause: Shutdown) {
        let mut requests = self.exit.lock().expect("rpc exit");
        if requests.first.is_none() {
            requests.first = Some(cause);
        } else {
            requests.repeated.get_or_insert(cause);
        }
        drop(requests);
        self.wake.notify_one();
    }
    fn initial_exit(&self) -> Option<Shutdown> {
        self.exit.lock().expect("rpc exit").first
    }
    fn forced_exit(&self) -> Option<Shutdown> {
        self.exit.lock().expect("rpc exit").repeated
    }
    fn check_shutdown(&self) {
        if self.shutdown_requested.load(Ordering::SeqCst) {
            self.exit(Shutdown::default());
        }
    }
    async fn pressure(&self) -> Result<()> {
        self.output
            .wait_for_raw_stdout_backpressure()
            .await
            .map_err(|error| anyhow!(error.to_string()))
    }
    async fn rebind(self: &Arc<Self>) -> Result<()> {
        if self.ended.load(Ordering::SeqCst) {
            return Err(anyhow!("RPC mode has ended"));
        }
        let weak = Arc::downgrade(self);
        let errors = weak.clone();
        self.runtime.session().bind_extensions(ExtensionBindings {
            ui_context:Some(Arc::new(self.ui.clone())),mode:Some(ExtensionMode::Rpc),
            command_context_actions:Some(command_actions(Arc::downgrade(&self.runtime))),
            shutdown_handler:Some(Arc::new(move|| {if let Some(state)=weak.upgrade() {state.shutdown_requested.store(true,Ordering::SeqCst);}})),
            on_error:Some(Arc::new(move|error| {if let Some(state)=errors.upgrade() {state.output(&json!({"type":"extension_error","extensionPath":error.extension_path,"event":error.event,"error":error.error}));}})),
            ..Default::default()
        }).await?;
        if self.ended.load(Ordering::SeqCst) {
            return Err(anyhow!("RPC mode has ended"));
        }
        let session = self.runtime.session();
        let mut subscriptions = self.subscriptions.lock().expect("rpc subscriptions");
        subscriptions.remove();
        let weak = Arc::downgrade(self);
        subscriptions.session = Some(session.subscribe(Arc::new(move |event| {
            if let Some(state) = weak.upgrade() {
                if !state.ended.load(Ordering::SeqCst) {
                    match to_json_event_string(event) {
                        Ok(wire) => state.output.write_raw_stdout(&(wire + "\n")),
                        Err(error) => state.fail(error),
                    }
                    if matches!(event, AgentSessionEvent::AgentSettled) {
                        state.check_shutdown();
                    }
                }
            }
        })));
        let weak = Arc::downgrade(self);
        subscriptions.pressure = Some(session.agent.subscribe(move |_, _| {
            let weak = weak.clone();
            Box::pin(async move {
                if let Some(state) = weak.upgrade() {
                    if let Err(error) = state.pressure().await {
                        state.fail(error.to_string());
                    }
                }
            })
        }));
        Ok(())
    }
    fn detach(&self) {
        self.ended.store(true, Ordering::SeqCst);
        self.subscriptions
            .lock()
            .expect("rpc subscriptions")
            .remove();
        self.runtime.set_rebind_session(None);
        self.ui.reject_pending("RPC mode has ended");
    }
}
struct ModeGuard {
    state: Arc<RpcState>,
    dispatcher: RpcDispatcher,
}
impl Drop for ModeGuard {
    fn drop(&mut self) {
        self.state.detach();
        self.dispatcher.abort_pending_prompts();
    }
}

async fn handle_line(state: Arc<RpcState>, dispatch: RpcDispatcher, line: String) -> Result<()> {
    let parsed: Value = match serde_json::from_str(&line) {
        Ok(value) => value,
        Err(error) => {
            state.output(&serde_json::to_value(RpcResponse::error(
                None,
                "parse",
                format!("Failed to parse command: {error}"),
            ))?);
            return state.pressure().await;
        }
    };
    if parsed["type"] == "extension_ui_response" {
        state.ui.respond(parsed);
        return Ok(());
    }
    let command: RpcCommand = match serde_json::from_value(parsed.clone()) {
        Ok(command) => command,
        Err(error) => {
            let kind = parsed.get("type");
            let message = if error.to_string().starts_with("unknown variant")
                || kind.is_none()
                || !parsed.is_object()
            {
                format!(
                    "Unknown command: {}",
                    kind.map(|kind| kind
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| kind.to_string()))
                        .unwrap_or_else(|| "undefined".into())
                )
            } else {
                format!("Invalid RPC command: {error}")
            };
            let mut response = serde_json::Map::new();
            if let Some(id) = parsed.get("id") {
                response.insert("id".into(), id.clone());
            }
            response.insert("type".into(), json!("response"));
            if let Some(kind) = kind {
                response.insert("command".into(), kind.clone());
            }
            response.insert("success".into(), json!(false));
            response.insert("error".into(), json!(message));
            state.output(&Value::Object(response));
            state.pressure().await?;
            state.check_shutdown();
            return Ok(());
        }
    };
    if let Some(response) = dispatch.handle(command).await {
        state.output(&serde_json::to_value(response)?);
        state.pressure().await?;
    }
    state.check_shutdown();
    Ok(())
}
/// Start each line through its first suspension before accepting the next one,
/// preserving the synchronous prefix of the upstream async callback. Awaiting
/// a complete command here would deadlock extension UI replies and aborts.
fn start_line(
    state: &Arc<RpcState>,
    dispatcher: &RpcDispatcher,
    line: String,
    tasks: &mut JoinSet<Result<()>>,
) -> Result<()> {
    let mut work = Box::pin(handle_line(state.clone(), dispatcher.clone(), line));
    use futures::task::noop_waker_ref;
    use std::future::Future;
    let mut context = std::task::Context::from_waker(noop_waker_ref());
    match work.as_mut().poll(&mut context) {
        std::task::Poll::Ready(result) => result,
        std::task::Poll::Pending => {
            tasks.spawn(work);
            Ok(())
        }
    }
}

/// Native stdin/stdout adapter. Called within Tokio; the process boundary exits
/// after cleanup, so an embedding/test process should use the injectable form.
pub async fn run_rpc_mode(runtime: Arc<AgentSessionRuntime>) -> Result<i32> {
    run_rpc_mode_with_io(
        runtime,
        tokio::io::stdin(),
        crate::coding_agent::core::output_guard::process_guard().clone(),
        Arc::new(NativePrintProcess),
    )
    .await
}

/// Only byte IO/process effects are injected. Session state, commands,
/// extension bindings, replacement, prompt work and event flow are real.
pub async fn run_rpc_mode_with_io<I: AsyncRead + Unpin>(
    runtime: Arc<AgentSessionRuntime>,
    mut input: I,
    output: OutputGuard,
    process: Arc<dyn PrintProcess>,
) -> Result<i32> {
    output.take_over_stdout();
    let state = Arc::new_cyclic(|weak: &Weak<RpcState>| {
        let output_state = weak.clone();
        RpcState {
            runtime: runtime.clone(),
            output: output.clone(),
            ui: RpcExtensionUi::new(Arc::new(move |request| {
                if let Some(state) = output_state.upgrade() {
                    state.output(&request);
                }
            })),
            subscriptions: Mutex::new(Subscriptions::default()),
            shutdown_requested: AtomicBool::new(false),
            exit: Mutex::new(ShutdownRequests::default()),
            wake: Notify::new(),
            ended: AtomicBool::new(false),
            wire_error: Mutex::new(None),
        }
    });
    let weak = Arc::downgrade(&state);
    let rebind: super::dispatch::RpcRebind = Arc::new(move || {
        let state = weak.upgrade();
        Box::pin(async move {
            state
                .ok_or_else(|| anyhow!("RPC mode has ended"))?
                .rebind()
                .await
        })
    });
    let callback = rebind.clone();
    runtime.set_rebind_session(Some(Arc::new(move |_| callback())));
    let weak = Arc::downgrade(&state);
    let dispatcher = RpcDispatcher::new(
        runtime.clone(),
        Arc::new(move |response| {
            if let Some(state) = weak.upgrade() {
                match serde_json::to_value(response) {
                    Ok(value) => state.output(&value),
                    Err(error) => state.fail(error.to_string()),
                }
            }
        }),
    )
    .with_rebind(rebind);
    let _guard = ModeGuard {
        state: state.clone(),
        dispatcher: dispatcher.clone(),
    };
    state.rebind().await?;
    let mut signals: Vec<SignalRegistration> = Vec::new();
    for signal in process.signals() {
        let weak = Arc::downgrade(&state);
        let process = process.clone();
        signals.push(process.clone().on_signal(
            signal,
            Arc::new(move || {
                if let Some(state) = weak.upgrade() {
                    process.kill_tracked_children();
                    state.exit(Shutdown {
                        code: signal.exit_code(),
                        signal: Some(signal),
                    });
                }
            }),
        )?);
    }
    let mut reader = JsonlLineReader::new();
    let mut tasks = JoinSet::new();
    let mut buffer = [0u8; 8192];
    let mut eof = false;
    let mut failure = None;
    let cause = loop {
        if let Some(cause) = state.initial_exit() {
            break cause;
        }
        tokio::select! {
            biased;
            _=state.wake.notified()=>{},
            result=tasks.join_next(),if !tasks.is_empty()=> { finish_command(&state, result); }
            read=input.read(&mut buffer)=> {
                match read {
                    Ok(0)=> {eof=true; state.exit(Shutdown::default());}
                    Ok(count)=>for line in reader.feed_bytes(&buffer[..count]) {
                        if let Err(error)=start_line(&state,&dispatcher,line.to_string_lossy(),&mut tasks) {state.fail(error.to_string());}
                    },
                    Err(error)=> {failure=Some(anyhow!(error));state.exit(Shutdown{code:1,signal:None});}
                }
            }
        }
    };
    // shutdown() first removes its signal/event listeners. It must NOT detach
    // input yet: a session_shutdown extension can await a client UI response.
    signals.clear();
    state
        .subscriptions
        .lock()
        .expect("rpc subscriptions")
        .remove();
    let mut disposing = Box::pin(runtime.dispose());
    let first = futures::poll!(disposing.as_mut());
    // The upstream EOF listener was installed before the JSONL EOF listener.
    // Its synchronous shutdown prefix precedes the final (unterminated) line.
    if eof {
        for line in reader.finish() {
            if let Err(error) = start_line(&state, &dispatcher, line.to_string_lossy(), &mut tasks)
            {
                failure = Some(error);
            }
        }
    }
    if let Some(forced) = state.forced_exit() {
        process.exit(forced.code);
        return Ok(forced.code);
    }
    let disposed = match first {
        std::task::Poll::Ready(result) => result,
        std::task::Poll::Pending => loop {
            if let Some(forced) = state.forced_exit() {
                process.exit(forced.code);
                return Ok(forced.code);
            }
            tokio::select! {
                biased;
                _=state.wake.notified()=>{},
                result=&mut disposing=>break result,
                result=tasks.join_next(),if !tasks.is_empty()=> {finish_command(&state,result);}
                read=input.read(&mut buffer),if !eof=> {
                    match read {
                        Ok(0)=> {
                            eof=true;
                            // A second shutdown call exits immediately, as in
                            // upstream's shuttingDown branch (no second dispose).
                            state.exit(Shutdown::default());
                        }
                        Ok(count)=>for line in reader.feed_bytes(&buffer[..count]) {
                            if let Err(error)=start_line(&state,&dispatcher,line.to_string_lossy(),&mut tasks) {state.fail(error.to_string());}
                        },
                        Err(error)=> {failure=Some(anyhow!(error));state.exit(Shutdown{code:1,signal:None});}
                    }
                }
            }
        },
    };
    reader.detach();
    disposed?;
    if let Some(forced) = state.forced_exit() {
        process.exit(forced.code);
        return Ok(forced.code);
    }
    if cause.signal != Some(PrintSignal::Terminate) {
        let mut flushing = Box::pin(state.output.flush_raw_stdout());
        loop {
            if let Some(forced) = state.forced_exit() {
                process.exit(forced.code);
                return Ok(forced.code);
            }
            tokio::select! {
                biased;
                _=state.wake.notified()=>{},
                result=&mut flushing=> {result.map_err(|error|anyhow!(error.to_string()))?;break;}
                result=tasks.join_next(),if !tasks.is_empty()=> {finish_command(&state,result);}
            }
        }
    }
    // As with process.exit, no command/prompt future may outlive the host.
    state.detach();
    dispatcher.abort_pending_prompts();
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    if let Some(error) = failure {
        return Err(error);
    }
    if let Some(error) = state.wire_error.lock().expect("rpc wire error").take() {
        return Err(anyhow!(error));
    }
    process.exit(cause.code);
    Ok(cause.code)
}

fn finish_command(
    state: &RpcState,
    result: Option<std::result::Result<Result<()>, tokio::task::JoinError>>,
) {
    match result {
        Some(Ok(Ok(()))) | None => {}
        Some(Ok(Err(error))) => state.fail(error.to_string()),
        Some(Err(error)) => state.fail(error.to_string()),
    }
}

fn live(
    state: &Weak<AgentSessionRuntime>,
) -> std::result::Result<Arc<AgentSessionRuntime>, String> {
    state
        .upgrade()
        .ok_or_else(|| "RPC mode has ended".to_string())
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
fn command_actions(weak: Weak<AgentSessionRuntime>) -> ExtensionCommandContextActions {
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
            let runtime = live(&new)?;
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
            let runtime = live(&fork)?;
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
            let runtime = live(&switch)?;
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

#[cfg(test)]
#[path = "mode_tests.rs"]
mod tests;
