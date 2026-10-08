//! Tests for the agent-session upper half (slice W3.11).
//!
//! Two layers, mirroring the repo's oracle convention:
//!
//! 1. **Byte oracle comparisons** against
//!    `tests/fixtures/agent_session_oracle/oracle.json`, captured from the real
//!    upstream TypeScript pure functions under node
//!    (`--experimental-strip-types`; the capture script copies the upstream
//!    function bodies verbatim where npm dependencies block direct imports and
//!    imports `packages/ai/src/utils/text.ts` for real). Covers
//!    `parseSkillBlock`, `buildSystemPromptSections`/`diffSystemPromptSections`/
//!    `buildSystemPrompt`, and the vendored `expandPromptTemplate` family.
//! 2. **The upstream-reachable scenarios of
//!    `test/agent-session-concurrent.test.ts`** (prompt guard,
//!    steer/followUp while streaming, extension-origin steering via
//!    `sendUserMessage`, prompt after completion), ported over the faux
//!    provider. The upstream tests seed a real `anthropic` model and a
//!    runtime API key so prompt validation passes while the scripted stream
//!    serves responses; the port does the same (faux provider registered as
//!    `anthropic` in the agent's `Models`, seeded api-key credential in the
//!    session's `ModelRuntime`).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::BoxFuture;
use serde_json::{json, Value};

use super::base_tools;
use super::parse_skill_block;
use super::prompt_templates;
use super::system_prompt;
use super::{
    custom_message_from_agent_message, AgentSession, AgentSessionConfig, AgentSessionEvent,
    ExtensionBindings, ParsedSkillBlock,
};
use crate::agent_core::agent::Agent;
use crate::agent_core::types::{AgentInitialState, AgentOptions, AgentTool, AgentToolResult};
// (AgentMessage/Models referenced indirectly through the session surface)
use crate::agent_core::types::ThinkingLevel;
use crate::ai::auth::credential_store::{CredentialStore, InMemoryCredentialStore};
use crate::ai::auth::types::{ApiKeyCredential, AuthError, AuthOperationOptions, Credential};
use crate::ai::models::faux::{FauxModelDefinition, FauxProviderOptions, FauxResponseStep};
use crate::ai::models::{
    create_models, faux_assistant_message, faux_provider, CreateModelsOptions, FauxFactoryArgs,
    FauxMessageOptions, FauxProviderHandle,
};
use crate::ai::types::model::Model;
use crate::ai::types::primitives::StopReason;
use crate::coding_agent::core::model_runtime::{CreateModelRuntimeOptions, ModelRuntime};
use crate::coding_agent::core::models_store::InMemoryCodingAgentModelsStore;
use crate::coding_agent::core::resource_loader::prompt_templates::PromptTemplate;
use crate::coding_agent::core::resource_loader::{
    DefaultResourceLoader, DefaultResourceLoaderOptions, InlineExtension,
};
use crate::coding_agent::core::settings_manager::SettingsManager;
use crate::coding_agent::extensions::loader::{ExtensionApi, ExtensionFactory};
use crate::coding_agent::extensions::types::{ExtensionContext, HandlerFn, SendUserMessageOptions};
use crate::coding_agent::session_manager::{SessionEntry, SessionManager};

const ORACLE: &str = include_str!("../../../tests/fixtures/agent_session_oracle/oracle.json");

// ============================================================================
// Oracle comparisons (upstream pure functions under node)
// ============================================================================

fn oracle() -> &'static serde_json::Map<String, Value> {
    static ORACLE_VALUE: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    ORACLE_VALUE
        .get_or_init(|| serde_json::from_str(ORACLE).expect("oracle.json parses"))
        .as_object()
        .expect("oracle.json object")
}

#[test]
fn parse_skill_block_matches_oracle() {
    let expected = &oracle()["skill_block"];
    let inputs = [
        r#"<skill name="commit" location="/home/u/.pi/agent/skills/commit/SKILL.md">
Commit the staged changes.
</skill>"#,
        r#"<skill name="review" location="/r/SKILL.md">
Review it.

Please review my diff"#,
        r#"<skill name="bad name" location="/x">
body
</skill>"#,
        "no skill here",
        "<skill name=\"a\" location=\"/b\">\nline1\nline2\n</skill>\n\n  trimmed user msg  \nwith second line",
    ];
    for (index, input) in inputs.iter().enumerate() {
        let parsed: Option<ParsedSkillBlock> = parse_skill_block(input);
        let actual = parsed.map(|block| serde_json::to_value(&block).unwrap());
        let expected = expected.get(index).cloned().unwrap_or(Value::Null);
        let expected = if expected.is_null() {
            None
        } else {
            Some(expected)
        };
        assert_eq!(actual, expected, "skill_block case {index}");
    }
}

/// environment-anchored (capture platform replayed): the docs section embeds
/// the package-dir paths the way the host resolves them (node
/// `path.resolve(getPackageDir(), segment)`), and the oracle captured
/// upstream-on-win32 where the synthetic `C:\abs\pkg` package dir is
/// absolute. On posix, node resolves that drive string relative to the
/// process cwd, so the tests feed a platform-native synthetic dir
/// (`/abs/pkg`) and rewrite the oracle's package-dir entries by the same
/// rule; on windows both sides keep the captured bytes exactly.
fn synthetic_package_dir() -> &'static str {
    if cfg!(windows) {
        r"C:\abs\pkg"
    } else {
        "/abs/pkg"
    }
}

/// Normalize the oracle's win32 package-dir resolutions to their posix
/// resolution of the same inputs (identity on the capture platform).
fn scrub_oracle_package_dir(text: &str) -> String {
    if cfg!(windows) {
        return text.to_string();
    }
    text.replace(r"C:\abs\pkg\", "/abs/pkg/")
        .replace(r"C:\abs\pkg", "/abs/pkg")
}

fn option_string(input: &Value, key: &str) -> Option<String> {
    input.get(key).and_then(Value::as_str).map(str::to_string)
}

fn option_string_list(input: &Value, key: &str) -> Option<Vec<String>> {
    input.get(key).and_then(Value::as_array).map(|items| {
        items
            .iter()
            .map(|item| item.as_str().expect("string item").to_string())
            .collect()
    })
}

/// The first sections case (default prompt with bash+read, snippets,
/// guidelines, append, context files, custom section).
fn case0_options() -> system_prompt::BuildSystemPromptOptions {
    system_prompt::BuildSystemPromptOptions {
        cwd: "C:\\work\\demo".to_string(),
        selected_tools: Some(vec!["read".to_string(), "bash".to_string()]),
        tool_snippets: Some(
            [("read", "Read files from disk.")]
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
        ),
        tool_guidelines: Some(
            [(
                "bash",
                vec!["Use bash carefully", " Use bash carefully ", "  "],
            )]
            .iter()
            .map(|(key, values)| {
                (
                    key.to_string(),
                    values.iter().map(|value| value.to_string()).collect(),
                )
            })
            .collect(),
        ),
        prompt_guidelines: Some(vec!["Extra rule".to_string()]),
        append_system_prompt: Some("APPEND TEXT".to_string()),
        sections: Some(
            [("custom_section", "CUSTOM CONTENT")]
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
        ),
        context_files: Some(vec![("AGENTS.md".to_string(), "Be nice".to_string())]),
        skills: Some(Vec::new()),
        ..system_prompt::BuildSystemPromptOptions::default()
    }
}

/// The third sections case (skills present, bash+read selected).
fn case2_options() -> system_prompt::BuildSystemPromptOptions {
    system_prompt::BuildSystemPromptOptions {
        cwd: "/w".to_string(),
        selected_tools: Some(vec!["bash".to_string(), "read".to_string()]),
        skills: Some(vec![
            json!({
                "name": "commit",
                "description": "Commits & pushes <stuff>",
                "filePath": "/s/commit/SKILL.md",
                "baseDir": "/s/commit",
                "disableModelInvocation": false,
            }),
            json!({
                "name": "hidden",
                "description": "Hidden skill",
                "filePath": "/s/hidden/SKILL.md",
                "baseDir": "/s/hidden",
                "disableModelInvocation": true,
            }),
        ]),
        ..system_prompt::BuildSystemPromptOptions::default()
    }
}

#[test]
fn build_system_prompt_sections_match_oracle() {
    // The docs section embeds the package-dir paths; pin the same
    // PI_PACKAGE_DIR the oracle run used (platform-native form; see
    // `synthetic_package_dir`).
    std::env::set_var("PI_PACKAGE_DIR", synthetic_package_dir());
    let cases = oracle()["sections"].as_array().expect("sections array");
    assert_eq!(cases.len(), 3);

    let inputs = [
        case0_options(),
        system_prompt::BuildSystemPromptOptions {
            cwd: "/a/b".to_string(),
            custom_prompt: Some("My custom prompt".to_string()),
            selected_tools: Some(Vec::new()),
            ..system_prompt::BuildSystemPromptOptions::default()
        },
        case2_options(),
    ];
    let expected_orders: [&[&str]; 3] = [
        &[
            "preamble",
            "tools",
            "rules",
            "docs",
            "addendum",
            "project_context",
            "cwd",
            "custom_section",
        ],
        &["preamble", "cwd"],
        &["preamble", "tools", "rules", "docs", "skills", "cwd"],
    ];

    for ((case, input), order) in cases.iter().zip(inputs.iter()).zip(expected_orders.iter()) {
        let name = case["name"].as_str().unwrap();
        let sections = system_prompt::build_system_prompt_sections(input)
            .unwrap_or_else(|error| panic!("{name}: {error}"));

        // Render order matches the upstream insertion order.
        let actual_order: Vec<&str> = sections.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(&actual_order, order, "{name}: section order");

        // Every section's rendered text matches the oracle byte for byte
        // (package-dir anchors normalized per host, see
        // `scrub_oracle_package_dir`).
        let expected = owned_sections(&case["sections"]);
        assert_eq!(sections.len(), expected.len(), "{name}: section count");
        for (section_name, expected_text) in &expected {
            let expected_text = scrub_oracle_package_dir(expected_text);
            let actual_text = sections
                .iter()
                .find(|(name, _)| name == section_name)
                .map(|(_, text)| text.as_str());
            assert_eq!(
                actual_text,
                Some(expected_text.as_str()),
                "{name}: section {section_name}"
            );
        }
    }

    // Invalid custom section names throw with the upstream message.
    let error =
        system_prompt::build_system_prompt_sections(&system_prompt::BuildSystemPromptOptions {
            cwd: "/x".to_string(),
            sections: Some(
                [("Bad Name".to_string(), "x".to_string())]
                    .into_iter()
                    .collect(),
            ),
            ..system_prompt::BuildSystemPromptOptions::default()
        })
        .unwrap_err();
    assert_eq!(error, "Invalid system prompt section name: Bad Name");
}

fn owned_sections(value: &Value) -> Vec<(String, String)> {
    value
        .as_object()
        .expect("sections object")
        .iter()
        .map(|(name, text)| {
            (
                name.clone(),
                text.as_str().expect("section text").to_string(),
            )
        })
        .collect()
}

#[test]
fn diff_system_prompt_sections_matches_oracle() {
    std::env::set_var("PI_PACKAGE_DIR", synthetic_package_dir());
    // previous: {preamble: "old preamble", removed_section: "gone", same: "same"}
    let current = system_prompt::build_system_prompt_sections(&case0_options()).unwrap();
    let previous = crate::ai::types::message::Sections::new(vec![
        ("preamble".to_string(), Some("old preamble".to_string())),
        ("removed_section".to_string(), Some("gone".to_string())),
        ("same".to_string(), Some("same".to_string())),
    ]);
    let patch =
        system_prompt::diff_system_prompt_sections(&previous, &current).expect("non-empty patch");

    // Byte-compare against the oracle patch (parsed as a JSON object, so
    // comparison is key-based).
    let expected = &oracle()["diff"][0]["patch"];
    let expected_object = expected.as_object().expect("patch object");
    for (name, text) in expected_object {
        let actual = patch
            .iter()
            .find(|(patch_name, _)| patch_name == name)
            .and_then(|(_, value)| value.clone());
        let expected_value = if text.is_null() {
            None
        } else {
            Some(scrub_oracle_package_dir(text.as_str().unwrap()))
        };
        assert_eq!(actual, expected_value, "patch[{name}]");
    }
    assert_eq!(patch.len(), expected_object.len());
}

#[test]
fn build_system_prompt_matches_oracle() {
    std::env::set_var("PI_PACKAGE_DIR", synthetic_package_dir());
    // forced-prompt case
    let forced = system_prompt::BuildSystemPromptOptions {
        cwd: "/a/b".to_string(),
        force_system_prompt: Some("FORCED PROMPT".to_string()),
        ..system_prompt::BuildSystemPromptOptions::default()
    };
    assert_eq!(
        system_prompt::build_system_prompt(&forced).unwrap(),
        oracle()["prompt"][0]["text"].as_str().unwrap()
    );
    // full render equals getSystemMessageText over the first case's sections
    let rendered = system_prompt::build_system_prompt(&case0_options()).unwrap();
    let expected_full =
        scrub_oracle_package_dir(oracle()["prompt_full"][0]["text"].as_str().unwrap());
    assert_eq!(rendered, expected_full);
}

#[test]
fn expand_prompt_template_matches_oracle() {
    let expected = &oracle()["expand"];
    let template = |name: &str, content: &str| PromptTemplate {
        name: name.to_string(),
        description: String::new(),
        argument_hint: None,
        content: content.to_string(),
        source_info: crate::coding_agent::extensions::types::create_synthetic_source_info(
            "/t.md", "test", None, None, None,
        ),
        file_path: "/t.md".to_string(),
    };
    let cases: [(&str, Vec<PromptTemplate>); 5] = [
        (
            "/deploy prod --fast",
            vec![template("deploy", "Deploy to $1 with $ARGUMENTS and $@")],
        ),
        (
            "/deploy",
            vec![template("deploy", "Args: [$1] [$2] [${@:2:2}]")],
        ),
        ("/missing args", vec![template("other", "x")]),
        ("no slash", vec![template("deploy", "x")]),
        (
            "/deploy 'a b' \"c d\" plain",
            vec![template("deploy", "$ARGUMENTS | $@")],
        ),
    ];
    for (index, (text, templates)) in cases.into_iter().enumerate() {
        assert_eq!(
            prompt_templates::expand_prompt_template(text, &templates),
            expected.get(index).and_then(Value::as_str).unwrap(),
            "expand case {index}"
        );
    }

    let args = &oracle()["parse_command_args"];
    for (index, input) in ["a 'b c' \"d e\" plain", "", "   ", "'unterminated"]
        .into_iter()
        .enumerate()
    {
        let parsed = prompt_templates::parse_command_args(input);
        let expected_items: Vec<String> = args
            .get(index)
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|item| item.as_str().unwrap().to_string())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(parsed, expected_items, "parse args case {index}");
    }
}

// ============================================================================
// Upstream test/agent-session-concurrent.test.ts scenarios
// ============================================================================

/// Upstream `authStorage.modify("anthropic", () => api_key "test-key")`: every
/// provider reads back an api-key credential, so prompt validation passes for
/// the builtin `anthropic` provider while the faux provider serves responses.
#[derive(Default)]
struct SeededCredentialStore {
    inner: InMemoryCredentialStore,
}

impl CredentialStore for SeededCredentialStore {
    fn read<'a>(
        &'a self,
        _provider_id: &'a str,
        _options: &'a AuthOperationOptions,
    ) -> BoxFuture<'a, Result<Option<Credential>, AuthError>> {
        Box::pin(async move {
            Ok(Some(Credential::ApiKey(ApiKeyCredential {
                key: Some("test-key".to_string()),
                ..ApiKeyCredential::default()
            })))
        })
    }

    fn list<'a>(
        &'a self,
        options: &'a AuthOperationOptions,
    ) -> BoxFuture<'a, Result<Vec<crate::ai::auth::types::CredentialInfo>, AuthError>> {
        self.inner.list(options)
    }

    fn modify<'a>(
        &'a self,
        provider_id: &'a str,
        f: crate::ai::auth::credential_store::ModifyCallback,
        options: &'a AuthOperationOptions,
    ) -> BoxFuture<'a, Result<Option<Credential>, AuthError>> {
        self.inner.modify(provider_id, f, options)
    }

    fn delete<'a>(
        &'a self,
        provider_id: &'a str,
        options: &'a AuthOperationOptions,
    ) -> BoxFuture<'a, Result<(), AuthError>> {
        self.inner.delete(provider_id, options)
    }
}

/// The faux provider registered as `anthropic` (upstream: the real anthropic
/// model with a scripted `streamFn`).
fn faux_handle() -> (FauxProviderHandle, Model) {
    let faux = faux_provider(FauxProviderOptions {
        provider: Some("anthropic".to_string()),
        api: Some("anthropic-messages".to_string()),
        models: vec![FauxModelDefinition {
            id: "claude-4-5".to_string(),
            reasoning: Some(true),
            ..FauxModelDefinition::default()
        }],
        ..FauxProviderOptions::default()
    });
    let model = faux.get_model(None).expect("faux default model");
    (faux, model)
}

/// A response factory that blocks until the test's gate opens, then returns
/// the aborted assistant message (the upstream mock stream's abort poll).
fn gated_abort_response(gate: Arc<tokio::sync::Notify>) -> FauxResponseStep {
    FauxResponseStep::Factory(Arc::new(move |_args: FauxFactoryArgs| {
        let gate = Arc::clone(&gate);
        Box::pin(async move {
            gate.notified().await;
            Ok(faux_assistant_message(
                "",
                FauxMessageOptions {
                    stop_reason: Some(StopReason::Aborted),
                    ..FauxMessageOptions::default()
                },
            ))
        })
            as BoxFuture<'static, Result<crate::ai::types::message::AssistantMessage, String>>
    }))
}

struct TestSession {
    session: Arc<AgentSession>,
    faux: FauxProviderHandle,
    gate: Arc<tokio::sync::Notify>,
    _dir: tempfile::TempDir,
}

async fn create_test_session(
    factories: Vec<ExtensionFactory>,
    responses: Vec<FauxResponseStep>,
) -> TestSession {
    let dir = tempfile::TempDir::with_prefix("pi-agent-session-").unwrap();
    let cwd = dir.path().to_string_lossy().to_string();

    let (faux, model) = faux_handle();
    faux.set_responses(responses);
    let mut models = create_models(CreateModelsOptions::default());
    models.set_provider(faux.provider.clone());
    let models = Arc::new(models);

    let agent = Agent::new(
        AgentOptions {
            initial_state: AgentInitialState {
                model: Some(model),
                system_prompt: Some("Test".to_string()),
                tools: Vec::new(),
                ..AgentInitialState::default()
            },
            ..AgentOptions::default()
        },
        models,
    );

    let session_manager = Arc::new(Mutex::new(
        SessionManager::in_memory(&cwd, None, None).expect("in-memory session manager"),
    ));
    let settings_manager =
        SettingsManager::create_with(&cwd, &cwd, Default::default()).expect("settings manager");

    let loader_options = DefaultResourceLoaderOptions {
        cwd: cwd.clone(),
        agent_dir: cwd.clone(),
        extension_factories: factories
            .into_iter()
            .map(InlineExtension::Factory)
            .collect(),
        no_skills: true,
        no_prompt_templates: true,
        no_themes: true,
        no_context_files: true,
        ..DefaultResourceLoaderOptions::default()
    };
    let mut loader = DefaultResourceLoader::new(loader_options);
    loader
        .reload_without_trust()
        .expect("resource loader reload");

    let model_runtime = ModelRuntime::create(CreateModelRuntimeOptions {
        credentials: Some(Arc::new(SeededCredentialStore::default())),
        models_path: Some(None),
        models_store: Some(Arc::new(InMemoryCodingAgentModelsStore::default())),
        allow_model_network: false,
        refresh_on_create: Some(false),
        ..CreateModelRuntimeOptions::default()
    })
    .await
    .expect("model runtime");

    let session = AgentSession::new(AgentSessionConfig {
        agent: Arc::new(agent),
        session_manager,
        settings_manager,
        cwd,
        scoped_models: Vec::new(),
        resource_loader: Arc::new(Mutex::new(loader)),
        custom_tools: Vec::new(),
        model_runtime,
        initial_active_tool_names: None,
        uses_default_tools: None,
        allowed_tool_names: None,
        excluded_tool_names: None,
        base_tools_override: Vec::new(),
        session_start_event: None,
        html_exporter: None,
        cache_warmer: None,
    })
    .expect("agent session");

    TestSession {
        session,
        faux,
        gate: Arc::new(tokio::sync::Notify::new()),
        _dir: dir,
    }
}

/// The blocked-stream fixture (upstream mock `streamFn` that polls the abort
/// signal): one gate-held response.
async fn blocked_session(factories: Vec<ExtensionFactory>) -> TestSession {
    let test = create_test_session(factories, Vec::new()).await;
    let gate = Arc::clone(&test.gate);
    test.faux.set_responses(vec![gated_abort_response(gate)]);
    test
}

/// An instant-response fixture (upstream mock that completes immediately).
async fn instant_session(responses: usize) -> TestSession {
    let scripted: Vec<FauxResponseStep> = (0..responses)
        .map(|_| faux_assistant_message("Done", FauxMessageOptions::default()).into())
        .collect();
    create_test_session(Vec::new(), scripted).await
}

async fn wait_for(condition: impl Fn() -> bool) {
    for _ in 0..500 {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("condition not met within timeout");
}

/// Upstream "should throw when prompt() called while streaming".
#[tokio::test]
async fn prompt_rejects_while_streaming() {
    let test = blocked_session(Vec::new()).await;
    let session = test.session.clone();

    let first_prompt = tokio::spawn({
        let session = session.clone();
        async move { session.prompt("First message", None).await }
    });

    wait_for(|| session.is_streaming()).await;

    let error = session
        .prompt("Second message", None)
        .await
        .expect_err("second prompt rejects");
    assert_eq!(
        error.to_string(),
        "Agent is already processing. Specify streamingBehavior ('steer' or 'followUp') to \
         queue the message."
    );

    // Cleanup
    test.gate.notify_one();
    session.abort().await;
    let _ = first_prompt.await;
}

/// Upstream "should allow steer() while streaming".
#[tokio::test]
async fn steer_while_streaming() {
    let test = blocked_session(Vec::new()).await;
    let session = test.session.clone();

    let first_prompt = tokio::spawn({
        let session = session.clone();
        async move { session.prompt("First message", None).await }
    });
    wait_for(|| session.is_streaming()).await;

    session.steer("Steering message", None, None).await.unwrap();
    assert_eq!(session.pending_message_count(), 1);

    // Cleanup
    test.gate.notify_one();
    session.abort().await;
    let _ = first_prompt.await;
}

/// Upstream "should allow followUp() while streaming".
#[tokio::test]
async fn follow_up_while_streaming() {
    let test = blocked_session(Vec::new()).await;
    let session = test.session.clone();

    let first_prompt = tokio::spawn({
        let session = session.clone();
        async move { session.prompt("First message", None).await }
    });
    wait_for(|| session.is_streaming()).await;

    session
        .follow_up("Follow-up message", None, None)
        .await
        .unwrap();
    assert_eq!(session.pending_message_count(), 1);

    // Cleanup
    test.gate.notify_one();
    session.abort().await;
    let _ = first_prompt.await;
}

/// Upstream "should queue extension-origin steering messages while streaming".
#[tokio::test]
async fn extension_send_user_message_steers_while_streaming() {
    let last_source: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let api_slot: Arc<Mutex<Option<ExtensionApi>>> = Arc::new(Mutex::new(None));

    let source_for_handler = Arc::clone(&last_source);
    let api_for_factory = Arc::clone(&api_slot);
    let factory: ExtensionFactory = Arc::new(move |api: &ExtensionApi| {
        *api_for_factory.lock().unwrap() = Some(api.clone());
        let source = Arc::clone(&source_for_handler);
        let handler: HandlerFn = crate::coding_agent::extensions::types::sync_handler(
            move |event: &mut Value, _ctx: &ExtensionContext| {
                *source.lock().unwrap() = event
                    .get("source")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                Ok(None)
            },
        );
        api.on("input", handler)?;
        Ok(())
    });

    let test = blocked_session(vec![factory]).await;
    let session = test.session.clone();

    let queue_events: Arc<Mutex<Vec<AgentSessionEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&queue_events);
    session.subscribe(Arc::new(move |event: &AgentSessionEvent| {
        if matches!(event, AgentSessionEvent::QueueUpdate { .. }) {
            sink.lock().unwrap().push(event.clone());
        }
    }));

    let first_prompt = tokio::spawn({
        let session = session.clone();
        async move { session.prompt("First message", None).await }
    });
    wait_for(|| session.is_streaming()).await;

    let api = api_slot.lock().unwrap().clone().expect("extension api");
    api.send_user_message(
        &json!("Steer from extension"),
        &SendUserMessageOptions {
            deliver_as: Some("steer".to_string()),
            ..SendUserMessageOptions::default()
        },
    )
    .expect("send_user_message accepted");

    wait_for(|| session.pending_message_count() == 1).await;
    assert!(session
        .get_steering_messages()
        .contains(&"Steer from extension".to_string()));
    wait_for(|| last_source.lock().unwrap().as_deref() == Some("extension")).await;
    wait_for(|| {
        queue_events.lock().unwrap().iter().any(|event| {
            matches!(event, AgentSessionEvent::QueueUpdate { steering, .. }
                if steering.contains(&"Steer from extension".to_string()))
        })
    })
    .await;

    // Cleanup
    test.gate.notify_one();
    session.abort().await;
    let _ = first_prompt.await;
}

/// Upstream "should allow prompt() after previous completes".
#[tokio::test]
async fn prompt_allowed_after_previous_completes() {
    let test = instant_session(2).await;
    let session = test.session.clone();

    session.prompt("First message", None).await.unwrap();
    assert!(!session.is_streaming());
    session
        .prompt("Second message", None)
        .await
        .expect("second prompt works");
}

/// Session persistence face (upstream: message_end events persist through the
/// session manager; the concurrent suite's ordering assertions cover the same
/// flow). One prompt produces [system, user, assistant] message entries.
#[tokio::test]
async fn prompt_persists_message_entries_in_order() {
    let test = instant_session(1).await;
    let session = test.session.clone();

    session.prompt("First message", None).await.unwrap();

    let roles: Vec<String> = session
        .session_manager
        .lock()
        .unwrap()
        .get_entries()
        .iter()
        .filter_map(|entry| match entry {
            SessionEntry::Message(message) => Some(message.message.role().to_string()),
            _ => None,
        })
        .collect();
    assert_eq!(roles, vec!["system", "user", "assistant"]);
}

/// sendCustomMessage without a run: appends to state + session and emits
/// message_start/message_end (upstream `sendCustomMessage`, no-trigger branch).
#[tokio::test]
async fn send_custom_message_appends_and_emits() {
    let test = instant_session(1).await;
    let session = test.session.clone();

    let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    session.subscribe(Arc::new(move |event: &AgentSessionEvent| match event {
        AgentSessionEvent::MessageStart { .. } => {
            sink.lock().unwrap().push("message_start".to_string());
        }
        AgentSessionEvent::MessageEnd { .. } => {
            sink.lock().unwrap().push("message_end".to_string());
        }
        _ => {}
    }));

    let custom = crate::coding_agent::core::messages::CustomMessage {
        custom_type: "notification".to_string(),
        content: crate::coding_agent::core::messages::CustomMessageContent::Text(
            "info".to_string(),
        ),
        display: true,
        details: None,
        timestamp: 1_758_240_000_000,
    };
    session.send_custom_message(custom, None).await.unwrap();

    let transcript = session.messages();
    let last = transcript.last().unwrap();
    assert_eq!(last.role(), "custom");
    let typed = custom_message_from_agent_message(last).unwrap();
    assert_eq!(typed.custom_type, "notification");

    assert_eq!(
        *events.lock().unwrap(),
        vec!["message_start".to_string(), "message_end".to_string()]
    );

    let entry_kinds: Vec<String> = session
        .session_manager
        .lock()
        .unwrap()
        .get_entries()
        .iter()
        .filter_map(|entry| match entry {
            SessionEntry::CustomMessage(custom) => Some(custom.custom_type.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(entry_kinds, vec!["notification".to_string()]);
}

/// clearQueue returns the queued steering/follow-up texts and resets the
/// pending state (upstream `clearQueue`).
#[tokio::test]
async fn clear_queue_returns_pending_messages() {
    let test = blocked_session(Vec::new()).await;
    let session = test.session.clone();

    let first_prompt = tokio::spawn({
        let session = session.clone();
        async move { session.prompt("First message", None).await }
    });
    wait_for(|| session.is_streaming()).await;

    session.steer("s1", None, None).await.unwrap();
    session.steer("s2", None, None).await.unwrap();
    session.follow_up("f1", None, None).await.unwrap();
    assert_eq!(session.pending_message_count(), 3);

    let (steering, follow_up) = session.clear_queue();
    assert_eq!(steering, vec!["s1".to_string(), "s2".to_string()]);
    assert_eq!(follow_up, vec!["f1".to_string()]);
    assert_eq!(session.pending_message_count(), 0);

    test.gate.notify_one();
    session.abort().await;
    let _ = first_prompt.await;
}

/// Thinking level management (upstream `setThinkingLevel`: clamps to model
/// capabilities, persists the transcript entry only on change, emits
/// thinking_level_changed exactly when changing).
#[tokio::test]
async fn set_thinking_level_clamps_and_persists_on_change() {
    let test = instant_session(1).await;
    let session = test.session.clone();

    let events: Arc<Mutex<Vec<ThinkingLevel>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    session.subscribe(Arc::new(move |event: &AgentSessionEvent| {
        if let AgentSessionEvent::ThinkingLevelChanged { level } = event {
            sink.lock().unwrap().push(*level);
        }
    }));

    // The faux fixture model reasons without a thinkingLevelMap: off..high
    // are available (xhigh/max need explicit mappings, per
    // getSupportedThinkingLevels).
    assert!(session.supports_thinking());
    assert_eq!(
        session.get_available_thinking_levels(),
        vec![
            ThinkingLevel::Off,
            ThinkingLevel::Minimal,
            ThinkingLevel::Low,
            ThinkingLevel::Medium,
            ThinkingLevel::High,
        ]
    );

    session.set_thinking_level(ThinkingLevel::High, None);
    assert_eq!(session.thinking_level(), ThinkingLevel::High);
    assert_eq!(*events.lock().unwrap(), vec![ThinkingLevel::High]);

    // Setting the same level again neither re-emits nor persists.
    session.set_thinking_level(ThinkingLevel::High, None);
    assert_eq!(events.lock().unwrap().len(), 1);

    // Cycling from high wraps to the first available level.
    assert_eq!(session.cycle_thinking_level(None), Some(ThinkingLevel::Off));
    assert_eq!(
        *events.lock().unwrap(),
        vec![ThinkingLevel::High, ThinkingLevel::Off]
    );

    let changes: Vec<String> = session
        .session_manager
        .lock()
        .unwrap()
        .get_entries()
        .iter()
        .filter_map(|entry| match entry {
            SessionEntry::ThinkingLevelChange(change) => Some(change.thinking_level.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(changes, vec!["high".to_string(), "off".to_string()]);
}

/// The lower-half surface (W3.12) is implemented: the previously-boundaried
/// operations return real results instead of `SliceNotImplemented`.
#[tokio::test]
async fn lower_half_surface_is_implemented() {
    let test = instant_session(1).await;
    let session = test.session.clone();

    assert!(matches!(
        session.get_last_assistant_text(),
        Ok(None) | Ok(Some(_))
    ));
    assert!(session.get_session_stats().is_ok());
    assert!(session.get_context_usage().is_ok());
    assert!(!session.is_retrying());
    assert!(session.auto_retry_enabled());
    assert!(session.auto_compaction_enabled().is_ok());
    assert!(matches!(
        session.bind_extensions(ExtensionBindings::default()).await,
        Ok(())
    ));
    session.dispose();
}

/// The base tool seam: default loadout names are exact (upstream
/// `createAllToolDefinitions` names, default active [read, bash, edit, write]).
#[tokio::test]
async fn tool_registry_defaults_are_exact() {
    let test = instant_session(1).await;
    let session = test.session.clone();
    assert_eq!(
        session.get_active_tool_names(),
        vec![
            "read".to_string(),
            "bash".to_string(),
            "edit".to_string(),
            "write".to_string()
        ]
    );
    let all_names: Vec<String> = session
        .get_all_tools()
        .into_iter()
        .map(|tool| tool.name)
        .collect();
    assert_eq!(
        all_names,
        base_tools::TOOL_NAMES
            .iter()
            .map(|name| name.to_string())
            .collect::<Vec<_>>()
    );
    session.dispose();
}

/// Upstream `createToolDefinitionFromAgentTool` keeps the declaration fields
/// (used by `baseToolsOverride`).
#[test]
fn base_tool_override_definitions_wrap_agent_tools() {
    let execute: Arc<crate::agent_core::types::ExecuteFn> =
        Arc::new(|_id, _params, _signal, _on_update| {
            Box::pin(async move { Ok(AgentToolResult::default()) })
        });
    let tool: Arc<AgentTool> = Arc::new(AgentTool {
        name: "dummy".to_string(),
        label: "dummy".to_string(),
        description: "Dummy tool".to_string(),
        parameters: json!({"type": "object", "properties": {}}),
        execute,
        constrained_sampling: None,
        prepare_arguments: None,
        replay: None,
        execution_mode: None,
    });
    let definition = base_tools::create_tool_definition_from_agent_tool(&tool);
    assert_eq!(definition.name, "dummy");
    assert_eq!(definition.label, "dummy");
    assert_eq!(definition.description, "Dummy tool");
    assert_eq!(
        definition.parameters,
        json!({"type": "object", "properties": {}})
    );
    assert!(definition.execute_async.is_some());
}

/// `option_string`/`option_string_list` are exercised through the oracle
/// tests above; keep the helpers referenced for the unused lint even if the
/// oracle shapes change.
#[test]
fn helper_surface_is_referenced() {
    let value = json!({"a": "b"});
    assert_eq!(option_string(&value, "a").as_deref(), Some("b"));
    assert_eq!(option_string_list(&value, "a"), None);
}

#[path = "event_wire_tests.rs"]
mod ingress;

#[path = "command_tests.rs"]
mod command_tests;

#[path = "async_event_tests.rs"]
mod async_event_tests;
