//! Full Runtime source traces, consumed by the real native session/manager.
//! The explicit factory below is a test collaborator, NOT the SDK factory.
use super::super::agent_session_services::DiagnosticType;
use super::super::session_cwd::*;
use super::*;
use crate::agent_core::agent::Agent;
use crate::agent_core::types::{AgentMessage, AgentOptions};
use crate::ai::auth::credential_store::InMemoryCredentialStore;
use crate::ai::models::{create_models, CreateModelsOptions};
use crate::coding_agent::agent_session::{AgentSessionConfig, ExtensionBindings};
use crate::coding_agent::core::model_runtime::{CreateModelRuntimeOptions, ModelRuntime};
use crate::coding_agent::core::models_store::InMemoryCodingAgentModelsStore;
use crate::coding_agent::core::resource_loader::{
    DefaultResourceLoader, DefaultResourceLoaderOptions, InlineExtension,
};
use crate::coding_agent::core::settings_manager::SettingsManager;
use crate::coding_agent::extensions::loader::ExtensionFactory;
use crate::coding_agent::extensions::types::{
    sync_handler, ExtensionMode, HandlerFn, HandlerResult,
};
use crate::coding_agent::session_manager::{FileEntry, SessionEntry, SessionHeader};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Weak,
};
use std::time::Duration;
use tokio::sync::oneshot;

type Trace = Arc<Mutex<Vec<Value>>>;
type RuntimeSlot = Arc<Mutex<Weak<AgentSessionRuntime>>>;
fn trace(log: &Trace, row: Value) {
    log.lock().unwrap().push(row);
}
fn oracle() -> Value {
    serde_json::from_str(include_str!("session_runtime_oracle.json")).unwrap()
}
fn active(session: &AgentSession) -> bool {
    session.extension_runner().create_context().cwd().is_ok()
}
fn host(slot: &RuntimeSlot) -> Arc<AgentSessionRuntime> {
    slot.lock().unwrap().upgrade().unwrap()
}
async fn bounded<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(8), future)
        .await
        .expect("Runtime future deadlocked")
}
async fn bounded_stage<T>(stage: &str, future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(8), future)
        .await
        .unwrap_or_else(|error| panic!("Runtime stage {stage} timed out: {error}"))
}
fn seed_entries() -> Vec<Value> {
    vec![
        json!({"type":"message","id":"root","parentId":null,"timestamp":"2026-09-27T00:00:00.000Z","message":{"role":"user","content":"first","timestamp":1}}),
        json!({"type":"message","id":"second","parentId":"root","timestamp":"2026-09-27T00:00:00.000Z","message":{"role":"user","content":[{"type":"text","text":"A"},{"type":"image","data":"","mimeType":"image/png"},{"type":"text","text":"B"}],"timestamp":2}}),
        json!({"type":"custom","id":"custom","parentId":"second","timestamp":"2026-09-27T00:00:00.000Z","customType":"test","data":{}}),
    ]
}
fn seed_header(cwd: &str) -> SessionHeader {
    serde_json::from_value(json!({"type":"session","version":3,"id":"0199aaaa-0000-7000-8000-000000000001","timestamp":"2026-09-27T00:00:00.000Z","cwd":cwd})).unwrap()
}
fn write_session(file: &Path, cwd: &str) {
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    let mut rows = vec![serde_json::to_value(FileEntry::Session(seed_header(cwd))).unwrap()];
    rows.extend(seed_entries());
    // The FileEntry wire wrapper (not SessionHeader itself) adds type:session.
    fs::write(
        file,
        rows.iter()
            .map(|r| serde_json::to_string(r).unwrap() + "\n")
            .collect::<String>(),
    )
    .unwrap();
}
fn memory_manager(cwd: &str) -> SessionManager {
    let mut rows = vec![FileEntry::Session(seed_header(cwd))];
    rows.extend(
        seed_entries()
            .into_iter()
            .map(|v| FileEntry::Entry(serde_json::from_value::<SessionEntry>(v).unwrap())),
    );
    SessionManager::in_memory(cwd, None, Some(rows)).unwrap()
}
async fn offline_models() -> ModelRuntime {
    ModelRuntime::create(CreateModelRuntimeOptions {
        credentials: Some(Arc::new(InMemoryCredentialStore::default())),
        models_path: Some(None),
        models_store: Some(Arc::new(InMemoryCodingAgentModelsStore::default())),
        allow_model_network: false,
        refresh_on_create: Some(false),
        ..Default::default()
    })
    .await
    .unwrap()
}
fn event_factory(spec: &Value, log: &Trace) -> ExtensionFactory {
    let spec = spec.clone();
    let log = log.clone();
    Arc::new(move |api| {
        for kind in [
            "session_before_switch",
            "session_before_fork",
            "session_shutdown",
        ] {
            let log = log.clone();
            let spec = spec.clone();
            api.on(
                kind,
                sync_handler(move |event, _| {
                    let phase = if event["type"] == "session_shutdown" {
                        "shutdown"
                    } else {
                        "before"
                    };
                    trace(&log, json!({"phase":phase,"event":event}));
                    if phase == "before" {
                        if spec["race"] == true {
                            fs::write(event["targetSessionFile"].as_str().unwrap(), "competitor")
                                .unwrap();
                        }
                        if let Some(cancel) = spec.get("cancel") {
                            return Ok(Some(HandlerResult::Json(json!({"cancel":cancel}))));
                        }
                    }
                    Ok(None)
                }),
            )?;
        }
        Ok(())
    })
}
fn native_session(
    options: &CreateAgentSessionRuntimeOptions,
    models: &ModelRuntime,
    factories: Vec<ExtensionFactory>,
) -> CreateAgentSessionRuntimeResult {
    native_session_with_agent(
        options,
        models,
        factories,
        Arc::new(Agent::new(
            AgentOptions::default(),
            Arc::new(create_models(CreateModelsOptions::default())),
        )),
    )
}
fn native_session_with_agent(
    options: &CreateAgentSessionRuntimeOptions,
    models: &ModelRuntime,
    factories: Vec<ExtensionFactory>,
    agent: Arc<Agent>,
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
    let loader = Arc::new(Mutex::new(loader));
    let services = Arc::new(AgentSessionServices {
        cwd: options.cwd.clone(),
        agent_dir: options.agent_dir.clone(),
        model_runtime: models.clone(),
        settings_manager: settings.clone(),
        resource_loader: loader.clone(),
        diagnostics: vec![],
    });
    agent.state().messages = options
        .session_manager
        .lock()
        .unwrap()
        .build_session_context()
        .messages;
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
            .map(|v| serde_json::to_value(v).unwrap()),
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
fn normalize(value: &Value, root: &Path) -> Value {
    match value {
        Value::String(s) => {
            let s = s
                .replace(root.to_str().unwrap(), "@ROOT")
                .replace('\\', "/");
            let generated = regex::Regex::new(r"@ROOT/[^\s]*?/\d{4}-[^\s/]*\.jsonl").unwrap();
            Value::String(generated.replace_all(&s, "@NEW").into_owned())
        }
        Value::Array(values) => Value::Array(values.iter().map(|v| normalize(v, root)).collect()),
        Value::Object(values) => Value::Object(
            values
                .iter()
                .map(|(k, v)| (k.clone(), normalize(v, root)))
                .collect(),
        ),
        other => other.clone(),
    }
}
fn current_id(list: &Arc<Mutex<Vec<Arc<AgentSession>>>>, session: &Arc<AgentSession>) -> usize {
    list.lock()
        .unwrap()
        .iter()
        .position(|s| Arc::ptr_eq(s, session))
        .unwrap()
}

async fn run_case(spec: &Value) -> Value {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().join(spec["name"].as_str().unwrap());
    let a = base.join("a");
    let b = base.join("b");
    let store = base.join("store");
    let agent = base.join("agent");
    for dir in [&a, &b, &store, &agent] {
        fs::create_dir_all(dir).unwrap();
    }
    let a = a.to_str().unwrap().to_owned();
    let b = b.to_str().unwrap().to_owned();
    let agent = agent.to_str().unwrap().to_owned();
    let op = spec["op"].as_str().unwrap();
    let missing = base.join("missing-cwd").to_str().unwrap().to_owned();
    let cwd = if op == "initial" && spec["missingCwd"] == true {
        &missing
    } else {
        &a
    };
    let initial = store.join("current.jsonl");
    let target = base.join("target").join("target.jsonl");
    write_session(&initial, cwd);
    write_session(
        &target,
        if spec["missingCwd"] == true {
            &missing
        } else {
            &b
        },
    );
    let manager = if spec["persist"] == true {
        SessionManager::open(
            initial.to_str().unwrap(),
            Some(store.to_str().unwrap()),
            None,
        )
        .unwrap()
    } else {
        memory_manager(cwd)
    };
    if spec["unflushed"] == true {
        fs::remove_file(&initial).unwrap();
    }
    let log = Trace::default();
    let slot = RuntimeSlot::default();
    let sessions = Arc::new(Mutex::new(Vec::new()));
    let models = offline_models().await;
    let options = CreateAgentSessionRuntimeOptions {
        cwd: cwd.clone(),
        agent_dir: agent.clone(),
        session_manager: Arc::new(Mutex::new(manager)),
        session_start_event: None,
        project_trust_context: None,
    };
    let mut first = native_session(&options, &models, vec![event_factory(spec, &log)]);
    // Runtime's service cwd is a fallback and can differ from the stored cwd.
    Arc::make_mut(&mut first.services).cwd = a.clone();
    sessions.lock().unwrap().push(first.session.clone());
    let factory: CreateAgentSessionRuntimeFactory = {
        let log = log.clone();
        let slot = slot.clone();
        let spec = spec.clone();
        let sessions = sessions.clone();
        Arc::new(move |options| {
            let log = log.clone();
            let slot = slot.clone();
            let spec = spec.clone();
            let sessions = sessions.clone();
            let models = models.clone();
            Box::pin(async move {
                let old_active = slot.lock().unwrap().upgrade().map(|h| active(&h.session()));
                let mut row = json!({"phase":"factory","cwd":options.cwd,"agentDir":options.agent_dir,"trust":options.project_trust_context.is_some(),"oldActive":old_active});
                if let Some(start) = &options.session_start_event {
                    row["start"] = serde_json::to_value(start).unwrap();
                }
                trace(&log, row);
                if spec["fail"] == "factory" {
                    bail!("factory rejected");
                }
                let mut result =
                    native_session(&options, &models, vec![event_factory(&spec, &log)]);
                let id = {
                    let mut list = sessions.lock().unwrap();
                    let id = list.len();
                    list.push(result.session.clone());
                    id
                };
                result.diagnostics = vec![AgentSessionRuntimeDiagnostic {
                    kind: DiagnosticType::Warning,
                    message: format!("runtime-{id}"),
                }];
                result.model_fallback_message = Some(format!("fallback-{id}"));
                Ok(result)
            })
        })
    };
    let runtime = Arc::new(AgentSessionRuntime::new(
        first.session,
        first.services,
        factory.clone(),
        vec![AgentSessionRuntimeDiagnostic {
            kind: DiagnosticType::Info,
            message: "initial".into(),
        }],
        Some("initial-fallback".into()),
    ));
    *slot.lock().unwrap() = Arc::downgrade(&runtime);
    runtime.set_before_session_invalidate(Some({
        let log = log.clone();
        let slot = slot.clone();
        let fail = spec["fail"] == "invalidate";
        Arc::new(move || {
            trace(
                &log,
                json!({"phase":"invalidate","active":active(&host(&slot).session())}),
            );
            if fail {
                bail!("invalidate rejected");
            }
            Ok(())
        })
    }));
    runtime.set_rebind_session(Some({
        let log = log.clone();
        let sessions = sessions.clone();
        let fail = spec["fail"] == "rebind";
        Arc::new(move |s| {
            trace(
                &log,
                json!({"phase":"rebind","id":current_id(&sessions,&s)}),
            );
            Box::pin(async move {
                if fail {
                    bail!("rebind rejected");
                }
                Ok(())
            })
        })
    }));
    let with: WithSession = {
        let log = log.clone();
        let fail = spec["fail"] == "with";
        Arc::new(move |ctx| {
            trace(&log, json!({"phase":"with","cwd":ctx.cwd().unwrap()}));
            Box::pin(async move {
                if fail {
                    bail!("with rejected");
                }
                Ok(())
            })
        })
    };
    let result:Result<Value>=async {
        match op {
            "new"=> {
                let setup:SetupSession={let log=log.clone();let slot=slot.clone();let sessions=sessions.clone();let spec=spec.clone();Arc::new(move |manager| {
                    let log=log.clone();let slot=slot.clone();let sessions=sessions.clone();let spec=spec.clone();Box::pin(async move {
                        let runtime=host(&slot);trace(&log,json!({"phase":"setup","id":current_id(&sessions,&runtime.session())}));
                        if spec["fail"]=="setup" {bail!("setup rejected");}
                        manager.lock().unwrap().append_message(serde_json::from_value::<AgentMessage>(json!({"role":"user","content":"setup","timestamp":3})).unwrap())?;
                        if spec["reenter"]==true {runtime.new_session(Default::default()).await?;}
                        Ok(())
                    })})};
                Ok(serde_json::to_value(runtime.new_session(NewSessionOptionsRuntime{parent_session:spec["parent"].as_str().map(str::to_owned),setup:Some(setup),with_session:Some(with)}).await?)?)
            }
            "switch"=> {
                let trust:ProjectTrustContextFactory={let log=log.clone();let slot=slot.clone();let fail=spec["fail"]=="trust";Arc::new(move |cwd| {
                    trace(&log,json!({"phase":"trust","cwd":cwd,"oldActive":active(&host(&slot).session())}));if fail {bail!("trust rejected");}
                    Ok(ProjectTrustContext{cwd:cwd.into(),mode:ExtensionMode::Print,has_ui:false,ui:None})
                })};
                Ok(serde_json::to_value(runtime.switch_session(target.to_str().unwrap(),SwitchSessionOptions{cwd_override:(spec["override"]==true).then(||a.clone()),with_session:Some(with),project_trust_context_factory:Some(trust)}).await?)?)
            }
            "fork"=>Ok(serde_json::to_value(runtime.fork(spec["entry"].as_str().unwrap(),ForkOptions{position:if spec["position"]=="at"{ForkPosition::At}else{ForkPosition::Before},with_session:Some(with)}).await?)?),
            "import"=> {
                let source=match spec["kind"].as_str().unwrap(){"stored"=>initial.clone(),"missing"=>base.join("missing.jsonl"),_=>target.clone()};
                if spec["kind"]=="collision" {fs::write(store.join("target.jsonl"),"existing")?;fs::write(store.join("target-1.jsonl"),"also-existing")?;}
                Ok(serde_json::to_value(runtime.import_from_jsonl(source.to_str().unwrap(),(spec["override"]==true).then_some(a.as_str())).await?)?)
            }
            "dispose"=> {runtime.dispose().await?;if spec["twice"]==true {runtime.dispose().await?;}Ok(Value::Null)}
            "initial"=> {let _=create_agent_session_runtime(factory,CreateAgentSessionRuntimeOptions{cwd:a.clone(),..options}).await?;Ok(Value::Null)}
            _=>panic!("unknown oracle operation"),
        }
    }.await;
    let (result, error) = match result {
        Ok(v) => (v, Value::Null),
        Err(error) => {
            let text = if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::AlreadyExists)
            {
                "exclusive copy refused".into()
            } else {
                error.to_string()
            };
            (Value::Null, Value::String(text))
        }
    };
    let s = runtime.session();
    let parent = s
        .session_manager
        .lock()
        .unwrap()
        .get_header()
        .and_then(|h| h.parent_session);
    let messages = s
        .agent
        .state()
        .messages
        .iter()
        .filter(|m| m.role() == "user")
        .map(|m| serde_json::to_value(m).unwrap()["content"].clone())
        .collect::<Vec<_>>();
    let mut files = serde_json::Map::new();
    for name in ["target.jsonl", "target-1.jsonl", "target-2.jsonl"] {
        let path = store.join(name);
        if path.exists() {
            let text = fs::read_to_string(path).unwrap();
            files.insert(
                name.into(),
                json!(if text.starts_with('{') {
                    "session"
                } else {
                    &text
                }),
            );
        }
    }
    let value = normalize(
        &json!({"trace":log.lock().unwrap().clone(),"result":result,"error":error,"slot":{"id":current_id(&sessions,&s),"cwd":runtime.cwd(),"active":active(&s),"diagnostics":runtime.diagnostics(),"fallback":runtime.model_fallback_message(),"parent":parent,"messages":messages},"files":files}),
        root.path(),
    );
    for s in sessions.lock().unwrap().iter() {
        s.dispose();
    }
    value
}

#[tokio::test]
async fn session_runtime_all_41_actual_source_lifecycle_cases() {
    let fixture = oracle();
    assert_eq!(fixture["rows"].as_array().unwrap().len(), 41);
    for row in fixture["rows"].as_array().unwrap() {
        eprintln!("actual-source lifecycle scenario: {}", row["spec"]["name"]);
        let actual = bounded(run_case(&row["spec"])).await;
        assert_eq!(actual, row["observed"], "scenario: {}", row["spec"]["name"]);
    }
}

#[test]
fn session_runtime_cwd_leaf_matches_actual_source() {
    struct Source {
        file: Option<String>,
        cwd: String,
    }
    impl SessionCwdSource for Source {
        fn get_cwd(&self) -> &str {
            &self.cwd
        }
        fn get_session_file(&self) -> Option<&str> {
            self.file.as_deref()
        }
    }
    let root = tempfile::tempdir().unwrap();
    let absent = root.path().join("absent").to_str().unwrap().to_owned();
    for row in oracle()["cwdRows"].as_array().unwrap() {
        let name = row["name"].as_str().unwrap();
        let file = match name {
            "memory" => None,
            "empty_file" => Some("".into()),
            _ => Some("session.jsonl".into()),
        };
        let cwd = match name {
            "empty_cwd" => "".into(),
            "present" => root.path().to_str().unwrap().into(),
            _ => absent.clone(),
        };
        let issue = get_missing_session_cwd_issue(&Source { file, cwd }, "fallback");
        let actual = json!({"issue":issue,"error":issue.as_ref().map(format_missing_session_cwd_error),"prompt":issue.as_ref().map(format_missing_session_cwd_prompt)});
        assert_eq!(normalize(&actual, root.path()), row["observed"], "{name}");
    }
    // existsSync accepts a regular file. Do not tighten it to is_dir().
    let file = root.path().join("not-a-directory");
    fs::write(&file, "").unwrap();
    assert!(get_missing_session_cwd_issue(
        &Source {
            file: Some("session.jsonl".into()),
            cwd: file.to_str().unwrap().into()
        },
        "fallback"
    )
    .is_none());
}

// Native suspension/identity regressions supplement the actual-source oracle.
// These are real sessions and managers; their factory is explicitly test-only.
struct PendingGate {
    entered: Mutex<Option<oneshot::Sender<()>>>,
    release: Mutex<Option<oneshot::Receiver<()>>>,
}
impl PendingGate {
    async fn wait(&self) {
        let release = self.release.lock().unwrap().take().expect("one gate use");
        self.entered
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .send(())
            .unwrap();
        release.await.unwrap();
    }
}
fn gate() -> (Arc<PendingGate>, oneshot::Receiver<()>, oneshot::Sender<()>) {
    let (entered_tx, entered_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    (
        Arc::new(PendingGate {
            entered: Mutex::new(Some(entered_tx)),
            release: Mutex::new(Some(release_rx)),
        }),
        entered_rx,
        release_tx,
    )
}
fn extension_handler(kind: &'static str, handler: HandlerFn) -> ExtensionFactory {
    Arc::new(move |api| api.on(kind, handler.clone()).map(|_| ()))
}
fn pending_handler(gate: Arc<PendingGate>) -> HandlerFn {
    Arc::new(move |_, _| {
        let gate = gate.clone();
        Box::pin(async move {
            gate.wait().await;
            Ok(None)
        })
    })
}
struct NativeHarness {
    _dir: tempfile::TempDir,
    options: CreateAgentSessionRuntimeOptions,
    models: ModelRuntime,
}
impl NativeHarness {
    async fn new(persisted: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().join("cwd");
        let agent = dir.path().join("agent");
        let store = dir.path().join("store");
        for p in [&cwd, &agent, &store] {
            fs::create_dir_all(p).unwrap();
        }
        let cwd = cwd.to_str().unwrap().to_owned();
        let manager = if persisted {
            let path = store.join("current.jsonl");
            write_session(&path, &cwd);
            SessionManager::open(path.to_str().unwrap(), Some(store.to_str().unwrap()), None)
                .unwrap()
        } else {
            memory_manager(&cwd)
        };
        Self {
            _dir: dir,
            options: CreateAgentSessionRuntimeOptions {
                cwd,
                agent_dir: agent.to_string_lossy().into_owned(),
                session_manager: Arc::new(Mutex::new(manager)),
                session_start_event: None,
                project_trust_context: None,
            },
            models: offline_models().await,
        }
    }
    fn first(&self, factories: Vec<ExtensionFactory>) -> CreateAgentSessionRuntimeResult {
        native_session(&self.options, &self.models, factories)
    }
    fn factory(&self, log: &Trace) -> CreateAgentSessionRuntimeFactory {
        let models = self.models.clone();
        let log = log.clone();
        Arc::new(move |options| {
            let models = models.clone();
            let log = log.clone();
            Box::pin(async move {
                trace(&log, json!("factory"));
                Ok(native_session(&options, &models, vec![]))
            })
        })
    }
}
fn runtime_from(
    first: CreateAgentSessionRuntimeResult,
    factory: CreateAgentSessionRuntimeFactory,
) -> Arc<AgentSessionRuntime> {
    Arc::new(AgentSessionRuntime::new(
        first.session,
        first.services,
        factory,
        first.diagnostics,
        first.model_fallback_message,
    ))
}
fn user_message(text: &str) -> AgentMessage {
    serde_json::from_value(json!({"role":"user", "content":text, "timestamp":1})).unwrap()
}
fn user_texts(session: &AgentSession) -> Vec<Value> {
    session
        .agent
        .state()
        .messages
        .iter()
        .filter(|m| m.role() == "user")
        .map(|m| serde_json::to_value(m).unwrap()["content"].clone())
        .collect()
}

#[tokio::test]
async fn shutdown_pending_keeps_context_alive_then_sync_invalidate_is_reentrant() {
    let h = NativeHarness::new(false).await;
    let log = Trace::default();
    let (gate, entered, release) = gate();
    let first = h.first(vec![extension_handler(
        "session_shutdown",
        pending_handler(gate),
    )]);
    let old = first.session.clone();
    let old_ctx = old.extension_runner().create_context();
    let runtime = runtime_from(first, h.factory(&log));
    let weak = Arc::downgrade(&runtime);
    let callback_log = log.clone();
    runtime.set_before_session_invalidate(Some(Arc::new(move || {
        let host = weak.upgrade().unwrap();
        assert!(active(&host.session()));
        assert_eq!(host.cwd(), host.services().cwd);
        assert!(host.diagnostics().is_empty());
        assert!(host.model_fallback_message().is_none());
        host.set_before_session_invalidate(None); // no runtime lock across this sync callback
        let rebind_log = callback_log.clone();
        host.set_rebind_session(Some(Arc::new(move |session| {
            let log = rebind_log.clone();
            Box::pin(async move {
                assert!(active(&session));
                session
                    .bind_extensions(ExtensionBindings {
                        mode: Some(ExtensionMode::Print),
                        ..Default::default()
                    })
                    .await?;
                trace(&log, json!("rebind"));
                Ok(())
            })
        })));
        trace(&callback_log, json!("invalidate"));
        Ok(())
    })));
    let r = runtime.clone();
    let task = tokio::spawn(async move { r.new_session(Default::default()).await });
    bounded(entered).await.unwrap();
    assert!(!task.is_finished());
    assert!(Arc::ptr_eq(&runtime.session(), &old));
    assert!(old_ctx.cwd().is_ok());
    assert!(log.lock().unwrap().is_empty());
    release.send(()).unwrap();
    assert!(!bounded(task).await.unwrap().unwrap().cancelled);
    assert!(old_ctx.cwd().is_err());
    assert!(!Arc::ptr_eq(&runtime.session(), &old));
    assert_eq!(
        *log.lock().unwrap(),
        vec![json!("invalidate"), json!("factory"), json!("rebind")]
    );
    bounded(runtime.dispose()).await.unwrap();
}

#[tokio::test]
async fn factory_pending_rejection_does_not_restore_disposed_outgoing_slot() {
    let h = NativeHarness::new(false).await;
    let first = h.first(vec![]);
    let old = first.session.clone();
    let services = first.services.clone();
    let (gate, entered, release) = gate();
    let runtime = runtime_from(
        first,
        Arc::new(move |_| {
            let gate = gate.clone();
            Box::pin(async move {
                gate.wait().await;
                bail!("controlled factory rejection")
            })
        }),
    );
    let r = runtime.clone();
    let task = tokio::spawn(async move { r.new_session(Default::default()).await });
    bounded(entered).await.unwrap();
    assert!(!task.is_finished());
    assert!(Arc::ptr_eq(&runtime.session(), &old));
    assert!(Arc::ptr_eq(&runtime.services(), &services));
    assert!(!active(&old));
    release.send(()).unwrap();
    assert_eq!(
        bounded(task).await.unwrap().unwrap_err().to_string(),
        "controlled factory rejection"
    );
    assert!(Arc::ptr_eq(&runtime.session(), &old));
    assert!(!active(&old));
}

#[tokio::test]
async fn setup_pending_has_applied_new_slot_but_defers_transcript_and_rebind() {
    let h = NativeHarness::new(false).await;
    let log = Trace::default();
    let first = h.first(vec![]);
    let old = first.session.clone();
    let runtime = runtime_from(first, h.factory(&log));
    let rebind_log = log.clone();
    runtime.set_rebind_session(Some(Arc::new(move |session| {
        assert_eq!(user_texts(&session), vec![json!("setup pending")]);
        trace(&rebind_log, json!("rebind"));
        Box::pin(async { Ok(()) })
    })));
    let (gate, entered, release) = gate();
    let setup: SetupSession = Arc::new(move |manager| {
        let gate = gate.clone();
        Box::pin(async move {
            manager
                .lock()
                .unwrap()
                .append_message(user_message("setup pending"))?;
            gate.wait().await;
            Ok(())
        })
    });
    let r = runtime.clone();
    let task = tokio::spawn(async move {
        r.new_session(NewSessionOptionsRuntime {
            setup: Some(setup),
            ..Default::default()
        })
        .await
    });
    bounded(entered).await.unwrap();
    assert!(!active(&old));
    assert!(!Arc::ptr_eq(&runtime.session(), &old));
    assert!(active(&runtime.session()));
    assert!(user_texts(&runtime.session()).is_empty());
    assert_eq!(
        runtime
            .session()
            .session_manager
            .lock()
            .unwrap()
            .build_session_context()
            .messages
            .len(),
        1
    );
    assert_eq!(*log.lock().unwrap(), vec![json!("factory")]);
    release.send(()).unwrap();
    bounded(task).await.unwrap().unwrap();
    assert_eq!(user_texts(&runtime.session()), vec![json!("setup pending")]);
    assert_eq!(
        *log.lock().unwrap(),
        vec![json!("factory"), json!("rebind")]
    );
    bounded(runtime.dispose()).await.unwrap();
}

#[tokio::test]
async fn rebind_pending_allows_replacement_and_with_session_reads_latest_identity() {
    let h = NativeHarness::new(false).await;
    let log = Trace::default();
    let runtime = runtime_from(h.first(vec![]), h.factory(&log));
    let (gate, entered, release) = gate();
    let count = Arc::new(AtomicUsize::new(0));
    runtime.set_rebind_session(Some(Arc::new(move |_| {
        let first = count.fetch_add(1, Ordering::SeqCst) == 0;
        let gate = gate.clone();
        Box::pin(async move {
            if first {
                gate.wait().await;
            }
            Ok(())
        })
    })));
    let observed = Arc::new(Mutex::new(None));
    let output = observed.clone();
    let with: WithSession = Arc::new(move |ctx| {
        let manager = ctx
            .session_manager()
            .unwrap()
            .downcast::<Mutex<SessionManager>>()
            .unwrap();
        *output.lock().unwrap() = Some(manager);
        Box::pin(async { Ok(()) })
    });
    let r = runtime.clone();
    let task = tokio::spawn(async move {
        r.new_session(NewSessionOptionsRuntime {
            with_session: Some(with),
            ..Default::default()
        })
        .await
    });
    bounded(entered).await.unwrap();
    let interim = runtime.session();
    assert!(active(&interim));
    bounded(runtime.new_session(Default::default()))
        .await
        .unwrap();
    let latest = runtime.session();
    assert!(!Arc::ptr_eq(&latest, &interim));
    assert!(!active(&interim));
    release.send(()).unwrap();
    bounded(task).await.unwrap().unwrap();
    assert!(Arc::ptr_eq(
        observed.lock().unwrap().as_ref().unwrap(),
        &latest.session_manager
    ));
    bounded(runtime.dispose()).await.unwrap();
}

#[tokio::test]
async fn before_switch_pending_reads_live_slot_after_another_replacement() {
    let h = NativeHarness::new(false).await;
    let log = Trace::default();
    let (gate, entered, release) = gate();
    let hits = Arc::new(AtomicUsize::new(0));
    let handler: HandlerFn = Arc::new(move |_, _| {
        let first = hits.fetch_add(1, Ordering::SeqCst) == 0;
        let gate = gate.clone();
        Box::pin(async move {
            if first {
                gate.wait().await;
            }
            Ok(None)
        })
    });
    let first = h.first(vec![extension_handler("session_before_switch", handler)]);
    let original = first.session.clone();
    let runtime = runtime_from(first, h.factory(&log));
    let r = runtime.clone();
    let task = tokio::spawn(async move { r.new_session(Default::default()).await });
    bounded(entered).await.unwrap();
    assert!(active(&original));
    assert!(log.lock().unwrap().is_empty());
    bounded(runtime.new_session(Default::default()))
        .await
        .unwrap();
    let interim = runtime.session();
    assert!(!active(&original));
    assert!(active(&interim));
    release.send(()).unwrap();
    bounded(task).await.unwrap().unwrap();
    assert!(!active(&interim));
    assert!(!Arc::ptr_eq(&runtime.session(), &interim));
    assert_eq!(log.lock().unwrap().len(), 2);
    bounded(runtime.dispose()).await.unwrap();
}

#[tokio::test]
async fn fork_memory_reuses_manager_only_after_shutdown_but_disk_uses_new_manager() {
    for persisted in [false, true] {
        let h = NativeHarness::new(persisted).await;
        let log = Trace::default();
        let (gate, entered, release) = gate();
        let first = h.first(vec![extension_handler(
            "session_shutdown",
            pending_handler(gate),
        )]);
        let old = first.session.clone();
        let manager = old.session_manager.clone();
        let previous = old.session_file();
        let before = manager.lock().unwrap().get_entries();
        let runtime = runtime_from(first, h.factory(&log));
        let r = runtime.clone();
        let task = tokio::spawn(async move { r.fork("second", Default::default()).await });
        bounded(entered).await.unwrap();
        assert!(active(&old));
        assert_eq!(manager.lock().unwrap().get_entries(), before);
        release.send(()).unwrap();
        assert_eq!(
            bounded(task).await.unwrap().unwrap().selected_text,
            Some("AB".into())
        );
        let new = runtime.session();
        assert!(!active(&old));
        assert_eq!(Arc::ptr_eq(&manager, &new.session_manager), !persisted);
        assert_eq!(user_texts(&new), vec![json!("first")]);
        if persisted {
            assert_eq!(manager.lock().unwrap().get_entries(), before);
            assert_eq!(
                new.session_manager
                    .lock()
                    .unwrap()
                    .get_header()
                    .unwrap()
                    .parent_session,
                previous
            );
            assert_ne!(new.session_file(), old.session_file());
        }
        bounded(runtime.dispose()).await.unwrap();
    }
}

#[tokio::test]
async fn import_cancel_creates_directory_and_post_copy_cwd_failure_retains_copy() {
    let h = NativeHarness::new(true).await;
    let log = Trace::default();
    let source = h._dir.path().join("external.jsonl");
    let missing_cwd = h._dir.path().join("missing-cwd");
    write_session(&source, missing_cwd.to_str().unwrap());
    let store = PathBuf::from(h.options.session_manager.lock().unwrap().get_session_dir());
    let current = PathBuf::from(
        h.options
            .session_manager
            .lock()
            .unwrap()
            .get_session_file()
            .unwrap(),
    );
    // Both paths are created in this test's TempDir. Remove only the known file
    // and the now-empty directory, never recursively or against computed globals.
    assert!(current.starts_with(h._dir.path()) && store.starts_with(h._dir.path()));
    fs::remove_file(current).unwrap();
    fs::remove_dir(&store).unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let handler = sync_handler(move |_, _| {
        Ok(Some(HandlerResult::Json(
            json!({"cancel":hits.fetch_add(1, Ordering::SeqCst) == 0}),
        )))
    });
    let first = h.first(vec![extension_handler("session_before_switch", handler)]);
    let old = first.session.clone();
    let runtime = runtime_from(first, h.factory(&log));
    assert!(
        bounded(runtime.import_from_jsonl(source.to_str().unwrap(), None))
            .await
            .unwrap()
            .cancelled
    );
    assert!(store.is_dir());
    assert!(!store.join("external.jsonl").exists());
    assert!(active(&old));
    let err = bounded(runtime.import_from_jsonl(source.to_str().unwrap(), None))
        .await
        .unwrap_err();
    assert!(err.downcast_ref::<MissingSessionCwdError>().is_some());
    assert_eq!(
        fs::read(store.join("external.jsonl")).unwrap(),
        fs::read(&source).unwrap()
    );
    assert!(active(&old));
    assert!(Arc::ptr_eq(&runtime.session(), &old));
    assert!(log.lock().unwrap().is_empty());
    bounded(runtime.dispose()).await.unwrap();
}

#[tokio::test]
async fn session_services_and_loader_share_live_settings_in_both_directions() {
    let h = NativeHarness::new(false).await;
    let first = h.first(vec![]);
    first
        .services
        .settings_manager
        .set_default_model("from services");
    assert_eq!(
        first
            .session
            .settings_manager
            .get_default_model()
            .as_deref(),
        Some("from services")
    );
    first
        .session
        .settings_manager
        .set_default_provider("from session");
    assert_eq!(
        first
            .services
            .settings_manager
            .get_default_provider()
            .as_deref(),
        Some("from session")
    );
    assert!(first.services.settings_manager.is_project_trusted());
    first
        .services
        .resource_loader
        .lock()
        .unwrap()
        .load_project_trust_extensions()
        .unwrap();
    assert!(!first.services.settings_manager.is_project_trusted());
    assert!(!first.session.settings_manager.is_project_trusted());
    first.session.settings_manager.set_project_trusted(true);
    first
        .services
        .resource_loader
        .lock()
        .unwrap()
        .reload_without_trust()
        .unwrap();
    assert!(first.services.settings_manager.is_project_trusted());
    assert_eq!(
        first
            .session
            .settings_manager
            .get_default_model()
            .as_deref(),
        Some("from services")
    );
    first.session.dispose();
}

#[test]
fn settings_clone_shares_failed_write_fields_storage_and_error_queue() {
    use crate::coding_agent::core::settings_manager::{
        InMemorySettingsStorage, SettingsScope, SettingsStorage,
    };
    let storage = Arc::new(InMemorySettingsStorage::default());
    let settings = SettingsManager::from_storage(storage.clone(), Default::default());
    let other = settings.clone();
    storage
        .with_lock(SettingsScope::Global, Box::new(|_| Ok(Some("{".into()))))
        .unwrap();
    settings.set_default_model("pending after bad JSON");
    assert_eq!(
        other.get_default_model().as_deref(),
        Some("pending after bad JSON")
    );
    assert_eq!(other.drain_errors().len(), 1);
    assert!(settings.drain_errors().is_empty());
    storage
        .with_lock(SettingsScope::Global, Box::new(|_| Ok(Some("{}".into()))))
        .unwrap();
    other.set_default_provider("shared store"); // flushes the same pending field set
    let reloaded = SettingsManager::from_storage(storage.clone(), Default::default());
    assert_eq!(reloaded.get_default_model(), settings.get_default_model());
    assert_eq!(
        reloaded.get_default_provider().as_deref(),
        Some("shared store")
    );
    storage
        .with_lock(SettingsScope::Global, Box::new(|_| Ok(Some("{".into()))))
        .unwrap();
    other.reload();
    assert_eq!(settings.drain_errors().len(), 1);
    assert!(other.drain_errors().is_empty());
}

// Start a real Agent/AgentSession run against the in-process faux provider.
// No real credentials, paid provider, stream placeholder or real network is used.
// Session prompting uses an explicitly fake, in-memory-only runtime key.
async fn live_agent(provider_gate: Arc<PendingGate>, runtime: &ModelRuntime) -> Arc<Agent> {
    use crate::agent_core::types::AgentInitialState;
    use crate::ai::models::faux::{FauxModelDefinition, FauxProviderOptions, FauxResponseStep};
    use crate::ai::models::{faux_assistant_message, faux_provider, FauxMessageOptions};
    use crate::ai::types::primitives::StopReason;
    let faux = faux_provider(FauxProviderOptions {
        provider: Some("runtime-r13-faux".into()),
        models: vec![FauxModelDefinition {
            id: "offline".into(),
            ..Default::default()
        }],
        ..Default::default()
    });
    faux.set_responses(vec![FauxResponseStep::Factory(Arc::new(move |_| {
        let gate = provider_gate.clone();
        Box::pin(async move {
            gate.wait().await;
            Ok(faux_assistant_message(
                "",
                FauxMessageOptions {
                    stop_reason: Some(StopReason::Aborted),
                    ..Default::default()
                },
            ))
        })
    }))]);
    let model = faux.get_model(None).unwrap();
    // Agent transport and AgentSession auth must know the same faux provider.
    // An in-memory key alone does not register a provider in ModelRuntime.
    runtime
        .register_native_provider(faux.provider.clone())
        .await
        .unwrap();
    assert!(runtime
        .check_auth(&model.provider, None)
        .await
        .unwrap()
        .is_some());
    let mut models = create_models(Default::default());
    models.set_provider(faux.provider.clone());
    Arc::new(Agent::new(
        AgentOptions {
            initial_state: AgentInitialState {
                model: Some(model),
                ..Default::default()
            },
            ..Default::default()
        },
        Arc::new(models),
    ))
}

#[tokio::test]
async fn replacement_waits_for_abort_final_message_persistence_before_shutdown() {
    let h = NativeHarness::new(true).await;
    let log = Trace::default();
    let (provider_gate, provider_entered, provider_release) = gate();
    let agent = live_agent(provider_gate, &h.models).await;
    let signal_slot = Arc::new(Mutex::new(None));
    let seen = signal_slot.clone();
    let (persist_gate, persist_entered, persist_release) = gate();
    // Registered BEFORE AgentSession's subscriber: hold message_end before its
    // persistence callback, proving abort cannot return at signal cancellation.
    let _subscription = agent.subscribe(move |event, signal| {
        use crate::agent_core::types::AgentEvent;
        let should_wait = matches!(event, AgentEvent::MessageEnd { ref message } if message.role() == "assistant");
        let gate = persist_gate.clone(); *seen.lock().unwrap() = Some(signal);
        Box::pin(async move { if should_wait { gate.wait().await; } })
    });
    let saved = h.options.session_manager.clone();
    let shutdown_log = log.clone();
    let first = native_session_with_agent(
        &h.options,
        &h.models,
        vec![extension_handler(
            "session_shutdown",
            sync_handler(move |_, _| {
                let manager = saved.lock().unwrap();
                assert!(manager
                    .build_session_context()
                    .messages
                    .iter()
                    .any(|m| m.role() == "assistant"));
                let file = manager.get_session_file().unwrap();
                assert!(fs::read_to_string(file)
                    .unwrap()
                    .contains("\"stopReason\":\"aborted\""));
                trace(&shutdown_log, json!("shutdown-after-persist"));
                Ok(None)
            }),
        )],
        agent.clone(),
    );
    let old = first.session.clone();
    let runtime = runtime_from(first, h.factory(&log));
    h.models
        .set_runtime_api_key("runtime-r13-faux", "offline-test-key-not-a-secret", None)
        .await
        .unwrap();
    let running_session = old.clone();
    let mut run =
        tokio::spawn(async move { running_session.prompt("pending real run", None).await });
    bounded_stage("provider entered", async {
        tokio::select! {
            entered = provider_entered => entered.expect("provider gate dropped"),
            result = &mut run => panic!("session prompt completed before provider gate: {result:?}"),
        }
    }).await;
    let signal = signal_slot.lock().unwrap().clone().unwrap();
    let r = runtime.clone();
    let replacement = tokio::spawn(async move { r.new_session(Default::default()).await });
    bounded_stage("abort signal", signal.cancelled()).await;
    assert!(!replacement.is_finished());
    assert!(active(&old));
    assert!(log.lock().unwrap().is_empty());
    provider_release.send(()).unwrap();
    bounded_stage("final message persistence entered", persist_entered)
        .await
        .unwrap();
    assert!(!replacement.is_finished());
    assert!(active(&old));
    assert!(!old
        .session_manager
        .lock()
        .unwrap()
        .build_session_context()
        .messages
        .iter()
        .any(|m| m.role() == "assistant"));
    persist_release.send(()).unwrap();
    bounded_stage("session run completed", run)
        .await
        .unwrap()
        .unwrap();
    bounded_stage("replacement completed", replacement)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        *log.lock().unwrap(),
        vec![json!("shutdown-after-persist"), json!("factory")]
    );
    assert!(!active(&old));
    bounded_stage("new runtime disposed", runtime.dispose())
        .await
        .unwrap();
}

#[tokio::test]
async fn runtime_dispose_awaits_shutdown_without_pre_abort_or_wait_for_run() {
    let h = NativeHarness::new(false).await;
    let log = Trace::default();
    let (provider_gate, provider_entered, provider_release) = gate();
    let agent = live_agent(provider_gate, &h.models).await;
    let (shutdown_gate, shutdown_entered, shutdown_release) = gate();
    let first = native_session_with_agent(
        &h.options,
        &h.models,
        vec![extension_handler(
            "session_shutdown",
            pending_handler(shutdown_gate),
        )],
        agent.clone(),
    );
    let old = first.session.clone();
    let runtime = runtime_from(first, h.factory(&log));
    h.models
        .set_runtime_api_key("runtime-r13-faux", "offline-test-key-not-a-secret", None)
        .await
        .unwrap();
    let running_session = old.clone();
    let mut run =
        tokio::spawn(async move { running_session.prompt("still active at quit", None).await });
    bounded_stage("provider entered", async {
        tokio::select! {
            entered = provider_entered => entered.expect("provider gate dropped"),
            result = &mut run => panic!("session prompt completed before provider gate: {result:?}"),
        }
    }).await;
    let signal = agent.signal().unwrap();
    let r = runtime.clone();
    let dispose = tokio::spawn(async move { r.dispose().await });
    bounded_stage("shutdown entered", shutdown_entered)
        .await
        .unwrap();
    assert!(!signal.is_cancelled());
    assert!(!run.is_finished());
    assert!(active(&old));
    shutdown_release.send(()).unwrap();
    bounded_stage("runtime disposed while run pending", dispose)
        .await
        .unwrap()
        .unwrap();
    assert!(signal.is_cancelled());
    assert!(!active(&old));
    assert!(!run.is_finished());
    provider_release.send(()).unwrap();
    bounded_stage("session run completed", run)
        .await
        .unwrap()
        .unwrap();
    assert!(log.lock().unwrap().is_empty());
}
