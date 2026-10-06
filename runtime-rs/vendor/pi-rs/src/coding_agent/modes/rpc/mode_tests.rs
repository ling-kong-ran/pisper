//! RPC process-boundary tests: real services, SDK, runtime, extension runner
//! and optional offline faux model. Only byte streams and process effects are fake.
use super::*;
use crate::ai::auth::credential_store::InMemoryCredentialStore;
use crate::ai::models::{
    faux_assistant_message, faux_provider, FauxMessageOptions, FauxProviderHandle,
    FauxProviderOptions,
};
use crate::coding_agent::core::agent_session_runtime::{
    CreateAgentSessionRuntimeFactory, CreateAgentSessionRuntimeOptions,
    CreateAgentSessionRuntimeResult,
};
use crate::coding_agent::core::agent_session_services::{
    create_agent_session_from_services, create_agent_session_services,
    CreateAgentSessionFromServicesOptions, CreateAgentSessionServicesOptions,
};
use crate::coding_agent::core::model_runtime::{CreateModelRuntimeOptions, ModelRuntime};
use crate::coding_agent::core::models_store::InMemoryCodingAgentModelsStore;
use crate::coding_agent::core::output_guard::{
    OutputChunk, OutputError, OutputStream, WriteCallback,
};
use crate::coding_agent::core::resource_loader::{DefaultResourceLoaderOptions, InlineExtension};
use crate::coding_agent::core::sdk::NoTools;
use crate::coding_agent::core::settings_manager::SettingsManager;
use crate::coding_agent::extensions::loader::ExtensionFactory;
use crate::coding_agent::extensions::types::{
    sync_handler, ExtensionUiDialogOptions, HandlerResult,
};
use crate::coding_agent::modes::print_mode::SignalHandler;
use crate::coding_agent::session_manager::SessionManager;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

// This oracle executes the complete upstream runRpcMode and its real JSONL
// reader, with mock session/process/output boundaries. The tests below use the
// real native runtime instead. Compare the process-effect trace and flushes;
// fake upstream session data is NOT evidence of full SDK/CLI equivalence.
fn oracle_case(id: &str) -> Value {
    let oracle: Value = serde_json::from_str(include_str!("mode_oracle.json")).unwrap();
    oracle["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == id)
        .unwrap()
        .clone()
}
fn assert_process_oracle(id: &str, output: &MemoryOutput, process: &TestProcess) {
    let case = oracle_case(id);
    assert_eq!(
        json!(*process.log.lock().unwrap()),
        case["log"],
        "{id}: process trace"
    );
    let flushes = output
        .writes
        .lock()
        .unwrap()
        .iter()
        .filter(|wire| wire.is_empty())
        .count();
    assert_eq!(json!(flushes), case["flushCalls"], "{id}: stdout flushes");
}
fn normalized_request(mut request: Value) -> Value {
    assert!(request["id"].is_string());
    request["id"] = json!("request-1");
    request
}

type Trace = Arc<Mutex<Vec<Value>>>;
async fn bounded<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(15), future)
        .await
        .expect("RPC mode timed out")
}
async fn make_runtime(
    faux: Option<&FauxProviderHandle>,
    factories: Vec<ExtensionFactory>,
) -> (tempfile::TempDir, Arc<AgentSessionRuntime>) {
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
    let model = faux.and_then(|faux| faux.get_model(None));
    let provider = faux.map(|faux| faux.provider.clone());
    let factory: CreateAgentSessionRuntimeFactory = Arc::new(move |options| {
        let model = model.clone();
        let provider = provider.clone();
        let models = models.clone();
        let mut factories = factories.clone();
        Box::pin(async move {
            if let Some(provider) = provider {
                factories.insert(
                    0,
                    Arc::new(move |api| {
                        api.register_native_provider(&provider)?;
                        Ok(())
                    }),
                );
            }
            let services = create_agent_session_services(CreateAgentSessionServicesOptions {
                cwd: options.cwd,
                agent_dir: Some(options.agent_dir),
                model_runtime: Some(models),
                settings_manager: Some(SettingsManager::in_memory(
                    serde_json::from_value(json!({})).unwrap(),
                )),
                resource_loader_options: Some(DefaultResourceLoaderOptions {
                    extension_factories: factories
                        .into_iter()
                        .map(InlineExtension::Factory)
                        .collect(),
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
            create.model = model;
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
    let first = bounded(factory(CreateAgentSessionRuntimeOptions {
        cwd: cwd.clone(),
        agent_dir,
        session_manager: Arc::new(Mutex::new(
            SessionManager::in_memory(&cwd, None, None).unwrap(),
        )),
        session_start_event: None,
        project_trust_context: None,
    }))
    .await
    .unwrap();
    let runtime = Arc::new(AgentSessionRuntime::new(
        first.session,
        first.services,
        factory,
        first.diagnostics,
        first.model_fallback_message,
    ));
    (directory, runtime)
}

#[derive(Default)]
struct MemoryOutput {
    writes: Mutex<Vec<String>>,
    hold_next: AtomicBool,
    pending: Mutex<Vec<WriteCallback>>,
    written: Notify,
}
impl MemoryOutput {
    fn rows(&self) -> Vec<Value> {
        self.writes
            .lock()
            .unwrap()
            .concat()
            .lines()
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
    fn release(&self) {
        for callback in std::mem::take(&mut *self.pending.lock().unwrap()) {
            callback(Ok(()));
        }
    }
    async fn matching(&self, predicate: impl Fn(&Value) -> bool) -> Value {
        bounded(async {
            loop {
                let notified = self.written.notified();
                if let Some(row) = self.rows().into_iter().find(&predicate) {
                    return row;
                }
                notified.await;
            }
        })
        .await
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
            panic!("expected text")
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
}
impl TestProcess {
    fn fire(&self, signal: PrintSignal) {
        let handler = self
            .handlers
            .lock()
            .unwrap()
            .iter()
            .find(|(kind, _)| *kind == signal)
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
            this.handlers
                .lock()
                .unwrap()
                .retain(|(kind, _)| *kind != signal);
            this.log
                .lock()
                .unwrap()
                .push(json!(["remove", signal.exit_code()]));
        }))
    }
    fn error(&self, message: &str) {
        self.log.lock().unwrap().push(json!(["error", message]));
    }
    fn kill_tracked_children(&self) {
        self.log.lock().unwrap().push(json!(["kill"]));
    }
    fn exit(&self, code: i32) {
        self.log.lock().unwrap().push(json!(["exit", code]));
    }
}
struct Running {
    _directory: tempfile::TempDir,
    runtime: Arc<AgentSessionRuntime>,
    input: tokio::io::DuplexStream,
    output: Arc<MemoryOutput>,
    process: Arc<TestProcess>,
    guard: OutputGuard,
    task: tokio::task::JoinHandle<Result<i32>>,
}
impl Running {
    async fn start(faux: Option<&FauxProviderHandle>, factories: Vec<ExtensionFactory>) -> Self {
        let (directory, runtime) = make_runtime(faux, factories).await;
        let (input, reader) = tokio::io::duplex(8192);
        let output = Arc::new(MemoryOutput::default());
        let process = Arc::new(TestProcess::default());
        let guard = OutputGuard::new(
            output.clone(),
            Arc::new(MemoryOutput::default()),
            Arc::new(|code| panic!("unexpected writer exit {code}")),
        );
        let task = tokio::spawn(run_rpc_mode_with_io(
            runtime.clone(),
            reader,
            guard.clone(),
            process.clone(),
        ));
        let mut running = Self {
            _directory: directory,
            runtime,
            input,
            output,
            process,
            guard,
            task,
        };
        running.send(json!({"type":"get_state","id":"ready"})).await;
        running.output.matching(|v| v["id"] == "ready").await;
        running
    }
    async fn send(&mut self, value: Value) {
        self.input
            .write_all(serialize_json_line(&value).unwrap().as_bytes())
            .await
            .unwrap();
    }
    async fn stop(mut self) -> (Arc<MemoryOutput>, Arc<TestProcess>) {
        self.input.shutdown().await.unwrap();
        assert_eq!(bounded(self.task).await.unwrap().unwrap(), 0);
        (self.output, self.process)
    }
}

#[tokio::test]
async fn fragmented_jsonl_routes_commands_unknowns_parse_errors_and_eof_tail() {
    let mut f = Running::start(None, vec![]).await;
    f.input
        .write_all(b"{\"id\":\"split\",\"type\":\"set_session_")
        .await
        .unwrap();
    f.input
        .write_all("name\",\"name\":\"\u{feff} named \u{feff}\"}\r\n".as_bytes())
        .await
        .unwrap();
    assert_eq!(
        f.output.matching(|v| v["id"] == "split").await["success"],
        true
    );
    f.send(json!({"id":"query","type":"get_state"})).await;
    assert_eq!(
        f.output.matching(|v| v["id"] == "query").await["data"]["sessionName"],
        "named"
    );
    f.send(json!({"id":"unknown","type":"not_a_command"})).await;
    assert_eq!(
        f.output.matching(|v| v["id"] == "unknown").await,
        json!({"id":"unknown","type":"response","command":"not_a_command","success":false,"error":"Unknown command: not_a_command"})
    );
    f.input.write_all(b"{\n").await.unwrap();
    assert_eq!(
        f.output.matching(|v| v["command"] == "parse").await["success"],
        false
    );
    f.input
        .write_all(b"{\"id\":\"tail\",\"type\":\"get_state\"}")
        .await
        .unwrap();
    let (output, process) = f.stop().await;
    assert_process_oracle("eof-tail", &output, &process);
    let actual_tail = output
        .rows()
        .into_iter()
        .find(|row| row["id"] == "tail")
        .unwrap();
    let expected_tail = &oracle_case("eof-tail")["outputs"][0];
    for key in ["type", "id", "command", "success"] {
        assert_eq!(actual_tail[key], expected_tail[key]);
    }
    assert!(output
        .rows()
        .iter()
        .any(|row| row["id"] == "tail" && row["success"] == true));
    assert_eq!(output.writes.lock().unwrap().last().unwrap(), "");
    assert!(process.handlers.lock().unwrap().is_empty());
    assert_eq!(
        process.log.lock().unwrap().last().unwrap(),
        &json!(["exit", 0])
    );
}

#[tokio::test]
async fn extension_ui_reply_is_not_blocked_by_prompt_and_stays_bound_after_replacement() {
    let extension: ExtensionFactory = Arc::new(|api| {
        api.on(
            "input",
            Arc::new(|_, ctx| {
                Box::pin(async move {
                    let reply = ctx
                        .ui()?
                        .input(
                            "client question",
                            None,
                            &ExtensionUiDialogOptions::default(),
                        )
                        .await?;
                    if reply.as_deref() != Some("answer") {
                        return Err("bad client reply".into());
                    }
                    Ok(Some(HandlerResult::Json(json!({"action":"handled"}))))
                })
            }),
        )?;
        Ok(())
    });
    let mut f = Running::start(None, vec![extension]).await;
    let old = f.runtime.session();
    f.send(json!({"type":"new_session","id":"new"})).await;
    assert_eq!(
        f.output.matching(|v| v["id"] == "new").await["success"],
        true
    );
    assert!(!Arc::ptr_eq(&old, &f.runtime.session()));
    f.send(json!({"type":"prompt","id":"prompt","message":"ask"}))
        .await;
    let request = f
        .output
        .matching(|v| v["type"] == "extension_ui_request")
        .await;
    assert!(!f.output.rows().iter().any(|v| v["id"] == "prompt"));
    f.send(json!({"type":"get_state","id":"while-dialog"}))
        .await;
    assert_eq!(
        f.output.matching(|v| v["id"] == "while-dialog").await["success"],
        true
    );
    f.send(json!({"type":"extension_ui_response","id":request["id"],"value":"answer"}))
        .await;
    assert_eq!(
        f.output.matching(|v| v["id"] == "prompt").await["success"],
        true
    );
    let (output, _) = f.stop().await;
    assert_eq!(
        output.rows().iter().filter(|v| v["id"] == "prompt").count(),
        1
    );
}

#[tokio::test]
async fn real_turn_streams_events_and_persists_before_rpc_history_query() {
    let faux = faux_provider(FauxProviderOptions {
        provider: Some("rpc-mode-faux".into()),
        ..Default::default()
    });
    faux.set_responses(vec![faux_assistant_message(
        "mode answer",
        FauxMessageOptions::default(),
    )
    .into()]);
    let mut f = Running::start(Some(&faux), vec![]).await;
    f.send(json!({"type":"prompt","id":"prompt","message":"question"}))
        .await;
    assert_eq!(
        f.output.matching(|v| v["id"] == "prompt").await["success"],
        true
    );
    f.output.matching(|v| v["type"] == "agent_settled").await;
    f.send(json!({"type":"get_last_assistant_text","id":"last"}))
        .await;
    assert_eq!(
        f.output.matching(|v| v["id"] == "last").await["data"],
        json!({"text":"mode answer"})
    );
    let (output, _) = f.stop().await;
    let rows = output.rows();
    for kind in [
        "agent_start",
        "message_start",
        "message_end",
        "agent_end",
        "agent_settled",
    ] {
        assert!(rows.iter().any(|v| v["type"] == kind), "missing {kind}");
    }
    assert_eq!(
        rows.iter().filter(|v| v["type"] == "agent_settled").count(),
        1
    );
}

#[tokio::test]
async fn eof_waits_for_output_flush_and_does_not_repeat_disposal() {
    let shutdowns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = shutdowns.clone();
    let extension: ExtensionFactory = Arc::new(move |api| {
        let count = count.clone();
        api.on(
            "session_shutdown",
            sync_handler(move |_, _| {
                count.fetch_add(1, Ordering::SeqCst);
                Ok(None)
            }),
        )?;
        Ok(())
    });
    let mut f = Running::start(None, vec![extension]).await;
    assert!(f.guard.is_stdout_taken_over());
    f.output.hold_next.store(true, Ordering::SeqCst);
    f.send(json!({"type":"get_state","id":"hold"})).await;
    f.output.matching(|v| v["id"] == "hold").await;
    f.input.shutdown().await.unwrap();
    tokio::task::yield_now().await;
    assert!(!f.task.is_finished());
    f.output.release();
    assert_eq!(bounded(f.task).await.unwrap().unwrap(), 0);
    assert_eq!(shutdowns.load(Ordering::SeqCst), 1);
    assert_eq!(f.output.writes.lock().unwrap().last().unwrap(), "");
}

#[tokio::test]
async fn sigterm_skips_flush_but_hangup_drains_and_uses_correct_exit_codes() {
    for signal in [PrintSignal::Terminate, PrintSignal::Hangup] {
        let mut f = Running::start(None, vec![]).await;
        f.output.hold_next.store(true, Ordering::SeqCst);
        f.send(json!({"type":"get_state","id":"hold"})).await;
        f.output.matching(|v| v["id"] == "hold").await;
        f.process.fire(signal);
        if signal == PrintSignal::Hangup {
            tokio::task::yield_now().await;
            assert!(!f.task.is_finished());
            f.output.release();
        }
        assert_eq!(bounded(f.task).await.unwrap().unwrap(), signal.exit_code());
        assert_process_oracle(
            if signal == PrintSignal::Terminate {
                "sigterm"
            } else {
                "hangup"
            },
            &f.output,
            &f.process,
        );
        assert!(f.process.handlers.lock().unwrap().is_empty());
        let rows = f.process.log.lock().unwrap();
        assert_eq!(rows.first(), Some(&json!(["kill"])));
        assert_eq!(rows.last(), Some(&json!(["exit", signal.exit_code()])));
        drop(rows);
        if signal == PrintSignal::Terminate {
            assert_ne!(f.output.writes.lock().unwrap().last().unwrap(), "");
            f.output.release();
        } else {
            assert_eq!(f.output.writes.lock().unwrap().last().unwrap(), "");
        }
    }
}

#[tokio::test]
async fn extension_shutdown_flag_is_honored_after_command_preflight() {
    let extension: ExtensionFactory = Arc::new(|api| {
        api.on(
            "input",
            sync_handler(|_, ctx| {
                ctx.shutdown()?;
                Ok(Some(HandlerResult::Json(json!({"action":"handled"}))))
            }),
        )?;
        Ok(())
    });
    let mut f = Running::start(None, vec![extension]).await;
    f.send(json!({"type":"prompt","id":"shutdown","message":"end"}))
        .await;
    assert_eq!(bounded(f.task).await.unwrap().unwrap(), 0);
    assert!(f
        .output
        .rows()
        .iter()
        .any(|v| v["id"] == "shutdown" && v["success"] == true));
    assert_process_oracle("extension-shutdown", &f.output, &f.process);
    let replies = f
        .output
        .rows()
        .into_iter()
        .filter(|v| v["id"] == "shutdown")
        .collect::<Vec<_>>();
    assert_eq!(json!(replies), oracle_case("extension-shutdown")["outputs"]);
    assert_eq!(
        f.process.log.lock().unwrap().last(),
        Some(&json!(["exit", 0]))
    );
}

#[tokio::test]
async fn extension_errors_are_forwarded_as_events_not_stdout_noise() {
    let extension: ExtensionFactory = Arc::new(|api| {
        api.on("input", sync_handler(|_, _| Err("extension broke".into())))?;
        Ok(())
    });
    let mut f = Running::start(None, vec![extension]).await;
    f.send(json!({"type":"prompt","id":"failure","message":"run"}))
        .await;
    let event = f.output.matching(|v| v["type"] == "extension_error").await;
    assert_eq!(event["event"], "input");
    assert_eq!(event["error"], "extension broke");
    assert!(event["extensionPath"].is_string());
    assert_eq!(
        f.output.matching(|v| v["id"] == "failure").await["success"],
        false
    );
    f.stop().await;
}

#[tokio::test]
async fn shutdown_hook_can_receive_client_ui_response_before_input_detaches() {
    let completed = Arc::new(AtomicBool::new(false));
    let mark = completed.clone();
    let extension: ExtensionFactory = Arc::new(move |api| {
        let mark = mark.clone();
        api.on(
            "session_shutdown",
            Arc::new(move |_, ctx| {
                let mark = mark.clone();
                Box::pin(async move {
                    let answer = ctx
                        .ui()?
                        .confirm("exit", "save?", &ExtensionUiDialogOptions::default())
                        .await?;
                    if !answer {
                        return Err("shutdown reply was not delivered".into());
                    }
                    mark.store(true, Ordering::SeqCst);
                    Ok(None)
                })
            }),
        )?;
        Ok(())
    });
    let mut f = Running::start(None, vec![extension]).await;
    f.process.fire(PrintSignal::Hangup);
    let request = f
        .output
        .matching(|v| v["type"] == "extension_ui_request" && v["method"] == "confirm")
        .await;
    assert!(!f.task.is_finished());
    assert!(f.process.handlers.lock().unwrap().is_empty());
    f.send(json!({"type":"extension_ui_response","id":request["id"],"confirmed":true}))
        .await;
    assert_eq!(bounded(f.task).await.unwrap().unwrap(), 129);
    assert_process_oracle("shutdown-dialog", &f.output, &f.process);
    assert_eq!(
        normalized_request(request),
        oracle_case("shutdown-dialog")["outputs"][0]
    );
    assert!(completed.load(Ordering::SeqCst));
    assert_eq!(f.output.writes.lock().unwrap().last().unwrap(), "");
}

#[tokio::test]
async fn second_shutdown_exits_without_waiting_for_first_dispose_or_flush() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = calls.clone();
    let extension: ExtensionFactory = Arc::new(move |api| {
        let count = count.clone();
        api.on(
            "session_shutdown",
            Arc::new(move |_, ctx| {
                count.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move {
                    ctx.ui()?
                        .confirm("exit", "save?", &ExtensionUiDialogOptions::default())
                        .await?;
                    Ok(None)
                })
            }),
        )?;
        Ok(())
    });
    let mut f = Running::start(None, vec![extension]).await;
    f.process.fire(PrintSignal::Hangup);
    f.output
        .matching(|v| v["type"] == "extension_ui_request")
        .await;
    f.input.shutdown().await.unwrap();
    assert_eq!(bounded(f.task).await.unwrap().unwrap(), 0);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_process_oracle("second-eof", &f.output, &f.process);
    let request = f
        .output
        .rows()
        .into_iter()
        .find(|v| v["type"] == "extension_ui_request")
        .unwrap();
    assert_eq!(
        normalized_request(request),
        oracle_case("second-eof")["outputs"][0]
    );
    assert_ne!(f.output.writes.lock().unwrap().last().unwrap(), "");
    assert_eq!(
        f.process.log.lock().unwrap().last(),
        Some(&json!(["exit", 0]))
    );
}

#[tokio::test]
async fn cancelling_host_drops_pending_prompt_ui_and_detaches_process_handlers() {
    struct Dropped(Arc<AtomicBool>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let dropped = Arc::new(AtomicBool::new(false));
    let capture = dropped.clone();
    let extension: ExtensionFactory = Arc::new(move |api| {
        let capture = capture.clone();
        api.on(
            "input",
            Arc::new(move |_, ctx| {
                let capture = capture.clone();
                Box::pin(async move {
                    let _drop = Dropped(capture);
                    ctx.ui()?
                        .input("pending", None, &ExtensionUiDialogOptions::default())
                        .await?;
                    Ok(Some(HandlerResult::Json(json!({"action":"handled"}))))
                })
            }),
        )?;
        Ok(())
    });
    let mut f = Running::start(None, vec![extension]).await;
    f.send(json!({"type":"prompt","id":"pending","message":"ask"}))
        .await;
    f.output
        .matching(|v| v["type"] == "extension_ui_request")
        .await;
    f.task.abort();
    assert!(bounded(f.task).await.unwrap_err().is_cancelled());
    bounded(async {
        while !dropped.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(f.process.handlers.lock().unwrap().is_empty());
    assert!(!f.output.rows().iter().any(|v| v["id"] == "pending"));
}
