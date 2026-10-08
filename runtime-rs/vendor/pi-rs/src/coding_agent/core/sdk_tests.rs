use super::*;
use crate::ai::api::ApiImpl;
use crate::ai::auth::credential_store::InMemoryCredentialStore;
use crate::ai::auth::types::{
    ApiKeyAuth, ApiKeyAuthInput, AuthError, AuthResult, ModelAuth, ProviderAuth,
};
use crate::ai::models::provider::{create_provider, ApiImpls, CreateProviderOptions};
use crate::ai::models::{faux_assistant_message, FauxMessageOptions, ModelsRefreshOptions};
use crate::ai::transcript::{normalize_context, TranscriptContext};
use crate::ai::types::{AssistantMessageEvent, SimpleStreamOptions, StreamOptions, SuccessReason};
use crate::ai::ProviderConfig;
use crate::coding_agent::core::models_store::InMemoryCodingAgentModelsStore;
use crate::coding_agent::core::resource_loader::InlineExtension;
use crate::coding_agent::core::settings_manager::{parse_settings_value, SettingsValue};
use crate::coding_agent::extensions::loader::ExtensionFactory;
use crate::coding_agent::extensions::types::{sync_handler, HandlerResult};
use crate::coding_agent::session_manager::NewSessionOptions;
use futures::future::BoxFuture;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::{mpsc, Notify, Semaphore};

type Captures = Arc<Mutex<Vec<(Model, TranscriptContext, SimpleStreamOptions)>>>;
fn oracle() -> Value {
    serde_json::from_str(include_str!("sdk_oracle.json")).unwrap()
}
fn object_settings(value: Value) -> SettingsValue {
    parse_settings_value(&value.to_string()).unwrap()
}
fn memory_settings(value: Value) -> SettingsManager {
    SettingsManager::in_memory(object_settings(value))
}
fn model() -> Model {
    serde_json::from_value(json!({"id":"m1","name":"m1","provider":"r15-provider","api":"openai-completions","baseUrl":"https://r15.invalid/v1","reasoning":true,"input":["text","image"],"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0},"contextWindow":100000,"maxTokens":2000})).unwrap()
}
fn user(text: &str) -> AgentMessage {
    serde_json::from_value(json!({"role":"user","content":text,"timestamp":1})).unwrap()
}
async fn bounded<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(12), future)
        .await
        .expect("SDK operation deadlocked")
}
struct OfflineAuth(bool);
impl ApiKeyAuth for OfflineAuth {
    fn name(&self) -> &str {
        "r15 offline auth"
    }
    fn resolve<'a>(
        &'a self,
        _: ApiKeyAuthInput<'a>,
    ) -> BoxFuture<'a, Result<Option<AuthResult>, AuthError>> {
        Box::pin(async move {
            Ok(self.0.then(|| AuthResult {
                auth: ModelAuth {
                    api_key: Some("r15-test-not-real".into()),
                    ..Default::default()
                },
                ..Default::default()
            }))
        })
    }
}
struct CaptureApi(Captures);
impl ApiImpl for CaptureApi {
    fn supports_request_callbacks(&self) -> bool {
        true
    }
    fn stream(
        &self,
        _: &ProviderConfig,
        _: &Model,
        _: &TranscriptContext,
        _: &StreamOptions,
    ) -> mpsc::Receiver<AssistantMessageEvent> {
        panic!("SDK must use stream_simple")
    }
    fn stream_simple(
        &self,
        _: &ProviderConfig,
        model: &Model,
        context: &TranscriptContext,
        options: &SimpleStreamOptions,
    ) -> mpsc::Receiver<AssistantMessageEvent> {
        self.0
            .lock()
            .unwrap()
            .push((model.clone(), context.clone(), options.clone()));
        let mut message = faux_assistant_message(
            "ok",
            FauxMessageOptions {
                timestamp: Some(2),
                ..Default::default()
            },
        );
        message.api = model.api.clone();
        message.provider = model.provider.clone();
        message.model = model.id.clone();
        let (tx, rx) = mpsc::channel(1);
        tx.try_send(AssistantMessageEvent::Done {
            reason: SuccessReason::Stop,
            message,
        })
        .unwrap();
        rx
    }
}
async fn runtime(models: Vec<Model>, configured: &[String], api: Arc<dyn ApiImpl>) -> ModelRuntime {
    let runtime = ModelRuntime::create(CreateModelRuntimeOptions {
        credentials: Some(Arc::new(InMemoryCredentialStore::default())),
        models_path: Some(None),
        models_store: Some(Arc::new(InMemoryCodingAgentModelsStore::default())),
        allow_model_network: false,
        refresh_on_create: Some(false),
        ..Default::default()
    })
    .await
    .unwrap();
    let mut names = vec![];
    for m in &models {
        if !names.contains(&m.provider) {
            names.push(m.provider.clone());
        }
    }
    for provider in names {
        runtime
            .register_native_provider(create_provider(CreateProviderOptions {
                filter_all_models: None,
                images: crate::ai::models::provider::ImagesImpls::new(),
                classifiers: crate::ai::models::provider::ClassifiersImpls::new(),
                id: provider.clone(),
                name: None,
                base_url: None,
                headers: None,
                auth: ProviderAuth {
                    api_key: Some(Arc::new(OfflineAuth(configured.contains(&provider)))),
                    oauth: None,
                },
                models: models
                    .iter()
                    .filter(|m| m.provider == provider)
                    .cloned()
                    .map(crate::ai::types::AnyModel::Chat)
                    .collect(),
                fetch_models: None,
                filter_models: None,
                api: ApiImpls::Single(api.clone()),
            }))
            .await
            .unwrap();
    }
    if !models.is_empty() {
        runtime
            .refresh(ModelsRefreshOptions {
                allow_network: Some(false),
                ..Default::default()
            })
            .await
            .unwrap();
    }
    runtime
}
fn loader(
    cwd: &str,
    dir: &str,
    settings: &SettingsManager,
    factories: Vec<ExtensionFactory>,
) -> Arc<Mutex<DefaultResourceLoader>> {
    let mut loader = DefaultResourceLoader::new(DefaultResourceLoaderOptions {
        cwd: cwd.into(),
        agent_dir: dir.into(),
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
    Arc::new(Mutex::new(loader))
}
struct Fixture {
    _dir: tempfile::TempDir,
    cwd: String,
    agent_dir: String,
    runtime: ModelRuntime,
    settings: SettingsManager,
    loader: Arc<Mutex<DefaultResourceLoader>>,
    captures: Captures,
}
impl Fixture {
    async fn new(value: Value, factories: Vec<ExtensionFactory>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().join("project");
        let agent_dir = dir.path().join("agent");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&agent_dir).unwrap();
        let cwd = cwd.to_str().unwrap().to_owned();
        let agent_dir = agent_dir.to_str().unwrap().to_owned();
        let settings = memory_settings(value);
        let captures = Captures::default();
        let runtime = runtime(
            vec![model()],
            &[model().provider],
            Arc::new(CaptureApi(captures.clone())),
        )
        .await;
        let loader = loader(&cwd, &agent_dir, &settings, factories);
        Self {
            _dir: dir,
            cwd,
            agent_dir,
            runtime,
            settings,
            loader,
            captures,
        }
    }
    fn manager(&self) -> Arc<Mutex<SessionManager>> {
        Arc::new(Mutex::new(
            SessionManager::in_memory(
                &self.cwd,
                Some(&NewSessionOptions {
                    id: Some("r15-session".into()),
                    ..Default::default()
                }),
                None,
            )
            .unwrap(),
        ))
    }
    fn options(&self) -> CreateAgentSessionOptions {
        CreateAgentSessionOptions {
            cwd: Some(self.cwd.clone()),
            agent_dir: Some(self.agent_dir.clone()),
            model_runtime: Some(self.runtime.clone()),
            settings_manager: Some(self.settings.clone()),
            session_manager: Some(self.manager()),
            resource_loader: Some(self.loader.clone()),
            ..Default::default()
        }
    }
    async fn session(&self) -> Arc<AgentSession> {
        bounded(create_agent_session(self.options()))
            .await
            .unwrap()
            .session
    }
}
async fn stream(session: &AgentSession, options: SimpleStreamOptions) {
    let factory = session.agent.runtime().stream_fn.clone().unwrap();
    let mut rx = bounded(factory(
        session.model().unwrap(),
        normalize_context(&Context::default()),
        options,
    ))
    .await
    .unwrap();
    let mut done = false;
    while let Some(event) = bounded(rx.recv()).await {
        match event {
            AssistantMessageEvent::Done { .. } => done = true,
            AssistantMessageEvent::Error { error, .. } => {
                panic!("{}", error.error_message.unwrap_or_default())
            }
            _ => (),
        }
    }
    assert!(done);
}
fn writes(manager: &SessionManager, start: usize) -> Vec<Value> {
    manager
        .get_entries()
        .into_iter()
        .skip(start)
        .filter_map(|entry| match entry {
            SessionEntry::ThinkingLevelChange(e) => {
                Some(json!({"type":"thinking_level_change","thinkingLevel":e.thinking_level}))
            }
            SessionEntry::ModelChange(e) => {
                Some(json!({"type":"model_change","provider":e.provider,"modelId":e.model_id}))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn attribution_and_telemetry_match_actual_upstream_sources() {
    for case in oracle()["attribution"].as_array().unwrap() {
        let i = &case["input"];
        let mut m = model();
        m.provider = i["provider"].as_str().unwrap().into();
        m.base_url = i["baseUrl"].as_str().unwrap().into();
        let sources: Vec<Option<ProviderHeaders>> = i
            .get("sources")
            .map(|v| serde_json::from_value(v.clone()).unwrap())
            .unwrap_or_default();
        let actual = super::super::provider_attribution::merge_headers(
            &m,
            i["enabled"].as_bool().unwrap(),
            i["sessionId"].as_str(),
            &sources.iter().map(Option::as_ref).collect::<Vec<_>>(),
        );
        assert_eq!(json!(actual), case["expected"], "{i}");
    }
    for case in oracle()["telemetry"].as_array().unwrap() {
        let i = &case["input"];
        let settings = memory_settings(json!({"enableInstallTelemetry":i["enabled"]}));
        assert_eq!(
            json!(super::super::provider_attribution::telemetry_enabled(
                &settings,
                i["env"].as_str()
            )),
            case["expected"],
            "{i}"
        );
    }
}

#[tokio::test]
async fn factory_model_thinking_and_initial_entries_match_sdk_oracle() {
    for case in oracle()["factories"].as_array().unwrap() {
        let i = &case["input"];
        let e = &case["expected"];
        let f = Fixture::new(i.get("settings").cloned().unwrap_or(json!({})), vec![]).await;
        let models = i
            .get("models")
            .map(|v| serde_json::from_value(v.clone()).unwrap())
            .unwrap_or_else(|| vec![model()]);
        let configured = i
            .get("configured")
            .map(|v| serde_json::from_value(v.clone()).unwrap())
            .unwrap_or_else(|| vec!["r15-provider".into()]);
        let runtime = runtime(
            models,
            &configured,
            Arc::new(CaptureApi(f.captures.clone())),
        )
        .await;
        let mut options = f.options();
        options.model_runtime = Some(runtime);
        let manager = options.session_manager.clone().unwrap();
        {
            let mut manager = manager.lock().unwrap();
            if let Some(saved) = i.get("savedModel") {
                manager
                    .append_model_change(
                        saved["provider"].as_str().unwrap(),
                        saved["modelId"].as_str().unwrap(),
                    )
                    .unwrap();
            }
            if i["hasThinking"] == true {
                manager
                    .append_thinking_level_change(i["savedThinking"].as_str().unwrap_or("off"))
                    .unwrap();
            }
            if i["existing"] == true {
                manager.append_message(user("saved")).unwrap();
            }
        }
        let start = manager.lock().unwrap().get_entries().len();
        let o = &i["options"];
        options.model = o
            .get("model")
            .map(|v| serde_json::from_value(v.clone()).unwrap());
        options.thinking_level = o["thinkingLevel"].as_str().and_then(thinking);
        options.tools = o
            .get("tools")
            .map(|v| serde_json::from_value(v.clone()).unwrap());
        options.exclude_tools = o
            .get("excludeTools")
            .map(|v| serde_json::from_value(v.clone()).unwrap());
        options.no_tools = match o["noTools"].as_str() {
            Some("all") => Some(NoTools::All),
            Some("builtin") => Some(NoTools::Builtin),
            _ => None,
        };
        if let Some(scoped) = o["scopedModels"].as_array() {
            options.scoped_models = scoped
                .iter()
                .map(|s| ScopedModel {
                    model: serde_json::from_value(s["model"].clone()).unwrap(),
                    thinking_level: s["thinkingLevel"].as_str().and_then(thinking),
                })
                .collect();
        }
        let result = bounded(create_agent_session(options)).await.unwrap();
        let s = &result.session;
        // Oracle reads Agent.state.model, not the native Session getter that
        // intentionally maps the inherited unknown-model sentinel to None.
        let actual_model = {
            let state = s.agent.state();
            format!("{}/{}", state.model.provider, state.model.id)
        };
        assert_eq!(json!(actual_model), e["model"], "{} model", i["name"]);
        assert_eq!(
            json!(s.thinking_level()),
            e["thinking"],
            "{} thinking",
            i["name"]
        );
        assert_eq!(
            json!(writes(&manager.lock().unwrap(), start)),
            e["writes"],
            "{} writes",
            i["name"]
        );
        let fallback = result.model_fallback_message.map(|v| {
            if v.starts_with("No models available.") {
                "NO_MODELS".into()
            } else {
                v
            }
        });
        assert_eq!(json!(fallback), e["fallback"], "{} fallback", i["name"]);
        let mut active = vec![];
        for name in e["initial"].as_array().unwrap() {
            let name = name.as_str().unwrap().to_owned();
            if !active.contains(&name) {
                active.push(name);
            }
        }
        assert_eq!(
            s.get_active_tool_names(),
            active,
            "{} active tools",
            i["name"]
        );
        assert_eq!(json!(s.agent.steering_mode()), e["steering"]);
        assert_eq!(json!(s.agent.follow_up_mode()), e["followUp"]);
        {
            let run = s.agent.runtime();
            assert_eq!(json!(run.transport), e["transport"]);
            assert_eq!(json!(run.session_id), e["sessionId"]);
            assert_eq!(json!(run.thinking_budgets), e["thinkingBudgets"]);
            assert_eq!(json!(run.max_retry_delay_ms), e["maxRetryDelayMs"]);
        }
        if i["existing"] == true {
            assert!(s.agent.state().messages.iter().any(|m| *m == user("saved")));
        }
        s.dispose();
    }
}

#[tokio::test]
async fn request_options_and_live_settings_match_sdk_oracle() {
    for case in oracle()["streams"].as_array().unwrap() {
        let i = &case["input"];
        let f = Fixture::new(i.get("settings").cloned().unwrap_or(json!({})), vec![]).await;
        let session = f.session().await;
        if let Some(change) = i.get("change") {
            f.settings.apply_overrides(&object_settings(change.clone()));
        }
        let opts: SimpleStreamOptions =
            serde_json::from_value(i.get("request").cloned().unwrap_or(json!({}))).unwrap();
        stream(&session, opts).await;
        let actual = f.captures.lock().unwrap()[0].2.clone();
        let mut got = json!(actual);
        // ModelRuntime (not the SDK's mocked collaborator) resolves auth. The
        // fake key and normalized optional empty headers are a lower boundary.
        got.as_object_mut().unwrap().remove("apiKey");
        got.as_object_mut().unwrap().remove("headers");
        let mut expected = case["expected"]["options"].clone();
        expected.as_object_mut().unwrap().remove("apiKey");
        expected.as_object_mut().unwrap().remove("headers");
        assert_eq!(got, expected, "{} options", i["name"]);
        assert_eq!(
            json!(actual.stream.headers.unwrap_or_default()),
            case["expected"]["headers"],
            "{} headers",
            i["name"]
        );
        session.dispose();
    }
}

#[tokio::test]
async fn block_images_custom_messages_and_dynamic_policy_match_oracle() {
    let f = Fixture::new(json!({}), vec![]).await;
    let s = f.session().await;
    let convert = s.agent.runtime().convert_to_llm.clone();
    for case in oracle()["images"].as_array().unwrap() {
        let i = &case["input"];
        f.settings.set_block_images(i["block"].as_bool().unwrap());
        let messages = serde_json::from_value(i["messages"].clone()).unwrap();
        assert_eq!(
            json!(convert(messages).await),
            case["expected"],
            "{}",
            i["name"]
        );
    }
    s.dispose();
}

#[tokio::test]
async fn default_loader_session_path_and_settings_share_real_handles() {
    let f = Fixture::new(json!({}), vec![]).await;
    let mut options = f.options();
    options.resource_loader = None;
    options.session_manager = None;
    let result = bounded(create_agent_session(options)).await.unwrap();
    let s = result.session;
    assert_eq!(
        s.session_manager.lock().unwrap().get_session_dir(),
        get_default_session_dir_with(&f.cwd, &f.agent_dir)
    );
    f.settings.set_block_images(true);
    assert!(s.settings_manager.get_block_images());
    let manager = f.manager();
    let mut options = f.options();
    options.cwd = None;
    options.session_manager = Some(manager.clone());
    let s2 = create_agent_session(options).await.unwrap().session;
    assert!(Arc::ptr_eq(&manager, &s2.session_manager));
    assert_eq!(s2.extension_runner().create_context().cwd().unwrap(), f.cwd);
    s.dispose();
    s2.dispose();
}

#[tokio::test]
async fn sdk_prompt_persists_provider_response_and_drops_without_strong_cycle() {
    let f = Fixture::new(json!({}), vec![]).await;
    let s = f.session().await;
    let weak = Arc::downgrade(&s);
    bounded(s.prompt("hello", None)).await.unwrap();
    assert_eq!(f.captures.lock().unwrap().len(), 1);
    assert!(s.session_manager.lock().unwrap().get_entries().iter().any(|entry| matches!(entry,SessionEntry::Message(m) if matches!(m.message,AgentMessage::Assistant(_)))));
    assert!(!s.is_streaming());
    s.dispose();
    drop(s);
    assert!(weak.upgrade().is_none());
}

fn hooks(generation: Arc<AtomicUsize>, phases: Arc<Mutex<Vec<String>>>) -> ExtensionFactory {
    Arc::new(move |api| {
        let n = generation.fetch_add(1, Ordering::SeqCst);
        for name in [
            "before_provider_headers",
            "before_provider_request",
            "after_provider_response",
            "context",
        ] {
            let phases = phases.clone();
            api.on(name, Arc::new(move |event, _| {
                let phases = phases.clone();
                Box::pin(async move {
                    tokio::task::yield_now().await;
                    phases.lock().unwrap().push(format!("{n}:{name}"));
                    match name {
                        "before_provider_headers" => {
                            event["headers"]["x-generation"] = json!(n.to_string());
                            Ok(None)
                        }
                        "before_provider_request" => {
                            let mut payload = event["payload"].clone();
                            payload["generation"] = json!(n);
                            Ok(Some(HandlerResult::Json(payload)))
                        }
                        "context" => {
                            let mut messages = event["messages"].as_array().unwrap().clone();
                            messages.push(json!({"role":"user", "content":format!("context-{n}"), "timestamp":3}));
                            Ok(Some(HandlerResult::Json(json!({"messages":messages}))))
                        }
                        _ => Ok(None),
                    }
                })
            }))?;
        }
        Ok(())
    })
}

#[tokio::test]
async fn all_hooks_use_reloaded_runner_without_replacing_sdk_adapter() {
    let phases = Arc::new(Mutex::new(vec![]));
    let generation = Arc::new(AtomicUsize::new(0));
    let f = Fixture::new(json!({}), vec![hooks(generation.clone(), phases.clone())]).await;
    let s = f.session().await;
    let factory = s.agent.runtime().stream_fn.clone().unwrap();
    let callbacks = s.agent.runtime().callbacks.clone();
    let transform = s.agent.runtime().transform_context.clone().unwrap();
    for n in 0..2 {
        if n == 1 {
            bounded(s.reload(None)).await.unwrap();
        }
        stream(&s, SimpleStreamOptions::default()).await;
        assert_eq!(
            f.captures
                .lock()
                .unwrap()
                .last()
                .unwrap()
                .2
                .stream
                .headers
                .as_ref()
                .unwrap()["x-generation"],
            Some(n.to_string())
        );
        assert_eq!(
            callbacks.payload(json!({}), &model()).await.unwrap()["generation"],
            n
        );
        callbacks
            .response(
                crate::ai::types::request_callbacks::ProviderResponse {
                    status: 201,
                    headers: Default::default(),
                },
                &model(),
            )
            .await
            .unwrap();
        let messages = transform(vec![user("before")]).await;
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0], user("before"));
        assert_eq!(json!(messages[1])["timestamp"], 3);
        assert_eq!(
            json!(messages.last().unwrap())["content"],
            format!("context-{n}")
        );
        assert!(Arc::ptr_eq(
            &factory,
            s.agent.runtime().stream_fn.as_ref().unwrap()
        ));
    }
    for n in 0..2 {
        for name in [
            "before_provider_headers",
            "before_provider_request",
            "after_provider_response",
            "context",
        ] {
            assert!(phases.lock().unwrap().contains(&format!("{n}:{name}")));
        }
    }
    s.dispose();
}

#[tokio::test]
async fn headers_are_awaited_and_mutation_before_rejection_survives() {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Semaphore::new(0));
    let e = entered.clone();
    let r = release.clone();
    let ext: ExtensionFactory = Arc::new(move |api| {
        let e = e.clone();
        let r = r.clone();
        api.on(
            "before_provider_headers",
            Arc::new(move |event, _| {
                let e = e.clone();
                let r = r.clone();
                Box::pin(async move {
                    event["headers"]["x-before-reject"] = json!("kept");
                    e.notify_one();
                    r.acquire().await.unwrap().forget();
                    Err("hook rejected".into())
                })
            }),
        )?;
        Ok(())
    });
    let f = Fixture::new(json!({}), vec![ext]).await;
    let s = f.session().await;
    let factory = s.agent.runtime().stream_fn.clone().unwrap();
    let mut rx = factory(
        model(),
        normalize_context(&Context::default()),
        SimpleStreamOptions::default(),
    )
    .await
    .unwrap();
    bounded(entered.notified()).await;
    assert!(f.captures.lock().unwrap().is_empty());
    release.add_permits(1);
    assert!(matches!(
        bounded(rx.recv()).await,
        Some(AssistantMessageEvent::Done { .. })
    ));
    assert_eq!(
        f.captures.lock().unwrap()[0]
            .2
            .stream
            .headers
            .as_ref()
            .unwrap()["x-before-reject"],
        Some("kept".into())
    );
    s.dispose();
}

#[tokio::test]
async fn invalid_native_context_and_headers_report_error_without_dropping_input() {
    let ext: ExtensionFactory = Arc::new(|api| {
        api.on(
            "context",
            sync_handler(|_, _| Ok(Some(HandlerResult::Json(json!({"messages":[42]}))))),
        )?;
        api.on(
            "before_provider_headers",
            sync_handler(|event, _| {
                event["headers"] = json!({"bad":42});
                Ok(None)
            }),
        )?;
        Ok(())
    });
    let f = Fixture::new(json!({}), vec![ext]).await;
    let s = f.session().await;
    let errors = Arc::new(Mutex::new(vec![]));
    let e = errors.clone();
    let _keep = s.extension_runner().on_error(Arc::new(move |error| {
        e.lock().unwrap().push(error.error.clone());
    }));
    let transform = s.agent.runtime().transform_context.clone().unwrap();
    assert_eq!(transform(vec![user("keep")]).await, vec![user("keep")]);
    stream(&s, SimpleStreamOptions::default()).await;
    assert_eq!(errors.lock().unwrap().len(), 2);
    s.dispose();
}

#[tokio::test]
async fn invalid_timeout_setting_rejects_stream_factory_without_provider_call() {
    let f = Fixture::new(json!({}), vec![]).await;
    let s = f.session().await;
    f.settings
        .apply_overrides(&object_settings(json!({"httpIdleTimeoutMs":-1})));
    let factory = s.agent.runtime().stream_fn.clone().unwrap();
    let mut opts = SimpleStreamOptions::default();
    opts.stream.timeout_ms = Some(10);
    let error = factory(model(), normalize_context(&Context::default()), opts)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("httpIdleTimeoutMs"));
    assert!(f.captures.lock().unwrap().is_empty());
    s.dispose();
}

#[test]
fn thinking_budget_settings_accept_exact_js_integers_without_truncation() {
    assert_eq!(
        native_thinking_budgets(object_settings(
            json!({"minimal":0,"low":99,"high":4294967295_u64})
        ))
        .unwrap(),
        ThinkingBudgets {
            minimal: Some(0),
            low: Some(99),
            high: Some(u32::MAX),
            medium: None
        }
    );
    for invalid in [
        json!(-1),
        json!(0.5),
        json!(4294967296_u64),
        json!("99"),
        json!(true),
    ] {
        let error = native_thinking_budgets(object_settings(json!({"high":invalid}))).unwrap_err();
        assert!(error.to_string().contains("thinkingBudgets.high"));
    }
    assert!(native_thinking_budgets(SettingsValue::obj(vec![(
        "high",
        SettingsValue::Num(f64::NAN)
    )]))
    .is_err());
    assert!(native_thinking_budgets(SettingsValue::Null).is_err());
}

#[tokio::test]
async fn no_tools_policies_control_custom_and_extension_tools_not_only_builtins() {
    let ext: ExtensionFactory = Arc::new(|api| {
        api.register_tool(ToolDefinition::new(
            "extension_probe",
            "Extension probe",
            "Not executed",
            json!({"type":"object"}),
        ))?;
        Ok(())
    });
    // Literal expectations follow sdk.ts and AgentSession._refreshToolRegistry:
    // "builtin" disables builtin defaults but allows custom/extension tools;
    // "all" supplies an empty allowlist, unless an explicit tools list wins.
    for (policy, tools, excluded, expected) in [
        (
            Some(NoTools::Builtin),
            None,
            None,
            vec!["extension_probe", "custom_probe"],
        ),
        (Some(NoTools::All), None, None, vec![]),
        (
            Some(NoTools::All),
            Some(vec!["read", "custom_probe"]),
            None,
            vec!["read", "custom_probe"],
        ),
        (
            Some(NoTools::Builtin),
            None,
            Some(vec!["extension_probe"]),
            vec!["custom_probe"],
        ),
    ] {
        let f = Fixture::new(json!({}), vec![ext.clone()]).await;
        let mut options = f.options();
        options.no_tools = policy;
        options.tools = tools.map(|names| names.into_iter().map(str::to_owned).collect());
        options.exclude_tools =
            excluded.map(|names| names.into_iter().map(str::to_owned).collect());
        options.custom_tools = vec![Arc::new(ToolDefinition::new(
            "custom_probe",
            "Custom probe",
            "Not executed",
            json!({"type":"object"}),
        ))];
        let s = bounded(create_agent_session(options))
            .await
            .unwrap()
            .session;
        assert_eq!(s.get_active_tool_names(), expected, "{policy:?}");
        s.dispose();
    }
}

#[tokio::test]
async fn headers_snapshot_runner_before_auth_while_other_hooks_read_current_runner() {
    use crate::ai::auth::types::{AuthCheck, AuthType};
    struct GatedAuth {
        entered: Arc<Notify>,
        gate: Arc<Semaphore>,
    }
    impl ApiKeyAuth for GatedAuth {
        fn name(&self) -> &str {
            "r15 gated offline auth"
        }
        fn check<'a>(
            &'a self,
            _: ApiKeyAuthInput<'a>,
        ) -> Option<BoxFuture<'a, Result<Option<AuthCheck>, AuthError>>> {
            Some(Box::pin(async {
                Ok(Some(AuthCheck {
                    source: None,
                    r#type: AuthType::ApiKey,
                }))
            }))
        }
        fn resolve<'a>(
            &'a self,
            _: ApiKeyAuthInput<'a>,
        ) -> BoxFuture<'a, Result<Option<AuthResult>, AuthError>> {
            Box::pin(async move {
                self.entered.notify_one();
                self.gate.acquire().await.unwrap().forget();
                Ok(Some(AuthResult {
                    auth: ModelAuth {
                        api_key: Some("r15-test-not-real".into()),
                        ..Default::default()
                    },
                    ..Default::default()
                }))
            })
        }
    }
    let phases = Arc::new(Mutex::new(vec![]));
    let generation = Arc::new(AtomicUsize::new(0));
    let f = Fixture::new(json!({}), vec![hooks(generation.clone(), phases.clone())]).await;
    let entered = Arc::new(Notify::new());
    let gate = Arc::new(Semaphore::new(0));
    f.runtime
        .register_native_provider(create_provider(CreateProviderOptions {
            filter_all_models: None,
            images: crate::ai::models::provider::ImagesImpls::new(),
            classifiers: crate::ai::models::provider::ClassifiersImpls::new(),
            id: model().provider,
            name: None,
            base_url: None,
            headers: None,
            auth: ProviderAuth {
                api_key: Some(Arc::new(GatedAuth {
                    entered: entered.clone(),
                    gate: gate.clone(),
                })),
                oauth: None,
            },
            models: vec![crate::ai::types::AnyModel::Chat(model())],
            fetch_models: None,
            filter_models: None,
            api: ApiImpls::Single(Arc::new(CaptureApi(f.captures.clone()))),
        }))
        .await
        .unwrap();
    let mut opts = f.options();
    opts.model = Some(model());
    let s = bounded(create_agent_session(opts)).await.unwrap().session;
    assert_eq!(
        generation.load(Ordering::SeqCst),
        1,
        "supplied loader is not reloaded by SDK"
    );
    let factory = s.agent.runtime().stream_fn.clone().unwrap();
    let callbacks = s.agent.runtime().callbacks.clone();
    let transform = s.agent.runtime().transform_context.clone().unwrap();
    let mut rx = bounded(factory(
        model(),
        normalize_context(&Context::default()),
        SimpleStreamOptions::default(),
    ))
    .await
    .unwrap();
    bounded(entered.notified()).await;
    assert!(f.captures.lock().unwrap().is_empty());
    bounded(s.reload(None)).await.unwrap();
    assert_eq!(generation.load(Ordering::SeqCst), 2);
    gate.add_permits(1);
    assert!(matches!(
        bounded(rx.recv()).await,
        Some(AssistantMessageEvent::Done { .. })
    ));
    assert_eq!(
        f.captures.lock().unwrap()[0]
            .2
            .stream
            .headers
            .as_ref()
            .unwrap()["x-generation"],
        Some("0".into())
    );
    assert_eq!(
        callbacks.payload(json!({}), &model()).await.unwrap()["generation"],
        1
    );
    callbacks
        .response(
            crate::ai::types::request_callbacks::ProviderResponse {
                status: 200,
                headers: Default::default(),
            },
            &model(),
        )
        .await
        .unwrap();
    assert_eq!(
        json!(transform(vec![user("original")]).await.last().unwrap())["content"],
        "context-1"
    );
    gate.add_permits(1);
    stream(&s, SimpleStreamOptions::default()).await;
    assert_eq!(
        f.captures.lock().unwrap()[1]
            .2
            .stream
            .headers
            .as_ref()
            .unwrap()["x-generation"],
        Some("1".into())
    );
    assert_eq!(
        *phases.lock().unwrap(),
        vec![
            "0:before_provider_headers",
            "1:before_provider_request",
            "1:after_provider_response",
            "1:context",
            "1:before_provider_headers"
        ]
    );
    s.dispose();
}

#[tokio::test]
async fn real_sdk_prompt_through_http_provider_awaits_hooks_and_persists_result() {
    let server = wiremock::MockServer::start().await;
    let body = concat!(
        "data: {\"id\":\"r15\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"sdk response\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"r15\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n"
    );
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/v1/chat/completions"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .insert_header("x-r15-response", "received")
                .set_body_string(body),
        )
        .mount(&server)
        .await;
    let phases = Arc::new(Mutex::new(Vec::new()));
    let entered = Arc::new(Notify::new());
    let gate = Arc::new(Semaphore::new(0));
    let p = phases.clone();
    let e = entered.clone();
    let g = gate.clone();
    let ext: ExtensionFactory = Arc::new(move |api| {
        let p = p.clone();
        let q = p.clone();
        api.on(
            "context",
            sync_handler(move |event, _| {
                q.lock().unwrap().push("context");
                let mut messages = event["messages"].as_array().unwrap().clone();
                messages
                    .push(json!({"role":"user","content":"r15 injected context","timestamp":3}));
                Ok(Some(HandlerResult::Json(json!({"messages":messages}))))
            }),
        )?;
        let q = p.clone();
        api.on(
            "before_provider_headers",
            sync_handler(move |event, _| {
                q.lock().unwrap().push("headers");
                assert_eq!(event["headers"]["x-opencode-session"], "r15-session");
                assert_eq!(event["headers"]["x-opencode-client"], "pi");
                assert_eq!(event["headers"]["x-model"], "model-header");
                event["headers"]["x-r15-hook"] = json!("native-sdk");
                Ok(None)
            }),
        )?;
        let q = p.clone();
        let e = e.clone();
        let g = g.clone();
        api.on(
            "before_provider_request",
            Arc::new(move |event, _| {
                let q = q.clone();
                let e = e.clone();
                let g = g.clone();
                Box::pin(async move {
                    q.lock().unwrap().push("payload");
                    e.notify_one();
                    g.acquire().await.unwrap().forget();
                    let mut payload = event["payload"].clone();
                    payload["r15_probe"] = json!("changed");
                    Ok(Some(HandlerResult::Json(payload)))
                })
            }),
        )?;
        api.on(
            "after_provider_response",
            sync_handler(move |event, _| {
                p.lock().unwrap().push("response");
                assert_eq!(event["status"], 200);
                assert_eq!(event["headers"]["x-r15-response"], "received");
                Ok(None)
            }),
        )?;
        Ok(())
    });
    let f=Fixture::new(json!({"enableInstallTelemetry":false,"retry":{"provider":{"maxRetries":0,"maxRetryDelayMs":0}}}),vec![ext]).await;
    let mut m = model();
    m.provider = "opencode".into();
    m.reasoning = false;
    m.base_url = format!("{}/v1", server.uri());
    m.headers = Some([("x-model".into(), Some("model-header".into()))].into());
    let runtime = runtime(
        vec![m.clone()],
        std::slice::from_ref(&m.provider),
        Arc::new(crate::ai::api::openai_completions::OpenAiCompletions),
    )
    .await;
    let mut opts = f.options();
    opts.model_runtime = Some(runtime);
    opts.model = Some(m);
    let s = bounded(create_agent_session(opts)).await.unwrap().session;
    let task_session = s.clone();
    let task = tokio::spawn(async move { task_session.prompt("hello via SDK", None).await });
    bounded(entered.notified()).await;
    assert!(server.received_requests().await.unwrap().is_empty());
    assert!(!task.is_finished());
    gate.add_permits(1);
    bounded(task).await.unwrap().unwrap();
    assert_eq!(
        *phases.lock().unwrap(),
        vec!["context", "headers", "payload", "response"]
    );
    assert_eq!(s.agent.state().error_message, None);
    assert!(!s.is_streaming());
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].headers.get("authorization").unwrap(),
        "Bearer r15-test-not-real"
    );
    assert_eq!(requests[0].headers.get("x-r15-hook").unwrap(), "native-sdk");
    assert_eq!(
        requests[0].headers.get("x-opencode-session").unwrap(),
        "r15-session"
    );
    let payload: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(payload["r15_probe"], "changed");
    assert!(payload["messages"]
        .to_string()
        .contains("r15 injected context"));
    let entries = s.session_manager.lock().unwrap().get_entries();
    assert!(entries.iter().any(|entry| matches!(entry,SessionEntry::Message(m) if matches!(m.message,AgentMessage::Assistant(_)) && json!(m.message).to_string().contains("sdk response"))));
    assert!(
        !entries
            .iter()
            .any(|entry| json!(entry).to_string().contains("r15 injected context")),
        "request-only transformed context must not become persisted user input"
    );
    s.dispose();
}

#[test]
fn empty_agent_dir_uses_default_without_forcing_auth_paths() {
    // Literal source-derived regression for sdk.ts:174/177-178. This pure
    // selection helper does not create or read any real home auth files.
    let expected = (get_agent_dir(), false);
    assert_eq!(resolve_sdk_agent_dir(None).unwrap(), expected);
    assert_eq!(resolve_sdk_agent_dir(Some("")).unwrap(), expected);
    // Whitespace is a truthy JS string and must not be trimmed into default.
    for path in [".", " "] {
        assert_eq!(
            resolve_sdk_agent_dir(Some(path)).unwrap(),
            (resolve_path_auto_base(path).unwrap(), true)
        );
    }
}
