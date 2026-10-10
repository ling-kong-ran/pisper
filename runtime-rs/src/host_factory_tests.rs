//! Exercise the production Pi factory without the vendored crate's unpublished
//! oracle fixture files. No provider network or personal profile is used.
use pi_rust::{
    ai::auth::credential_store::InMemoryCredentialStore,
    coding_agent::{
        cli::{args::Args, project_trust::AppMode},
        core::{
            agent_session_runtime::{
                create_agent_session_runtime, CreateAgentSessionRuntimeOptions,
                SwitchSessionOptions,
            },
            model_runtime::{CreateModelRuntimeOptions, ModelRuntime},
            models_store::InMemoryCodingAgentModelsStore,
            settings_manager::{SettingsManager, SettingsValue},
            tools::bash::create_bash_tool_definition,
        },
        main::runtime::{create_cli_runtime_factory, CliRuntimeFactoryOptions},
        session_manager::SessionManager,
    },
};
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        if self.0.parent() == Some(std::env::temp_dir().as_path()) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
fn seed(directory: &Path, cwd: &Path, id: &str) -> String {
    let path = directory.join(format!("{id}.jsonl"));
    let header = json!({"type":"session","version":3,"id":id,
        "timestamp":"2026-10-05T00:00:00.000Z","cwd":cwd.to_string_lossy()});
    std::fs::write(&path, format!("{header}\n")).unwrap();
    path.to_string_lossy().into_owned()
}

#[tokio::test]
async fn host_shell_definition_survives_reload_and_rebinds_to_switched_cwd() {
    let directory =
        std::env::temp_dir().join(format!("pisper-host-factory-{}", uuid::Uuid::new_v4()));
    let _fixture = Fixture(directory.clone());
    let first = directory.join("first");
    let second = directory.join("second");
    let agent = directory.join("agent");
    for path in [&first, &second, &agent] {
        std::fs::create_dir_all(path).unwrap();
    }
    std::fs::write(agent.join("AGENTS.md"), "GLOBAL_INSTRUCTIONS_SENTINEL").unwrap();
    std::fs::write(
        first.join("AGENTS.md"),
        "FIRST_PROJECT_INSTRUCTIONS_SENTINEL",
    )
    .unwrap();
    std::fs::write(
        second.join("AGENTS.md"),
        "SECOND_PROJECT_INSTRUCTIONS_SENTINEL",
    )
    .unwrap();
    for (directory, name) in [
        (&first, "first-project-skill"),
        (&second, "second-project-skill"),
    ] {
        let path = directory.join(".pisper/skills").join(name);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(
            path.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: Project skill fixture\n---\nSkill body\n"),
        )
        .unwrap();
    }
    let first_file = seed(&agent, &first, "host-first");
    let second_file = seed(&agent, &second, "host-second");
    let visited = Arc::new(Mutex::new(Vec::new()));
    let observed = visited.clone();
    let factory = create_cli_runtime_factory(CliRuntimeFactoryOptions {
        parsed: Args::default(),
        startup_cwd: first.to_string_lossy().into_owned(),
        initial_session_cwd: first.to_string_lossy().into_owned(),
        agent_dir: agent.to_string_lossy().into_owned(),
        startup_settings_manager: SettingsManager::in_memory(SettingsValue::obj(vec![])),
        app_mode: AppMode::Print,
        extension_factories: vec![
            pi_rust::coding_agent::core::resource_loader::InlineExtension::Factory(
                crate::project_resources::extension(),
            ),
        ],
        extension_module_loader: None,
        model_runtime_factory: Some(Arc::new(|_, _, signal| {
            Box::pin(async move {
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
            })
        })),
        custom_tool_factory: Some(Arc::new(move |cwd, _| {
            let observed = observed.clone();
            Box::pin(async move {
                observed.lock().unwrap().push(cwd.clone());
                let mut definition = create_bash_tool_definition(&cwd, Default::default())
                    .as_ref()
                    .clone();
                definition.label = format!("host shell at {cwd}");
                Ok(vec![Arc::new(definition)])
            })
        })),
        model_scope_warning: None,
    })
    .unwrap();
    let runtime = create_agent_session_runtime(
        factory.create_runtime,
        CreateAgentSessionRuntimeOptions {
            cwd: first.to_string_lossy().into_owned(),
            agent_dir: agent.to_string_lossy().into_owned(),
            session_manager: Arc::new(Mutex::new(
                SessionManager::open(&first_file, None, None).unwrap(),
            )),
            session_start_event: None,
            project_trust_context: None,
        },
    )
    .await
    .unwrap();
    runtime
        .session()
        .bind_extensions(crate::extension_bindings())
        .await
        .unwrap();
    let assert_cwd = |cwd: &Path| {
        let prompt = runtime.session().system_prompt();
        assert!(prompt.contains("GLOBAL_INSTRUCTIONS_SENTINEL"));
        assert_model_protocols_preserve_project_instructions(&prompt);
        let (present, absent, skill) = if cwd == first.as_path() {
            (
                "FIRST_PROJECT_INSTRUCTIONS_SENTINEL",
                "SECOND_PROJECT_INSTRUCTIONS_SENTINEL",
                "first-project-skill",
            )
        } else {
            (
                "SECOND_PROJECT_INSTRUCTIONS_SENTINEL",
                "FIRST_PROJECT_INSTRUCTIONS_SENTINEL",
                "second-project-skill",
            )
        };
        assert!(
            prompt.contains(present),
            "Project AGENTS must reach the real session system prompt"
        );
        assert!(
            !prompt.contains(absent),
            "Other workspace instructions must not leak"
        );
        assert!(
            prompt.contains(skill),
            "Project .pisper skills must reach the model; active={:?}, loaded={:?}",
            runtime.session().get_active_tool_names(),
            runtime
                .services()
                .resource_loader
                .lock()
                .unwrap()
                .get_skills()
                .skills
                .iter()
                .map(|skill| skill.name.clone())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            runtime.session().get_tool_definition("bash").unwrap().label,
            format!("host shell at {}", cwd.to_string_lossy())
        );
        assert_eq!(
            runtime
                .session()
                .get_active_tool_names()
                .iter()
                .filter(|name| name.as_str() == "bash")
                .count(),
            1
        );
    };
    assert_cwd(&first);
    runtime.session().reload(None).await.unwrap();
    assert_cwd(&first);
    assert!(
        !runtime
            .switch_session(&second_file, SwitchSessionOptions::default())
            .await
            .unwrap()
            .cancelled
    );
    runtime
        .session()
        .bind_extensions(crate::extension_bindings())
        .await
        .unwrap();
    assert_cwd(&second);
    assert!(
        !runtime
            .switch_session(&first_file, SwitchSessionOptions::default())
            .await
            .unwrap()
            .cancelled
    );
    runtime
        .session()
        .bind_extensions(crate::extension_bindings())
        .await
        .unwrap();
    assert_cwd(&first);
    assert_eq!(
        *visited.lock().unwrap(),
        vec![
            first.to_string_lossy().into_owned(),
            second.to_string_lossy().into_owned(),
            first.to_string_lossy().into_owned()
        ]
    );
    runtime.dispose().await.unwrap();
}

// 使用真实会话生成的提示构造两种供应商请求，排除“换模型就漏传项目规则”。
fn assert_model_protocols_preserve_project_instructions(prompt: &str) {
    use pi_rust::ai::{
        api::{anthropic, openai_completions},
        transcript::{normalize_context, Context},
        Model, ProviderConfig, SimpleStreamOptions,
    };
    let context = normalize_context(&Context {
        system_prompt: Some(prompt.into()),
        messages: vec![],
        tools: None,
    });
    for (id, api) in [
        ("claude-opus-5-5", "anthropic-messages"),
        ("glm-fixture", "openai-completions"),
    ] {
        let model:Model=serde_json::from_value(json!({"id":id,"name":id,"provider":"fixture","api":api,"baseUrl":"http://127.0.0.1:1","reasoning":false,"input":["text"],"contextWindow":200000,"maxTokens":1000,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0}})).unwrap();
        let cfg = ProviderConfig {
            base_url: model.base_url.clone(),
            api_key: "synthetic".into(),
            max_tokens: 1000,
        };
        let options = SimpleStreamOptions::default();
        let system = if api == "anthropic-messages" {
            let body = anthropic::build_request(
                &model,
                &cfg,
                &context,
                &anthropic::options_from_simple(&model, &context, &options),
            )
            .unwrap()
            .body;
            body["system"].to_string()
        } else {
            let body = openai_completions::build_request(
                &model,
                &cfg,
                &context,
                &options,
                &Default::default(),
            )
            .unwrap()
            .body;
            body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|m| m["role"] == "system" || m["role"] == "developer")
                .map(|m| m["content"].to_string())
                .collect::<String>()
        };
        assert!(
            system.contains("GLOBAL_INSTRUCTIONS_SENTINEL"),
            "{api} must carry project/global rules in system context"
        );
        assert!(
            system.contains("PROJECT_INSTRUCTIONS_SENTINEL"),
            "{api} must carry the selected project rules"
        );
    }
}
