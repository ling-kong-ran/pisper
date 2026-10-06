//! Port of `packages/agent/test/harness/compaction/compaction.test.ts` (the
//! executable spec for the compaction module). One disclosed substitution:
//! the oracle's `createModelsWithSimpleResponses` stub overrides
//! `completeSimple` on a shared collection; the port cannot monkey-patch, so
//! the usage-combination case drives the same values through the
//! `compactWithRequest` boundary with a scripted request closure instead —
//! the logic under test (usage addition) is unchanged.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use super::*;
use crate::agent_core::harness::compaction::utils::FileOperations;
use crate::agent_core::harness::context::background_context;
use crate::agent_core::harness::session::context::build_session_context;
use crate::agent_core::harness::session::types::Entry;
use crate::agent_core::harness::types::CompactionErrorCode;
use crate::agent_core::types::{AgentMessage, CustomAgentMessage};
use crate::ai::models::faux::{
    faux_assistant_message, faux_provider, FauxFactoryArgs, FauxMessageOptions,
    FauxModelDefinition, FauxProviderOptions, FauxResponseStep,
};
use crate::ai::models::{create_models, CreateModelsOptions, Models};
use crate::ai::retry::{RetryCallbacks, RetryPolicy};
use crate::ai::types::message::{AssistantBlock, AssistantMessage, Message, StringOrBlocks};
use crate::ai::types::model::Model;
use crate::ai::types::options::SimpleStreamOptions;
use crate::ai::types::primitives::{CacheRetention, StopReason, Usage, UsageCost};
use crate::ai::types::{TextContent, TextOrImageBlock, ThinkingContent, ToolCall, UserMessage};
use futures::future::BoxFuture;

// ---------------------------------------------------------------------------
// Fixtures (upstream test head, compaction.test.ts:43-149)
// ---------------------------------------------------------------------------

/// `createMockUsage` (compaction.test.ts:48-57).
fn mock_usage(input: u64, output: u64, cache_read: u64, cache_write: u64) -> Usage {
    Usage {
        input,
        output,
        cache_read,
        cache_write,
        cache_write_1h: None,
        reasoning: None,
        total_tokens: input + output + cache_read + cache_write,
        cost: UsageCost::default(),
    }
}

fn user_message(text: &str) -> AgentMessage {
    AgentMessage::User(UserMessage {
        content: StringOrBlocks::Text(text.to_string()),
        timestamp: crate::ai::now_ms(),
    })
}

/// `createAssistantMessage` (compaction.test.ts:67-78).
fn assistant_message(text: &str, usage: Usage) -> AssistantMessage {
    AssistantMessage {
        content: vec![AssistantBlock::Text(TextContent {
            text: text.to_string(),
            text_signature: None,
        })],
        api: "anthropic-messages".to_string(),
        provider: "anthropic".to_string(),
        model: "claude-sonnet-4-5".to_string(),
        response_model: None,
        response_id: None,
        provider_thinking_level: None,
        diagnostics: None,
        usage,
        stop_reason: StopReason::Stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: crate::ai::now_ms(),
    }
}

/// The entry/message id-and-seq counter (upstream module-level `nextId`,
/// reset per test by `beforeEach`; each Rust test owns one).
#[derive(Default)]
struct Fixture {
    next: i64,
}

impl Fixture {
    fn create_id(&mut self) -> String {
        let id = format!("entry-{}", self.next);
        self.next += 1;
        id
    }

    /// `createMessageEntry` (compaction.test.ts:80-89).
    fn message_entry(&mut self, message: AgentMessage, parent_id: Option<&str>) -> Entry {
        self.next += 1;
        Entry::Message {
            id: self.create_id(),
            parent_id: parent_id.map(str::to_string),
            seq: self.next,
            timestamp: crate::ai::now_ms(),
            message,
            terminate: None,
        }
    }

    /// `createCompactionEntry` (compaction.test.ts:91-107).
    fn compaction_entry(
        &mut self,
        summary: &str,
        parent_id: Option<&str>,
        retained_tail: Vec<AgentMessage>,
    ) -> Entry {
        self.next += 1;
        Entry::Compaction {
            id: self.create_id(),
            parent_id: parent_id.map(str::to_string),
            seq: self.next,
            timestamp: crate::ai::now_ms(),
            summary: summary.to_string(),
            retained_tail,
            tokens_before: 1234,
            details: None,
            usage: None,
            from_hook: false,
        }
    }

    /// `createCustomEntry` (compaction.test.ts:109-118).
    fn custom_entry(&mut self, custom_type: &str, parent_id: Option<&str>) -> Entry {
        self.next += 1;
        Entry::Custom {
            id: self.create_id(),
            parent_id: parent_id.map(str::to_string),
            seq: self.next,
            timestamp: crate::ai::now_ms(),
            custom_type: custom_type.to_string(),
            data: None,
        }
    }
}

/// Unique-per-call faux provider suffix (upstream shared `fauxCount`); the
/// counter is global so coexisting fakes never collide across tests.
static FAUX_COUNT: AtomicUsize = AtomicUsize::new(0);

/// `createFauxModel` (compaction.test.ts:124-138): one scripted provider with
/// a single model, registered into a fresh `Models` collection.
fn create_faux_model(
    reasoning: bool,
    max_tokens: u64,
) -> (
    Arc<Models>,
    crate::ai::models::faux::FauxProviderHandle,
    Model,
) {
    let count = FAUX_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
    let faux = faux_provider(FauxProviderOptions {
        provider: Some(format!("faux-{count}")),
        models: vec![FauxModelDefinition {
            id: if reasoning {
                "reasoning-model".to_string()
            } else {
                "non-reasoning-model".to_string()
            },
            reasoning: Some(reasoning),
            context_window: Some(200_000),
            max_tokens: Some(max_tokens),
            ..FauxModelDefinition::default()
        }],
        ..FauxProviderOptions::default()
    });
    let model = faux.get_model(None).unwrap();
    let mut models = create_models(CreateModelsOptions::default());
    models.set_provider(Arc::clone(&faux.provider));
    (Arc::new(models), faux, model)
}

/// `setResponses` with factories that record the observed request options.
fn factory_recording_options(
    faux: &crate::ai::models::faux::FauxProviderHandle,
    seen: Arc<Mutex<Vec<Option<SimpleStreamOptions>>>>,
    text: &str,
) {
    faux.set_responses(vec![FauxResponseStep::Factory(record_options_factory(
        seen, text,
    ))]);
}

fn record_options_factory(
    seen: Arc<Mutex<Vec<Option<SimpleStreamOptions>>>>,
    text: &str,
) -> crate::ai::models::faux::FauxResponseFactory {
    let text = text.to_string();
    Arc::new(move |args: FauxFactoryArgs| {
        let seen = Arc::clone(&seen);
        let text = text.clone();
        Box::pin(async move {
            seen.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(args.options);
            Ok(faux_assistant_message(text, FauxMessageOptions::default()))
        }) as BoxFuture<'static, Result<AssistantMessage, String>>
    })
}

/// The compaction settings the plain generation oracles use.
fn generation_settings() -> CompactionSettings {
    CompactionSettings {
        enabled: true,
        reserve_tokens: 2_000,
        keep_recent_tokens: 20,
    }
}

fn preparation_with(
    messages: &[AgentMessage],
    is_split_turn: bool,
    settings: CompactionSettings,
) -> CompactionPreparation {
    CompactionPreparation {
        messages_to_summarize: messages.to_vec(),
        turn_prefix_messages: if is_split_turn {
            messages.to_vec()
        } else {
            Vec::new()
        },
        retained_tail: messages.to_vec(),
        is_split_turn,
        tokens_before: 100,
        previous_summary: None,
        file_ops: FileOperations::new(),
        settings,
    }
}

fn empty_file_ops() -> FileOperations {
    FileOperations::new()
}

// ---------------------------------------------------------------------------
// Pure helpers (upstream "calculates total context tokens" through
// "does not prepare compaction")
// ---------------------------------------------------------------------------

/// Oracle "calculates total context tokens from usage".
#[test]
fn calculates_total_context_tokens_from_usage() {
    assert_eq!(
        calculate_context_tokens(mock_usage(1000, 500, 200, 100)),
        1800
    );
    assert_eq!(calculate_context_tokens(mock_usage(0, 0, 0, 0)), 0);
}

/// Oracle "checks compaction threshold".
#[test]
fn checks_compaction_threshold() {
    let settings = CompactionSettings {
        enabled: true,
        reserve_tokens: 10_000,
        keep_recent_tokens: 20_000,
    };
    assert!(should_compact(95_000, 100_000, settings));
    assert!(!should_compact(89_000, 100_000, settings));
    assert!(!should_compact(
        95_000,
        100_000,
        CompactionSettings {
            enabled: false,
            ..settings
        }
    ));
}

/// Oracle "finds a cut point based on token differences".
#[test]
fn finds_a_cut_point_based_on_token_differences() {
    let mut fixture = Fixture::default();
    let mut entries: Vec<Entry> = Vec::new();
    let mut parent_id: Option<String> = None;
    for i in 0..10 {
        let user = fixture.message_entry(user_message(&format!("User {i}")), parent_id.as_deref());
        let assistant = fixture.message_entry(
            AgentMessage::Assistant(assistant_message(
                &format!("Assistant {i}"),
                mock_usage(0, 100, ((i + 1) * 1000) as u64, 0),
            )),
            Some(user.id()),
        );
        parent_id = Some(assistant.id().to_string());
        entries.push(user);
        entries.push(assistant);
    }

    let result = find_cut_point(&entries, 0, entries.len(), 2500);
    assert!(matches!(
        entries.get(result.first_kept_entry_index),
        Some(Entry::Message { .. })
    ));
}

/// Oracle "covers cut-point and turn-start edge cases".
#[test]
fn covers_cut_point_and_turn_start_edge_cases() {
    let mut fixture = Fixture::default();
    let first_custom = fixture.custom_entry("first", None);
    let second_custom = fixture.custom_entry("second", Some(first_custom.id()));
    assert_eq!(
        find_cut_point(&[first_custom.clone(), second_custom.clone()], 0, 2, 1),
        CutPointResult {
            first_kept_entry_index: 0,
            turn_start_index: None,
            is_split_turn: false,
        }
    );

    let branch_summary = Entry::BranchSummary {
        id: fixture.create_id(),
        parent_id: Some(second_custom.id().to_string()),
        seq: fixture.next + 1,
        timestamp: crate::ai::now_ms(),
        from_id: Some("branch".to_string()),
        summary: "branch summary".to_string(),
        details: None,
        usage: None,
        from_hook: false,
    };
    assert_eq!(
        find_turn_start_index(&[first_custom.clone(), branch_summary.clone()], 1, 0),
        Some(1)
    );
    assert_eq!(
        find_turn_start_index(&[first_custom.clone(), second_custom.clone()], 1, 0),
        None
    );

    let result = find_cut_point(&[first_custom.clone(), branch_summary.clone()], 0, 2, 1);
    assert_eq!(result.first_kept_entry_index, 0);

    let tool_result = fixture.message_entry(
        AgentMessage::ToolResult(crate::ai::types::message::ToolResultMessage {
            tool_call_id: "call-1".to_string(),
            tool_name: "read".to_string(),
            content: vec![TextOrImageBlock::Text(TextContent {
                text: "tool output".to_string(),
                text_signature: None,
            })],
            details: None,
            usage: None,
            is_error: false,
            timestamp: crate::ai::now_ms(),
        }),
        None,
    );
    assert_eq!(
        find_cut_point(&[tool_result], 0, 1, 1),
        CutPointResult {
            first_kept_entry_index: 0,
            turn_start_index: None,
            is_split_turn: false,
        }
    );

    let user = fixture.message_entry(user_message("user"), None);
    let compaction = fixture.compaction_entry("summary", Some(user.id()), Vec::new());
    let assistant = fixture.message_entry(
        AgentMessage::Assistant(assistant_message("assistant", mock_usage(100, 50, 0, 0))),
        Some(compaction.id()),
    );
    assert_eq!(
        find_cut_point(&[user, compaction, assistant], 0, 3, 1).first_kept_entry_index,
        2
    );
}

/// Oracle "estimates tokens and context usage across supported message roles".
#[test]
fn estimates_tokens_and_context_usage_across_supported_message_roles() {
    let usage = mock_usage(10, 5, 3, 2);
    let assistant = assistant_message("assistant", usage);
    let assistant_with_thinking_and_tool = AssistantMessage {
        content: vec![
            AssistantBlock::Thinking(ThinkingContent {
                thinking: "thinking".to_string(),
                thinking_signature: None,
                redacted: None,
            }),
            AssistantBlock::ToolCall(ToolCall {
                id: "call-1".to_string(),
                name: "read".to_string(),
                arguments: serde_json::json!({"path": "file.ts"}),
                thought_signature: None,
                namespace: None,
            }),
        ],
        ..assistant.clone()
    };
    let mut custom_data = serde_json::Map::new();
    custom_data.insert("customType".into(), serde_json::json!("note"));
    custom_data.insert("content".into(), serde_json::json!("custom text"));
    custom_data.insert("display".into(), serde_json::json!(true));
    custom_data.insert("timestamp".into(), serde_json::json!(crate::ai::now_ms()));
    let mut custom = CustomAgentMessage::new("custom");
    custom.data = custom_data;
    let custom_string = AgentMessage::Custom(custom);
    let tool_result_with_image =
        AgentMessage::ToolResult(crate::ai::types::message::ToolResultMessage {
            tool_call_id: "call-1".to_string(),
            tool_name: "read".to_string(),
            content: vec![
                TextOrImageBlock::Text(TextContent {
                    text: "tool text".to_string(),
                    text_signature: None,
                }),
                TextOrImageBlock::Image(crate::ai::types::content::ImageContent {
                    mime_type: "image/png".to_string(),
                    data: "abc".to_string(),
                }),
            ],
            details: None,
            usage: None,
            is_error: false,
            timestamp: crate::ai::now_ms(),
        });
    let bash_execution = AgentMessage::Custom({
        let mut data = serde_json::Map::new();
        data.insert("command".into(), serde_json::json!("npm run check"));
        data.insert("output".into(), serde_json::json!("ok"));
        data.insert("exitCode".into(), serde_json::json!(0));
        data.insert("cancelled".into(), serde_json::json!(false));
        data.insert("truncated".into(), serde_json::json!(false));
        data.insert("timestamp".into(), serde_json::json!(crate::ai::now_ms()));
        let mut message = CustomAgentMessage::new("bashExecution");
        message.data = data;
        message
    });
    let branch_summary_message = AgentMessage::Custom({
        let mut data = serde_json::Map::new();
        data.insert("summary".into(), serde_json::json!("branch"));
        data.insert("fromId".into(), serde_json::json!("x"));
        data.insert("timestamp".into(), serde_json::json!(crate::ai::now_ms()));
        let mut message = CustomAgentMessage::new("branchSummary");
        message.data = data;
        message
    });
    let compaction_summary_message = AgentMessage::Custom({
        let mut data = serde_json::Map::new();
        data.insert("summary".into(), serde_json::json!("compact"));
        data.insert("tokensBefore".into(), serde_json::json!(123));
        data.insert("timestamp".into(), serde_json::json!(crate::ai::now_ms()));
        let mut message = CustomAgentMessage::new("compactionSummary");
        message.data = data;
        message
    });

    assert!(estimate_tokens(&user_message("plain user")) > 0);
    assert!(estimate_tokens(&AgentMessage::Assistant(assistant_with_thinking_and_tool)) > 0);
    assert!(estimate_tokens(&custom_string) > 0);
    assert!(estimate_tokens(&tool_result_with_image) > 1000);
    assert!(estimate_tokens(&bash_execution) > 0);
    assert!(estimate_tokens(&branch_summary_message) > 0);
    assert!(estimate_tokens(&compaction_summary_message) > 0);
    assert_eq!(
        estimate_tokens(&AgentMessage::Custom(CustomAgentMessage::new("unknown"))),
        0
    );

    let mut fixture = Fixture::default();
    let assistant_agent = AgentMessage::Assistant(assistant.clone());
    assert_eq!(
        get_last_assistant_usage(&[
            fixture.message_entry(user_message("user"), None),
            fixture.message_entry(assistant_agent.clone(), None),
        ]),
        Some(usage)
    );
    let mut aborted = assistant.clone();
    aborted.stop_reason = StopReason::Aborted;
    let mut errored = assistant.clone();
    errored.stop_reason = StopReason::Error;
    assert_eq!(
        get_last_assistant_usage(&[
            fixture.message_entry(AgentMessage::Assistant(aborted), None),
            fixture.message_entry(AgentMessage::Assistant(errored), None),
        ]),
        None
    );
    let mut zeroed = assistant.clone();
    zeroed.usage = mock_usage(0, 0, 0, 0);
    assert_eq!(
        get_last_assistant_usage(&[
            fixture.message_entry(user_message("user"), None),
            fixture.message_entry(assistant_agent.clone(), None),
            fixture.message_entry(AgentMessage::Assistant(zeroed), None),
        ]),
        Some(usage)
    );

    let estimate = estimate_context_tokens(&[user_message("no usage")]);
    assert_eq!(estimate.last_usage_index, None);

    let estimate = estimate_context_tokens(&[
        AgentMessage::Assistant(assistant.clone()),
        user_message("tail"),
    ]);
    assert_eq!(estimate.usage_tokens, 20);
    assert_eq!(estimate.last_usage_index, Some(0));

    let estimate = estimate_context_tokens(&[
        user_message("Hello"),
        AgentMessage::Assistant(assistant.clone()),
        user_message("continue"),
        AgentMessage::Assistant(assistant_message(
            "Partial thinking",
            mock_usage(0, 0, 0, 0),
        )),
    ]);
    assert_eq!(estimate.usage_tokens, 20);
    assert_eq!(estimate.last_usage_index, Some(1));
    assert!(estimate.trailing_tokens > 0);
    assert_eq!(estimate.tokens, 20 + estimate.trailing_tokens);
}

/// Oracle "builds session context with a compaction entry".
#[tokio::test]
async fn builds_session_context_with_a_compaction_entry() {
    let mut fixture = Fixture::default();
    let u1 = fixture.message_entry(user_message("1"), None);
    let a1 = fixture.message_entry(
        AgentMessage::Assistant(assistant_message("a", mock_usage(100, 50, 0, 0))),
        Some(u1.id()),
    );
    let u2 = fixture.message_entry(user_message("2"), Some(a1.id()));
    let a2 = fixture.message_entry(
        AgentMessage::Assistant(assistant_message("b", mock_usage(100, 50, 0, 0))),
        Some(u2.id()),
    );
    let compaction = fixture.compaction_entry(
        "Summary of 1,a,2,b",
        Some(a2.id()),
        vec![
            user_message("2"),
            AgentMessage::Assistant(assistant_message("b", mock_usage(100, 50, 0, 0))),
        ],
    );
    let u3 = fixture.message_entry(user_message("3"), Some(compaction.id()));
    let a3 = fixture.message_entry(
        AgentMessage::Assistant(assistant_message("c", mock_usage(100, 50, 0, 0))),
        Some(u3.id()),
    );
    let loaded = build_session_context(
        &[u1, a1, u2, a2, compaction, u3, a3],
        None,
        background_context(),
    )
    .await
    .unwrap();
    assert_eq!(loaded.len(), 5);
    assert_eq!(
        loaded.iter().map(AgentMessage::role).collect::<Vec<_>>(),
        [
            "compactionSummary",
            "user",
            "assistant",
            "user",
            "assistant"
        ]
    );
}

/// Oracle "prepares compaction using the latest compaction summary as
/// previousSummary".
#[tokio::test]
async fn prepares_compaction_using_latest_compaction_summary_as_previous_summary() {
    let mut fixture = Fixture::default();
    let u1 = fixture.message_entry(user_message("user msg 1"), None);
    let a1 = fixture.message_entry(
        AgentMessage::Assistant(assistant_message(
            "assistant msg 1",
            mock_usage(100, 50, 0, 0),
        )),
        Some(u1.id()),
    );
    let u2 = fixture.message_entry(user_message("user msg 2"), Some(a1.id()));
    let a2 = fixture.message_entry(
        AgentMessage::Assistant(assistant_message(
            "assistant msg 2",
            mock_usage(5000, 1000, 0, 0),
        )),
        Some(u2.id()),
    );
    let compaction1 = fixture.compaction_entry("First summary", Some(a2.id()), Vec::new());
    let u3 = fixture.message_entry(user_message("user msg 3"), Some(compaction1.id()));
    let a3 = fixture.message_entry(
        AgentMessage::Assistant(assistant_message(
            "assistant msg 3",
            mock_usage(8000, 2000, 0, 0),
        )),
        Some(u3.id()),
    );
    let path_entries = [u1, a1, u2, a2, compaction1, u3, a3];
    let preparation = prepare_compaction(&path_entries, DEFAULT_COMPACTION_SETTINGS)
        .unwrap()
        .expect("preparation");
    assert_eq!(
        preparation.previous_summary.as_deref(),
        Some("First summary")
    );
    assert!(!preparation.retained_tail.is_empty());
    let expected = estimate_context_tokens(
        &build_session_context(&path_entries, None, background_context())
            .await
            .unwrap(),
    )
    .tokens;
    assert_eq!(preparation.tokens_before, expected);
}

/// Oracle "carries a previous compaction's retained tail into the next
/// preparation".
#[test]
fn carries_previous_compactions_retained_tail_into_next_preparation() {
    let mut fixture = Fixture::default();
    let retained_user = user_message("retained user");
    let retained_assistant = AgentMessage::Assistant(assistant_message(
        "retained assistant",
        mock_usage(100, 50, 0, 0),
    ));
    let compaction = fixture.compaction_entry(
        "previous summary",
        None,
        vec![retained_user.clone(), retained_assistant.clone()],
    );
    let user = fixture.message_entry(user_message("new user"), Some(compaction.id()));
    let user_message_payload = match &user {
        Entry::Message { message, .. } => message.clone(),
        other => panic!("expected message entry, got {other:?}"),
    };
    let assistant = fixture.message_entry(
        AgentMessage::Assistant(assistant_message(
            "new assistant",
            mock_usage(100, 50, 0, 0),
        )),
        Some(user.id()),
    );
    let assistant_message_payload = match &assistant {
        Entry::Message { message, .. } => message.clone(),
        other => panic!("expected message entry, got {other:?}"),
    };

    let preparation = prepare_compaction(
        &[compaction, user, assistant],
        CompactionSettings {
            enabled: true,
            reserve_tokens: 100,
            keep_recent_tokens: 1,
        },
    )
    .unwrap()
    .expect("preparation");
    assert_eq!(
        preparation.previous_summary.as_deref(),
        Some("previous summary")
    );
    let mut combined = preparation.messages_to_summarize;
    combined.extend(preparation.turn_prefix_messages);
    combined.extend(preparation.retained_tail);
    assert_eq!(
        combined,
        vec![
            retained_user,
            retained_assistant,
            user_message_payload,
            assistant_message_payload,
        ]
    );
}

/// Oracle "prepares split-turn compaction with prior file-operation details".
#[test]
fn prepares_split_turn_compaction_with_prior_file_operation_details() {
    let mut fixture = Fixture::default();
    let u1 = fixture.message_entry(user_message("user msg 1"), None);
    let assistant_message_payload = AssistantMessage {
        content: vec![AssistantBlock::ToolCall(ToolCall {
            id: "tool-1".to_string(),
            name: "write".to_string(),
            arguments: serde_json::json!({"path": "written.ts"}),
            thought_signature: None,
            namespace: None,
        })],
        ..assistant_message("assistant msg 1", mock_usage(100, 50, 0, 0))
    };
    let a1 = fixture.message_entry(
        AgentMessage::Assistant(assistant_message_payload),
        Some(u1.id()),
    );
    let mut compaction1 = fixture.compaction_entry("First summary", Some(a1.id()), Vec::new());
    if let Entry::Compaction { details, .. } = &mut compaction1 {
        *details = Some(serde_json::json!({
            "readFiles": ["old-read.ts"],
            "modifiedFiles": ["old-edit.ts", "written.ts"],
        }));
    }
    let u2 = fixture.message_entry(user_message("large turn"), Some(compaction1.id()));
    let a2 = fixture.message_entry(
        AgentMessage::Assistant(assistant_message(
            "large assistant message",
            mock_usage(100, 50, 0, 0),
        )),
        Some(u2.id()),
    );
    let preparation = prepare_compaction(
        &[u1, a1, compaction1, u2, a2],
        CompactionSettings {
            enabled: true,
            reserve_tokens: 100,
            keep_recent_tokens: 1,
        },
    )
    .unwrap()
    .expect("preparation");

    assert_eq!(
        preparation.previous_summary.as_deref(),
        Some("First summary")
    );
    assert!(preparation.is_split_turn);
    assert_eq!(
        preparation
            .turn_prefix_messages
            .iter()
            .map(AgentMessage::role)
            .collect::<Vec<_>>(),
        ["user"]
    );
    assert!(preparation.file_ops.read.contains("old-read.ts"));
    assert!(preparation.file_ops.edited.contains("old-edit.ts"));
    assert!(preparation.file_ops.edited.contains("written.ts"));
}

/// Oracle "does not prepare compaction when there is nothing valid to
/// compact".
#[test]
fn does_not_prepare_compaction_when_nothing_valid_to_compact() {
    let mut fixture = Fixture::default();
    let compaction = fixture.compaction_entry("already compacted", None, Vec::new());
    assert!(
        prepare_compaction(&[compaction], DEFAULT_COMPACTION_SETTINGS)
            .unwrap()
            .is_none()
    );
    assert!(prepare_compaction(&[], DEFAULT_COMPACTION_SETTINGS)
        .unwrap()
        .is_none());
}

// ---------------------------------------------------------------------------
// Summary generation oracles
// ---------------------------------------------------------------------------

/// Oracle "passes reasoning through generateSummary only for reasoning models
/// with thinking enabled".
#[tokio::test]
async fn passes_reasoning_only_for_reasoning_models_with_thinking_enabled() {
    let messages = vec![user_message("Summarize this.")];
    let seen: Arc<Mutex<Vec<Option<SimpleStreamOptions>>>> = Arc::new(Mutex::new(Vec::new()));
    let (models, faux_reasoning, reasoning_model) = create_faux_model(true, 8192);
    factory_recording_options(&faux_reasoning, Arc::clone(&seen), "## Goal\nTest summary");
    generate_summary(
        &messages,
        Arc::clone(&models),
        reasoning_model,
        2000,
        None,
        None,
        Some(crate::agent_core::types::ThinkingLevel::Medium),
        None,
        RetryCallbacks::default(),
        background_context(),
    )
    .await
    .unwrap();
    assert_eq!(
        seen.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)[0]
            .as_ref()
            .and_then(|options| options.reasoning),
        Some(crate::ai::types::primitives::ThinkingLevel::Medium)
    );

    let (models, faux_off, off_model) = create_faux_model(true, 8192);
    factory_recording_options(&faux_off, Arc::clone(&seen), "## Goal\nTest summary");
    generate_summary(
        &messages,
        Arc::clone(&models),
        off_model,
        2000,
        None,
        None,
        Some(crate::agent_core::types::ThinkingLevel::Off),
        None,
        RetryCallbacks::default(),
        background_context(),
    )
    .await
    .unwrap();
    assert_eq!(
        seen.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)[1]
            .as_ref()
            .and_then(|options| options.reasoning),
        None
    );

    let (models, faux_non_reasoning, non_reasoning_model) = create_faux_model(false, 8192);
    factory_recording_options(
        &faux_non_reasoning,
        Arc::clone(&seen),
        "## Goal\nTest summary",
    );
    generate_summary(
        &messages,
        Arc::clone(&models),
        non_reasoning_model,
        2000,
        None,
        None,
        Some(crate::agent_core::types::ThinkingLevel::Medium),
        None,
        RetryCallbacks::default(),
        background_context(),
    )
    .await
    .unwrap();
    assert_eq!(
        seen.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)[2]
            .as_ref()
            .and_then(|options| options.reasoning),
        None
    );
}

/// Oracle "includes previous summaries and custom instructions in
/// generateSummary prompts".
#[tokio::test]
async fn includes_previous_summaries_and_custom_instructions_in_prompts() {
    let messages = vec![user_message("Summarize this.")];
    let prompt_text = Arc::new(Mutex::new(String::new()));
    let (models, faux, model) = create_faux_model(false, 8192);
    let prompt_for_factory = Arc::clone(&prompt_text);
    faux.set_responses(vec![FauxResponseStep::Factory(Arc::new(
        move |args: FauxFactoryArgs| {
            let prompt = Arc::clone(&prompt_for_factory);
            Box::pin(async move {
                // The transcript leads with the summarization system prompt;
                // the request is the first user message.
                let message = args
                    .context
                    .messages()
                    .iter()
                    .find(|message| matches!(message, Message::User(_)));
                if let Some(Message::User(user)) = message {
                    if let StringOrBlocks::Blocks(blocks) = &user.content {
                        if let Some(TextOrImageBlock::Text(text)) = blocks.first() {
                            *prompt
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                                text.text.clone();
                        }
                    }
                }
                Ok(faux_assistant_message(
                    "## Goal\nTest summary",
                    FauxMessageOptions::default(),
                ))
            }) as BoxFuture<'static, Result<AssistantMessage, String>>
        },
    ))]);

    let summary = generate_summary_with_usage(
        &messages,
        models,
        model,
        2000,
        Some("focus".to_string()),
        Some("old summary".to_string()),
        None,
        None,
        RetryCallbacks::default(),
        background_context(),
    )
    .await
    .unwrap();

    assert!(summary.text.contains("Test summary"));
    assert!(summary.usage.input > 0);
    assert!(summary.usage.output > 0);
    assert_eq!(
        summary.usage.total_tokens,
        summary.usage.input
            + summary.usage.output
            + summary.usage.cache_read
            + summary.usage.cache_write
    );
    assert!(prompt_text
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .contains("<previous-summary>\nold summary\n</previous-summary>"));
    assert!(prompt_text
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .contains("Additional focus: focus"));
}

/// Oracle "preserves the string result from generateSummary".
#[tokio::test]
async fn preserves_the_string_result_from_generate_summary() {
    let messages = vec![user_message("Summarize this.")];
    let (models, faux, model) = create_faux_model(false, 8192);
    faux.set_responses(vec![faux_assistant_message(
        "## Goal\nTest summary",
        FauxMessageOptions::default(),
    )
    .into()]);

    assert_eq!(
        generate_summary(
            &messages,
            models,
            model,
            2000,
            None,
            None,
            None,
            None,
            RetryCallbacks::default(),
            background_context(),
        )
        .await
        .unwrap(),
        "## Goal\nTest summary"
    );
}

/// Oracle "returns error results for failed or aborted summary generations".
#[tokio::test]
async fn returns_error_results_for_failed_or_aborted_generations() {
    let messages = vec![user_message("Summarize this.")];
    let (models, error_faux, error_model) = create_faux_model(false, 8192);
    error_faux.set_responses(vec![faux_assistant_message(
        "",
        FauxMessageOptions {
            stop_reason: Some(StopReason::Error),
            error_message: Some("boom".to_string()),
            ..FauxMessageOptions::default()
        },
    )
    .into()]);
    let error = generate_summary(
        &messages,
        models,
        error_model,
        2000,
        None,
        None,
        None,
        None,
        RetryCallbacks::default(),
        background_context(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, CompactionErrorCode::SummarizationFailed);
    assert_eq!(error.message, "Summarization failed: boom");

    let (models, aborted_faux, aborted_model) = create_faux_model(false, 8192);
    aborted_faux.set_responses(vec![faux_assistant_message(
        "",
        FauxMessageOptions {
            stop_reason: Some(StopReason::Aborted),
            error_message: Some("stopped".to_string()),
            ..FauxMessageOptions::default()
        },
    )
    .into()]);
    let aborted = generate_summary(
        &messages,
        models,
        aborted_model,
        2000,
        None,
        None,
        None,
        None,
        RetryCallbacks::default(),
        background_context(),
    )
    .await
    .unwrap_err();
    assert_eq!(aborted.code, CompactionErrorCode::Aborted);
    assert_eq!(aborted.message, "stopped");
}

// ---------------------------------------------------------------------------
// Compaction generation oracles
// ---------------------------------------------------------------------------

/// Oracle "clamps compaction summary maxTokens to the model output cap".
#[tokio::test]
async fn clamps_compaction_summary_max_tokens_to_model_output_cap() {
    let messages = vec![user_message("Summarize this.")];
    let seen: Arc<Mutex<Vec<Option<SimpleStreamOptions>>>> = Arc::new(Mutex::new(Vec::new()));
    let (models, faux, model) = create_faux_model(false, 128_000);
    faux.set_responses(vec![
        FauxResponseStep::Factory(record_options_factory(
            Arc::clone(&seen),
            "## Goal\nTest summary",
        )),
        FauxResponseStep::Factory(record_options_factory(
            Arc::clone(&seen),
            "## Goal\nTest summary",
        )),
    ]);
    let preparation = preparation_with(
        &messages,
        true,
        CompactionSettings {
            enabled: true,
            reserve_tokens: 500_000,
            keep_recent_tokens: 20_000,
        },
    );

    compact(
        preparation,
        models,
        model,
        None,
        None,
        None,
        RetryCallbacks::default(),
        background_context(),
    )
    .await
    .unwrap();

    let seen = seen
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let max_tokens: Vec<Option<u64>> = seen
        .iter()
        .map(|options| {
            options
                .as_ref()
                .and_then(|options| options.stream.max_tokens)
        })
        .collect();
    assert_eq!(max_tokens, vec![Some(128_000), Some(128_000)]);
    let cache_retentions: Vec<Option<CacheRetention>> = seen
        .iter()
        .map(|options| {
            options
                .as_ref()
                .and_then(|options| options.stream.cache_retention)
        })
        .collect();
    assert_eq!(
        cache_retentions,
        vec![Some(CacheRetention::None), Some(CacheRetention::None)]
    );
    let session_ids: Vec<Option<String>> = seen
        .iter()
        .map(|options| {
            options
                .as_ref()
                .and_then(|options| options.stream.session_id.clone())
        })
        .collect();
    assert_eq!(session_ids.len(), 2);
    assert!(session_ids[0].is_some());
    assert_ne!(session_ids[0], session_ids[1]);
}

/// Oracle "retains per-request retries for non-harness compaction callers".
#[tokio::test]
async fn retains_per_request_retries_for_non_harness_callers() {
    let messages = vec![user_message("Summarize this.")];
    let preparation = preparation_with(&messages, false, generation_settings());
    let (models, faux, model) = create_faux_model(false, 8192);
    faux.set_responses(vec![
        faux_assistant_message(
            "",
            FauxMessageOptions {
                stop_reason: Some(StopReason::Error),
                error_message: Some("rate limit exceeded".to_string()),
                ..FauxMessageOptions::default()
            },
        )
        .into(),
        faux_assistant_message("recovered summary", FauxMessageOptions::default()).into(),
    ]);

    let result = compact(
        preparation,
        models,
        model,
        None,
        None,
        Some(RetryPolicy {
            enabled: true,
            max_retries: 1,
            base_delay_ms: 0,
            max_agent_delay_ms: None,
        }),
        RetryCallbacks::default(),
        background_context(),
    )
    .await
    .unwrap();
    assert!(result.summary.contains("recovered summary"));
}

/// Oracle "returns compaction error results without throwing".
#[tokio::test]
async fn returns_compaction_error_results_without_throwing() {
    let messages = vec![user_message("Summarize this.")];
    let preparation = preparation_with(
        &messages,
        false,
        CompactionSettings {
            enabled: true,
            reserve_tokens: 2000,
            keep_recent_tokens: 20,
        },
    );
    let (models, history_faux, history_model) = create_faux_model(false, 8192);
    history_faux.set_responses(vec![faux_assistant_message(
        "",
        FauxMessageOptions {
            stop_reason: Some(StopReason::Error),
            error_message: Some("history failed".to_string()),
            ..FauxMessageOptions::default()
        },
    )
    .into()]);
    let error = compact(
        preparation.clone(),
        models,
        history_model,
        None,
        None,
        None,
        RetryCallbacks::default(),
        background_context(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, CompactionErrorCode::SummarizationFailed);
    assert_eq!(error.message, "Summarization failed: history failed");
}

/// Oracle "combines usage for split-turn compaction summaries" (via the
/// scripted request boundary; see the module docs for the substitution).
#[tokio::test]
async fn combines_usage_for_split_turn_compaction_summaries() {
    let messages = vec![user_message("Summarize this.")];
    let (_models, _faux, model) = create_faux_model(false, 8192);
    let history_usage = mock_usage(1, 2, 3, 4);
    let turn_prefix_usage = mock_usage(5, 6, 7, 8);
    let mut history_response =
        faux_assistant_message("history summary", FauxMessageOptions::default());
    history_response.usage = history_usage;
    let mut prefix_response =
        faux_assistant_message("turn prefix summary", FauxMessageOptions::default());
    prefix_response.usage = turn_prefix_usage;
    let responses = Arc::new(Mutex::new(vec![history_response, prefix_response]));
    let request: SummaryRequest = {
        let responses = Arc::clone(&responses);
        Arc::new(move |_ai_context, _options, _context| {
            let responses = Arc::clone(&responses);
            Box::pin(async move {
                responses
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remove(0)
            })
        })
    };
    let preparation = preparation_with(&messages, true, generation_settings());

    let result = compact_with_request(
        preparation,
        &CompactGenerationOptions {
            model,
            custom_instructions: None,
            thinking_level: None,
        },
        &request,
        background_context(),
    )
    .await
    .unwrap();
    assert_eq!(result.usage, Some(mock_usage(6, 8, 10, 12)));
}

/// Oracle "passes reasoning through turn-prefix summaries when enabled".
#[tokio::test]
async fn passes_reasoning_through_turn_prefix_summaries_when_enabled() {
    let messages = vec![user_message("Summarize this.")];
    let seen: Arc<Mutex<Vec<Option<SimpleStreamOptions>>>> = Arc::new(Mutex::new(Vec::new()));
    let (models, faux, model) = create_faux_model(true, 8192);
    factory_recording_options(
        &faux,
        Arc::clone(&seen),
        "## Original Request\nTest summary",
    );
    let preparation = CompactionPreparation {
        messages_to_summarize: Vec::new(),
        turn_prefix_messages: messages.clone(),
        retained_tail: messages,
        is_split_turn: true,
        tokens_before: 100,
        previous_summary: None,
        file_ops: empty_file_ops(),
        settings: generation_settings(),
    };

    compact(
        preparation,
        models,
        model,
        None,
        Some(crate::agent_core::types::ThinkingLevel::High),
        None,
        RetryCallbacks::default(),
        background_context(),
    )
    .await
    .unwrap();

    let seen = seen
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].as_ref().and_then(|options| options.reasoning),
        Some(crate::ai::types::primitives::ThinkingLevel::High)
    );
}

/// Oracle "returns turn-prefix compaction errors without throwing".
#[tokio::test]
async fn returns_turn_prefix_compaction_errors_without_throwing() {
    let messages = vec![user_message("Summarize this.")];
    let preparation = CompactionPreparation {
        messages_to_summarize: Vec::new(),
        turn_prefix_messages: messages.clone(),
        retained_tail: messages,
        is_split_turn: true,
        tokens_before: 100,
        previous_summary: None,
        file_ops: empty_file_ops(),
        settings: generation_settings(),
    };
    let (models, faux, model) = create_faux_model(false, 8192);
    faux.set_responses(vec![faux_assistant_message(
        "",
        FauxMessageOptions {
            stop_reason: Some(StopReason::Error),
            error_message: Some("prefix failed".to_string()),
            ..FauxMessageOptions::default()
        },
    )
    .into()]);
    let error = compact(
        preparation.clone(),
        models,
        model,
        None,
        None,
        None,
        RetryCallbacks::default(),
        background_context(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, CompactionErrorCode::SummarizationFailed);
    assert_eq!(
        error.message,
        "Turn prefix summarization failed: prefix failed"
    );

    let (models, aborted_faux, aborted_model) = create_faux_model(false, 8192);
    aborted_faux.set_responses(vec![faux_assistant_message(
        "",
        FauxMessageOptions {
            stop_reason: Some(StopReason::Aborted),
            error_message: Some("prefix stopped".to_string()),
            ..FauxMessageOptions::default()
        },
    )
    .into()]);
    let aborted = compact(
        preparation.clone(),
        models,
        aborted_model,
        None,
        None,
        None,
        RetryCallbacks::default(),
        background_context(),
    )
    .await
    .unwrap_err();
    assert_eq!(aborted.code, CompactionErrorCode::Aborted);
    assert_eq!(aborted.message, "prefix stopped");
}

/// Oracle "returns a compaction result with file details".
#[tokio::test]
async fn returns_a_compaction_result_with_file_details() {
    let mut fixture = Fixture::default();
    let u1 = fixture.message_entry(user_message("read a file"), None);
    let assistant = AssistantMessage {
        content: vec![AssistantBlock::ToolCall(ToolCall {
            id: "tool-1".to_string(),
            name: "read".to_string(),
            arguments: serde_json::json!({"path": "src/index.ts"}),
            thought_signature: None,
            namespace: None,
        })],
        ..assistant_message("calling tool", mock_usage(1000, 200, 0, 0))
    };
    let a1 = fixture.message_entry(AgentMessage::Assistant(assistant), Some(u1.id()));
    let u2 = fixture.message_entry(user_message("continue"), Some(a1.id()));
    let a2 = fixture.message_entry(
        AgentMessage::Assistant(assistant_message("done", mock_usage(4000, 500, 0, 0))),
        Some(u2.id()),
    );
    let preparation = prepare_compaction(&[u1, a1, u2, a2], DEFAULT_COMPACTION_SETTINGS)
        .unwrap()
        .expect("preparation");
    let (models, faux, model) = create_faux_model(false, 8192);
    faux.set_responses(vec![faux_assistant_message(
        "## Goal\nTest summary",
        FauxMessageOptions::default(),
    )
    .into()]);
    let result = compact(
        preparation,
        models,
        model,
        None,
        None,
        None,
        RetryCallbacks::default(),
        background_context(),
    )
    .await
    .unwrap();
    assert!(!result.summary.is_empty());
    assert!(result.usage.expect("usage").total_tokens > 0);
    assert!(!result.retained_tail.is_empty());
    assert!(result.details.is_some());
}
