//! Real Runtime/AgentSession integration. Only process IO, signals and model IO
//! are injected; prompts, extension commands, replacement and disposal are real.
use super::*;
use crate::agent_core::agent::Agent;
use crate::agent_core::types::{unknown_model, AgentInitialState, AgentOptions, StreamFn};
use crate::ai::auth::credential_store::InMemoryCredentialStore;
use crate::ai::models::{
    create_models, faux_assistant_message, CreateModelsOptions, FauxMessageOptions,
};
use crate::ai::types::events::{AssistantMessageEvent, SuccessReason};
use crate::coding_agent::agent_session::AgentSessionConfig;
use crate::coding_agent::core::agent_session_runtime::{
    CreateAgentSessionRuntimeFactory, CreateAgentSessionRuntimeOptions,
    CreateAgentSessionRuntimeResult,
};
use crate::coding_agent::core::agent_session_services::AgentSessionServices;
use crate::coding_agent::core::model_runtime::{CreateModelRuntimeOptions, ModelRuntime};
use crate::coding_agent::core::models_store::InMemoryCodingAgentModelsStore;
use crate::coding_agent::core::output_guard::{
    OutputChunk, OutputError, OutputStream, WriteCallback,
};
use crate::coding_agent::core::resource_loader::{
    DefaultResourceLoader, DefaultResourceLoaderOptions, InlineExtension,
};
use crate::coding_agent::core::settings_manager::SettingsManager;
use crate::coding_agent::extensions::loader::ExtensionFactory;
use crate::coding_agent::extensions::types::{sync_handler, HandlerResult};
use crate::coding_agent::session_manager::SessionManager;
use serde_json::{json, Value};
use std::sync::atomic::AtomicUsize;
use std::time::Duration;
use tokio::sync::{Notify, Semaphore};

type Trace = Arc<Mutex<Vec<Value>>>;
fn record(trace: &Trace, row: Value) {
    trace.lock().unwrap().push(row);
}
fn snapshot(trace: &Trace) -> Vec<Value> {
    trace.lock().unwrap().clone()
}
async fn bounded<T>(work: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(15), work)
        .await
        .expect("print mode deadlocked")
}
#[derive(Default)]
struct MemoryOutput {
    writes: Mutex<Vec<String>>,
    hold_next: AtomicBool,
    pending: Mutex<Vec<WriteCallback>>,
    written: Notify,
}
impl MemoryOutput {
    fn bytes(&self) -> String {
        self.writes.lock().unwrap().concat()
    }
    fn release(&self) {
        let pending = std::mem::take(&mut *self.pending.lock().unwrap());
        for callback in pending {
            callback(Ok(()));
        }
    }
}
impl OutputStream for MemoryOutput {
    fn write(
        &self,
        chunk: OutputChunk,
        _: Option<String>,
        callback: Option<WriteCallback>,
    ) -> std::result::Result<bool, Arc<OutputError>> {
        let OutputChunk::Text(text) = chunk else {
            panic!("raw stdout must be text")
        };
        self.writes.lock().unwrap().push(text);
        let hold = self.hold_next.swap(false, Ordering::SeqCst);
        if let Some(callback) = callback {
            if hold {
                self.pending.lock().unwrap().push(callback);
            } else {
                callback(Ok(()));
            }
        }
        self.written.notify_one();
        Ok(!hold)
    }
}
#[derive(Clone, Default)]
struct TestProcess {
    log: Trace,
    handlers: Arc<Mutex<Vec<(PrintSignal, SignalHandler)>>>,
    exited: Arc<Notify>,
}
impl TestProcess {
    fn fire(&self, signal: PrintSignal) {
        let handler = self
            .handlers
            .lock()
            .unwrap()
            .iter()
            .find(|(s, _)| *s == signal)
            .unwrap()
            .1
            .clone();
        handler();
    }
}
impl PrintProcess for TestProcess {
    fn signals(&self) -> Vec<PrintSignal> {
        vec![PrintSignal::Terminate, PrintSignal::Hangup]
    }
    fn on_signal(&self, signal: PrintSignal, handler: SignalHandler) -> Result<SignalRegistration> {
        self.handlers.lock().unwrap().push((signal, handler));
        let this = self.clone();
        Ok(SignalRegistration::new(move || {
            this.handlers.lock().unwrap().retain(|(s, _)| *s != signal);
            record(&this.log, json!(["remove_signal", signal.exit_code()]));
        }))
    }
    fn error(&self, message: &str) {
        record(&self.log, json!(["error", message]));
    }
    fn kill_tracked_children(&self) {
        record(&self.log, json!(["kill_children"]));
    }
    fn exit(&self, code: i32) {
        record(&self.log, json!(["exit", code]));
        self.exited.notify_one();
    }
}
struct Fixture {
    _directory: tempfile::TempDir,
    host: Arc<AgentSessionRuntime>,
    sessions: Arc<Mutex<Vec<Arc<AgentSession>>>>,
    log: Trace,
    output: Arc<MemoryOutput>,
    guard: OutputGuard,
    process: Arc<TestProcess>,
}
fn native_session(
    options: &CreateAgentSessionRuntimeOptions,
    models: &ModelRuntime,
    factories: Vec<ExtensionFactory>,
    stream: Option<Arc<StreamFn>>,
) -> CreateAgentSessionRuntimeResult {
    let settings = SettingsManager::in_memory(serde_json::from_value(json!({})).unwrap());
    let mut loader = DefaultResourceLoader::new(DefaultResourceLoaderOptions {
        cwd: options.cwd.clone(),
        agent_dir: options.agent_dir.clone(),
        settings_manager: Some(Arc::new(settings.clone())),
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
    let extensions_result = loader.get_extensions();
    assert!(
        extensions_result.errors.is_empty(),
        "extension fixture failed: {:?}",
        extensions_result.errors
    );
    let loader = Arc::new(Mutex::new(loader));
    let agent = Arc::new(Agent::new(
        AgentOptions {
            initial_state: AgentInitialState {
                model: stream.as_ref().map(|_| unknown_model()),
                ..Default::default()
            },
            stream_fn: stream,
            ..Default::default()
        },
        Arc::new(create_models(CreateModelsOptions::default())),
    ));
    agent.state().messages = options
        .session_manager
        .lock()
        .unwrap()
        .build_session_context()
        .messages;
    let services = Arc::new(AgentSessionServices {
        cwd: options.cwd.clone(),
        agent_dir: options.agent_dir.clone(),
        model_runtime: models.clone(),
        settings_manager: settings.clone(),
        resource_loader: loader.clone(),
        diagnostics: vec![],
    });
    let session = AgentSession::new(AgentSessionConfig {
        agent,
        session_manager: options.session_manager.clone(),
        settings_manager: settings,
        cwd: options.cwd.clone(),
        scoped_models: vec![],
        resource_loader: loader,
        custom_tools: vec![],
        model_runtime: models.clone(),
        initial_active_tool_names: Some(vec![]),
        uses_default_tools: None,
        allowed_tool_names: None,
        excluded_tool_names: None,
        base_tools_override: vec![],
        session_start_event: options
            .session_start_event
            .as_ref()
            .map(|event| serde_json::to_value(event).unwrap()),
        html_exporter: None,
        cache_warmer: None,
    })
    .unwrap();
    CreateAgentSessionRuntimeResult {
        session,
        services,
        extensions_result,
        diagnostics: vec![],
        model_fallback_message: None,
    }
}
impl Fixture {
    async fn new(
        messages: Vec<AgentMessage>,
        extra: Vec<ExtensionFactory>,
        stream: Option<Arc<StreamFn>>,
    ) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let cwd = directory.path().to_str().unwrap().to_owned();
        let agent_dir = directory.path().join("agent").to_str().unwrap().to_owned();
        std::fs::create_dir_all(&agent_dir).unwrap();
        let models = ModelRuntime::create(CreateModelRuntimeOptions {
            credentials: Some(Arc::new(InMemoryCredentialStore::default())),
            models_path: Some(None),
            models_store: Some(Arc::new(InMemoryCodingAgentModelsStore::default())),
            allow_model_network: false,
            refresh_on_create: Some(false),
            ..Default::default()
        })
        .await
        .unwrap();
        let log = Trace::default();
        let observe: ExtensionFactory = {
            let log = log.clone();
            Arc::new(move |api| {
                for event in ["session_start", "session_shutdown"] {
                    let log = log.clone();
                    api.on(
                        event,
                        sync_handler(move |event, ctx| {
                            record(&log, json!({"event":event,"mode":ctx.mode()?.as_str()}));
                            Ok(None)
                        }),
                    )?;
                }
                let log = log.clone();
                api.on("input", sync_handler(move |event,ctx| {
                    record(&log, json!({"event":event, "session":ctx.session_manager()?.downcast::<Mutex<SessionManager>>().map_err(|_|"manager type")?.lock().unwrap().get_session_id()}));
                    Ok(Some(HandlerResult::Json(json!({"action":"handled"}))))
                }))?;
                Ok(())
            })
        };
        let mut factories = extra;
        factories.push(observe);
        let manager = Arc::new(Mutex::new(
            SessionManager::in_memory(&cwd, None, None).unwrap(),
        ));
        for message in messages {
            manager.lock().unwrap().append_message(message).unwrap();
        }
        let options = CreateAgentSessionRuntimeOptions {
            cwd,
            agent_dir,
            session_manager: manager,
            session_start_event: None,
            project_trust_context: None,
        };
        let first = native_session(&options, &models, factories.clone(), stream.clone());
        let sessions = Arc::new(Mutex::new(vec![first.session.clone()]));
        let factory: CreateAgentSessionRuntimeFactory = {
            let sessions = sessions.clone();
            Arc::new(move |options| {
                let models = models.clone();
                let factories = factories.clone();
                let stream = stream.clone();
                let sessions = sessions.clone();
                Box::pin(async move {
                    let result = native_session(&options, &models, factories, stream);
                    sessions.lock().unwrap().push(result.session.clone());
                    Ok(result)
                })
            })
        };
        let host = Arc::new(AgentSessionRuntime::new(
            first.session,
            first.services,
            factory,
            vec![],
            None,
        ));
        let output = Arc::new(MemoryOutput::default());
        let process = Arc::new(TestProcess {
            log: log.clone(),
            ..Default::default()
        });
        let guard = OutputGuard::new(
            output.clone(),
            Arc::new(MemoryOutput::default()),
            Arc::new(|code| panic!("unexpected stdout exit {code}")),
        );
        Self {
            _directory: directory,
            host,
            sessions,
            log,
            output,
            guard,
            process,
        }
    }
    async fn run(&self, options: PrintModeOptions) -> Result<i32> {
        bounded(run_print_mode_with_io(
            self.host.clone(),
            options,
            self.guard.clone(),
            self.process.clone(),
        ))
        .await
    }
    fn state(&self, mode: PrintMode) -> Arc<PrintState> {
        Arc::new(PrintState {
            runtime: self.host.clone(),
            session: Mutex::new(self.host.session()),
            mode,
            output: self.guard.clone(),
            process: self.process.clone(),
            subscriptions: Mutex::new(Subscriptions::default()),
            disposed: AtomicBool::new(false),
            wire_error: Mutex::new(None),
        })
    }
}
fn assistant(reason: StopReason, error: Option<&str>) -> AgentMessage {
    AgentMessage::Assistant(faux_assistant_message(
        "done",
        FauxMessageOptions {
            stop_reason: Some(reason),
            error_message: error.map(str::to_owned),
            timestamp: Some(2),
            ..Default::default()
        },
    ))
}
fn event_rows(trace: &Trace, kind: &str) -> Vec<Value> {
    snapshot(trace)
        .into_iter()
        .filter(|row| row["event"]["type"] == kind)
        .collect()
}

#[tokio::test]
async fn text_prints_only_text_blocks_then_shuts_down_and_flushes() {
    let AgentMessage::Assistant(mut last) = assistant(StopReason::Stop, None) else {
        unreachable!()
    };
    last.content = serde_json::from_value(json!([{"type":"thinking","thinking":"hidden"},{"type":"text","text":"one"},{"type":"text","text":""},{"type":"text","text":"two\n"}])).unwrap();
    let f = Fixture::new(vec![AgentMessage::Assistant(last)], vec![], None).await;
    assert_eq!(f.run(PrintModeOptions::default()).await.unwrap(), 0);
    assert_eq!(f.output.bytes(), "one\n\ntwo\n\n");
    assert_eq!(f.output.writes.lock().unwrap().last().unwrap(), "");
    assert_eq!(event_rows(&f.log, "session_shutdown").len(), 1);
    assert_eq!(
        event_rows(&f.log, "session_shutdown")[0]["event"],
        json!({"type":"session_shutdown","reason":"quit"})
    );
    assert!(f.process.handlers.lock().unwrap().is_empty());
    let rows = snapshot(&f.log);
    assert!(
        rows.iter().position(|r| r[0] == "remove_signal").unwrap()
            < rows
                .iter()
                .position(|r| r["event"]["type"] == "session_shutdown")
                .unwrap()
    );
    assert!(f
        .host
        .session()
        .extension_runner()
        .create_context()
        .cwd()
        .is_err());
}
#[tokio::test]
async fn text_error_and_abort_use_js_truthy_fallback_and_return_one() {
    for (reason, error, expected) in [
        (StopReason::Error, None, "Request error"),
        (StopReason::Error, Some(""), "Request error"),
        (StopReason::Aborted, Some(""), "Request aborted"),
        (
            StopReason::Aborted,
            Some("custom failure"),
            "custom failure",
        ),
    ] {
        let f = Fixture::new(vec![assistant(reason, error)], vec![], None).await;
        assert_eq!(f.run(PrintModeOptions::default()).await.unwrap(), 1);
        assert_eq!(f.output.bytes(), "");
        assert!(snapshot(&f.log).contains(&json!(["error", expected])));
        assert_eq!(event_rows(&f.log, "session_shutdown").len(), 1);
    }
}
#[tokio::test]
async fn text_does_not_search_past_a_non_assistant_tail_or_invent_empty_output() {
    for messages in [
        vec![],
        vec![
            assistant(StopReason::Stop, None),
            serde_json::from_value(json!({"role":"user","content":"later","timestamp":3})).unwrap(),
        ],
    ] {
        let f = Fixture::new(messages, vec![], None).await;
        assert_eq!(f.run(PrintModeOptions::default()).await.unwrap(), 0);
        assert_eq!(*f.output.writes.lock().unwrap(), [""]);
    }
}
#[tokio::test]
async fn initial_images_and_follow_up_prompts_use_real_extension_input_path() {
    let f = Fixture::new(vec![], vec![], None).await;
    let images =
        serde_json::from_value(json!([{"type":"image","mimeType":"image/png","data":"abc"}]))
            .unwrap();
    assert_eq!(
        f.run(PrintModeOptions {
            initial_message: Some("first".into()),
            initial_images: Some(images),
            messages: vec!["second".into(), "".into()],
            ..Default::default()
        })
        .await
        .unwrap(),
        0
    );
    let inputs = event_rows(&f.log, "input");
    assert_eq!(
        inputs
            .iter()
            .map(|r| r["event"]["text"].clone())
            .collect::<Vec<_>>(),
        [json!("first"), json!("second"), json!("")]
    );
    assert_eq!(
        inputs[0]["event"]["images"],
        json!([{"type":"image","mimeType":"image/png","data":"abc"}])
    );
    assert!(inputs[1]["event"]["images"].is_null());
    assert_eq!(event_rows(&f.log, "session_start")[0]["mode"], "print");
}
#[tokio::test]
async fn json_header_is_first_and_empty_initial_prompt_is_skipped() {
    let f = Fixture::new(
        vec![assistant(StopReason::Error, Some("not a text-mode error"))],
        vec![],
        None,
    )
    .await;
    let header = f
        .host
        .session()
        .session_manager
        .lock()
        .unwrap()
        .get_header()
        .unwrap();
    assert_eq!(
        f.run(PrintModeOptions {
            mode: PrintMode::Json,
            initial_message: Some(String::new()),
            ..Default::default()
        })
        .await
        .unwrap(),
        0
    );
    assert_eq!(
        f.output.bytes(),
        crate::serde_support::to_json_string_with_js_numbers(&FileEntry::Session(header)).unwrap()
            + "\n"
    );
    assert!(event_rows(&f.log, "input").is_empty());
    assert_eq!(event_rows(&f.log, "session_start")[0]["mode"], "json");
    assert_eq!(event_rows(&f.log, "session_shutdown").len(), 1);
}
#[tokio::test]
async fn dispose_failure_overrides_success_and_skips_flush() {
    let f = Fixture::new(vec![], vec![], None).await;
    f.host
        .set_before_session_invalidate(Some(Arc::new(|| Err(anyhow!("dispose failure")))));
    assert_eq!(
        f.run(PrintModeOptions::default())
            .await
            .unwrap_err()
            .to_string(),
        "dispose failure"
    );
    assert!(f.output.writes.lock().unwrap().is_empty());
    assert!(f.process.handlers.lock().unwrap().is_empty());
    assert_eq!(event_rows(&f.log, "session_shutdown").len(), 1);
}
#[tokio::test]
async fn new_session_command_rebinds_and_awaits_setup_and_with_session() {
    let calls = Trace::default();
    let extra: ExtensionFactory = {
        let calls = calls.clone();
        Arc::new(move |api| {
            let calls = calls.clone();
            api.register_command(
                "replace",
                None,
                Arc::new(move |_, ctx| {
                    let setup: extension::SessionSetupHandler = {
                        let calls = calls.clone();
                        Arc::new(move |handle| {
                            let manager = handle
                                .clone()
                                .downcast::<Mutex<SessionManager>>()
                                .map_err(|_| "manager type")?;
                            let calls = calls.clone();
                            CommandFuture::spawn(async move {
                                tokio::task::yield_now().await;
                                record(&calls, json!("setup"));
                                manager
                                    .lock()
                                    .unwrap()
                                    .append_message(assistant(StopReason::Stop, None))
                                    .map_err(|e| e.to_string())?;
                                Ok(())
                            })
                        })
                    };
                    let with: extension::WithSessionHandler = {
                        let calls = calls.clone();
                        Arc::new(move |ctx| {
                            assert!(ctx.cwd().is_ok());
                            let calls = calls.clone();
                            CommandFuture::spawn(async move {
                                tokio::task::yield_now().await;
                                record(&calls, json!("with"));
                                Ok(())
                            })
                        })
                    };
                    let pending = ctx.new_session(Some(extension::NewSessionOptions {
                        setup: Some(setup),
                        with_session: Some(with),
                        ..Default::default()
                    }))?;
                    let calls = calls.clone();
                    CommandFuture::spawn(async move {
                        assert!(!pending.await?.cancelled);
                        record(&calls, json!("done"));
                        Ok(())
                    })
                    .map(Some)
                }),
            )
        })
    };
    let f = Fixture::new(vec![], vec![extra], None).await;
    let old = f.host.session();
    let old_id = old.session_id();
    assert_eq!(
        f.run(PrintModeOptions {
            initial_message: Some("/replace".into()),
            messages: vec!["after".into()],
            ..Default::default()
        })
        .await
        .unwrap(),
        0
    );
    assert_eq!(
        snapshot(&calls),
        [json!("setup"), json!("with"), json!("done")]
    );
    assert_eq!(f.sessions.lock().unwrap().len(), 2);
    assert_ne!(f.host.session().session_id(), old_id);
    assert_eq!(
        event_rows(&f.log, "input")[0]["session"],
        f.host.session().session_id()
    );
    assert_eq!(f.output.bytes(), "done\n");
    assert!(old.extension_runner().create_context().cwd().is_err());
    assert_eq!(
        event_rows(&f.log, "session_shutdown")
            .iter()
            .map(|v| v["event"]["reason"].clone())
            .collect::<Vec<_>>(),
        [json!("new"), json!("quit")]
    );
}
#[tokio::test]
async fn callback_adapters_preserve_sync_throw_and_async_rejection() {
    let f = Fixture::new(vec![], vec![], None).await;
    for immediate in [true, false] {
        let callback: extension::SessionSetupHandler = Arc::new(move |_| {
            if immediate {
                Err("setup error".into())
            } else {
                Ok(CommandFuture::rejected("setup error".into()))
            }
        });
        assert_eq!(
            setup_session(Some(callback)).unwrap()(f.host.session().session_manager.clone())
                .await
                .unwrap_err()
                .to_string(),
            "setup error"
        );
        let callback: extension::WithSessionHandler = Arc::new(move |_| {
            if immediate {
                Err("with error".into())
            } else {
                Ok(CommandFuture::rejected("with error".into()))
            }
        });
        assert_eq!(
            with_session(Some(callback)).unwrap()(
                f.host.session().extension_runner().create_command_context()
            )
            .await
            .unwrap_err()
            .to_string(),
            "with error"
        );
    }
    f.host.dispose().await.unwrap();
}
#[tokio::test]
async fn dispose_guard_has_synchronous_unsubscribe_prefix_and_does_not_await_twice() {
    let gate = Arc::new(Semaphore::new(0));
    let entered = Arc::new(Notify::new());
    let extra: ExtensionFactory = {
        let gate = gate.clone();
        let entered = entered.clone();
        Arc::new(move |api| {
            let gate = gate.clone();
            let entered = entered.clone();
            api.on(
                "session_shutdown",
                Arc::new(move |_, _| {
                    let gate = gate.clone();
                    let entered = entered.clone();
                    Box::pin(async move {
                        entered.notify_one();
                        gate.acquire().await.unwrap().forget();
                        Ok(None)
                    })
                }),
            )
            .map(|_| ())
        })
    };
    let f = Fixture::new(vec![], vec![extra], None).await;
    let state = f.state(PrintMode::Json);
    state.rebind().await.unwrap();
    let first = state.dispose();
    assert!(state.disposed.load(Ordering::SeqCst));
    assert!(state.subscriptions.lock().unwrap().session.is_none());
    assert!(state.subscriptions.lock().unwrap().pressure.is_none());
    let task = tokio::spawn(first);
    bounded(entered.notified()).await;
    bounded(state.dispose()).await.unwrap();
    assert!(!task.is_finished());
    gate.add_permits(1);
    bounded(task).await.unwrap().unwrap();
    assert_eq!(event_rows(&f.log, "session_shutdown").len(), 1);
}
#[tokio::test]
async fn signal_kills_children_disposes_and_uses_correct_exit_codes() {
    for signal in [PrintSignal::Terminate, PrintSignal::Hangup] {
        let wait = Arc::new(Semaphore::new(0));
        let entered = Arc::new(Notify::new());
        let extra: ExtensionFactory = {
            let wait = wait.clone();
            let entered = entered.clone();
            Arc::new(move |api| {
                let wait = wait.clone();
                let entered = entered.clone();
                api.register_command(
                    "wait",
                    None,
                    Arc::new(move |_, _| {
                        let wait = wait.clone();
                        let entered = entered.clone();
                        CommandFuture::spawn(async move {
                            entered.notify_one();
                            wait.acquire().await.unwrap().forget();
                            Ok(())
                        })
                        .map(Some)
                    }),
                )
            })
        };
        let f = Fixture::new(vec![], vec![extra], None).await;
        let task = tokio::spawn(run_print_mode_with_io(
            f.host.clone(),
            PrintModeOptions {
                initial_message: Some("/wait".into()),
                ..Default::default()
            },
            f.guard.clone(),
            f.process.clone(),
        ));
        bounded(entered.notified()).await;
        f.process.fire(signal);
        bounded(f.process.exited.notified()).await;
        let rows = snapshot(&f.log);
        let kill = rows.iter().position(|r| r[0] == "kill_children").unwrap();
        let shutdown = rows
            .iter()
            .position(|r| r["event"]["type"] == "session_shutdown")
            .unwrap();
        let exit = rows
            .iter()
            .position(|r| r == &json!(["exit", signal.exit_code()]))
            .unwrap();
        assert!(kill < shutdown && shutdown < exit);
        wait.add_permits(1);
        assert_eq!(bounded(task).await.unwrap().unwrap(), 0);
        assert_eq!(event_rows(&f.log, "session_shutdown").len(), 1);
        assert!(f.process.handlers.lock().unwrap().is_empty());
    }
}
#[tokio::test]
async fn json_output_awaits_agent_backpressure_and_emits_native_stream_events() {
    let calls = Arc::new(AtomicUsize::new(0));
    let stream: Arc<StreamFn> = {
        let calls = calls.clone();
        Arc::new(move |_, _, _| {
            calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {
                let AgentMessage::Assistant(final_message) = assistant(StopReason::Stop, None)
                else {
                    unreachable!()
                };
                let mut start = final_message.clone();
                start.content.clear();
                start.stop_reason = StopReason::Pending;
                let events = vec![
                    AssistantMessageEvent::Start { message: start },
                    AssistantMessageEvent::TextStart { content_index: 0 },
                    AssistantMessageEvent::TextDelta {
                        content_index: 0,
                        delta: "done".into(),
                    },
                    AssistantMessageEvent::TextEnd {
                        content_index: 0,
                        content: "done".into(),
                    },
                    AssistantMessageEvent::Done {
                        reason: SuccessReason::Stop,
                        message: final_message,
                    },
                ];
                let (tx, rx) = tokio::sync::mpsc::channel(events.len());
                for event in events {
                    tx.try_send(event).unwrap();
                }
                Ok(rx)
            })
        })
    };
    let agent_slot = Arc::new(Mutex::new(Weak::<Agent>::new()));
    let entered = Arc::new(Notify::new());
    let extra: ExtensionFactory = {
        let agent_slot = agent_slot.clone();
        let entered = entered.clone();
        Arc::new(move |api| {
            let agent_slot = agent_slot.clone();
            let entered = entered.clone();
            api.register_command(
                "generate",
                None,
                Arc::new(move |_, _| {
                    let agent = agent_slot.lock().unwrap().upgrade().unwrap();
                    let entered = entered.clone();
                    CommandFuture::spawn(async move {
                        entered.notify_one();
                        agent.prompt("hello").await.map_err(|e| e.to_string())
                    })
                    .map(Some)
                }),
            )
        })
    };
    let f = Fixture::new(vec![], vec![extra], Some(stream)).await;
    *agent_slot.lock().unwrap() = Arc::downgrade(&f.host.session().agent);
    f.output.hold_next.store(true, Ordering::SeqCst);
    let task = tokio::spawn(run_print_mode_with_io(
        f.host.clone(),
        PrintModeOptions {
            mode: PrintMode::Json,
            initial_message: Some("/generate".into()),
            ..Default::default()
        },
        f.guard.clone(),
        f.process.clone(),
    ));
    bounded(entered.notified()).await;
    bounded(f.output.written.notified()).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "provider must not be entered while stdout is blocked"
    );
    assert!(!task.is_finished());
    assert_eq!(
        f.output.writes.lock().unwrap().len(),
        1,
        "only the header may have reached the sink"
    );
    f.output.release();
    assert_eq!(bounded(task).await.unwrap().unwrap(), 0);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let rows: Vec<Value> = f
        .output
        .bytes()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows[0]["type"], "session");
    assert!(rows.iter().any(
        |r| r["type"] == "message_update" && r["assistantMessageEvent"]["type"] == "text_delta"
    ));
    assert!(rows.iter().any(|r| r["type"] == "agent_end"));
    assert_eq!(event_rows(&f.log, "session_shutdown").len(), 1);
}
#[tokio::test]
async fn successful_print_does_not_create_a_runtime_ownership_cycle() {
    let f = Fixture::new(vec![], vec![], None).await;
    let weak = Arc::downgrade(&f.host);
    f.run(PrintModeOptions::default()).await.unwrap();
    drop(f);
    assert!(weak.upgrade().is_none());
}

// Complete native SDK/provider path, with no prompt interception and no direct
// Agent.prompt bypass. Faux supplies only offline provider IO.
#[tokio::test]
async fn print_mode_runs_the_real_services_sdk_provider_and_persists_each_turn() {
    use crate::ai::models::{faux_provider, FauxProviderOptions};
    use crate::coding_agent::core::agent_session_services::{
        create_agent_session_from_services, create_agent_session_services,
        CreateAgentSessionFromServicesOptions, CreateAgentSessionServicesOptions,
    };
    use crate::coding_agent::core::sdk::NoTools;
    for mode in [PrintMode::Text, PrintMode::Json] {
        let f = Fixture::new(vec![], vec![], None).await;
        f.host.dispose().await.unwrap();
        f.log.lock().unwrap().clear();
        let faux = faux_provider(FauxProviderOptions {
            provider: Some("print-native-faux".into()),
            ..Default::default()
        });
        faux.set_responses(vec![
            faux_assistant_message("first answer", FauxMessageOptions::default()).into(),
            faux_assistant_message("last answer", FauxMessageOptions::default()).into(),
        ]);
        let model = faux.get_model(None).unwrap();
        let provider = faux.provider.clone();
        let models = f.host.services().model_runtime.clone();
        let log = f.log.clone();
        let factory: CreateAgentSessionRuntimeFactory = Arc::new(move |options| {
            let models = models.clone();
            let model = model.clone();
            let provider = provider.clone();
            let log = log.clone();
            Box::pin(async move {
                let extension: ExtensionFactory = Arc::new(move |api| {
                    api.register_native_provider(&provider)?;
                    let log = log.clone();
                    for name in ["input", "before_agent_start", "session_shutdown"] {
                        let log = log.clone();
                        api.on(
                            name,
                            sync_handler(move |event, _| {
                                record(&log, json!({"event":event}));
                                Ok(None)
                            }),
                        )?;
                    }
                    Ok(())
                });
                let services = create_agent_session_services(CreateAgentSessionServicesOptions {
                    cwd: options.cwd,
                    agent_dir: Some(options.agent_dir),
                    model_runtime: Some(models),
                    settings_manager: Some(SettingsManager::in_memory(
                        serde_json::from_value(json!({})).unwrap(),
                    )),
                    resource_loader_options: Some(DefaultResourceLoaderOptions {
                        extension_factories: vec![InlineExtension::Factory(extension)],
                        no_skills: true,
                        no_prompt_templates: true,
                        no_themes: true,
                        no_context_files: true,
                        ..Default::default()
                    }),
                    ..Default::default()
                })
                .await?;
                let mut create = CreateAgentSessionFromServicesOptions::new(
                    services.clone(),
                    options.session_manager,
                );
                create.model = Some(model);
                create.no_tools = Some(NoTools::All);
                create.session_start_event = options
                    .session_start_event
                    .map(|event| serde_json::to_value(event).unwrap());
                let result = create_agent_session_from_services(create).await?;
                Ok(CreateAgentSessionRuntimeResult {
                    session: result.session,
                    extensions_result: result.extensions_result,
                    model_fallback_message: result.model_fallback_message,
                    diagnostics: services.diagnostics.clone(),
                    services: Arc::new(services),
                })
            })
        });
        let manager = Arc::new(Mutex::new(
            SessionManager::in_memory(&f.host.cwd(), None, None).unwrap(),
        ));
        let first = bounded(factory(CreateAgentSessionRuntimeOptions {
            cwd: f.host.cwd(),
            agent_dir: f.host.services().agent_dir.clone(),
            session_manager: manager.clone(),
            session_start_event: None,
            project_trust_context: None,
        }))
        .await
        .unwrap();
        let host = Arc::new(AgentSessionRuntime::new(
            first.session,
            first.services,
            factory,
            first.diagnostics,
            first.model_fallback_message,
        ));
        assert_eq!(
            bounded(run_print_mode_with_io(
                host.clone(),
                PrintModeOptions {
                    mode,
                    initial_message: Some("first prompt".into()),
                    initial_images: Some(vec![ImageContent {
                        data: "abc".into(),
                        mime_type: "image/png".into()
                    }]),
                    messages: vec!["second prompt".into()],
                },
                f.guard.clone(),
                f.process.clone()
            ))
            .await
            .unwrap(),
            0
        );
        assert_eq!(faux.state().lock().unwrap().call_count, 2);
        let entries = manager.lock().unwrap().get_entries().to_vec();
        assert_eq!(entries.iter().filter(|e|matches!(e,crate::coding_agent::session_manager::SessionEntry::Message(m) if matches!(m.message,AgentMessage::Assistant(_)))).count(),2);
        if mode == PrintMode::Text {
            assert_eq!(f.output.bytes(), "last answer\n");
        } else {
            let rows: Vec<Value> = f
                .output
                .bytes()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            assert_eq!(rows[0]["type"], "session");
            assert_eq!(rows.iter().filter(|r| r["type"] == "agent_end").count(), 2);
            assert!(rows.iter().any(|r| r["type"] == "message_end"
                && r["message"]["content"][0]["text"] == "last answer"));
        }
        for name in ["input", "before_agent_start"] {
            let rows = event_rows(&f.log, name);
            assert_eq!(rows.len(), 2);
            assert_eq!(
                rows[0]["event"]["images"],
                json!([{"type":"image","mimeType":"image/png","data":"abc"}])
            );
            assert!(rows[1]["event"]["images"].is_null());
        }
        assert_eq!(event_rows(&f.log, "session_shutdown").len(), 1);
        assert!(host
            .session()
            .extension_runner()
            .create_context()
            .cwd()
            .is_err());
    }
}
