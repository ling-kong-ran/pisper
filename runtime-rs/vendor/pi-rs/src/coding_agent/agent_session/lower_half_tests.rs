//! Tests for the agent-session lower half (slice W3.12), mirroring the repo's
//! oracle convention:
//!
//! 1. **Byte oracle comparisons** against
//!    `tests/fixtures/agent_session_oracle/oracle_w312.json`, captured from the real
//!    upstream TypeScript function bodies under node
//!    (`--experimental-strip-types`; see `capture_oracle_w312.mjs`). Covers
//!    `getSessionStats`, `getContextUsage`, `getUserMessagesForForking`,
//!    `getLastAssistantText`, the `navigateTree` target decision,
//!    `retryDelayMs`, and the JSONL export document bytes.
//! 2. **The upstream-reachable scenarios of
//!    `test/agent-session-auto-compaction-queue.test.ts`,
//!    `test/agent-session-retry.test.ts`, `test/agent-session-stats.test.ts`,
//!    the deterministic core of `agent-session-compaction.test.ts` /
//!    `agent-session-tree-navigation.test.ts` (the key-gated E2E flows run
//!    over the faux provider), and the session-level surface of
//!    `agent-session-runtime-events.test.ts` (`bindExtensions`/`reload`
//!    lifecycle events; the runtime-host flows stay with the
//!    agent-session-runtime slice).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::BoxFuture;
use serde_json::{json, Value};

use super::bash_executor::BashOperationsHandle;
use super::{
    AgentSession, AgentSessionConfig, AgentSessionEvent, CompactionReason, ExtensionBindings,
    NavigateTreeOptions,
};
use crate::agent_core::agent::Agent;
use crate::agent_core::types::{AgentInitialState, AgentMessage, AgentOptions, AgentTool};
use crate::ai::auth::credential_store::{CredentialStore, InMemoryCredentialStore};
use crate::ai::auth::types::{ApiKeyCredential, AuthError, AuthOperationOptions, Credential};
use crate::ai::models::faux::{FauxModelDefinition, FauxProviderOptions, FauxResponseStep};
use crate::ai::models::{
    create_models, faux_assistant_message, faux_provider, CreateModelsOptions, FauxFactoryArgs,
    FauxMessageOptions, FauxProviderHandle,
};
use crate::ai::types::content::TextContent;
use crate::ai::types::message::{
    AssistantBlock, AssistantMessage, StringOrBlocks, ToolResultMessage,
};
use crate::ai::types::primitives::{StopReason, Usage, UsageCost};
use crate::coding_agent::core::model_runtime::{CreateModelRuntimeOptions, ModelRuntime};
use crate::coding_agent::core::models_store::InMemoryCodingAgentModelsStore;
use crate::coding_agent::core::resource_loader::{
    DefaultResourceLoader, DefaultResourceLoaderOptions, InlineExtension,
};
use crate::coding_agent::core::settings_manager::{RetrySettings, SettingsManager, SettingsValue};
use crate::coding_agent::extensions::loader::{ExtensionApi, ExtensionFactory};
use crate::coding_agent::extensions::types::{ExtensionContext, HandlerFn, HandlerResult};
use crate::coding_agent::session_manager::{MessageEntry, SessionEntry, SessionManager};

const ORACLE: &str = include_str!("../../../tests/fixtures/agent_session_oracle/oracle_w312.json");

fn oracle() -> &'static serde_json::Map<String, Value> {
    static ORACLE_VALUE: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    ORACLE_VALUE
        .get_or_init(|| serde_json::from_str(ORACLE).expect("oracle_w312.json parses"))
        .as_object()
        .expect("oracle_w312.json object")
}

// ============================================================================
// Fixture (upstream: faux provider as `anthropic`, seeded api key, in-memory
// session, settings overrides)
// ============================================================================

/// Upstream `authStorage.modify("anthropic", () => api_key "test-key")`.
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

struct LowerTest {
    session: Arc<AgentSession>,
    faux: FauxProviderHandle,
    events: Arc<Mutex<Vec<AgentSessionEvent>>>,
    session_manager: Arc<Mutex<SessionManager>>,
    _dir: tempfile::TempDir,
}

impl LowerTest {
    fn call_count(&self) -> u64 {
        self.faux.state().lock().unwrap().call_count
    }

    fn recorded(&self) -> Vec<AgentSessionEvent> {
        self.events.lock().unwrap().clone()
    }
}

fn scripted(text: &str) -> FauxResponseStep {
    faux_assistant_message(text, FauxMessageOptions::default()).into()
}

fn error_step(message: &str) -> FauxResponseStep {
    faux_assistant_message(
        "",
        FauxMessageOptions {
            stop_reason: Some(StopReason::Error),
            error_message: Some(message.to_string()),
            ..FauxMessageOptions::default()
        },
    )
    .into()
}

async fn create_lower_session(
    overrides: Option<SettingsValue>,
    responses: Vec<FauxResponseStep>,
    base_tools_override: Vec<Arc<AgentTool>>,
    factories: Vec<ExtensionFactory>,
) -> LowerTest {
    let dir = tempfile::TempDir::with_prefix("pi-agent-session-w312-").unwrap();
    let cwd = dir.path().to_string_lossy().to_string();

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
    if let Some(overrides) = overrides {
        settings_manager.apply_overrides(&overrides);
    }

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
        session_manager: Arc::clone(&session_manager),
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
        base_tools_override,
        session_start_event: None,
        html_exporter: None,
        cache_warmer: None,
    })
    .expect("agent session");

    let events: Arc<Mutex<Vec<AgentSessionEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    session.subscribe(Arc::new(move |event: &AgentSessionEvent| {
        sink.lock().unwrap().push(event.clone());
    }));

    LowerTest {
        session,
        faux,
        events,
        session_manager,
        _dir: dir,
    }
}

fn keep_recent_override() -> SettingsValue {
    SettingsValue::Obj(vec![(
        "compaction".to_string(),
        SettingsValue::Obj(vec![(
            "keepRecentTokens".to_string(),
            SettingsValue::Num(1.0),
        )]),
    )])
}

fn retry_override(max_retries: i64, max_agent_delay_ms: i64) -> SettingsValue {
    SettingsValue::Obj(vec![(
        "retry".to_string(),
        SettingsValue::Obj(vec![
            ("enabled".to_string(), SettingsValue::Bool(true)),
            (
                "maxRetries".to_string(),
                SettingsValue::Num(max_retries as f64),
            ),
            ("baseDelayMs".to_string(), SettingsValue::Num(1.0)),
            (
                "maxAgentDelayMs".to_string(),
                SettingsValue::Num(max_agent_delay_ms as f64),
            ),
        ]),
    )])
}

// --- message builders (upstream test literals) ---

fn user_message(text: &str, timestamp: i64) -> AgentMessage {
    AgentMessage::User(crate::ai::types::message::UserMessage {
        content: StringOrBlocks::Text(text.to_string()),
        timestamp,
    })
}

fn text_block(text: &str) -> AssistantBlock {
    AssistantBlock::Text(TextContent {
        text: text.to_string(),
        text_signature: None,
    })
}

fn assistant_message(text: &str, input_tokens: u64, timestamp: i64) -> AgentMessage {
    AgentMessage::Assistant(AssistantMessage {
        content: vec![text_block(text)],
        api: "anthropic-messages".to_string(),
        provider: "anthropic".to_string(),
        model: "claude-4-5".to_string(),
        response_model: None,
        response_id: None,
        provider_thinking_level: None,
        diagnostics: None,
        usage: Usage {
            input: input_tokens,
            output: 0,
            cache_read: 0,
            cache_write: 0,
            cache_write_1h: None,
            reasoning: None,
            total_tokens: input_tokens,
            cost: UsageCost::default(),
        },
        stop_reason: StopReason::Stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp,
    })
}

fn error_assistant_message(error: &str, input_tokens: u64, timestamp: i64) -> AgentMessage {
    match assistant_message("", input_tokens, timestamp) {
        AgentMessage::Assistant(mut assistant) => {
            assistant.stop_reason = StopReason::Error;
            assistant.error_message = Some(error.to_string());
            AgentMessage::Assistant(assistant)
        }
        _ => unreachable!(),
    }
}

fn aborted_assistant_message(text: &str, timestamp: i64, strip_content: bool) -> AgentMessage {
    match assistant_message(text, 5, timestamp) {
        AgentMessage::Assistant(mut assistant) => {
            assistant.stop_reason = StopReason::Aborted;
            if strip_content {
                assistant.content = Vec::new();
            }
            AgentMessage::Assistant(assistant)
        }
        _ => unreachable!(),
    }
}

fn tool_result_message(usage: Usage) -> AgentMessage {
    AgentMessage::ToolResult(ToolResultMessage {
        tool_call_id: "tool-call-1".to_string(),
        tool_name: "test_tool".to_string(),
        content: vec![crate::ai::types::message::TextOrImageBlock::Text(
            TextContent {
                text: "tool result".to_string(),
                text_signature: None,
            },
        )],
        details: None,
        usage: Some(usage),
        is_error: false,
        timestamp: 1,
    })
}

fn usage_full() -> Usage {
    Usage {
        input: 10,
        output: 20,
        cache_read: 30,
        cache_write: 40,
        cache_write_1h: None,
        reasoning: None,
        total_tokens: 100,
        cost: UsageCost {
            input: 0.1,
            output: 0.2,
            cache_read: 0.3,
            cache_write: 0.4,
            total: 1.0,
        },
    }
}

fn assistant_of(message: &AgentMessage) -> AssistantMessage {
    match message {
        AgentMessage::Assistant(assistant) => assistant.clone(),
        _ => unreachable!("assistant expected"),
    }
}

/// Normalize every JSON number to f64 so the oracle's JS number rendering
/// (`3`) compares equal to serde's (`3.0`).
fn normalize_numbers(value: Value) -> Value {
    match value {
        Value::Number(number) => match number.as_f64() {
            Some(as_f64) => serde_json::Number::from_f64(as_f64)
                .map(Value::Number)
                .unwrap_or(Value::Null),
            None => Value::Null,
        },
        Value::Array(items) => Value::Array(items.into_iter().map(normalize_numbers).collect()),
        Value::Object(object) => Value::Object(
            object
                .into_iter()
                .map(|(key, value)| (key, normalize_numbers(value)))
                .collect(),
        ),
        other => other,
    }
}

fn canonical(value: &Value) -> String {
    let mut normalized = normalize_numbers(value.clone());
    // Match capture_oracle_w312.mjs canon; do not canonicalize JSONL bytes.
    normalized.sort_all_objects();
    serde_json::to_string(&normalized).unwrap()
}

fn event_tag(event: &AgentSessionEvent) -> String {
    serde_json::to_value(event)
        .ok()
        .and_then(|value| {
            value
                .get("type")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default()
}

fn find_compaction_entries(test: &LowerTest) -> Vec<SessionEntry> {
    test.session_manager
        .lock()
        .unwrap()
        .get_entries()
        .into_iter()
        .filter(|entry| matches!(entry, SessionEntry::Compaction(_)))
        .collect()
}

// ============================================================================
// Oracle comparisons (upstream agent-session.ts lower half, under node)
// ============================================================================

/// Seed the fixture's session manager with the oracle "plain" scenario entries
/// and sync agent state (upstream `syncAgentMessages`).
fn seed_plain_scenario(test: &LowerTest) {
    let mut manager = test.session_manager.lock().unwrap();
    manager.append_message(user_message("hello", 1)).unwrap();
    manager
        .append_message(assistant_message("hi", 200, 2))
        .unwrap();
    let messages = manager.build_session_context().messages;
    test.session.agent.state().messages = messages;
}

fn seed_compacted_scenario(test: &LowerTest, with_post_response: bool) {
    let mut manager = test.session_manager.lock().unwrap();
    manager.append_message(user_message("first", 1)).unwrap();
    manager
        .append_message(assistant_message("response1", 180_000, 2))
        .unwrap();
    let kept_user_id = manager.append_message(user_message("second", 3)).unwrap();
    manager
        .append_message(assistant_message("response2", 195_000, 4))
        .unwrap();
    manager
        .append_compaction("summary", Some(&kept_user_id), 195_000, None, None, None)
        .unwrap();
    manager.append_message(user_message("third", 5)).unwrap();
    if with_post_response {
        manager
            .append_message(assistant_message("response3", 25_000, 6))
            .unwrap();
    }
    let messages = manager.build_session_context().messages;
    test.session.agent.state().messages = messages;
}

fn assert_stats_matches(key: &str, actual: &super::SessionStats) {
    let mut expected: Value =
        serde_json::from_str(oracle()[key].as_str().unwrap()).expect("oracle stats object");
    // The oracle pins the session file/id; the fixture mints its own.
    if let Some(object) = expected.as_object_mut() {
        // The pinned oracle uses a disk session. Our in-memory fixture has
        // undefined sessionFile: JSON.stringify omits it, rather than null.
        match &actual.session_file {
            Some(file) => {
                object.insert("sessionFile".into(), Value::String(file.clone()));
            }
            None => {
                object.remove("sessionFile");
            }
        }
        object.insert(
            "sessionId".to_string(),
            Value::String(actual.session_id.clone()),
        );
    }
    assert_eq!(
        canonical(&serde_json::to_value(actual).unwrap()),
        canonical(&expected),
        "{key}"
    );
}

/// Upstream "exposes the current context usage alongside token totals".
#[tokio::test]
async fn stats_expose_context_usage_alongside_token_totals() {
    let test = create_lower_session(None, Vec::new(), Vec::new(), Vec::new()).await;
    seed_plain_scenario(&test);
    assert_stats_matches("stats_plain", &test.session.get_session_stats().unwrap());
}

/// Upstream "reports unknown current context usage immediately after
/// compaction".
#[tokio::test]
async fn stats_report_unknown_context_usage_after_compaction() {
    let test = create_lower_session(None, Vec::new(), Vec::new(), Vec::new()).await;
    seed_compacted_scenario(&test, false);
    assert_stats_matches(
        "stats_compacted_unknown",
        &test.session.get_session_stats().unwrap(),
    );
}

/// Upstream "uses post-compaction usage for current context instead of stale
/// kept usage".
#[tokio::test]
async fn stats_use_post_compaction_usage() {
    let test = create_lower_session(None, Vec::new(), Vec::new(), Vec::new()).await;
    seed_compacted_scenario(&test, true);
    assert_stats_matches(
        "stats_compacted_known",
        &test.session.get_session_stats().unwrap(),
    );
}

/// Upstream "includes branch summary usage in session totals",
/// "includes compaction usage in session totals", and "includes tool result
/// usage in session totals" over one fixture.
#[tokio::test]
async fn stats_include_summary_and_tool_result_usage() {
    let test = create_lower_session(None, Vec::new(), Vec::new(), Vec::new()).await;
    {
        let mut manager = test.session_manager.lock().unwrap();
        manager
            .branch_with_summary(
                None,
                "branch summary",
                None,
                Some(false),
                Some(usage_full()),
            )
            .unwrap();
        manager.append_message(user_message("hello", 1)).unwrap();
        manager
            .append_message(tool_result_message(usage_full()))
            .unwrap();
        let user_id = manager
            .get_entries()
            .iter()
            .find(|entry| match entry {
                SessionEntry::Message(message) => message.message.role() == "user",
                _ => false,
            })
            .and_then(|entry| entry.id().map(str::to_string))
            .unwrap();
        manager
            .append_compaction(
                "summary",
                Some(&user_id),
                100,
                None,
                Some(false),
                Some(usage_full()),
            )
            .unwrap();
        let messages = manager.build_session_context().messages;
        test.session.agent.state().messages = messages;
    }
    assert_stats_matches(
        "stats_summaries",
        &test.session.get_session_stats().unwrap(),
    );
}

/// Upstream "ignores zero-usage messages when checking for post-compaction
/// context usage": the estimate falls back to message estimation instead of
/// the zero usage.
#[tokio::test]
async fn stats_ignore_zero_usage_for_post_compaction_usage() {
    let test = create_lower_session(None, Vec::new(), Vec::new(), Vec::new()).await;
    {
        let mut manager = test.session_manager.lock().unwrap();
        manager.append_message(user_message("first", 1)).unwrap();
        manager
            .append_message(assistant_message("response1", 180_000, 2))
            .unwrap();
        let kept_user_id = manager.append_message(user_message("second", 3)).unwrap();
        manager
            .append_message(assistant_message("response2", 195_000, 4))
            .unwrap();
        manager
            .append_compaction("summary", Some(&kept_user_id), 195_000, None, None, None)
            .unwrap();
        manager.append_message(user_message("third", 5)).unwrap();
        manager
            .append_message(assistant_message("response3", 25_000, 6))
            .unwrap();
        manager.append_message(user_message("continue", 7)).unwrap();
        manager
            .append_message(assistant_message("partial", 0, 8))
            .unwrap();
        let messages = manager.build_session_context().messages;
        test.session.agent.state().messages = messages;
    }
    let stats = test.session.get_session_stats().unwrap();
    let usage = stats.context_usage.as_ref().expect("context usage present");
    let tokens = usage.tokens.expect("tokens known");
    assert!(tokens > 25_000);
}

/// Upstream `getUserMessagesForForking` over the compacted scenario
/// (`agent-session-branching.test.ts` selector surface).
#[tokio::test]
async fn forking_user_messages_match_oracle() {
    let test = create_lower_session(None, Vec::new(), Vec::new(), Vec::new()).await;
    seed_compacted_scenario(&test, false);

    let pairs = test.session.get_user_messages_for_forking().unwrap();
    let expected: Vec<Value> =
        serde_json::from_str(oracle()["fork_compacted"].as_str().unwrap()).expect("oracle fork");
    assert_eq!(pairs.len(), expected.len());
    for ((entry_id, text), expected) in pairs.iter().zip(expected.iter()) {
        assert_eq!(text, expected["text"].as_str().unwrap());
        assert!(!entry_id.is_empty());
    }
}

/// Upstream `getLastAssistantText` (skips aborted-empty, trims).
#[tokio::test]
async fn last_assistant_text_matches_oracle() {
    let test = create_lower_session(None, Vec::new(), Vec::new(), Vec::new()).await;
    test.session.agent.state().messages = vec![
        user_message("q", 1),
        // Whitespace-only assistant trims to undefined when last.
        assistant_message("  \n  ", 5, 2),
        aborted_assistant_message("first answer", 3, false),
        assistant_message("final answer", 5, 4),
        // Aborted with NO content is skipped.
        aborted_assistant_message("", 5, true),
        user_message("later", 6),
    ];

    // The oracle stores the returned value JSON-encoded; unwrap that string.
    let expected_last_text: Option<String> =
        serde_json::from_str(oracle()["last_assistant_text"].as_str().unwrap()).unwrap();
    assert_eq!(
        test.session.get_last_assistant_text().unwrap(),
        expected_last_text
    );

    // Empty session -> None (oracle last_assistant_text_empty).
    let empty = create_lower_session(None, Vec::new(), Vec::new(), Vec::new()).await;
    assert_eq!(empty.session.get_last_assistant_text().unwrap(), None);
}

/// The JSONL export document matches the oracle bytes for the same fixed
/// header and entries (the trailing-entry hook has no session-level caller).
#[test]
fn export_to_jsonl_matches_oracle_bytes() {
    let entry1 = MessageEntry {
        id: "e1".to_string(),
        parent_id: None,
        timestamp: "2026-01-01T00:00:00.000Z".to_string(),
        message: user_message("hello", 1),
    };
    let entry2 = MessageEntry {
        id: "e2".to_string(),
        parent_id: Some("e1".to_string()),
        timestamp: "2026-01-01T00:00:01.000Z".to_string(),
        message: assistant_message("hi", 200, 2),
    };
    let document = super::jsonl_document(
        "fixed-session-id",
        "/fixed/cwd",
        "2026-01-01T00:00:00.000Z",
        vec![SessionEntry::Message(entry1), SessionEntry::Message(entry2)],
    )
    .unwrap();
    assert_eq!(document, oracle()["jsonl_bytes"].as_str().unwrap());
}

/// A live export writes the branch re-chained from root: header, entries with
/// rewritten parent ids, newline-terminated.
#[tokio::test]
async fn export_to_jsonl_live_session_writes_branch() {
    let test = create_lower_session(None, vec![scripted("a1")], Vec::new(), Vec::new()).await;
    test.session.prompt("Hello", None).await.unwrap();

    let out_dir = tempfile::TempDir::with_prefix("pi-w312-export-").unwrap();
    let output_path = out_dir.path().join("export.jsonl");
    let written = test
        .session
        .export_to_jsonl(Some(output_path.to_string_lossy().to_string()))
        .unwrap();

    let content = std::fs::read_to_string(&written).unwrap();
    assert!(content.ends_with('\n'));
    let lines: Vec<&str> = content.trim_end().split('\n').collect();
    // [system, user, assistant] message entries + the header line.
    assert_eq!(lines.len(), 4);
    let header: Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(header["type"], json!("session"));
    assert_eq!(header["version"], json!(3));
    assert_eq!(header["id"], json!(test.session.session_id()));
    let cwd = test
        .session
        .session_manager
        .lock()
        .unwrap()
        .get_cwd()
        .to_string();
    assert_eq!(header["cwd"], json!(cwd));

    let mut expected_parent = Value::Null;
    for line in &lines[1..] {
        let entry: Value = serde_json::from_str(line).unwrap();
        assert_eq!(entry["parentId"], expected_parent);
        expected_parent = entry["id"].clone();
    }
}

/// The retry delay schedule (upstream "caps agent retry delay" regression,
/// extended with the default-cap case).
#[test]
fn retry_delay_schedule_matches_oracle() {
    let policy = |max_agent_delay_ms: i64| RetrySettings {
        enabled: true,
        max_retries: 100_000,
        base_delay_ms: 1,
        max_agent_delay_ms,
    };
    let delays: Vec<u64> = (1..=7)
        .map(|attempt| super::retry_delay_ms_from_settings(&policy(5), attempt))
        .collect();
    let expected: Vec<u64> =
        serde_json::from_str(oracle()["retry_delays"].as_str().unwrap()).unwrap();
    assert_eq!(delays, expected);

    let default_cap: Vec<u64> = [1u32, 8, 64, 4096, 65536]
        .iter()
        .map(|attempt| super::retry_delay_ms_from_settings(&policy(60_000), *attempt))
        .collect();
    let expected_default: Vec<u64> =
        serde_json::from_str(oracle()["retry_delays_default_cap"].as_str().unwrap()).unwrap();
    assert_eq!(default_cap, expected_default);
}

/// The `navigateTree` target decision (upstream agent-session.ts:3336-3351,
/// captured verbatim as `target_decision`).
#[test]
fn navigate_target_decision_matches_oracle() {
    let expected: Value =
        serde_json::from_str(oracle()["target_decision"].as_str().unwrap()).unwrap();

    // rootUser: parent null, editor text.
    let root_user = MessageEntry {
        id: "e1".to_string(),
        parent_id: None,
        timestamp: "2026-01-01T00:00:00.000Z".to_string(),
        message: user_message("hello", 1),
    };
    let (new_leaf_id, editor_text) =
        super::navigate_target_decision(&SessionEntry::Message(root_user), "e1");
    assert_eq!(new_leaf_id, None);
    assert_eq!(
        editor_text.as_deref(),
        expected["rootUser"]["editorText"].as_str()
    );

    // nestedUser: leaf moves to the parent (e2), editor text "second".
    let nested_user = MessageEntry {
        id: "e3".to_string(),
        parent_id: Some("e2".to_string()),
        timestamp: "2026-01-01T00:00:02.000Z".to_string(),
        message: user_message("second", 3),
    };
    let (new_leaf_id, editor_text) =
        super::navigate_target_decision(&SessionEntry::Message(nested_user), "e3");
    assert_eq!(
        new_leaf_id.as_deref(),
        expected["nestedUser"]["newLeafId"].as_str()
    );
    assert_eq!(
        editor_text.as_deref(),
        expected["nestedUser"]["editorText"].as_str()
    );

    // assistant target: leaf = target, no editor text (field absent upstream).
    let assistant = MessageEntry {
        id: "e2".to_string(),
        parent_id: Some("e1".to_string()),
        timestamp: "2026-01-01T00:00:01.000Z".to_string(),
        message: assistant_message("hi", 200, 2),
    };
    let (new_leaf_id, editor_text) =
        super::navigate_target_decision(&SessionEntry::Message(assistant), "e2");
    assert_eq!(
        new_leaf_id.as_deref(),
        expected["assistant"]["newLeafId"].as_str()
    );
    assert_eq!(
        editor_text.is_none(),
        expected["assistant"].get("editorText").is_none()
    );

    // customMessage: leaf = parent, editor text from the custom content.
    let custom = crate::coding_agent::session_manager::CustomMessageEntry {
        custom_type: "note".to_string(),
        content: Some(
            crate::coding_agent::core::messages::CustomMessageContent::Text(
                "custom text".to_string(),
            ),
        ),
        details: None,
        display: true,
        id: "c1".to_string(),
        parent_id: Some("e2".to_string()),
        timestamp: "2026-01-01T00:00:09.000Z".to_string(),
    };
    let (new_leaf_id, editor_text) =
        super::navigate_target_decision(&SessionEntry::CustomMessage(custom), "c1");
    assert_eq!(
        new_leaf_id.as_deref(),
        expected["customMessage"]["newLeafId"].as_str()
    );
    assert_eq!(
        editor_text.as_deref(),
        expected["customMessage"]["editorText"].as_str()
    );
}

// ============================================================================
// Compaction (deterministic core of agent-session-compaction.test.ts and the
// agent-session-auto-compaction-queue.test.ts scenarios)
// ============================================================================

/// Upstream "should trigger manual compaction via compact()" and "should emit
/// compaction events during manual compaction", over the faux summarizer.
#[tokio::test]
async fn manual_compaction_appends_entry_rebuilds_and_emits() {
    let test = create_lower_session(
        Some(keep_recent_override()),
        // The split-turn compaction makes two summarization calls (history +
        // turn prefix).
        vec![
            scripted("compacted history text"),
            scripted("turn prefix text"),
        ],
        Vec::new(),
        Vec::new(),
    )
    .await;
    {
        let mut manager = test.session_manager.lock().unwrap();
        manager.append_message(user_message("hello", 1)).unwrap();
        manager
            .append_message(assistant_message("hi", 200, 2))
            .unwrap();
        manager.append_message(user_message("more", 3)).unwrap();
        manager
            .append_message(assistant_message("ho", 200, 4))
            .unwrap();
        let messages = manager.build_session_context().messages;
        test.session.agent.state().messages = messages;
    }

    let result = test.session.compact(None).await.expect("manual compaction");

    assert!(!result["summary"].as_str().unwrap().is_empty());
    assert!(result["tokensBefore"].as_u64().unwrap() > 0);
    assert!(result["estimatedTokensAfter"].is_u64());
    assert!(result["firstKeptEntryId"].is_string());

    // Exactly one compaction entry persisted with the summary.
    let compactions = find_compaction_entries(&test);
    assert_eq!(compactions.len(), 1);
    match &compactions[0] {
        SessionEntry::Compaction(compaction) => {
            assert_eq!(compaction.summary, result["summary"].as_str().unwrap());
            assert!(compaction.tokens_before > 0);
            assert!(compaction.first_kept_entry_id.is_some());
        }
        _ => unreachable!(),
    }

    // Agent state rebuilt from the session context: the transcript now replays
    // the compaction summary as the first message (compactionSummary custom).
    let messages = test.session.messages();
    assert!(!messages.is_empty());
    assert_eq!(messages[0].role(), "compactionSummary");

    // compaction_start(manual) then compaction_end(manual, aborted false).
    let tags: Vec<String> = test
        .recorded()
        .iter()
        .filter(|event| {
            matches!(
                event_tag(event).as_str(),
                "compaction_start" | "compaction_end"
            )
        })
        .map(event_tag)
        .collect();
    assert_eq!(tags, vec!["compaction_start", "compaction_end"]);
    let recorded = test.recorded();
    let end = recorded
        .iter()
        .find(|event| matches!(event, AgentSessionEvent::CompactionEnd { .. }))
        .unwrap();
    match end {
        AgentSessionEvent::CompactionEnd {
            reason,
            aborted,
            will_retry,
            error_message,
            ..
        } => {
            assert_eq!(*reason, CompactionReason::Manual);
            assert!(!*aborted);
            assert!(!*will_retry);
            assert!(error_message.is_none());
        }
        _ => unreachable!(),
    }
}

/// Upstream "Already compacted" (trailing compaction entry).
#[tokio::test]
async fn manual_compaction_already_compacted_error() {
    let test = create_lower_session(
        Some(keep_recent_override()),
        vec![scripted("compacted")],
        Vec::new(),
        Vec::new(),
    )
    .await;
    {
        let mut manager = test.session_manager.lock().unwrap();
        let user = manager.append_message(user_message("hello", 1)).unwrap();
        manager
            .append_compaction("summary", Some(&user), 100, None, None, None)
            .unwrap();
    }
    let error = test.session.compact(None).await.unwrap_err();
    assert_eq!(error.to_string(), "Already compacted");
}

/// Upstream "Nothing to compact (session too small)".
#[tokio::test]
async fn manual_compaction_too_small_error() {
    let test =
        create_lower_session(None, vec![scripted("compacted")], Vec::new(), Vec::new()).await;
    {
        let mut manager = test.session_manager.lock().unwrap();
        manager.append_message(user_message("hello", 1)).unwrap();
    }
    let error = test.session.compact(None).await.unwrap_err();
    assert_eq!(error.to_string(), "Nothing to compact (session too small)");
}

/// An extension's `session_before_compact` cancel surfaces "Compaction
/// cancelled" and the aborted compaction_end (result null).
#[tokio::test]
async fn manual_compaction_extension_cancel() {
    let factory: ExtensionFactory = Arc::new(|api: &ExtensionApi| {
        let handler: HandlerFn = crate::coding_agent::extensions::types::sync_handler(
            |_event: &mut Value, _ctx: &ExtensionContext| {
                Ok(Some(HandlerResult::Json(json!({ "cancel": true }))))
            },
        );
        api.on("session_before_compact", handler)?;
        Ok(())
    });
    let test = create_lower_session(
        Some(keep_recent_override()),
        vec![scripted("compacted")],
        Vec::new(),
        vec![factory],
    )
    .await;
    {
        let mut manager = test.session_manager.lock().unwrap();
        manager.append_message(user_message("hello", 1)).unwrap();
        manager
            .append_message(assistant_message("hi", 200, 2))
            .unwrap();
        manager.append_message(user_message("more", 3)).unwrap();
        manager
            .append_message(assistant_message("ho", 200, 4))
            .unwrap();
    }

    let error = test.session.compact(None).await.unwrap_err();
    assert_eq!(error.to_string(), "Compaction cancelled");

    let ends: Vec<Value> = test
        .recorded()
        .iter()
        .filter(|event| matches!(event, AgentSessionEvent::CompactionEnd { .. }))
        .map(|event| serde_json::to_value(event).unwrap())
        .collect();
    assert_eq!(ends.len(), 1);
    assert_eq!(ends[0]["aborted"], json!(true));
    assert_eq!(ends[0]["result"], Value::Null);
    assert_eq!(find_compaction_entries(&test).len(), 0);
}

/// Upstream "should resume after threshold compaction when only agent-level
/// queued messages exist" (auto-compaction-queue suite).
#[tokio::test]
async fn auto_compaction_returns_true_with_agent_queued_messages() {
    let test = create_lower_session(
        Some(keep_recent_override()),
        vec![scripted("compacted")],
        Vec::new(),
        Vec::new(),
    )
    .await;
    {
        let mut manager = test.session_manager.lock().unwrap();
        manager
            .append_message(user_message("message to compact", 1))
            .unwrap();
        manager
            .append_message(assistant_message("assistant response to compact", 100, 2))
            .unwrap();
        let messages = manager.build_session_context().messages;
        test.session.agent.state().messages = messages;
    }
    // Agent-level queued message (upstream queues a custom message directly on
    // the agent's follow-up queue).
    test.session.agent.follow_up(AgentMessage::Custom(
        crate::agent_core::types::CustomAgentMessage {
            role: "custom".to_string(),
            data: serde_json::from_value(json!({
                "customType": "test",
                "content": [{ "type": "text", "text": "Queued custom" }],
                "display": false,
            }))
            .unwrap(),
        },
    ));
    assert_eq!(test.session.pending_message_count(), 0);
    assert!(test.session.agent.has_queued_messages());

    let should_continue = test
        .session
        .run_auto_compaction(CompactionReason::Threshold, false)
        .await
        .unwrap();
    assert!(should_continue);
    assert_eq!(find_compaction_entries(&test).len(), 1);
}

/// Upstream "should not compact repeatedly after overflow recovery already
/// attempted": the second check emits the failure compaction_end instead of
/// compacting again.
#[tokio::test]
async fn overflow_recovery_attempted_does_not_compact_twice() {
    let test = create_lower_session(
        Some(keep_recent_override()),
        // The split-turn compaction makes two summarization calls.
        vec![scripted("compacted"), scripted("turn prefix")],
        Vec::new(),
        Vec::new(),
    )
    .await;
    {
        let mut manager = test.session_manager.lock().unwrap();
        manager.append_message(user_message("hello", 1)).unwrap();
        manager
            .append_message(assistant_message("hi", 200, 2))
            .unwrap();
        manager.append_message(user_message("more", 3)).unwrap();
        manager
            .append_message(assistant_message("ho", 200, 4))
            .unwrap();
    }

    let now = crate::ai::now_ms();
    let first = test
        .session
        .check_compaction(
            &assistant_of(&error_assistant_message("prompt is too long", 0, now)),
            true,
        )
        .await
        .unwrap();
    assert!(first);

    // Strictly after the compaction boundary the first call appended.
    let after_first = crate::ai::now_ms() + 1;
    let second = test
        .session
        .check_compaction(
            &assistant_of(&error_assistant_message(
                "prompt is too long",
                0,
                after_first,
            )),
            true,
        )
        .await
        .unwrap();
    assert!(!second);

    // Exactly one real compaction ran; the second attempt emitted the exact
    // upstream recovery-failure message.
    assert_eq!(find_compaction_entries(&test).len(), 1);
    let failures: Vec<String> = test
        .recorded()
        .iter()
        .filter_map(|event| match event {
            AgentSessionEvent::CompactionEnd { error_message, .. } => error_message.clone(),
            _ => None,
        })
        .collect();
    assert_eq!(
        failures,
        vec![
            "Context overflow recovery failed after one compact-and-retry attempt. Try \
              reducing context or switching to a larger-context model."
                .to_string()
        ]
    );
}

/// Upstream "should ignore stale pre-compaction assistant usage on pre-prompt
/// compaction checks".
#[tokio::test]
async fn stale_pre_compaction_usage_is_ignored() {
    let test = create_lower_session(None, Vec::new(), Vec::new(), Vec::new()).await;
    let stale_timestamp = crate::ai::now_ms() - 10_000;
    {
        let mut manager = test.session_manager.lock().unwrap();
        manager
            .append_message(user_message("before compaction", stale_timestamp - 1000))
            .unwrap();
        manager
            .append_message(assistant_message(
                "large response before compaction",
                600_000,
                stale_timestamp,
            ))
            .unwrap();
        let first_kept_entry_id = manager.get_entries()[0].id().unwrap().to_string();
        manager
            .append_compaction(
                "summary",
                Some(&first_kept_entry_id),
                610_000,
                None,
                None,
                None,
            )
            .unwrap();
        manager
            .append_message(user_message(
                "session recovery payload",
                crate::ai::now_ms(),
            ))
            .unwrap();
    }

    let stale = assistant_message("large response before compaction", 600_000, stale_timestamp);
    let should_continue = test
        .session
        .check_compaction(&assistant_of(&stale), false)
        .await
        .unwrap();

    assert!(!should_continue);
    assert!(!test
        .recorded()
        .iter()
        .any(|event| event_tag(event) == "compaction_start"));
    assert_eq!(find_compaction_entries(&test).len(), 1);
}

/// Upstream "should trigger threshold compaction for error messages using last
/// successful usage".
#[tokio::test]
async fn threshold_compaction_for_error_uses_last_usage() {
    let test = create_lower_session(
        Some(keep_recent_override()),
        vec![scripted("compacted")],
        Vec::new(),
        Vec::new(),
    )
    .await;
    // Seed compactable session entries so the auto-compaction can actually
    // run once triggered (upstream spies on _runAutoCompaction instead).
    {
        let mut manager = test.session_manager.lock().unwrap();
        manager.append_message(user_message("hello", 1)).unwrap();
        manager
            .append_message(assistant_message("hi", 200, 2))
            .unwrap();
        manager.append_message(user_message("more", 3)).unwrap();
        manager
            .append_message(assistant_message("ho", 200, 4))
            .unwrap();
    }
    let threshold_tokens = 128_000u64.saturating_sub(
        crate::coding_agent::core::compaction::DEFAULT_COMPACTION_SETTINGS.reserve_tokens,
    ) + 1;
    let now = crate::ai::now_ms();
    test.session.agent.state().messages = vec![
        user_message("hello", now - 1000),
        assistant_message("large successful response", threshold_tokens, now),
        user_message("another prompt", now + 500),
        error_assistant_message("529 overloaded", 0, now + 1000),
    ];

    let error = error_assistant_message("529 overloaded", 0, now + 1000);
    let should_continue = test
        .session
        .check_compaction(&assistant_of(&error), true)
        .await
        .unwrap();
    assert!(!should_continue);

    // The threshold path ran a real auto-compaction (upstream
    // `toHaveBeenCalledWith("threshold", false)`).
    let starts: Vec<Value> = test
        .recorded()
        .iter()
        .filter(|event| event_tag(event) == "compaction_start")
        .map(|event| serde_json::to_value(event).unwrap())
        .collect();
    assert_eq!(starts.len(), 1);
    assert_eq!(starts[0]["reason"], json!("threshold"));
}

/// Upstream "should not trigger threshold compaction for error messages when
/// no prior usage exists".
#[tokio::test]
async fn no_threshold_compaction_without_prior_usage() {
    let test = create_lower_session(None, Vec::new(), Vec::new(), Vec::new()).await;
    let now = crate::ai::now_ms();
    test.session.agent.state().messages = vec![
        user_message("hello", now - 1000),
        error_assistant_message("529 overloaded", 0, now),
    ];

    let error = error_assistant_message("529 overloaded", 0, now);
    let should_continue = test
        .session
        .check_compaction(&assistant_of(&error), true)
        .await
        .unwrap();
    assert!(!should_continue);
    assert!(!test
        .recorded()
        .iter()
        .any(|event| event_tag(event) == "compaction_start"));
}

/// Upstream "should not trigger threshold compaction for error messages when
/// only kept pre-compaction usage exists".
#[tokio::test]
async fn no_threshold_compaction_with_only_kept_pre_compaction_usage() {
    let test = create_lower_session(None, Vec::new(), Vec::new(), Vec::new()).await;
    let pre_compaction_timestamp = crate::ai::now_ms() - 10_000;
    {
        let mut manager = test.session_manager.lock().unwrap();
        manager
            .append_message(user_message(
                "before compaction",
                pre_compaction_timestamp - 1000,
            ))
            .unwrap();
        manager
            .append_message(assistant_message(
                "kept response from before compaction",
                190_000,
                pre_compaction_timestamp,
            ))
            .unwrap();
        let first_kept_entry_id = manager.get_entries()[0].id().unwrap().to_string();
        manager
            .append_compaction(
                "summary",
                Some(&first_kept_entry_id),
                190_000,
                None,
                None,
                None,
            )
            .unwrap();
    }
    let now = crate::ai::now_ms();
    test.session.agent.state().messages = vec![
        user_message("kept user msg", pre_compaction_timestamp - 1000),
        assistant_message(
            "kept response from before compaction",
            190_000,
            pre_compaction_timestamp,
        ),
        user_message("new prompt", now - 500),
        error_assistant_message("529 overloaded", 0, now),
    ];

    let error = error_assistant_message("529 overloaded", 0, now);
    let should_continue = test
        .session
        .check_compaction(&assistant_of(&error), true)
        .await
        .unwrap();
    assert!(!should_continue);
    assert!(!test
        .recorded()
        .iter()
        .any(|event| event_tag(event) == "compaction_start"));
}

// ============================================================================
// Auto-retry (agent-session-retry.test.ts)
// ============================================================================

fn retry_event_strings(events: &[AgentSessionEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            AgentSessionEvent::AutoRetryStart { attempt, .. } => Some(format!("start:{attempt}")),
            AgentSessionEvent::AutoRetryEnd { success, .. } => {
                Some(format!("end:success={success}"))
            }
            _ => None,
        })
        .collect()
}

/// Upstream "retries after a transient error and succeeds".
#[tokio::test]
async fn retries_after_transient_error_and_succeeds() {
    let test = create_lower_session(
        Some(retry_override(3, 60_000)),
        vec![error_step("overloaded_error"), scripted("Success")],
        Vec::new(),
        Vec::new(),
    )
    .await;

    test.session.prompt("Test", None).await.unwrap();

    assert_eq!(test.call_count(), 2);
    assert_eq!(
        retry_event_strings(&test.recorded()),
        vec!["start:1", "end:success=true"]
    );
    assert!(!test.session.is_retrying());
}

/// Upstream "exhausts max retries and emits failure".
#[tokio::test]
async fn exhausts_max_retries_and_emits_failure() {
    let responses: Vec<FauxResponseStep> = (0..3).map(|_| error_step("overloaded_error")).collect();
    let test = create_lower_session(
        Some(retry_override(2, 60_000)),
        responses,
        Vec::new(),
        Vec::new(),
    )
    .await;

    test.session.prompt("Test", None).await.unwrap();

    assert_eq!(test.call_count(), 3);
    let retry_events = retry_event_strings(&test.recorded());
    assert!(retry_events.contains(&"start:1".to_string()));
    assert!(retry_events.contains(&"start:2".to_string()));
    assert!(retry_events.contains(&"end:success=false".to_string()));
    assert!(!test.session.is_retrying());
}

/// Upstream "caps agent retry delay" (regression #8826): delays follow
/// baseDelayMs * 2^(attempt-1) capped by maxAgentDelayMs.
#[tokio::test]
async fn caps_agent_retry_delay() {
    let responses: Vec<FauxResponseStep> = (0..4).map(|_| error_step("overloaded_error")).collect();
    let test = create_lower_session(
        Some(retry_override(5, 5)),
        responses,
        Vec::new(),
        Vec::new(),
    )
    .await;
    test.session.prompt("Test", None).await.unwrap();

    let delays: Vec<u64> = test
        .recorded()
        .iter()
        .filter_map(|event| match event {
            AgentSessionEvent::AutoRetryStart { delay_ms, .. } => Some(*delay_ms),
            _ => None,
        })
        .collect();
    assert_eq!(delays, vec![1, 2, 4, 5]);
}

/// Upstream "retries provider network_error failures".
#[tokio::test]
async fn retries_provider_network_error_failures() {
    let test = create_lower_session(
        Some(retry_override(3, 60_000)),
        vec![
            error_step("Provider finish_reason: network_error"),
            scripted("Recovered after retry"),
        ],
        Vec::new(),
        Vec::new(),
    )
    .await;

    test.session.prompt("Test", None).await.unwrap();

    assert_eq!(test.call_count(), 2);
    assert_eq!(
        retry_event_strings(&test.recorded()),
        vec!["start:1", "end:success=true"]
    );
}

/// Multiple retries settle before returning, and the following prompt works.
#[tokio::test]
async fn retry_settles_full_loop_before_prompt_returns() {
    let test = create_lower_session(
        Some(retry_override(3, 60_000)),
        vec![
            error_step("overloaded_error"),
            error_step("overloaded_error"),
            scripted("Recovered after retries."),
        ],
        Vec::new(),
        Vec::new(),
    )
    .await;

    test.session.prompt("Test", None).await.unwrap();

    // prompt() returns only after the loop settled: three LLM calls (two
    // retried errors + the success), no streaming in flight, and a follow-up
    // prompt works.
    assert_eq!(test.call_count(), 3);
    assert!(!test.session.is_streaming());
    assert!(!test.session.is_retrying());
    test.session.prompt("Follow-up", None).await.unwrap();
    assert_eq!(test.call_count(), 4);
}

/// The original upstream retry + tool-use regression, now exercised through
/// the real session wrappers rather than replaced with a text-only retry.
#[tokio::test]
async fn retry_with_async_tool_waits_for_continuation_and_next_prompt() {
    use crate::agent_core::types::AgentToolResult;
    use crate::ai::models::{faux_tool_call, FauxToolCallOptions};
    use std::sync::atomic::{AtomicBool, Ordering};
    let executed = Arc::new(AtomicBool::new(false));
    let flag = executed.clone();
    let tool = Arc::new(AgentTool {
        name: "echo".into(),
        label: "Echo".into(),
        description: "Echo text back".into(),
        parameters: json!({"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}),
        constrained_sampling: None,
        prepare_arguments: None,
        replay: None,
        execution_mode: None,
        execute: Arc::new(move |_, args, _, _| {
            let flag = flag.clone();
            Box::pin(async move {
                assert_eq!(args["text"], "hello");
                tokio::time::sleep(Duration::from_millis(3)).await;
                flag.store(true, Ordering::SeqCst);
                Ok(serde_json::from_value::<AgentToolResult>(
                    json!({"content":[{"type":"text","text":"echoed"}]}),
                )
                .unwrap())
            })
        }),
    });
    let tool_response = faux_assistant_message(
        faux_tool_call(
            "echo",
            json!({"text":"hello"}),
            FauxToolCallOptions {
                id: Some("call_1".into()),
            },
        ),
        FauxMessageOptions {
            stop_reason: Some(StopReason::ToolUse),
            ..Default::default()
        },
    )
    .into();
    let test = create_lower_session(
        Some(retry_override(3, 60_000)),
        vec![
            error_step("overloaded_error"),
            tool_response,
            scripted("Final answer."),
            scripted("Follow-up answer."),
        ],
        vec![tool],
        Vec::new(),
    )
    .await;
    tokio::time::timeout(Duration::from_secs(10), test.session.prompt("Test", None))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(test.call_count(), 3);
    assert!(executed.load(Ordering::SeqCst));
    assert!(!test.session.is_streaming());
    assert!(!test.session.is_retrying());
    assert_eq!(
        test.session.get_last_assistant_text().unwrap().as_deref(),
        Some("Final answer.")
    );
    tokio::time::timeout(
        Duration::from_secs(10),
        test.session.prompt("Follow-up", None),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(test.call_count(), 4);
}

// ============================================================================
// Tree navigation (deterministic core of agent-session-tree-navigation.test.ts)
// ============================================================================

/// Drives two faux prompts to build u1 -> a1 -> u2 -> a2.
async fn seed_two_turn_conversation(test: &LowerTest) {
    test.session.prompt("First message", None).await.unwrap();
    test.session.prompt("Second message", None).await.unwrap();
}

fn first_user_entry_id(test: &LowerTest) -> String {
    test.session_manager
        .lock()
        .unwrap()
        .get_entries()
        .iter()
        .find(|entry| match entry {
            SessionEntry::Message(message) => message.message.role() == "user",
            _ => false,
        })
        .and_then(|entry| entry.id().map(str::to_string))
        .expect("user entry")
}

fn entry_ids(test: &LowerTest) -> Vec<String> {
    test.session_manager
        .lock()
        .unwrap()
        .get_entries()
        .iter()
        .filter_map(|entry| entry.id().map(str::to_string))
        .collect()
}

/// Upstream "should navigate to user message and put text in editor".
#[tokio::test]
async fn navigate_to_user_message_puts_text_in_editor() {
    let test = create_lower_session(
        None,
        vec![scripted("a1"), scripted("a2")],
        Vec::new(),
        Vec::new(),
    )
    .await;
    seed_two_turn_conversation(&test).await;

    let root_id = first_user_entry_id(&test);
    // The ported transcript persists the system prompt as the root entry
    // (upstream fixtures root the tree at the first user message), so the
    // first user entry's parent is that system entry.
    let user_parent = test
        .session_manager
        .lock()
        .unwrap()
        .get_entry(&root_id)
        .and_then(|entry| entry.parent_id().map(str::to_string));
    let result = test
        .session
        .navigate_tree(
            &root_id,
            NavigateTreeOptions {
                summarize: Some(false),
                ..NavigateTreeOptions::default()
            },
        )
        .await
        .unwrap();

    assert!(!result.cancelled);
    assert_eq!(result.editor_text.as_deref(), Some("First message"));
    // The leaf moved to the user entry's parent (null when the user entry
    // roots the tree, as upstream asserts).
    assert_eq!(
        test.session_manager.lock().unwrap().get_leaf_id(),
        user_parent.as_deref()
    );
}

/// Upstream "should navigate to non-user message without editor text".
#[tokio::test]
async fn navigate_to_non_user_message_selects_the_node() {
    let test = create_lower_session(
        None,
        vec![scripted("a1"), scripted("more")],
        Vec::new(),
        Vec::new(),
    )
    .await;
    test.session.prompt("Hello", None).await.unwrap();

    let assistant_id = test
        .session_manager
        .lock()
        .unwrap()
        .get_entries()
        .iter()
        .find(|entry| match entry {
            SessionEntry::Message(message) => message.message.role() == "assistant",
            _ => false,
        })
        .and_then(|entry| entry.id().map(str::to_string))
        .expect("assistant entry");

    let result = test
        .session
        .navigate_tree(
            &assistant_id,
            NavigateTreeOptions {
                summarize: Some(false),
                ..NavigateTreeOptions::default()
            },
        )
        .await
        .unwrap();

    assert!(!result.cancelled);
    assert!(result.editor_text.is_none());
    assert_eq!(
        test.session_manager.lock().unwrap().get_leaf_id(),
        Some(assistant_id.as_str())
    );
}

/// Upstream "should not create summary when navigating without summarize
/// option".
#[tokio::test]
async fn navigate_without_summary_creates_no_entries() {
    let test = create_lower_session(
        None,
        vec![scripted("a1"), scripted("a2")],
        Vec::new(),
        Vec::new(),
    )
    .await;
    seed_two_turn_conversation(&test).await;

    let before = entry_ids(&test);
    let root_id = first_user_entry_id(&test);
    let result = test
        .session
        .navigate_tree(
            &root_id,
            NavigateTreeOptions {
                summarize: Some(false),
                ..NavigateTreeOptions::default()
            },
        )
        .await
        .unwrap();
    assert!(!result.cancelled);
    assert_eq!(entry_ids(&test), before);
    assert!(test
        .session_manager
        .lock()
        .unwrap()
        .get_entries()
        .iter()
        .all(|entry| !matches!(entry, SessionEntry::BranchSummary(_))));
}

/// Upstream "should handle navigation to same position (no-op)".
#[tokio::test]
async fn navigate_same_position_is_a_noop() {
    let test = create_lower_session(
        None,
        vec![scripted("a1"), scripted("a2")],
        Vec::new(),
        Vec::new(),
    )
    .await;
    test.session.prompt("Hello", None).await.unwrap();
    let leaf = test
        .session_manager
        .lock()
        .unwrap()
        .get_leaf_id()
        .expect("leaf")
        .to_string();
    let entries_before = entry_ids(&test);

    let result = test
        .session
        .navigate_tree(
            &leaf,
            NavigateTreeOptions {
                summarize: Some(false),
                ..NavigateTreeOptions::default()
            },
        )
        .await
        .unwrap();
    assert!(!result.cancelled);
    assert_eq!(
        test.session_manager.lock().unwrap().get_leaf_id(),
        Some(leaf.as_str())
    );
    assert_eq!(entry_ids(&test), entries_before);
}

/// Navigating to a missing entry errors with `Entry ${id} not found`.
#[tokio::test]
async fn navigate_missing_entry_errors() {
    let test = create_lower_session(None, Vec::new(), Vec::new(), Vec::new()).await;
    let error = test
        .session
        .navigate_tree("missing-entry", NavigateTreeOptions::default())
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "Entry missing-entry not found");
}

/// Upstream "should attach summary to correct parent when navigating to nested
/// user message" (summary over the faux summarizer).
#[tokio::test]
async fn navigate_with_summary_attaches_branch_summary_at_parent() {
    let test = create_lower_session(
        None,
        vec![
            scripted("a1"),
            scripted("a2"),
            scripted("summary of the abandoned branch."),
        ],
        Vec::new(),
        Vec::new(),
    )
    .await;
    seed_two_turn_conversation(&test).await;

    // u2's parent is a1.
    let (u2_id, a1_id) = {
        let manager = test.session_manager.lock().unwrap();
        let entries = manager.get_entries();
        let user_entries: Vec<&SessionEntry> = entries
            .iter()
            .filter(|entry| match entry {
                SessionEntry::Message(message) => message.message.role() == "user",
                _ => false,
            })
            .collect();
        assert_eq!(user_entries.len(), 2);
        let u2 = user_entries[1];
        let u2_id = u2.id().unwrap().to_string();
        let a1_id = u2.parent_id().unwrap().to_string();
        (u2_id, a1_id)
    };

    let result = test
        .session
        .navigate_tree(
            &u2_id,
            NavigateTreeOptions {
                summarize: Some(true),
                ..NavigateTreeOptions::default()
            },
        )
        .await
        .unwrap();

    assert!(!result.cancelled);
    assert_eq!(result.editor_text.as_deref(), Some("Second message"));
    let summary_entry = result.summary_entry.expect("summary entry");
    match &summary_entry {
        SessionEntry::BranchSummary(summary) => {
            assert_eq!(summary.parent_id.as_deref(), Some(a1_id.as_str()));
            assert!(!summary.summary.is_empty());
            assert!(!summary.from_hook.unwrap_or(false));
        }
        _ => panic!("expected branch summary entry"),
    }

    // a1 now has two children: u2 and the summary.
    let children = test.session_manager.lock().unwrap().get_children(&a1_id);
    assert_eq!(children.len(), 2);
    let child_types: Vec<String> = children
        .iter()
        .map(|entry| match entry {
            SessionEntry::Message(_) => "message".to_string(),
            SessionEntry::BranchSummary(_) => "branch_summary".to_string(),
            _ => "other".to_string(),
        })
        .collect();
    assert!(child_types.contains(&"branch_summary".to_string()));
    assert!(child_types.contains(&"message".to_string()));

    // Leaf is the summary entry.
    let leaf = test
        .session_manager
        .lock()
        .unwrap()
        .get_leaf_id()
        .unwrap()
        .to_string();
    assert_eq!(leaf, summary_entry.id().unwrap());
}

/// An extension's `session_before_tree` cancel returns `{ cancelled: true }`
/// and leaves the session unchanged.
#[tokio::test]
async fn navigate_extension_cancel() {
    let factory: ExtensionFactory = Arc::new(|api: &ExtensionApi| {
        let handler: HandlerFn = crate::coding_agent::extensions::types::sync_handler(
            |_event: &mut Value, _ctx: &ExtensionContext| {
                Ok(Some(HandlerResult::Json(json!({ "cancel": true }))))
            },
        );
        api.on("session_before_tree", handler)?;
        Ok(())
    });
    let test = create_lower_session(
        None,
        vec![scripted("a1"), scripted("a2")],
        Vec::new(),
        vec![factory],
    )
    .await;
    seed_two_turn_conversation(&test).await;

    let entries_before = entry_ids(&test);
    let root_id = first_user_entry_id(&test);
    let result = test
        .session
        .navigate_tree(
            &root_id,
            NavigateTreeOptions {
                summarize: Some(true),
                ..NavigateTreeOptions::default()
            },
        )
        .await
        .unwrap();

    assert!(result.cancelled);
    assert!(result.summary_entry.is_none());
    assert_eq!(entry_ids(&test), entries_before);
}

/// Upstream "should handle abort during summarization": aborting the branch
/// summary returns cancelled + aborted and leaves the session unchanged.
#[tokio::test]
async fn navigate_abort_during_summarization() {
    // The summarizer's transport blocks until the branch-summary signal
    // cancels, then returns an aborted response (the upstream abort poll).
    let abort_response = FauxResponseStep::Factory(Arc::new(move |args: FauxFactoryArgs| {
        let signal = args
            .options
            .as_ref()
            .and_then(|options| options.stream.signal.clone());
        Box::pin(async move {
            if let Some(signal) = signal {
                signal.cancelled().await;
            }
            Ok(faux_assistant_message(
                "",
                FauxMessageOptions {
                    stop_reason: Some(StopReason::Aborted),
                    ..FauxMessageOptions::default()
                },
            ))
        }) as BoxFuture<'static, Result<AssistantMessage, String>>
    }));
    let test = create_lower_session(
        None,
        vec![scripted("a1"), scripted("a2"), abort_response],
        Vec::new(),
        Vec::new(),
    )
    .await;
    seed_two_turn_conversation(&test).await;

    let entries_before = entry_ids(&test);
    let leaf_before = test
        .session_manager
        .lock()
        .unwrap()
        .get_leaf_id()
        .map(str::to_string);
    let root_id = first_user_entry_id(&test);

    let navigation = {
        let session = Arc::clone(&test.session);
        tokio::spawn(async move {
            session
                .navigate_tree(
                    &root_id,
                    NavigateTreeOptions {
                        summarize: Some(true),
                        ..NavigateTreeOptions::default()
                    },
                )
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(test.session.is_compacting());
    test.session.abort_branch_summary();

    let result = navigation.await.unwrap().unwrap();
    assert!(result.cancelled);
    assert_eq!(result.aborted, Some(true));
    assert!(result.summary_entry.is_none());
    assert_eq!(entry_ids(&test), entries_before);
    assert_eq!(
        test.session_manager
            .lock()
            .unwrap()
            .get_leaf_id()
            .map(str::to_string),
        leaf_before
    );
}

// ============================================================================
// Extension lifecycle (session-level surface of agent-session-runtime-events)
// ============================================================================

/// bindExtensions emits `session_start` (startup) and `resources_discover`
/// (with the startup reason).
#[tokio::test]
async fn bind_extensions_emits_session_start_and_discovers_resources() {
    let seen: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let seen_for_handler = Arc::clone(&seen);
    let factory: ExtensionFactory = Arc::new(move |api: &ExtensionApi| {
        let seen = Arc::clone(&seen_for_handler);
        let handler: HandlerFn = crate::coding_agent::extensions::types::sync_handler(
            move |event: &mut Value, _ctx: &ExtensionContext| {
                seen.lock().unwrap().push(event.clone());
                Ok(Some(HandlerResult::Json(json!({
                    "skillPaths": [],
                    "promptPaths": [],
                    "themePaths": [],
                }))))
            },
        );
        api.on("session_start", Arc::clone(&handler))?;
        api.on("resources_discover", handler)?;
        Ok(())
    });

    let test = create_lower_session(None, Vec::new(), Vec::new(), vec![factory]).await;
    test.session
        .bind_extensions(ExtensionBindings::default())
        .await
        .unwrap();

    let events = seen.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["type"], json!("session_start"));
    assert_eq!(events[0]["reason"], json!("startup"));
    assert_eq!(events[1]["type"], json!("resources_discover"));
    assert_eq!(events[1]["reason"], json!("startup"));
}

/// reload(): `session_shutdown` then `session_start(reload)` fire on the new
/// runner, and resources are re-discovered with the reload reason.
#[tokio::test]
async fn reload_emits_shutdown_and_start_reload() {
    let seen: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let seen_for_handler = Arc::clone(&seen);
    let factory: ExtensionFactory = Arc::new(move |api: &ExtensionApi| {
        let seen = Arc::clone(&seen_for_handler);
        let handler: HandlerFn = crate::coding_agent::extensions::types::sync_handler(
            move |event: &mut Value, _ctx: &ExtensionContext| {
                seen.lock().unwrap().push(event.clone());
                Ok(None)
            },
        );
        api.on("session_shutdown", Arc::clone(&handler))?;
        api.on("session_start", handler)?;
        Ok(())
    });

    let test = create_lower_session(None, Vec::new(), Vec::new(), vec![factory]).await;
    // A mode-level binding (the error listener) makes `hasBindings` true so
    // the reload path emits session_start(reload) on the new runner.
    test.session
        .bind_extensions(ExtensionBindings {
            on_error: Some(Arc::new(
                |_error: &crate::coding_agent::extensions::types::ExtensionError| {},
            )),
            ..ExtensionBindings::default()
        })
        .await
        .unwrap();
    seen.lock().unwrap().clear();

    test.session.reload(None).await.unwrap();

    let events = seen.lock().unwrap();
    let types: Vec<String> = events
        .iter()
        .map(|event| event["type"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(types, vec!["session_shutdown", "session_start"]);
    assert_eq!(events[0]["reason"], json!("reload"));
    assert_eq!(events[1]["reason"], json!("reload"));
}

// ============================================================================
// Bash execution (executeBash/recordBashResult control flow over an injected
// operations handle)
// ============================================================================

fn scripted_operations(output: &str, exit_code: Option<i64>) -> BashOperationsHandle {
    let output = output.to_string();
    BashOperationsHandle {
        exec: Arc::new(move |_command, _cwd, callbacks| {
            let output = output.clone();
            Box::pin(async move {
                (callbacks.on_data)(&output);
                Ok(exit_code)
            })
        }),
    }
}

/// executeBash streams `bash_execution_update` events (with the caller id) and
/// records the result into agent state + session.
#[tokio::test]
async fn execute_bash_streams_and_records() {
    let test = create_lower_session(None, Vec::new(), Vec::new(), Vec::new()).await;
    let options = super::ExecuteBashOptions {
        exclude_from_context: Some(true),
        id: Some("bash-1".to_string()),
        operations: Some(scripted_operations("hello output", Some(0))),
    };

    let result = test
        .session
        .execute_bash("echo hello", None, Some(options))
        .await
        .unwrap();

    assert_eq!(result.output, "hello output");
    assert_eq!(result.exit_code, Some(0));
    assert!(!result.cancelled);

    let updates: Vec<Value> = test
        .recorded()
        .iter()
        .filter(|event| event_tag(event) == "bash_execution_update")
        .map(|event| serde_json::to_value(event).unwrap())
        .collect();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0]["id"], json!("bash-1"));
    assert_eq!(updates[0]["delta"], json!("hello output"));

    // Recorded in agent state (role bashExecution) and persisted.
    let messages = test.session.messages();
    assert_eq!(messages.last().unwrap().role(), "bashExecution");
    assert_eq!(
        test.session_manager
            .lock()
            .unwrap()
            .get_entries()
            .iter()
            .filter(|entry| match entry {
                SessionEntry::Message(message) => message.message.role() == "bashExecution",
                _ => false,
            })
            .count(),
        1
    );
}

/// executeBash applies the configured shell command prefix.
#[tokio::test]
async fn execute_bash_applies_command_prefix() {
    let test = create_lower_session(
        Some(SettingsValue::Obj(vec![(
            "shellCommandPrefix".to_string(),
            SettingsValue::Str("echo PREFIXED;".to_string()),
        )])),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    )
    .await;

    let seen_command: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let sink = Arc::clone(&seen_command);
    let operations = BashOperationsHandle {
        exec: Arc::new(move |command, _cwd, callbacks| {
            *sink.lock().unwrap() = Some(command.clone());
            Box::pin(async move {
                (callbacks.on_data)("ok");
                Ok(Some(0))
            })
        }),
    };
    test.session
        .execute_bash(
            "whoami",
            None,
            Some(super::ExecuteBashOptions {
                operations: Some(operations),
                ..super::ExecuteBashOptions::default()
            }),
        )
        .await
        .unwrap();
    assert_eq!(
        seen_command.lock().unwrap().as_deref(),
        Some("echo PREFIXED;\nwhoami")
    );
}

/// recordBashResult defers while streaming and flushes after the run
/// (upstream `_flushPendingBashMessages` ordering).
#[tokio::test]
async fn record_bash_result_defers_while_streaming() {
    // Gate-held response keeps the agent streaming.
    let gate = Arc::new(tokio::sync::Notify::new());
    let gate_step: FauxResponseStep = {
        let gate = Arc::clone(&gate);
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
        }))
    };
    let test = create_lower_session(None, vec![gate_step], Vec::new(), Vec::new()).await;
    let prompt = {
        let session = Arc::clone(&test.session);
        tokio::spawn(async move { session.prompt("long running", None).await })
    };
    for _ in 0..500 {
        if test.session.is_streaming() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let bash_result = crate::coding_agent::extensions::types::BashResult {
        output: "streamed".to_string(),
        exit_code: Some(0),
        cancelled: false,
        truncated: false,
        full_output_path: None,
    };
    test.session
        .record_bash_result("echo streamed", &bash_result, None);
    assert!(test.session.has_pending_bash_messages());
    assert!(!test
        .session
        .messages()
        .iter()
        .any(|message| message.role() == "bashExecution"));

    gate.notify_one();
    let _ = prompt.await;
    // The run's finally flushed the pending bash message.
    assert!(!test.session.has_pending_bash_messages());
    assert!(test
        .session
        .messages()
        .iter()
        .any(|message| message.role() == "bashExecution"));
}

#[tokio::test]
async fn async_events_compact_action_waits_for_handler_before_error_callback() {
    use crate::coding_agent::extensions::types::CompactOptions;
    let (release, gate) = tokio::sync::oneshot::channel::<()>();
    let gate = Arc::new(Mutex::new(Some(gate)));
    let (entered, started) = tokio::sync::oneshot::channel::<()>();
    let entered = Arc::new(Mutex::new(Some(entered)));
    let factory: ExtensionFactory = Arc::new(move |api| {
        let gate = gate.clone();
        let entered = entered.clone();
        let handler: HandlerFn = Arc::new(move |_, _| {
            let gate = gate.lock().unwrap().take().unwrap();
            let entered = entered.lock().unwrap().take().unwrap();
            Box::pin(async move {
                entered.send(()).unwrap();
                gate.await.unwrap();
                Ok(Some(HandlerResult::Json(json!({"cancel":true}))))
            })
        });
        api.on("session_before_compact", handler).map(|_| ())
    });
    let test = create_lower_session(
        Some(keep_recent_override()),
        Vec::new(),
        Vec::new(),
        vec![factory],
    )
    .await;
    {
        let mut manager = test.session_manager.lock().unwrap();
        manager.append_message(user_message("hello", 1)).unwrap();
        manager
            .append_message(assistant_message("hi", 200, 2))
            .unwrap();
        manager.append_message(user_message("more", 3)).unwrap();
        manager
            .append_message(assistant_message("ho", 200, 4))
            .unwrap();
        test.session.agent.state().messages = manager.build_session_context().messages;
    }
    let (send, mut result) = tokio::sync::oneshot::channel();
    let send = Mutex::new(Some(send));
    test.session
        .extension_runner()
        .create_context()
        .compact(Some(CompactOptions {
            on_complete: Some(Arc::new(|_| {
                panic!("cancelled compaction must not succeed")
            })),
            on_error: Some(Arc::new(move |error| {
                send.lock()
                    .unwrap()
                    .take()
                    .unwrap()
                    .send(error.to_owned())
                    .unwrap();
            })),
            ..Default::default()
        }))
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), started)
        .await
        .unwrap()
        .unwrap();
    assert!(futures::poll!(&mut result).is_pending());
    assert!(find_compaction_entries(&test).is_empty());
    assert_eq!(test.call_count(), 0);
    release.send(()).unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), result)
            .await
            .unwrap()
            .unwrap(),
        "Compaction cancelled"
    );
    assert!(find_compaction_entries(&test).is_empty());
    assert_eq!(test.call_count(), 0);
    test.session.dispose();
}

/// Reload first installs fresh defaults, then replays the previous ordered Map.
/// Existing keys keep their new default positions; retained keys append in their
/// previous order. This exercises the real AgentSession reload/build path.
#[tokio::test]
async fn reload_flag_values_preserves_default_and_snapshot_order() {
    use crate::coding_agent::extensions::types::{FlagType, FlagValue};
    use std::sync::atomic::{AtomicUsize, Ordering};

    let generation = Arc::new(AtomicUsize::new(0));
    let generation_in_factory = generation.clone();
    let factory: ExtensionFactory = Arc::new(move |api: &ExtensionApi| {
        let round = generation_in_factory.fetch_add(1, Ordering::SeqCst);
        let names: &[&str] = if round == 0 {
            &["z-default", "2"]
        } else {
            &["2", "fresh", "z-default"]
        };
        for &name in names {
            api.register_flag(
                name,
                None,
                FlagType::String,
                Some(FlagValue::Str(format!("{name}:{round}"))),
            )?;
        }
        Ok(())
    });
    let test = create_lower_session(None, Vec::new(), Vec::new(), vec![factory]).await;
    let old_runner = test.session.extension_runner();
    old_runner.set_flag_value("old-extra-z", FlagValue::Str("Z".into()));
    old_runner.set_flag_value("old-extra-a", FlagValue::Str("A".into()));
    old_runner.set_flag_value("z-default", FlagValue::Str("updated".into()));
    assert_eq!(
        old_runner.get_flag_values().keys().collect::<Vec<_>>(),
        ["z-default", "2", "old-extra-z", "old-extra-a"]
    );
    test.session.reload(None).await.unwrap();
    assert_eq!(generation.load(Ordering::SeqCst), 2);
    let flags = test.session.extension_runner().get_flag_values();
    assert_eq!(
        flags.into_iter().collect::<Vec<_>>(),
        vec![
            ("2".into(), FlagValue::Str("2:0".into())),
            ("fresh".into(), FlagValue::Str("fresh:1".into())),
            ("z-default".into(), FlagValue::Str("updated".into())),
            ("old-extra-z".into(), FlagValue::Str("Z".into())),
            ("old-extra-a".into(), FlagValue::Str("A".into())),
        ]
    );
}

#[test]
fn session_stats_wire_omits_undefined_properties_not_null() {
    let mut stats = super::SessionStats::default();
    let value = serde_json::to_value(&stats).unwrap();
    assert!(value.get("sessionFile").is_none());
    assert!(value.get("contextUsage").is_none());
    stats.session_file = Some("/sessions/persisted.jsonl".into());
    assert_eq!(
        serde_json::to_value(stats).unwrap()["sessionFile"],
        "/sessions/persisted.jsonl"
    );
}
