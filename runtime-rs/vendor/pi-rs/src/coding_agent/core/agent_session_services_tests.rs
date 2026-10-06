//! Real services/SDK regressions. Native factories/providers are explicit seams;
//! these tests do not claim that a TS module or an embedded JS engine executed.
use super::*;
use crate::agent_core::AgentMessage;
use crate::ai::auth::credential_store::InMemoryCredentialStore;
use crate::ai::models::{
    faux_assistant_message, faux_provider, FauxMessageOptions, FauxProviderOptions,
};
use crate::coding_agent::core::models_store::InMemoryCodingAgentModelsStore;
use crate::coding_agent::core::resource_loader::InlineExtension;
use crate::coding_agent::core::settings_manager::parse_settings_value;
use crate::coding_agent::extensions::loader::{ExtensionApi, ExtensionFactory, ExtensionRuntime};
use crate::coding_agent::session_manager::{NewSessionOptions, SessionEntry};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

async fn bounded<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(12), future)
        .await
        .expect("services operation did not finish")
}

fn settings() -> SettingsManager {
    SettingsManager::in_memory(parse_settings_value("{}").unwrap())
}

struct Fixture {
    _dir: tempfile::TempDir,
    cwd: String,
    agent_dir: String,
    runtime: ModelRuntime,
    settings: SettingsManager,
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().join("project");
        let agent_dir = dir.path().join("agent");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&agent_dir).unwrap();
        let runtime = bounded(ModelRuntime::create(CreateModelRuntimeOptions {
            credentials: Some(Arc::new(InMemoryCredentialStore::default())),
            models_path: Some(None),
            models_store: Some(Arc::new(InMemoryCodingAgentModelsStore::default())),
            allow_model_network: false,
            refresh_on_create: Some(false),
            ..Default::default()
        }))
        .await
        .unwrap();
        Self {
            _dir: dir,
            cwd: cwd.to_str().unwrap().to_string(),
            agent_dir: agent_dir.to_str().unwrap().to_string(),
            runtime,
            settings: settings(),
        }
    }

    fn options(&self, factories: Vec<ExtensionFactory>) -> CreateAgentSessionServicesOptions {
        CreateAgentSessionServicesOptions {
            cwd: self.cwd.clone(),
            agent_dir: Some(self.agent_dir.clone()),
            model_runtime: Some(self.runtime.clone()),
            settings_manager: Some(self.settings.clone()),
            resource_loader_options: Some(DefaultResourceLoaderOptions {
                extension_factories: factories
                    .into_iter()
                    .enumerate()
                    .map(|(index, factory)| InlineExtension::Named {
                        factory,
                        name: format!("services-{index}"),
                        hidden: false,
                    })
                    .collect(),
                no_skills: true,
                no_prompt_templates: true,
                no_themes: true,
                no_context_files: true,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn manager(&self) -> Arc<Mutex<SessionManager>> {
        Arc::new(Mutex::new(
            SessionManager::in_memory(
                &self.cwd,
                Some(&NewSessionOptions {
                    id: Some("services-r16".into()),
                    ..Default::default()
                }),
                None,
            )
            .unwrap(),
        ))
    }
}

fn model_definition() -> Value {
    json!({"id":"m1","name":"Extension model","reasoning":true,"input":["text"],
        "cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0},
        "contextWindow":100000,"maxTokens":2048})
}

#[tokio::test]
async fn services_resolve_paths_override_loader_options_and_share_real_settings_and_runtime() {
    let f = Fixture::new().await;
    let shadow = settings();
    shadow.set_project_trusted(true);
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let mut options = f.options(vec![]);
    options.cwd.push_str("/.");
    options.agent_dir.as_mut().unwrap().push_str("/.");
    let loader_options = options.resource_loader_options.as_mut().unwrap();
    loader_options.cwd = "ignored-cwd".into();
    loader_options.agent_dir = "ignored-agent-dir".into();
    loader_options.settings_manager = Some(Arc::new(shadow.clone()));
    options.resource_loader_reload_options = Some(ResourceLoaderReloadOptions {
        resolve_project_trust: Some(Arc::new(move |_| {
            seen.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(false) })
        })),
    });
    let services = bounded(create_agent_session_services(options))
        .await
        .unwrap();
    assert_eq!(services.cwd, resolve_path_auto_base(&f.cwd).unwrap());
    assert_eq!(
        services.agent_dir,
        resolve_path_auto_base(&f.agent_dir).unwrap()
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "reload options must reach the actual loader"
    );
    assert!(!f.settings.is_project_trusted());
    assert!(
        shadow.is_project_trusted(),
        "user loader options may not replace services settings"
    );
    f.settings.set_block_images(true);
    assert!(services.settings_manager.get_block_images());
    let provider = faux_provider(FauxProviderOptions {
        provider: Some("services-live".into()),
        ..Default::default()
    });
    f.runtime
        .register_native_provider_sync(provider.provider.clone())
        .unwrap();
    assert!(Arc::ptr_eq(
        &services
            .model_runtime
            .get_provider("services-live")
            .await
            .unwrap(),
        &provider.provider
    ));
    assert!(services.diagnostics.is_empty());
}

#[tokio::test]
async fn services_flags_preserve_order_boolean_presence_empty_strings_and_last_declaration() {
    let first: ExtensionFactory = Arc::new(|api| {
        for name in ["false-bool", "string-bool", "duplicate"] {
            api.register_flag(name, None, FlagType::Boolean, None)?;
        }
        api.register_flag("empty", None, FlagType::String, None)?;
        api.register_flag(
            "needs",
            None,
            FlagType::String,
            Some(FlagValue::Str("default".into())),
        )?;
        Ok(())
    });
    let second: ExtensionFactory =
        Arc::new(|api| api.register_flag("duplicate", None, FlagType::String, None));
    let f = Fixture::new().await;
    let mut values = OrderedMap::new();
    values.set("unknown-z", FlagValue::Bool(false));
    values.set("false-bool", FlagValue::Bool(false));
    values.set("string-bool", FlagValue::Str("false".into()));
    values.set("empty", FlagValue::Str(String::new()));
    values.set("needs", FlagValue::Bool(true));
    values.set("duplicate", FlagValue::Str("last-definition".into()));
    values.set("unknown-a", FlagValue::Str("x".into()));
    let mut options = f.options(vec![first, second]);
    options.extension_flag_values = Some(values);
    let services = bounded(create_agent_session_services(options))
        .await
        .unwrap();
    let runtime = services
        .resource_loader
        .lock()
        .unwrap()
        .get_extensions()
        .runtime;
    assert_eq!(
        runtime.flag_value("false-bool"),
        Some(FlagValue::Bool(true))
    );
    assert_eq!(
        runtime.flag_value("string-bool"),
        Some(FlagValue::Bool(true))
    );
    assert_eq!(
        runtime.flag_value("empty"),
        Some(FlagValue::Str(String::new()))
    );
    assert_eq!(
        runtime.flag_value("needs"),
        Some(FlagValue::Str("default".into()))
    );
    assert_eq!(
        runtime.flag_value("duplicate"),
        Some(FlagValue::Str("last-definition".into()))
    );
    assert_eq!(
        services.diagnostics,
        vec![
            error("Extension flag \"--needs\" requires a value".into()),
            error("Unknown options: --unknown-z, --unknown-a".into()),
        ]
    );
}

#[tokio::test]
async fn services_flags_absent_keep_defaults_and_single_unknown_uses_singular() {
    let f = Fixture::new().await;
    let factory: ExtensionFactory = Arc::new(|api| {
        api.register_flag(
            "enabled",
            None,
            FlagType::Boolean,
            Some(FlagValue::Bool(false)),
        )
    });
    let services = bounded(create_agent_session_services(f.options(vec![factory])))
        .await
        .unwrap();
    let loader = services.resource_loader.lock().unwrap();
    assert_eq!(
        loader.get_extensions().runtime.flag_value("enabled"),
        Some(FlagValue::Bool(false))
    );
    assert!(apply_extension_flag_values(&loader, None).is_empty());
    let mut values = OrderedMap::new();
    values.set("only", FlagValue::Bool(true));
    assert_eq!(
        apply_extension_flag_values(&loader, Some(&values)),
        vec![error("Unknown option: --only".into())]
    );
}

#[tokio::test]
async fn services_register_normal_then_native_and_collect_each_error_before_flags() {
    let f = Fixture::new().await;
    let native = faux_provider(FauxProviderOptions {
        provider: Some("same-provider".into()),
        ..Default::default()
    });
    let invalid = faux_provider(FauxProviderOptions {
        provider: Some("   ".into()),
        ..Default::default()
    });
    let native_provider = native.provider.clone();
    let bad_provider = invalid.provider.clone();
    let factory: ExtensionFactory = Arc::new(move |api| {
        api.register_provider(
            "broken",
            &json!({"api":"openai-completions", "models":[model_definition()]}),
        )?;
        api.register_provider("same-provider", &json!({"baseUrl":"https://services.invalid/v1", "api":"openai-completions", "models":[model_definition()]}))?;
        api.register_native_provider(&bad_provider)?;
        api.register_native_provider(&native_provider)?;
        api.register_flag("needs", None, FlagType::String, None)?;
        Ok(())
    });
    let mut options = f.options(vec![factory]);
    let mut flags = OrderedMap::new();
    flags.set("needs", FlagValue::Bool(false));
    flags.set("unknown", FlagValue::Bool(false));
    options.extension_flag_values = Some(flags);
    let services = bounded(create_agent_session_services(options))
        .await
        .unwrap();
    assert!(
        Arc::ptr_eq(
            &f.runtime.get_provider("same-provider").await.unwrap(),
            &native.provider
        ),
        "native group must override the preceding normal registration"
    );
    assert!(f.runtime.get_provider("broken").await.is_none());
    assert_eq!(services.diagnostics, vec![
        error("Extension \"<inline:services-0>\" error: Provider broken: \"baseUrl\" is required when defining custom models.".into()),
        error("Extension \"<inline:services-0>\" error: Provider id must not be empty.".into()),
        error("Extension flag \"--needs\" requires a value".into()),
        error("Unknown option: --unknown".into()),
    ]);
    let runtime = services
        .resource_loader
        .lock()
        .unwrap()
        .get_extensions()
        .runtime;
    assert!(runtime.pending_provider_registrations().is_empty());
    assert!(runtime.pending_native_provider_registrations().is_empty());
}

#[tokio::test]
async fn services_normal_extension_models_inherit_provider_api_and_url_without_full_model_fields() {
    let f = Fixture::new().await;
    let factory: ExtensionFactory = Arc::new(|api| {
        let mut definition = model_definition();
        definition["thinkingLevelMap"] = json!({"high":"reason-hard"});
        definition["compat"] = json!({"supportsStore":false});
        api.register_provider("normal-provider", &json!({"api":"openai-completions", "baseUrl":"https://services.invalid/v1", "headers":{"x-service":"test"}, "models":[definition]}))
    });
    let services = bounded(create_agent_session_services(f.options(vec![factory])))
        .await
        .unwrap();
    assert!(services.diagnostics.is_empty());
    let provider = f.runtime.get_provider("normal-provider").await.unwrap();
    let models = provider.get_models().unwrap();
    assert_eq!(
        models.len(),
        1,
        "ProviderModelConfig is not a complete Model and must not be silently dropped"
    );
    assert_eq!(models[0].provider, "normal-provider");
    assert_eq!(models[0].api, "openai-completions");
    assert_eq!(models[0].base_url, "https://services.invalid/v1");
    assert_eq!(models[0].compat, Some(json!({"supportsStore":false})));
}

#[tokio::test]
async fn services_to_real_sdk_prompt_persists_faux_response_and_routes_live_native_registration() {
    let f = Fixture::new().await;
    let faux = faux_provider(FauxProviderOptions {
        provider: Some("services-faux".into()),
        ..Default::default()
    });
    faux.set_responses(vec![faux_assistant_message(
        "services reached provider",
        FauxMessageOptions::default(),
    )
    .into()]);
    let provider = faux.provider.clone();
    let captured: Arc<Mutex<Option<ExtensionApi>>> = Arc::new(Mutex::new(None));
    let slot = captured.clone();
    let factory: ExtensionFactory = Arc::new(move |api| {
        api.register_native_provider(&provider)?;
        *slot.lock().unwrap() = Some(api.clone());
        Ok(())
    });
    let services = bounded(create_agent_session_services(f.options(vec![factory])))
        .await
        .unwrap();
    let manager = f.manager();
    let resource_loader = services.resource_loader.clone();
    let mut options = CreateAgentSessionFromServicesOptions::new(services, manager.clone());
    options.model = faux.get_model(None);
    options.no_tools = Some(NoTools::All);
    let result = bounded(create_agent_session_from_services(options))
        .await
        .unwrap();
    let session = result.session;
    assert!(Arc::ptr_eq(&manager, &session.session_manager));
    assert!(session.get_active_tool_names().is_empty());
    assert_eq!(
        session.extension_runner().create_context().cwd().unwrap(),
        f.cwd
    );
    f.settings.set_block_images(true);
    assert!(session.settings_manager.get_block_images());
    bounded(session.prompt("hello services", None))
        .await
        .unwrap();
    assert_eq!(faux.state().lock().unwrap().call_count, 1);
    assert!(manager.lock().unwrap().get_entries().iter().any(|entry| {
        matches!(entry, SessionEntry::Message(message) if matches!(&message.message, AgentMessage::Assistant(_)))
    }));
    let api = captured.lock().unwrap().clone().unwrap();
    let live = faux_provider(FauxProviderOptions {
        provider: Some("services-live-post-bind".into()),
        ..Default::default()
    });
    api.register_native_provider(&live.provider).unwrap();
    assert!(Arc::ptr_eq(
        &f.runtime
            .get_provider("services-live-post-bind")
            .await
            .unwrap(),
        &live.provider
    ));
    assert!(
        resource_loader
            .lock()
            .unwrap()
            .get_extensions()
            .runtime
            .pending_native_provider_registrations()
            .is_empty(),
        "post-bind registration must reach ModelRuntime instead of accumulating in the queue"
    );
    api.unregister_provider("services-live-post-bind").unwrap();
    assert!(f
        .runtime
        .get_provider("services-live-post-bind")
        .await
        .is_none());
    let weak = Arc::downgrade(&session);
    session.dispose();
    drop(session);
    assert!(weak.upgrade().is_none());
}

#[tokio::test]
async fn services_from_options_preserve_model_scopes_thinking_custom_tools_and_exclusions() {
    let f = Fixture::new().await;
    let faux = faux_provider(FauxProviderOptions {
        provider: Some("services-options".into()),
        ..Default::default()
    });
    f.runtime
        .register_native_provider_sync(faux.provider.clone())
        .unwrap();
    let factory: ExtensionFactory = Arc::new(|api| {
        api.register_tool(ToolDefinition::new(
            "extension_probe",
            "Extension",
            "Not executed",
            json!({"type":"object"}),
        ))
    });
    let services = bounded(create_agent_session_services(f.options(vec![factory])))
        .await
        .unwrap();
    let mut options = CreateAgentSessionFromServicesOptions::new(services, f.manager());
    let model = faux.get_model(None).unwrap();
    options.model = Some(model.clone());
    options.thinking_level = Some(ThinkingLevel::Off);
    options.scoped_models = vec![ScopedModel {
        model: model.clone(),
        thinking_level: Some(ThinkingLevel::Off),
    }];
    options.no_tools = Some(NoTools::Builtin);
    options.tools = Some(vec!["custom_probe".into(), "extension_probe".into()]);
    options.exclude_tools = Some(vec!["extension_probe".into()]);
    options.custom_tools = vec![Arc::new(ToolDefinition::new(
        "custom_probe",
        "Custom",
        "Not executed",
        json!({"type":"object"}),
    ))];
    let session = bounded(create_agent_session_from_services(options))
        .await
        .unwrap()
        .session;
    assert_eq!(session.model(), Some(model));
    assert_eq!(session.thinking_level(), ThinkingLevel::Off);
    assert_eq!(session.scoped_models().len(), 1);
    assert_eq!(session.get_active_tool_names(), vec!["custom_probe"]);
    session.dispose();
}

#[test]
fn services_pending_normal_group_observes_live_append_and_clears_only_after_callbacks() {
    let runtime = ExtensionRuntime::new();
    runtime
        .register_provider("first", &json!({}), "test")
        .unwrap();
    let mut seen = Vec::new();
    runtime.flush_pending_providers(|registration| {
        assert!(!runtime.pending_provider_registrations().is_empty());
        if registration.name == "first" {
            runtime
                .register_provider("appended", &json!({}), "test")
                .unwrap();
        }
        seen.push(registration.name);
    });
    assert_eq!(seen, vec!["first", "appended"]);
    assert!(runtime.pending_provider_registrations().is_empty());
}

#[test]
fn services_pending_normal_filter_replaces_property_not_the_active_array_iterator() {
    let runtime = ExtensionRuntime::new();
    for name in ["first", "filtered"] {
        runtime.register_provider(name, &json!({}), "test").unwrap();
    }
    let mut seen = Vec::new();
    runtime.flush_pending_providers(|registration| {
        if registration.name == "first" {
            runtime.unregister_provider("filtered").unwrap();
            runtime
                .register_provider("new-array-only", &json!({}), "test")
                .unwrap();
        }
        seen.push(registration.name);
    });
    assert_eq!(seen, vec!["first", "filtered"]);
    assert!(runtime.pending_provider_registrations().is_empty());
}

#[test]
fn services_pending_native_group_preserves_append_filter_and_clear_array_identity() {
    for filtered in [false, true] {
        let runtime = ExtensionRuntime::new();
        let providers: Vec<_> = ["first", "second", "appended"]
            .into_iter()
            .map(|id| {
                faux_provider(FauxProviderOptions {
                    provider: Some(id.into()),
                    ..Default::default()
                })
                .provider
            })
            .collect();
        runtime
            .register_native_provider(&providers[0], "test")
            .unwrap();
        runtime
            .register_native_provider(&providers[1], "test")
            .unwrap();
        let mut seen = Vec::new();
        runtime.flush_pending_native_providers(|registration| {
            assert!(!runtime.pending_native_provider_registrations().is_empty());
            if registration.provider.id() == "first" {
                if filtered {
                    runtime.unregister_provider("second").unwrap();
                }
                runtime
                    .register_native_provider(&providers[2], "test")
                    .unwrap();
            }
            seen.push(registration.provider.id().to_string());
        });
        assert_eq!(
            seen,
            if filtered {
                vec!["first", "second"]
            } else {
                vec!["first", "second", "appended"]
            }
        );
        assert!(runtime.pending_native_provider_registrations().is_empty());
    }
}

#[test]
fn provider_json_conversion_preserves_model_metadata_and_js_header_order() {
    let mut model = model_definition();
    model["api"] = json!("model-api");
    model["baseUrl"] = json!("https://model.invalid/v1");
    model["thinkingLevelMap"] = json!({"minimal":null,"high":"reason-hard"});
    model["samplingParams"] = json!({"temperature":0.25,"seed":null});
    model["compat"] = json!({"supportsStore":false,"unknownOption":{"nested":true}});
    model["headers"] =
        serde_json::from_str(r#"{"z":"last-name","10":"ten","2":"two","a":"first-name"}"#).unwrap();
    model["cost"]["tiers"] =
        json!([{"inputTokensAbove":128000,"input":2,"output":3,"cacheRead":0.5,"cacheWrite":1}]);
    let config = json!({
        "name":"Display","api":"provider-api","baseUrl":"https://provider.invalid/v1",
        "apiKey":"$EXAMPLE_KEY","authHeader":false,
        "headers":serde_json::from_str::<Value>(r#"{"z":"Z","10":"10","2":"2","a":"A"}"#).unwrap(),
        "models":[model.clone()]
    });
    let input = provider_config_from_value(&config).unwrap();
    assert_eq!(input.name.as_deref(), Some("Display"));
    assert_eq!(input.api.as_deref(), Some("provider-api"));
    assert_eq!(
        input.base_url.as_deref(),
        Some("https://provider.invalid/v1")
    );
    assert_eq!(input.api_key.as_deref(), Some("$EXAMPLE_KEY"));
    assert_eq!(input.auth_header, Some(false));
    assert_eq!(
        input.headers.unwrap(),
        vec![
            ("2".into(), "2".into()),
            ("10".into(), "10".into()),
            ("z".into(), "Z".into()),
            ("a".into(), "A".into())
        ]
    );
    let models = input.models.unwrap();
    let actual = &models[0];
    assert_eq!(models.len(), 1);
    assert_eq!(actual.id, model["id"].as_str().unwrap());
    assert_eq!(actual.api.as_deref(), Some("model-api"));
    assert_eq!(actual.base_url.as_deref(), Some("https://model.invalid/v1"));
    assert_eq!(
        actual.thinking_level_map.as_ref().unwrap().get("minimal"),
        Some(&None)
    );
    assert_eq!(
        actual.thinking_level_map.as_ref().unwrap().get("high"),
        Some(&Some("reason-hard".into()))
    );
    assert_eq!(serde_json::to_value(&actual.cost).unwrap(), model["cost"]);
    assert_eq!(
        serde_json::to_value(&actual.sampling_params).unwrap(),
        model["samplingParams"]
    );
    assert_eq!(actual.compat, Some(model["compat"].clone()));
    assert_eq!(
        actual.headers.as_ref().unwrap(),
        &vec![
            ("2".into(), "two".into()),
            ("10".into(), "ten".into()),
            ("z".into(), "last-name".into()),
            ("a".into(), "first-name".into())
        ]
    );
}

#[test]
fn provider_json_conversion_leaves_inheritance_and_explicit_null_compat_intact() {
    let mut model = model_definition();
    model["compat"] = Value::Null;
    let input = provider_config_from_value(
        &json!({"api":"openai-completions","baseUrl":"https://inherit.invalid","models":[model]}),
    )
    .unwrap();
    let model = &input.models.as_ref().unwrap()[0];
    assert!(model.api.is_none());
    assert!(model.base_url.is_none());
    assert_eq!(model.compat, Some(Value::Null));
    assert!(model.headers.is_none());
    assert!(provider_config_from_value(&json!({}))
        .unwrap()
        .models
        .is_none());
    assert_eq!(
        provider_config_from_value(&json!({"models":[]}))
            .unwrap()
            .models
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn provider_json_conversion_rejects_malformed_entries_instead_of_dropping_them() {
    let mut missing = model_definition();
    missing.as_object_mut().unwrap().remove("cost");
    let mut bad_input = model_definition();
    bad_input["input"] = json!(["audio"]);
    let mut bad_headers = model_definition();
    bad_headers["headers"] = json!({"x-invalid":false});
    for config in [
        Value::Null,
        json!({"models":false}),
        json!({"models":[model_definition(),missing]}),
        json!({"models":[bad_input]}),
        json!({"models":[bad_headers]}),
        json!({"headers":{"x-invalid":7}}),
    ] {
        let failure = provider_config_from_value(&config).unwrap_err();
        assert!(
            failure.starts_with("Invalid extension provider configuration:"),
            "{failure}"
        );
    }
}

#[tokio::test]
async fn services_malformed_provider_is_nonfatal_and_does_not_discard_later_registrations() {
    let f = Fixture::new().await;
    let factory: ExtensionFactory = Arc::new(|api| {
        api.register_provider("malformed", &json!({"api":"openai-completions","baseUrl":"https://bad.invalid","models":[{"id":"missing-fields"}]}))?;
        api.register_provider("valid-after-malformed", &json!({"api":"openai-completions","baseUrl":"https://good.invalid","models":[model_definition()]}))?;
        api.register_flag("required-value", None, FlagType::String, None)?;
        Ok(())
    });
    let mut options = f.options(vec![factory]);
    let mut flags = OrderedMap::new();
    flags.set("required-value", FlagValue::Bool(true));
    options.extension_flag_values = Some(flags);
    let services = bounded(create_agent_session_services(options))
        .await
        .unwrap();
    assert!(f.runtime.get_provider("malformed").await.is_none());
    assert_eq!(
        f.runtime
            .get_provider("valid-after-malformed")
            .await
            .unwrap()
            .get_models()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(services.diagnostics.len(), 2);
    assert!(services.diagnostics[0].message.starts_with(
        "Extension \"<inline:services-0>\" error: Invalid extension provider configuration:"
    ));
    assert_eq!(
        services.diagnostics[1],
        error("Extension flag \"--required-value\" requires a value".into())
    );
    assert!(services
        .resource_loader
        .lock()
        .unwrap()
        .get_extensions()
        .runtime
        .pending_provider_registrations()
        .is_empty());
}

#[tokio::test]
async fn services_await_real_project_trust_extension_before_returning_infrastructure() {
    use crate::coding_agent::{
        core::{
            project_trust::{resolve_project_trusted, ResolveProjectTrustedOptions},
            trust_manager::ProjectTrustStore,
        },
        extensions::types::{ExtensionMode, HandlerFn, HandlerResult, ProjectTrustContext},
    };
    let f = Fixture::new().await;
    std::fs::create_dir_all(std::path::Path::new(&f.cwd).join(".pi")).unwrap();
    std::fs::write(std::path::Path::new(&f.cwd).join(".pi/settings.json"), "{}").unwrap();
    let (release, gate) = tokio::sync::oneshot::channel::<()>();
    let gate = Mutex::new(Some(gate));
    let called = Arc::new(AtomicUsize::new(0));
    let capture = called.clone();
    let handler: HandlerFn = Arc::new(move |_, ctx| {
        assert!(!ctx.has_ui().unwrap());
        capture.fetch_add(1, Ordering::SeqCst);
        let gate = gate.lock().unwrap().take().unwrap();
        Box::pin(async move {
            gate.await.unwrap();
            Ok(Some(HandlerResult::Json(json!({"trusted":"no"}))))
        })
    });
    let factory: ExtensionFactory =
        Arc::new(move |api| api.on("project_trust", handler.clone()).map(|_| ()));
    let mut options = f.options(vec![factory]);
    let store = ProjectTrustStore::new(&f.agent_dir).unwrap();
    let ctx = ProjectTrustContext {
        cwd: f.cwd.clone(),
        mode: ExtensionMode::Print,
        has_ui: false,
        ui: None,
    };
    let context = Arc::new((store, ctx));
    options.resource_loader_reload_options = Some(ResourceLoaderReloadOptions {
        resolve_project_trust: Some(Arc::new(move |extensions| {
            let context = context.clone();
            Box::pin(async move {
                let mut resolve =
                    ResolveProjectTrustedOptions::new(&context.1.cwd, &context.0, &context.1);
                resolve.extensions_result = Some(extensions);
                resolve_project_trusted(resolve).await
            })
        })),
    });
    let mut pending = Box::pin(create_agent_session_services(options));
    fn assert_send<T: Send>(_: &T) {}
    assert_send(&pending);
    assert!(futures::poll!(pending.as_mut()).is_pending());
    assert_eq!(called.load(Ordering::SeqCst), 1);
    assert!(!f.settings.is_project_trusted());
    release.send(()).unwrap();
    let services = bounded(pending).await.unwrap();
    assert!(!services.settings_manager.is_project_trusted());
    assert_eq!(called.load(Ordering::SeqCst), 1);
    assert_eq!(
        services
            .resource_loader
            .lock()
            .unwrap()
            .get_extensions()
            .extensions
            .len(),
        1
    );
}
