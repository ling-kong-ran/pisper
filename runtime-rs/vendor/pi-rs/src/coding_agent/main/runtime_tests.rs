use super::runtime::*;
use crate::coding_agent::{
    cli::{
        args::{Args, UnknownFlagValue},
        project_trust::AppMode,
    },
    core::{
        agent_session_runtime::CreateAgentSessionRuntimeOptions,
        agent_session_services::DiagnosticType, model_runtime::ModelRuntime,
        resource_loader::InlineExtension, settings_manager::SettingsManager,
    },
};
use crate::{
    ai::{
        auth::credential_store::InMemoryCredentialStore,
        models::{faux_assistant_message, faux_provider, FauxMessageOptions, FauxProviderOptions},
    },
    coding_agent::{
        core::{
            agent_session_runtime::create_agent_session_runtime,
            model_runtime::{CreateModelRuntimeOptions, ProviderOrModel},
            models_store::InMemoryCodingAgentModelsStore,
            settings_manager::parse_settings_value,
        },
        extensions::{
            loader::ExtensionFactory,
            types::{
                ExtensionMode, ExtensionUI, ExtensionUiDialogOptions, HandlerFn,
                ProjectTrustContext, SessionStartEvent, SessionStartReason, UiFuture,
            },
        },
        session_manager::{NewSessionOptions, SessionManager},
    },
};
use anyhow::Result;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

async fn offline(signal: CancellationToken) -> Result<ModelRuntime> {
    ModelRuntime::create(CreateModelRuntimeOptions {
        credentials: Some(Arc::new(InMemoryCredentialStore::default())),
        models_path: Some(None),
        models_store: Some(Arc::new(InMemoryCodingAgentModelsStore::default())),
        allow_model_network: false,
        refresh_on_create: Some(false),
        signal: Some(signal),
        ..Default::default()
    })
    .await
    .map_err(anyhow::Error::msg)
}
struct Fixture {
    paths: super::sessions_tests::Fixture,
    agent: String,
}
impl Fixture {
    fn new() -> Self {
        let paths = super::sessions_tests::Fixture::new();
        let agent = paths.p("/root/agent");
        std::fs::create_dir_all(&agent).unwrap();
        Self { paths, agent }
    }
    fn options(&self, parsed: Args) -> CliRuntimeFactoryOptions {
        CliRuntimeFactoryOptions {
            parsed,
            startup_cwd: self.paths.cwd.clone(),
            initial_session_cwd: self.paths.other.clone(),
            agent_dir: self.agent.clone(),
            startup_settings_manager: SettingsManager::in_memory(
                parse_settings_value("{}").unwrap(),
            ),
            app_mode: AppMode::Print,
            extension_factories: vec![],
            extension_module_loader: None,
            model_runtime_factory: Some(Arc::new(|_, _, signal| Box::pin(offline(signal)))),
            custom_tool_factory: None,
            model_scope_warning: None,
        }
    }
    fn input(
        &self,
        cwd: &str,
        ctx: Option<ProjectTrustContext>,
    ) -> CreateAgentSessionRuntimeOptions {
        CreateAgentSessionRuntimeOptions {
            cwd: cwd.into(),
            agent_dir: self.agent.clone(),
            session_manager: Arc::new(Mutex::new(
                SessionManager::in_memory(
                    cwd,
                    Some(&NewSessionOptions {
                        id: Some("main-test".into()),
                        ..Default::default()
                    }),
                    None,
                )
                .unwrap(),
            )),
            session_start_event: None,
            project_trust_context: ctx,
        }
    }
    fn project(&self, cwd: &str, content: &str) {
        let pi = std::path::Path::new(cwd).join(".pi");
        std::fs::create_dir_all(&pi).unwrap();
        std::fs::write(pi.join("settings.json"), content).unwrap();
    }
}
struct Select {
    choice: Option<usize>,
    calls: Arc<AtomicUsize>,
    delay: Duration,
    error: bool,
}
impl ExtensionUI for Select {
    fn select<'a>(
        &'a self,
        _: &'a str,
        choices: &'a [String],
        _: &'a ExtensionUiDialogOptions,
    ) -> UiFuture<'a, Option<String>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(self.delay).await;
            if self.error {
                return Err("trust UI failed".into());
            }
            Ok(self.choice.and_then(|i| choices.get(i).cloned()))
        })
    }
}
fn context(cwd: &str, choice: Option<usize>, calls: Arc<AtomicUsize>) -> ProjectTrustContext {
    ProjectTrustContext {
        cwd: cwd.into(),
        mode: ExtensionMode::Print,
        has_ui: true,
        ui: Some(Arc::new(Select {
            choice,
            calls,
            delay: Duration::ZERO,
            error: false,
        })),
    }
}
fn inline(factory: ExtensionFactory) -> InlineExtension {
    InlineExtension::Named {
        factory,
        name: "cli-test".into(),
        hidden: false,
    }
}

#[tokio::test]
async fn actual_factory_sdk_runtime_and_faux_provider_prompt_are_connected() {
    let f = Fixture::new();
    let faux = faux_provider(FauxProviderOptions {
        provider: Some("main-startup-faux".into()),
        ..Default::default()
    });
    let model = faux.get_model(None).unwrap();
    faux.set_responses(vec![faux_assistant_message(
        "startup reached native provider",
        FauxMessageOptions::default(),
    )
    .into()]);
    let parsed = Args {
        model: Some(format!("{}/{}:high", model.provider, model.id)),
        no_tools: Some(true),
        ..Default::default()
    };
    let provider = faux.provider.clone();
    let mut options = f.options(parsed);
    options.extension_factories = vec![inline(Arc::new(move |api| {
        api.register_native_provider(&provider)
    }))];
    let factory = create_cli_runtime_factory(options).unwrap();
    assert_eq!(
        factory.auto_trust_on_reload_cwd,
        Some(f.paths.other.clone())
    );
    let input = f.input(&f.paths.other, None);
    let manager = input.session_manager.clone();
    let runtime = create_agent_session_runtime(factory.create_runtime, input)
        .await
        .unwrap();
    assert_eq!(runtime.services().cwd, f.paths.other);
    assert_eq!(runtime.session().model().unwrap().id, model.id);
    assert!(runtime.session().get_active_tool_names().is_empty());
    tokio::time::timeout(
        Duration::from_secs(12),
        runtime.session().prompt("hello CLI", None),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(faux.state().lock().unwrap().call_count, 1);
    assert!(
        manager
            .lock()
            .unwrap()
            .build_session_context()
            .messages
            .len()
            >= 2
    );
    runtime.dispose().await.unwrap();
}
#[tokio::test]
async fn trust_waits_before_project_settings_and_caches_decisions_per_cwd() {
    let f = Fixture::new();
    f.project(&f.paths.other, r#"{"defaultModel":"project-secret"}"#);
    f.project(&f.paths.cwd, r#"{"defaultModel":"second-project"}"#);
    let factory = create_cli_runtime_factory(f.options(Args::default())).unwrap();
    assert_eq!(factory.auto_trust_on_reload_cwd, None);
    let calls = Arc::new(AtomicUsize::new(0));
    let approved = (factory.create_runtime)(f.input(
        &f.paths.other,
        Some(context(&f.paths.other, Some(2), calls.clone())),
    ))
    .await
    .unwrap();
    assert!(approved.services.settings_manager.is_project_trusted());
    assert_eq!(
        approved
            .services
            .settings_manager
            .get_default_model()
            .as_deref(),
        Some("project-secret")
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let again = (factory.create_runtime)(f.input(
        &f.paths.other,
        Some(context(&f.paths.other, Some(4), calls.clone())),
    ))
    .await
    .unwrap();
    assert!(again.services.settings_manager.is_project_trusted());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let denied = (factory.create_runtime)(f.input(
        &f.paths.cwd,
        Some(context(&f.paths.cwd, Some(4), calls.clone())),
    ))
    .await
    .unwrap();
    assert!(!denied.services.settings_manager.is_project_trusted());
    assert_eq!(denied.services.settings_manager.get_default_model(), None);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let again = (factory.create_runtime)(f.input(
        &f.paths.cwd,
        Some(context(&f.paths.cwd, Some(2), calls.clone())),
    ))
    .await
    .unwrap();
    assert!(!again.services.settings_manager.is_project_trusted());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(
        !std::path::Path::new(&f.agent).join("trust.json").exists(),
        "session-only choices must not persist"
    );
}
#[tokio::test]
async fn explicit_override_beats_ui_and_project_policy_cannot_approve_itself() {
    for decision in [true, false] {
        let f = Fixture::new();
        f.project(
            &f.paths.other,
            r#"{"defaultProjectTrust":"always","defaultModel":"not-global"}"#,
        );
        let calls = Arc::new(AtomicUsize::new(0));
        let factory = create_cli_runtime_factory(f.options(Args {
            project_trust_override: Some(decision),
            ..Default::default()
        }))
        .unwrap();
        let result = (factory.create_runtime)(f.input(
            &f.paths.other,
            Some(context(&f.paths.other, Some(2), calls.clone())),
        ))
        .await
        .unwrap();
        assert_eq!(
            result.services.settings_manager.is_project_trusted(),
            decision
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
    let f = Fixture::new();
    f.project(&f.paths.other, r#"{"defaultProjectTrust":"always"}"#);
    let factory = create_cli_runtime_factory(f.options(Args::default())).unwrap();
    let result = (factory.create_runtime)(f.input(&f.paths.other, None))
        .await
        .unwrap();
    assert!(!result.services.settings_manager.is_project_trusted());
}
#[tokio::test]
async fn cli_resources_stay_relative_to_startup_not_session_cwd() {
    let f = Fixture::new();
    std::fs::write(
        std::path::Path::new(&f.paths.cwd).join("prompt.md"),
        "from-startup",
    )
    .unwrap();
    std::fs::write(
        std::path::Path::new(&f.paths.other).join("prompt.md"),
        "wrong-cwd",
    )
    .unwrap();
    let factory = create_cli_runtime_factory(f.options(Args {
        prompt_templates: Some(vec!["./prompt.md".into()]),
        ..Default::default()
    }))
    .unwrap();
    let result = (factory.create_runtime)(f.input(&f.paths.other, None))
        .await
        .unwrap();
    assert_eq!(result.services.cwd, f.paths.other);
    let prompts = result
        .services
        .resource_loader
        .lock()
        .unwrap()
        .get_prompts();
    assert!(prompts.prompts.iter().any(|p| p.content == "from-startup"));
    assert!(!prompts.prompts.iter().any(|p| p.content == "wrong-cwd"));
}
#[tokio::test]
async fn metadata_and_replacement_trust_context_mode_ui_and_extension_warning_are_preserved() {
    let f = Fixture::new();
    f.project(&f.paths.other, "{}");
    f.project(&f.paths.cwd, "{}");
    let trace = Arc::new(Mutex::new(Vec::<Value>::new()));
    let seen = trace.clone();
    let ext: ExtensionFactory = Arc::new(move |api| {
        let seen = seen.clone();
        let handler: HandlerFn = Arc::new(move |_, ctx| {
            let seen = seen.clone();
            let event = json!([
                ctx.cwd().unwrap(),
                ctx.mode().unwrap().as_str(),
                ctx.has_ui().unwrap()
            ]);
            Box::pin(async move {
                seen.lock().unwrap().push(event);
                Err("deliberate trust warning".into())
            })
        });
        api.on("project_trust", handler)?;
        Ok(())
    });
    let mut options = f.options(Args {
        help: Some(true),
        ..Default::default()
    });
    options.app_mode = AppMode::Rpc;
    options.extension_factories = vec![inline(ext)];
    let factory = create_cli_runtime_factory(options).unwrap();
    let result = (factory.create_runtime)(f.input(&f.paths.other, None))
        .await
        .unwrap();
    assert_eq!(
        trace.lock().unwrap()[0],
        json!([f.paths.other, "print", false])
    );
    assert_eq!(result.diagnostics[0].kind, DiagnosticType::Warning);
    assert!(result.diagnostics[0]
        .message
        .ends_with("project_trust error: deliberate trust warning"));
    let mut next = f.input(&f.paths.cwd, None);
    next.session_start_event = Some(SessionStartEvent {
        event_type: "session_start".into(),
        reason: SessionStartReason::New,
        previous_session_file: None,
    });
    let _ = (factory.create_runtime)(next).await.unwrap();
    assert_eq!(trace.lock().unwrap()[1], json!([f.paths.cwd, "rpc", false]));
}
#[tokio::test(start_paused = true)]
async fn trust_prompt_may_outlive_model_deadline_and_cancel_or_error_is_not_cached_as_approval() {
    let f = Fixture::new();
    f.project(&f.paths.other, "{}");
    let calls = Arc::new(AtomicUsize::new(0));
    let factory = create_cli_runtime_factory(f.options(Args::default())).unwrap();
    let ctx = ProjectTrustContext {
        cwd: f.paths.other.clone(),
        mode: ExtensionMode::Print,
        has_ui: true,
        ui: Some(Arc::new(Select {
            choice: Some(2),
            calls: calls.clone(),
            delay: Duration::from_secs(20),
            error: true,
        })),
    };
    let result = (factory.create_runtime)(f.input(&f.paths.other, Some(ctx))).await;
    assert!(result
        .err()
        .unwrap()
        .to_string()
        .contains("trust UI failed"));
    let ctx = ProjectTrustContext {
        cwd: f.paths.other.clone(),
        mode: ExtensionMode::Print,
        has_ui: true,
        ui: Some(Arc::new(Select {
            choice: Some(2),
            calls: calls.clone(),
            delay: Duration::from_secs(20),
            error: false,
        })),
    };
    let result = (factory.create_runtime)(f.input(&f.paths.other, Some(ctx)))
        .await
        .unwrap();
    assert!(result.services.settings_manager.is_project_trusted());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}
#[tokio::test]
async fn runtime_api_key_is_not_persisted_and_missing_model_reports_ordered_diagnostics() {
    let f = Fixture::new();
    let factory = create_cli_runtime_factory(f.options(Args {
        model: Some("openai/gpt-5.5".into()),
        api_key: Some("offline-test-only".into()),
        no_tools: Some(true),
        ..Default::default()
    }))
    .unwrap();
    let result = (factory.create_runtime)(f.input(&f.paths.other, None))
        .await
        .unwrap();
    let auth = result
        .services
        .model_runtime
        .get_auth(ProviderOrModel::Provider("openai"), None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(auth.auth.api_key.as_deref(), Some("offline-test-only"));
    assert!(!std::path::Path::new(&f.agent).join("auth.json").exists());
    let mut parsed = Args {
        api_key: Some("offline-test-only".into()),
        ..Default::default()
    };
    parsed
        .unknown_flags
        .insert("unregistered", UnknownFlagValue::Boolean(true));
    let factory = create_cli_runtime_factory(f.options(parsed)).unwrap();
    let result = (factory.create_runtime)(f.input(&f.paths.other, None))
        .await
        .unwrap();
    assert!(result.diagnostics[0].message.starts_with("Unknown option"));
    assert!(result
        .diagnostics
        .last()
        .unwrap()
        .message
        .starts_with("--api-key requires a model"));
}
#[tokio::test]
async fn dropped_factory_cancels_model_operation_without_leaking_timer() {
    let f = Fixture::new();
    let signal = Arc::new(Mutex::new(None::<CancellationToken>));
    let seen = signal.clone();
    let mut options = f.options(Args::default());
    options.model_runtime_factory = Some(Arc::new(move |_, _, token| {
        let seen = seen.clone();
        Box::pin(async move {
            *seen.lock().unwrap() = Some(token);
            std::future::pending().await
        })
    }));
    let factory = create_cli_runtime_factory(options).unwrap();
    assert!(tokio::time::timeout(
        Duration::from_millis(10),
        (factory.create_runtime)(f.input(&f.paths.other, None))
    )
    .await
    .is_err());
    assert!(signal.lock().unwrap().as_ref().unwrap().is_cancelled());
}

#[tokio::test]
async fn project_extension_loader_cannot_run_before_ui_consent() {
    struct Loader(Arc<Mutex<Vec<String>>>);
    impl crate::coding_agent::extensions::loader::ExtensionModuleLoader for Loader {
        fn load(&self, _: &str) -> std::result::Result<Option<ExtensionFactory>, String> {
            self.0.lock().unwrap().push("load".into());
            Ok(Some(Arc::new(|_| Ok(()))))
        }
    }
    struct GateUi {
        trace: Arc<Mutex<Vec<String>>>,
        approve: bool,
    }
    impl ExtensionUI for GateUi {
        fn select<'a>(
            &'a self,
            _: &'a str,
            choices: &'a [String],
            _: &'a ExtensionUiDialogOptions,
        ) -> UiFuture<'a, Option<String>> {
            Box::pin(async move {
                assert!(
                    self.trace.lock().unwrap().is_empty(),
                    "project extension ran without consent"
                );
                self.trace.lock().unwrap().push("ask".into());
                tokio::task::yield_now().await;
                Ok(Some(choices[if self.approve { 2 } else { 4 }].clone()))
            })
        }
    }
    for approve in [true, false] {
        let f = Fixture::new();
        f.project(&f.paths.other, "{}");
        let ext = std::path::Path::new(&f.paths.other).join(".pi/extensions");
        std::fs::create_dir_all(&ext).unwrap();
        std::fs::write(ext.join("consent.ts"), "explicit native test-loader seam").unwrap();
        let trace = Arc::new(Mutex::new(vec![]));
        let mut options = f.options(Args::default());
        options.extension_module_loader = Some(Arc::new(Loader(trace.clone())));
        let factory = create_cli_runtime_factory(options).unwrap();
        let ctx = ProjectTrustContext {
            cwd: f.paths.other.clone(),
            mode: ExtensionMode::Print,
            has_ui: true,
            ui: Some(Arc::new(GateUi {
                trace: trace.clone(),
                approve,
            })),
        };
        let result = (factory.create_runtime)(f.input(&f.paths.other, Some(ctx)))
            .await
            .unwrap();
        assert_eq!(
            *trace.lock().unwrap(),
            if approve {
                vec!["ask", "load"]
            } else {
                vec!["ask"]
            }
        );
        assert_eq!(
            result.services.settings_manager.is_project_trusted(),
            approve
        );
    }
}

#[tokio::test]
async fn actual_session_switch_rebuilds_cwd_settings_and_reuses_per_project_trust() {
    use crate::coding_agent::core::agent_session_runtime::SwitchSessionOptions;
    let f = Fixture::new();
    f.project(&f.paths.other, r#"{"defaultModel":"approved-project"}"#);
    f.project(&f.paths.cwd, r#"{"defaultModel":"denied-project"}"#);
    let first_path = f.paths.seed(
        "approved",
        "approved",
        &f.paths.other,
        "2026-09-28T01:00:00.000Z",
    );
    let second_path = f
        .paths
        .seed("denied", "denied", &f.paths.cwd, "2026-09-28T02:00:00.000Z");
    let host_cwds = Arc::new(Mutex::new(Vec::<String>::new()));
    let observed_cwds = host_cwds.clone();
    let mut options = f.options(Args::default());
    options.custom_tool_factory = Some(Arc::new(move |cwd, _settings| {
        let observed = observed_cwds.clone();
        Box::pin(async move {
            observed.lock().unwrap().push(cwd.clone());
            let definition = crate::coding_agent::core::tools::bash::create_bash_tool_definition(
                &cwd,
                Default::default(),
            );
            let mut definition = definition.as_ref().clone();
            definition.label = format!("host shell at {cwd}");
            Ok(vec![Arc::new(definition)])
        })
    }));
    let factory = create_cli_runtime_factory(options).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let input = f.input(
        &f.paths.other,
        Some(context(&f.paths.other, Some(2), calls.clone())),
    );
    let runtime = create_agent_session_runtime(factory.create_runtime, input)
        .await
        .unwrap();
    let first_services = runtime.services();
    assert_eq!(
        runtime.session().get_tool_definition("bash").unwrap().label,
        format!("host shell at {}", f.paths.other)
    );
    runtime.session().reload(None).await.unwrap();
    assert_eq!(
        runtime.session().get_tool_definition("bash").unwrap().label,
        format!("host shell at {}", f.paths.other)
    );
    assert!(first_services.settings_manager.is_project_trusted());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let counter = calls.clone();
    let result = runtime
        .switch_session(
            &second_path,
            SwitchSessionOptions {
                project_trust_context_factory: Some(Arc::new(move |cwd| {
                    Ok(context(cwd, Some(4), counter.clone()))
                })),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(!result.cancelled);
    assert_eq!(runtime.cwd(), f.paths.cwd);
    assert_eq!(
        runtime.session().get_tool_definition("bash").unwrap().label,
        format!("host shell at {}", f.paths.cwd)
    );
    assert!(!Arc::ptr_eq(&first_services, &runtime.services()));
    assert!(!runtime.services().settings_manager.is_project_trusted());
    assert_eq!(
        runtime.services().settings_manager.get_default_model(),
        None
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    // Only the trust decision is cached: approved project's settings are read
    // again when a saved session returns to it, without another UI prompt.
    f.project(&f.paths.other, r#"{"defaultModel":"reloaded-project"}"#);
    let counter = calls.clone();
    let result = runtime
        .switch_session(
            &first_path,
            SwitchSessionOptions {
                project_trust_context_factory: Some(Arc::new(move |cwd| {
                    Ok(context(cwd, Some(4), counter.clone()))
                })),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(!result.cancelled);
    assert_eq!(runtime.cwd(), f.paths.other);
    assert_eq!(
        runtime.session().get_tool_definition("bash").unwrap().label,
        format!("host shell at {}", f.paths.other)
    );
    assert_eq!(
        *host_cwds.lock().unwrap(),
        vec![
            f.paths.other.clone(),
            f.paths.cwd.clone(),
            f.paths.other.clone()
        ]
    );
    assert!(runtime.services().settings_manager.is_project_trusted());
    assert_eq!(
        runtime
            .services()
            .settings_manager
            .get_default_model()
            .as_deref(),
        Some("reloaded-project")
    );
    assert_eq!(
        runtime.session().session_file().as_deref(),
        Some(first_path.as_str())
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(!std::path::Path::new(&f.agent).join("trust.json").exists());
    runtime.dispose().await.unwrap();
}
