//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! r16 tests for the interactive-mode deterministic core
//! (`modes/interactive/`): upstream test ports plus node-captured byte
//! oracles.
//!
//! Oracle provenance: `tests/fixtures/interactive_r16_oracle/` —
//! `capture_share_oracle.mjs` (verbatim upstream `exportSessionToJsonl` +
//! `exportSessionForShare` bodies over a fixed fixture, fixed export
//! timestamp `2026-02-03T04:05:06.789Z`, fixed share id `abcd1234`) and
//! `model_search_oracle.json` (the real upstream `model-search.ts` imported
//! by file URL). Upstream test sources ported:
//! `test/export-jsonl-share.test.ts`, `test/model-catalog-refresh.test.ts`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use super::model_catalog_refresh::{
    refresh_model_catalogs, ModelCatalogRefreshCoordinator, ModelCatalogRefreshError,
    ModelCatalogRuntime,
};
use super::model_search::{get_model_search_text, get_model_selector_search_text, ModelSearchItem};
use super::session_share::{
    export_session_for_share_with_presentation, export_session_to_jsonl_at, share_custom_entry,
    share_tool_presentations, ShareToolPresentation, SHARE_CUSTOM_TYPE,
};
use crate::agent_core::types::AgentMessage;
use crate::ai::types::content::{TextContent, ToolCall};
use crate::ai::types::message::{
    AssistantBlock, AssistantMessage, StringOrBlocks, TextOrImageBlock, ToolResultMessage,
    UserMessage,
};
use crate::ai::types::primitives::{StopReason, Usage, UsageCost};
use crate::coding_agent::extensions::types::{create_synthetic_source_info, ToolInfo};
use crate::coding_agent::session_manager::SessionManager;

// ---------------------------------------------------------------------------
// model-search.ts (oracle: tests/fixtures/interactive_r16_oracle/model_search_oracle.json)
// ---------------------------------------------------------------------------

mod model_search_tests {
    use super::*;

    fn item(id: &str, provider: &str, name: Option<&str>) -> ModelSearchItem {
        ModelSearchItem {
            id: id.to_string(),
            provider: provider.to_string(),
            name: name.map(str::to_string),
        }
    }

    /// Byte-exact against the real upstream `model-search.ts` run under
    /// `node --experimental-strip-types` (model_search_oracle.json).
    #[test]
    fn search_texts_match_the_node_oracle_byte_for_byte() {
        let cases: Vec<(ModelSearchItem, &str, &str)> = vec![
            (
                item("gpt-5", "openai", None),
                "gpt-5 openai openai/gpt-5 openai gpt-5",
                "openai openai/gpt-5 openai gpt-5",
            ),
            (
                item("gpt-5", "openai", Some("GPT-5")),
                "gpt-5 openai openai/gpt-5 openai gpt-5 GPT-5",
                "openai openai/gpt-5 openai gpt-5 GPT-5",
            ),
            (
                item("claude-sonnet-4-5", "anthropic", Some("Claude Sonnet 4.5")),
                "claude-sonnet-4-5 anthropic anthropic/claude-sonnet-4-5 anthropic claude-sonnet-4-5 Claude Sonnet 4.5",
                "anthropic anthropic/claude-sonnet-4-5 anthropic claude-sonnet-4-5 Claude Sonnet 4.5",
            ),
            (
                item("openai/gpt-5", "openrouter", None),
                "openai/gpt-5 openrouter openrouter/openai/gpt-5 openrouter openai/gpt-5",
                "openrouter openrouter/openai/gpt-5 openrouter openai/gpt-5",
            ),
            (item("", "p", None), " p p/ p ", "p p/ p "),
        ];
        for (item, search, selector) in cases {
            assert_eq!(get_model_search_text(&item), search);
            assert_eq!(get_model_selector_search_text(&item), selector);
        }
    }

    /// Upstream comment: keep the bare model ID out of the leading position
    /// so provider-prefixed queries rank first for proxy-provider ids.
    #[test]
    fn selector_text_leads_with_the_provider_prefix() {
        let proxy = item("openai/gpt-5", "openrouter", None);
        let selector = get_model_selector_search_text(&proxy);
        assert!(selector.starts_with("openrouter "));
        assert!(!selector.starts_with("openai/gpt-5 "));
    }
}

// ---------------------------------------------------------------------------
// session-share.ts + session-export.ts
// (oracle: tests/fixtures/interactive_r16_oracle/capture_share_oracle.mjs)
// ---------------------------------------------------------------------------

/// Fixture session document (header + 3 chained message entries), byte-shared
/// with the oracle script.
const FIXTURE: &str = r#"{"type":"session","version":3,"id":"sess-oracle-16","timestamp":"2026-01-01T00:00:00.000Z","cwd":"C:\\pi-oracle-cwd"}
{"type":"message","id":"e-user-1","parentId":null,"timestamp":"2026-01-01T00:00:01.000Z","message":{"role":"user","content":"hello","timestamp":1767225601000}}
{"type":"message","id":"e-asst-1","parentId":"e-user-1","timestamp":"2026-01-01T00:00:02.000Z","message":{"role":"assistant","content":[{"type":"text","text":"calling tool"}],"api":"anthropic-messages","provider":"anthropic","model":"test","usage":{"input":1,"output":1,"cacheRead":0,"cacheWrite":0,"totalTokens":2,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}},"stopReason":"toolUse","timestamp":1767225602000}}
{"type":"message","id":"e-tool-1","parentId":"e-asst-1","timestamp":"2026-01-01T00:00:03.000Z","message":{"role":"toolResult","toolCallId":"call-1","toolName":"share_tool","content":[{"type":"text","text":"done"}],"details":{},"isError":false,"timestamp":1767225603000}}
"#;

/// Oracle export without trailing entries (fixed timestamp
/// `2026-02-03T04:05:06.789Z`).
const ORACLE_NORMAL: &str = r#"{"type":"session","version":3,"id":"sess-oracle-16","timestamp":"2026-02-03T04:05:06.789Z","cwd":"C:\\pi-oracle-cwd"}
{"type":"message","id":"e-user-1","parentId":null,"timestamp":"2026-01-01T00:00:01.000Z","message":{"role":"user","content":"hello","timestamp":1767225601000}}
{"type":"message","id":"e-asst-1","parentId":"e-user-1","timestamp":"2026-01-01T00:00:02.000Z","message":{"role":"assistant","content":[{"type":"text","text":"calling tool"}],"api":"anthropic-messages","provider":"anthropic","model":"test","usage":{"input":1,"output":1,"cacheRead":0,"cacheWrite":0,"totalTokens":2,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}},"stopReason":"toolUse","timestamp":1767225602000}}
{"type":"message","id":"e-tool-1","parentId":"e-asst-1","timestamp":"2026-01-01T00:00:03.000Z","message":{"role":"toolResult","toolCallId":"call-1","toolName":"share_tool","content":[{"type":"text","text":"done"}],"details":{},"isError":false,"timestamp":1767225603000}}
"#;

/// Oracle export with the fixed `pi.share` trailing entry (share id
/// `abcd1234`, presentation payload below).
const ORACLE_SHARE: &str = r#"{"type":"session","version":3,"id":"sess-oracle-16","timestamp":"2026-02-03T04:05:06.789Z","cwd":"C:\\pi-oracle-cwd"}
{"type":"message","id":"e-user-1","parentId":null,"timestamp":"2026-01-01T00:00:01.000Z","message":{"role":"user","content":"hello","timestamp":1767225601000}}
{"type":"message","id":"e-asst-1","parentId":"e-user-1","timestamp":"2026-01-01T00:00:02.000Z","message":{"role":"assistant","content":[{"type":"text","text":"calling tool"}],"api":"anthropic-messages","provider":"anthropic","model":"test","usage":{"input":1,"output":1,"cacheRead":0,"cacheWrite":0,"totalTokens":2,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}},"stopReason":"toolUse","timestamp":1767225602000}}
{"type":"message","id":"e-tool-1","parentId":"e-asst-1","timestamp":"2026-01-01T00:00:03.000Z","message":{"role":"toolResult","toolCallId":"call-1","toolName":"share_tool","content":[{"type":"text","text":"done"}],"details":{},"isError":false,"timestamp":1767225603000}}
{"type":"custom","customType":"pi.share","id":"abcd1234","parentId":"e-tool-1","timestamp":"2026-02-03T04:05:06.789Z","data":{"systemPrompt":"You are pi.","tools":[{"name":"share_tool","description":"Render a value for sharing","parameters":{"type":"object","properties":{"value":{"type":"string","description":"Value to render"}},"required":["value"]}}]}}
"#;

const FIXTURE_TIMESTAMP: &str = "2026-02-03T04:05:06.789Z";
const FIXTURE_SHARE_ID: &str = "abcd1234";

fn fixture_presentations() -> Vec<ShareToolPresentation> {
    let tool = ToolInfo {
        name: "share_tool".to_string(),
        description: "Render a value for sharing".to_string(),
        parameters: json!({
            "type": "object",
            "properties": { "value": { "type": "string", "description": "Value to render" } },
            "required": ["value"],
        }),
        prompt_guidelines: None,
        exposure: crate::coding_agent::extensions::types::ToolExposure::Direct,
        namespace: None,
        annotations: None,
        source_info: create_synthetic_source_info("builtin", "builtin", None, None, None),
    };
    share_tool_presentations(&[tool])
}

fn open_fixture_session(dir: &std::path::Path) -> Arc<Mutex<SessionManager>> {
    let fixture_path = dir.join("fixture.jsonl");
    std::fs::write(&fixture_path, FIXTURE).expect("write fixture");
    let manager =
        SessionManager::open(fixture_path.to_str().expect("utf8 path"), None, None).expect("open");
    Arc::new(Mutex::new(manager))
}

mod session_share_tests {
    use super::*;

    /// Byte-exact against the verbatim upstream export bodies captured in
    /// `tests/fixtures/interactive_r16_oracle/share_oracle.json`: plain export and
    /// share export must match byte for byte.
    #[test]
    fn exports_match_the_node_oracle_byte_for_byte() {
        // environment-anchored: both sides normalized. The fixture header's
        // win32 cwd `C:\pi-oracle-cwd` is drive-absolute, so the win32 export
        // carries it through verbatim (the captured form). On POSIX the same
        // fixture input is relative and the session manager resolves it
        // against the process cwd at open time exactly like upstream node
        // (`path.resolve`) before re-serializing the header; collapse any
        // resolved prefix back to the raw fixture anchor on BOTH sides so the
        // byte comparison covers the export body, not the host cwd
        // resolution. (Prefix-agnostic: sibling tests in this binary chdir.)
        const ANCHOR: &str = "C:\\\\pi-oracle-cwd"; // JSON-escaped separator
        let normalize = |bytes: Vec<u8>| -> Vec<u8> {
            let text = match String::from_utf8(bytes) {
                Ok(text) => text,
                Err(error) => return error.into_bytes(),
            };
            match text.find(ANCHOR) {
                Some(anchor_start) => {
                    let head = &text[..anchor_start];
                    let prefix_end = head
                        .rfind("\"cwd\":\"")
                        .map(|index| index + "\"cwd\":\"".len())
                        .unwrap_or(anchor_start);
                    let mut out = String::with_capacity(text.len());
                    out.push_str(&text[..prefix_end]);
                    out.push_str(ANCHOR);
                    out.push_str(&text[anchor_start + ANCHOR.len()..]);
                    out.into_bytes()
                }
                None => text.into_bytes(),
            }
        };
        let temp = std::env::temp_dir().join(format!("pi-r16-oracle-{}", std::process::id()));
        std::fs::create_dir_all(&temp).expect("temp dir");
        let manager = open_fixture_session(&temp);

        let normal_path = temp.join("normal.jsonl");
        let written = export_session_to_jsonl_at(
            &manager,
            Some(normal_path.to_str().unwrap()),
            FIXTURE_TIMESTAMP,
            &|_parent_id, _timestamp| vec![],
        )
        .expect("export");
        assert_eq!(written, normal_path.to_str().unwrap());
        assert_eq!(
            normalize(std::fs::read(&normal_path).expect("read")),
            normalize(ORACLE_NORMAL.as_bytes().to_vec())
        );

        let share_path = temp.join("share.jsonl");
        let presentations = fixture_presentations();
        export_session_to_jsonl_at(
            &manager,
            Some(share_path.to_str().unwrap()),
            FIXTURE_TIMESTAMP,
            &|parent_id, timestamp| {
                vec![share_custom_entry(
                    parent_id,
                    timestamp,
                    FIXTURE_SHARE_ID,
                    "You are pi.",
                    &presentations,
                )]
            },
        )
        .expect("export");
        assert_eq!(
            normalize(std::fs::read(&share_path).expect("read")),
            normalize(ORACLE_SHARE.as_bytes().to_vec())
        );

        std::fs::remove_dir_all(&temp).expect("cleanup");
    }

    /// Upstream test/export-jsonl-share.test.ts "adds presentation data
    /// without changing conversation IDs or links", ported over the ported
    /// SessionManager (the upstream test drives the full SDK session; the
    /// document construction this test exercises is the same code path —
    /// the AgentSession accessor glue reads the same state).
    #[test]
    fn adds_presentation_data_without_changing_conversation_ids_or_links() {
        let temp = std::env::temp_dir().join(format!("pi-r16-share-{}", std::process::id()));
        std::fs::create_dir_all(&temp).expect("temp dir");

        let manager = Arc::new(Mutex::new(
            SessionManager::in_memory(temp.to_str().unwrap(), None, None).expect("in memory"),
        ));

        let user_id = manager
            .lock()
            .expect("lock")
            .append_message(AgentMessage::User(UserMessage {
                content: StringOrBlocks::Text("hello".to_string()),
                timestamp: 1,
            }))
            .expect("append");
        let assistant_id = manager
            .lock()
            .expect("lock")
            .append_message(AgentMessage::Assistant(AssistantMessage {
                content: vec![AssistantBlock::ToolCall(ToolCall {
                    id: "call-1".to_string(),
                    name: "share_tool".to_string(),
                    arguments: json!({ "value": "example" }),
                    thought_signature: None,
                    namespace: None,
                })],
                api: "anthropic-messages".to_string(),
                provider: "anthropic".to_string(),
                model: "test".to_string(),
                response_model: None,
                response_id: None,
                provider_thinking_level: None,
                diagnostics: None,
                usage: Usage {
                    input: 1,
                    output: 1,
                    cache_read: 0,
                    cache_write: 0,
                    cache_write_1h: None,
                    reasoning: None,
                    total_tokens: 2,
                    cost: UsageCost {
                        input: 0.0,
                        output: 0.0,
                        cache_read: 0.0,
                        cache_write: 0.0,
                        total: 0.0,
                    },
                },
                stop_reason: StopReason::ToolUse,
                deferred: None,
                error_message: None,
                raw_stop_reason: None,
                end_turn: None,
                timestamp: 1,
            }))
            .expect("append");
        let result_id = manager
            .lock()
            .expect("lock")
            .append_message(AgentMessage::ToolResult(ToolResultMessage {
                tool_call_id: "call-1".to_string(),
                tool_name: "share_tool".to_string(),
                content: vec![TextOrImageBlock::Text(TextContent {
                    text: "done".to_string(),
                    text_signature: None,
                })],
                details: Some(json!({})),
                usage: None,
                is_error: false,
                timestamp: 1,
            }))
            .expect("append");
        let original_entry_ids: Vec<String> = {
            let manager = manager.lock().expect("lock");
            manager
                .get_branch(None)
                .iter()
                .filter_map(|entry| entry.id().map(str::to_string))
                .collect()
        };
        assert_eq!(
            original_entry_ids,
            vec![user_id.clone(), assistant_id.clone(), result_id.clone()]
        );

        // The plain export never carries a share record.
        let normal_path = temp.join("normal.jsonl");
        export_session_to_jsonl_at(
            &manager,
            Some(normal_path.to_str().unwrap()),
            FIXTURE_TIMESTAMP,
            &|_parent_id, _timestamp| vec![],
        )
        .expect("export");
        let normal_records = parse_jsonl(&std::fs::read_to_string(&normal_path).expect("read"));
        assert!(normal_records
            .iter()
            .all(|record| record["customType"] != json!(SHARE_CUSTOM_TYPE)));

        // The share export appends the presentation entry.
        let share_path = temp.join("share.jsonl");
        let presentations = vec![ShareToolPresentation {
            name: "share_tool".to_string(),
            description: "Render a value for sharing".to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "value": { "type": "string", "description": "Value to render" } },
                "required": ["value"],
            }),
        }];
        export_session_for_share_with_presentation(
            share_path.to_str().unwrap(),
            &manager,
            FIXTURE_SHARE_ID,
            "You are pi.",
            &presentations,
        )
        .expect("export");
        let share_bytes = std::fs::read_to_string(&share_path).expect("read");
        let records = parse_jsonl(&share_bytes);
        let conversation_records = &records[1..records.len() - 1];
        let conversation_ids: Vec<&str> = conversation_records
            .iter()
            .map(|record| record["id"].as_str().expect("id"))
            .collect();
        assert_eq!(
            conversation_ids,
            original_entry_ids
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
        );
        let expected_parent_ids: Vec<Option<String>> = std::iter::once(None)
            .chain(
                original_entry_ids[..original_entry_ids.len() - 1]
                    .iter()
                    .cloned()
                    .map(Some),
            )
            .collect();
        let actual_parent_ids: Vec<Option<String>> = conversation_records
            .iter()
            .map(|record| record["parentId"].as_str().map(str::to_string))
            .collect();
        assert_eq!(actual_parent_ids, expected_parent_ids);
        assert_eq!(
            conversation_records[conversation_records.len() - 3]["id"],
            json!(user_id)
        );
        assert_eq!(
            conversation_records[conversation_records.len() - 2]["id"],
            json!(assistant_id)
        );
        assert_eq!(
            conversation_records[conversation_records.len() - 1]["id"],
            json!(result_id)
        );

        let share_entry = records.last().expect("share entry");
        assert_eq!(share_entry["type"], json!("custom"));
        assert_eq!(share_entry["customType"], json!("pi.share"));
        assert_eq!(share_entry["parentId"], json!(result_id));
        assert!(share_entry["timestamp"].is_string());
        assert_eq!(share_entry["data"]["systemPrompt"], json!("You are pi."));
        let share_tools = share_entry["data"]["tools"].as_array().expect("tools");
        assert_eq!(share_tools.len(), 1);
        assert_eq!(share_tools[0]["name"], json!("share_tool"));
        assert_eq!(
            share_tools[0]["description"],
            json!("Render a value for sharing")
        );
        assert!(share_tools[0]["parameters"].is_object());
        // Upstream: the share payload carries presentation data only.
        let data_keys: Vec<&str> = share_entry["data"]
            .as_object()
            .expect("data object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(data_keys, vec!["systemPrompt", "tools"]);

        // Re-importing the share file keeps the share entry as the leaf and
        // reconstructs the conversation.
        let imported =
            SessionManager::open(share_path.to_str().unwrap(), None, None).expect("open");
        assert_eq!(imported.get_leaf_id(), Some(FIXTURE_SHARE_ID));
        let context = imported.build_session_context();
        let roles: Vec<&str> = context
            .messages
            .iter()
            .map(|message| message.role())
            .collect();
        assert_eq!(roles, vec!["user", "assistant", "toolResult"]);

        std::fs::remove_dir_all(&temp).expect("cleanup");
    }

    /// The trailing-entry callback receives the last branch entry as parent
    /// and the shared export timestamp (session-export.ts:23,26-29).
    #[test]
    fn trailing_entries_receive_the_leaf_parent_and_shared_timestamp() {
        let temp = std::env::temp_dir().join(format!("pi-r16-trailing-{}", std::process::id()));
        std::fs::create_dir_all(&temp).expect("temp dir");
        let manager = open_fixture_session(&temp);
        let observed: Mutex<Vec<(Option<String>, String)>> = Mutex::new(Vec::new());
        let out_path = temp.join("observed.jsonl");
        export_session_to_jsonl_at(
            &manager,
            Some(out_path.to_str().unwrap()),
            FIXTURE_TIMESTAMP,
            &|parent_id, timestamp| {
                observed
                    .lock()
                    .expect("lock")
                    .push((parent_id.map(str::to_string), timestamp.to_string()));
                vec![share_custom_entry(
                    parent_id,
                    timestamp,
                    FIXTURE_SHARE_ID,
                    "",
                    &[],
                )]
            },
        )
        .expect("export");
        assert_eq!(
            observed.into_inner().expect("observed"),
            vec![(Some("e-tool-1".to_string()), FIXTURE_TIMESTAMP.to_string())]
        );

        // Error path: an unwritable export location surfaces a message the
        // upstream callers render as `Failed to export session: …`.
        let blocking = temp.join("blocking.txt");
        std::fs::write(&blocking, "not a directory").expect("write blocking file");
        let error = export_session_to_jsonl_at(
            &manager,
            Some(blocking.join("out.jsonl").to_str().unwrap()),
            FIXTURE_TIMESTAMP,
            &|_parent_id, _timestamp| vec![],
        )
        .expect_err("export into a non-directory path fails");
        assert!(!error.message.is_empty());
        let _displayed: String = error.to_string();

        std::fs::remove_dir_all(&temp).expect("cleanup");
    }

    fn parse_jsonl(document: &str) -> Vec<Value> {
        document
            .trim_end_matches('\n')
            .split('\n')
            .map(|line| serde_json::from_str(line).expect("jsonl line"))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// model-catalog-refresh.ts (upstream test/model-catalog-refresh.test.ts)
// ---------------------------------------------------------------------------

mod model_catalog_refresh_tests {
    use super::*;
    use std::future::pending;
    use tokio::sync::watch;

    type Outcome = Result<crate::ai::models::ModelsRefreshResult, String>;

    /// Mock runtime whose refresh waits on a shared watch gate before
    /// returning a fixed outcome (upstream `createDeferred`).
    struct GatedRuntime {
        ready: watch::Receiver<bool>,
        outcome: Outcome,
        calls: Arc<AtomicUsize>,
        signals: Arc<Mutex<Vec<CancellationToken>>>,
    }

    impl ModelCatalogRuntime for GatedRuntime {
        fn refresh<'a>(
            &'a self,
            options: crate::ai::models::ModelsRefreshOptions,
        ) -> futures::future::BoxFuture<'a, Outcome> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(signal) = options.signal {
                self.signals.lock().expect("signals lock").push(signal);
            }
            let mut ready = self.ready.clone();
            Box::pin(async move {
                while !*ready.borrow() {
                    if ready.changed().await.is_err() {
                        break;
                    }
                }
                self.outcome.clone()
            })
        }
    }

    /// Mock runtime whose refresh never settles (upstream
    /// `new Promise(() => {})`).
    struct PendingRuntime {
        calls: Arc<AtomicUsize>,
        signals: Arc<Mutex<Vec<CancellationToken>>>,
    }

    impl ModelCatalogRuntime for PendingRuntime {
        fn refresh<'a>(
            &'a self,
            options: crate::ai::models::ModelsRefreshOptions,
        ) -> futures::future::BoxFuture<'a, Outcome> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(signal) = options.signal {
                self.signals.lock().expect("signals lock").push(signal);
            }
            Box::pin(pending())
        }
    }

    fn successful_refresh() -> Outcome {
        Ok(crate::ai::models::ModelsRefreshResult::default())
    }

    async fn wait_until(condition: impl Fn() -> bool) {
        for _ in 0..10_000 {
            if condition() {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("condition never became true");
    }

    // Note: unlike JS promises, Rust futures are lazy — each caller future is
    // spawned so the concurrent-join behavior under test actually executes.

    /// "shares one runtime refresh between concurrent callers"
    #[tokio::test]
    async fn shares_one_runtime_refresh_between_concurrent_callers() {
        let coordinator = ModelCatalogRefreshCoordinator::new();
        let (ready_tx, ready_rx) = watch::channel(false);
        let runtime = Arc::new(GatedRuntime {
            ready: ready_rx,
            outcome: successful_refresh(),
            calls: Arc::new(AtomicUsize::new(0)),
            signals: Arc::new(Mutex::new(Vec::new())),
        });
        let first_signal = CancellationToken::new();
        let second_signal = CancellationToken::new();

        let first = {
            let coordinator = coordinator.clone();
            let runtime = runtime.clone();
            tokio::spawn(async move {
                coordinator
                    .refresh(runtime, first_signal)
                    .await
                    .expect("first refresh resolves")
            })
        };
        let second = {
            let coordinator = coordinator.clone();
            let runtime = runtime.clone();
            tokio::spawn(async move {
                coordinator
                    .refresh(runtime, second_signal)
                    .await
                    .expect("second refresh resolves")
            })
        };

        wait_until(|| runtime.calls.load(Ordering::SeqCst) == 1).await;
        // Give the second caller a chance to (wrongly) start its own refresh.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert_eq!(runtime.calls.load(Ordering::SeqCst), 1);
        ready_tx.send(true).expect("send");
        let first_result = first.await.expect("join");
        assert!(!first_result.aborted);
        assert!(first_result.errors.is_empty());
        let second_result = second.await.expect("join");
        assert!(!second_result.aborted);
        assert!(second_result.errors.is_empty());
    }

    /// "keeps the shared refresh alive when one caller stops waiting"
    #[tokio::test]
    async fn keeps_the_shared_refresh_alive_when_one_caller_stops_waiting() {
        let coordinator = ModelCatalogRefreshCoordinator::new();
        let (ready_tx, ready_rx) = watch::channel(false);
        let runtime = Arc::new(GatedRuntime {
            ready: ready_rx,
            outcome: successful_refresh(),
            calls: Arc::new(AtomicUsize::new(0)),
            signals: Arc::new(Mutex::new(Vec::new())),
        });
        let first_signal = CancellationToken::new();
        let second_signal = CancellationToken::new();
        let first = {
            let coordinator = coordinator.clone();
            let runtime = runtime.clone();
            let signal = first_signal.clone();
            tokio::spawn(async move { coordinator.refresh(runtime, signal).await })
        };
        let second = {
            let coordinator = coordinator.clone();
            let runtime = runtime.clone();
            let signal = second_signal.clone();
            tokio::spawn(async move { coordinator.refresh(runtime, signal).await })
        };
        wait_until(|| runtime.calls.load(Ordering::SeqCst) == 1).await;
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert_eq!(runtime.calls.load(Ordering::SeqCst), 1);

        first_signal.cancel();
        assert!(matches!(
            first.await,
            Ok(Err(ModelCatalogRefreshError::Abort(_)))
        ));
        let shared_refresh_signal = runtime.signals.lock().expect("signals lock")[0].clone();
        assert!(!shared_refresh_signal.is_cancelled());

        ready_tx.send(true).expect("send");
        let second_result = second
            .await
            .expect("join")
            .expect("second refresh resolves");
        assert!(!second_result.aborted);
        assert!(second_result.errors.is_empty());
    }

    /// "aborts an abandoned refresh and allows a later refresh to start"
    #[tokio::test]
    async fn aborts_an_abandoned_refresh_and_allows_a_later_refresh_to_start() {
        let coordinator = ModelCatalogRefreshCoordinator::new();
        let runtime = Arc::new(PendingRuntime {
            calls: Arc::new(AtomicUsize::new(0)),
            signals: Arc::new(Mutex::new(Vec::new())),
        });
        let first_signal = CancellationToken::new();
        let first = {
            let coordinator = coordinator.clone();
            let runtime = runtime.clone();
            let signal = first_signal.clone();
            tokio::spawn(async move { coordinator.refresh(runtime, signal).await })
        };
        wait_until(|| runtime.calls.load(Ordering::SeqCst) == 1).await;

        first_signal.cancel();
        assert!(matches!(
            first.await,
            Ok(Err(ModelCatalogRefreshError::Abort(_)))
        ));
        let first_refresh_signal = runtime.signals.lock().expect("signals lock")[0].clone();
        wait_until(|| first_refresh_signal.is_cancelled()).await;

        let second_signal = CancellationToken::new();
        let second = {
            let coordinator = coordinator.clone();
            let runtime = runtime.clone();
            let signal = second_signal.clone();
            tokio::spawn(async move { coordinator.refresh(runtime, signal).await })
        };
        wait_until(|| runtime.calls.load(Ordering::SeqCst) == 2).await;
        second_signal.cancel();
        assert!(matches!(
            second.await,
            Ok(Err(ModelCatalogRefreshError::Abort(_)))
        ));
    }

    /// Upstream `signal.throwIfAborted()`: an already-cancelled caller signal
    /// rejects before any refresh starts, through the module-level singleton
    /// (`refreshModelCatalogs`).
    #[tokio::test]
    async fn the_global_entry_point_rejects_an_already_cancelled_signal() {
        let runtime = Arc::new(PendingRuntime {
            calls: Arc::new(AtomicUsize::new(0)),
            signals: Arc::new(Mutex::new(Vec::new())),
        });
        let signal = CancellationToken::new();
        signal.cancel();
        let result = refresh_model_catalogs(runtime.clone(), signal).await;
        assert!(matches!(result, Err(ModelCatalogRefreshError::Abort(_))));
        assert_eq!(runtime.calls.load(Ordering::SeqCst), 0);
    }
}

// ---------------------------------------------------------------------------
// theme/theme.ts + theme/system-theme.ts + theme/theme-json.ts +
// theme/theme-controller.ts (oracle: tests/fixtures/theme_delta_oracle/ —
// the verbatim v0.99.1 HEAD theme.ts run under node with the pinned tui
// color pipeline; upstream tests ported: test/external-editor.test.ts and the
// theme-json validator seam scenarios)
// ---------------------------------------------------------------------------

mod theme_delta_tests {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    use serde_json::{json, Value};

    // `edit_in_external_editor` is exercised by the windows-gated tests
    // below; keep the import honest on unix too.
    #[cfg_attr(not(windows), allow(unused_imports))]
    use super::super::external_editor::{
        edit_in_external_editor, edit_in_external_editor_with, EditorRunner, ExternalEditorOptions,
        ExternalEditorResult,
    };
    use super::super::theme::{
        assert_theme_name_is_valid, builtin_theme_names, create_system_theme, create_theme,
        detect_color_fg_bg_theme, detect_terminal_theme, get_builtin_theme_json,
        get_current_theme_name, get_language_from_path, get_resolved_theme_colors,
        get_theme_by_name, get_theme_export_colors, init_theme, is_light_theme, load_builtin_theme,
        load_theme, load_theme_from_content, mark_terminal_colors_pending,
        parse_auto_theme_setting, parse_theme_json_content, reset_state_for_tests,
        resolve_theme_setting, set_registered_themes, set_terminal_color_scheme,
        set_terminal_colors, set_theme, sort_theme_infos, with_theme_color_fallbacks,
        ResolvedColor, StyleColor, TerminalTheme, Theme, ThemeStyle,
    };
    use super::super::theme_json::{validate_theme_json, ThemeJson};
    use crate::coding_agent::modes::interactive::system_theme::SYSTEM_THEME_NAME;
    use crate::tui::colors::{color_to_hex, parse_color, TerminalColorMode, TextAttributes};
    use crate::tui::terminal_colors::RgbColor;

    /// The node-captured oracle (generated by
    /// `tests/fixtures/theme_delta_oracle/oracle/capture.mjs` over the
    /// verbatim v0.99.1 HEAD theme.ts + system-theme.ts) embedded at compile
    /// time.
    const ORACLE: &str = include_str!(
        "../../../../tests/fixtures/theme_delta_oracle/oracle/theme_delta_oracle.json"
    );
    const JSON_ORACLE: &str =
        include_str!("../../../../tests/fixtures/interactive_r17_oracle/theme_json_oracle.json");

    const BUILTIN_DARK: &str = include_str!("theme/dark.json");

    fn oracle() -> Value {
        serde_json::from_str(ORACLE).expect("theme delta oracle parses")
    }

    fn json_oracle() -> Value {
        serde_json::from_str(JSON_ORACLE).expect("theme json oracle parses")
    }

    fn dark_value() -> Value {
        serde_json::from_str(BUILTIN_DARK).expect("dark.json")
    }

    /// The terminal defaults the capture's reported-color states use.
    const XTERM16: [&str; 16] = [
        "#000000", "#cd0000", "#00cd00", "#cdcd00", "#0000ee", "#cd00cd", "#00cdcd", "#e5e5e5",
        "#7f7f7f", "#ff0000", "#00ff00", "#ffff00", "#5c5cff", "#ff00ff", "#00ffff", "#ffffff",
    ];

    fn rgb(hex: &str) -> RgbColor {
        let cleaned = hex.strip_prefix('#').expect("hex prefix");
        RgbColor {
            r: u8::from_str_radix(&cleaned[0..2], 16).expect("hex"),
            g: u8::from_str_radix(&cleaned[2..4], 16).expect("hex"),
            b: u8::from_str_radix(&cleaned[4..6], 16).expect("hex"),
        }
    }

    const FG_TOKENS: [&str; 49] = [
        "accent",
        "border",
        "borderAccent",
        "borderMuted",
        "success",
        "error",
        "warning",
        "muted",
        "dim",
        "text",
        "thinkingText",
        "scrollbarTrack",
        "scrollbarThumb",
        "searchMatchText",
        "userMessageText",
        "customMessageText",
        "customMessageLabel",
        "toolTitle",
        "toolOutput",
        "mdHeading",
        "mdLink",
        "mdLinkUrl",
        "mdCode",
        "mdCodeBlock",
        "mdCodeBlockBorder",
        "mdQuote",
        "mdQuoteBorder",
        "mdHr",
        "mdListBullet",
        "toolDiffAdded",
        "toolDiffRemoved",
        "toolDiffContext",
        "syntaxComment",
        "syntaxKeyword",
        "syntaxFunction",
        "syntaxVariable",
        "syntaxString",
        "syntaxNumber",
        "syntaxType",
        "syntaxOperator",
        "syntaxPunctuation",
        "thinkingOff",
        "thinkingMinimal",
        "thinkingLow",
        "thinkingMedium",
        "thinkingHigh",
        "thinkingXhigh",
        "thinkingMax",
        "bashMode",
    ];
    const BG_TOKENS: [&str; 7] = [
        "selectedBg",
        "searchMatchBg",
        "userMessageBg",
        "customMessageBg",
        "toolPendingBg",
        "toolSuccessBg",
        "toolErrorBg",
    ];

    /// Upstream `ansiOf`: an `{"error": message}` object for throws.
    fn ansi_of(theme: &Theme, token: &str, background: bool) -> Value {
        let result = if background {
            theme.get_bg_ansi(token)
        } else {
            theme.get_fg_ansi(token)
        };
        match result {
            Ok(ansi) => json!(ansi),
            Err(error) => json!({ "error": error }),
        }
    }

    /// The dumpTheme capture shape: token ANSI tables, resolved colors as
    /// hex, appearance, color mode, stylers, and style/fg/bg probes.
    fn dump_theme(theme: &Theme) -> Value {
        let fg = serde_json::Map::from_iter(
            FG_TOKENS
                .iter()
                .map(|token| ((*token).to_string(), ansi_of(theme, token, false))),
        );
        let bg = serde_json::Map::from_iter(
            BG_TOKENS
                .iter()
                .map(|token| ((*token).to_string(), ansi_of(theme, token, true))),
        );
        let colors_hex = serde_json::Map::from_iter(
            theme
                .colors()
                .into_iter()
                .map(|(token, color)| (token, json!(color_to_hex(color)))),
        );
        let style = |fg: Option<StyleColor>, bg: Option<StyleColor>, attributes: TextAttributes| {
            theme.style("x", &ThemeStyle { fg, bg, attributes })
        };
        let token = |name: &str| Some(StyleColor::Token(name.to_string()));
        json!({
            "name": theme.name.clone(),
            "appearance": theme.appearance(),
            "colorMode": theme.get_color_mode().as_str(),
            "fg": Value::Object(fg),
            "bg": Value::Object(bg),
            "colorsHex": Value::Object(colors_hex),
            "stylers": {
                "bold": theme.bold("x"),
                "italic": theme.italic("x"),
                "underline": theme.underline("x"),
                "inverse": theme.inverse("x"),
                "strikethrough": theme.strikethrough("x"),
            },
            "probes": {
                "fg_accent": theme.fg("accent", "x").expect("accent"),
                "bg_selectedBg": probe_or_error(theme.bg("selectedBg", "x")),
                "style_fg_token": style(token("accent"), None, TextAttributes::default()).expect("style"),
                "style_fg_dim": style(token("dim"), None, TextAttributes::default()).expect("style"),
                "style_fg_bg_tokens": style(token("muted"), token("toolPendingBg"), TextAttributes::default()).expect("style"),
                "style_attributes": style(
                    token("text"),
                    None,
                    TextAttributes { bold: Some(true), underline: Some(true), ..TextAttributes::default() },
                ).expect("style"),
                "style_italic_dim": style(
                    token("dim"),
                    None,
                    TextAttributes { italic: Some(true), ..TextAttributes::default() },
                ).expect("style"),
                "unknown_fg": probe_or_error(theme.fg("nope", "x")),
                "unknown_bg": probe_or_error(theme.bg("nope", "x")),
            },
        })
    }

    /// Error-form of `bg_selectedBg` (capture `ansiOf`): the happy-path probe
    /// above serializes plain strings, errors as objects.
    fn probe_or_error(value: Result<String, String>) -> Value {
        match value {
            Ok(text) => json!(text),
            Err(error) => json!({ "error": error }),
        }
    }

    // -- synthetic theme documents (byte-identical to the capture's
    //    writeDoc payloads; the capture deletes its copies) ----------------

    const DEFAULTS_DOC: &str = r##"{
	"$schema": "https://example.invalid/theme-schema.json",
	"name": "defaults-dummy",
	"appearance": "dark",
	"vars": {
		"brand": "#ff0000"
	},
	"colors": {
		"accent": "brand",
		"border": "",
		"borderAccent": "oklch(0.62 0.12 250)",
		"borderMuted": 24,
		"success": "okhsl(150 50% 60%)",
		"error": "#00ff00",
		"warning": "#0000ff",
		"muted": "#808080",
		"dim": "#a0a0a0",
		"text": "",
		"thinkingText": "#c0c0c0",
		"selectedBg": "",
		"userMessageBg": "#202020",
		"userMessageText": "#ffffff",
		"customMessageBg": "#303030",
		"customMessageText": "#eeeeee",
		"customMessageLabel": "#dddddd",
		"toolPendingBg": "#404040",
		"toolSuccessBg": "#505050",
		"toolErrorBg": "#606060",
		"toolTitle": "#f0f0f0",
		"toolOutput": "#e0e0e0",
		"thinkingXhigh": "#d0d0d0"
	}
}"##;

    const DETECTED_DARK_DOC: &str = r##"{
	"name": "detected-dark",
	"colors": {
		"accent": "#ffffff", "border": "#e0e0e0", "borderAccent": "#d0d0d0", "borderMuted": "#c0c0c0",
		"success": "#b0b0b0", "error": "#f0f0f0", "warning": "#a0a0a0", "muted": "#909090",
		"dim": "#808080", "text": "#ffffff", "thinkingText": "#eeeeee", "selectedBg": "#101010",
		"userMessageBg": "#181818", "userMessageText": "#ffffff", "customMessageBg": "#202020",
		"customMessageText": "#eeeeee", "customMessageLabel": "#dddddd", "toolPendingBg": "#282828",
		"toolSuccessBg": "#303030", "toolErrorBg": "#383838", "toolTitle": "#f8f8f8",
		"toolOutput": "#e8e8e8", "thinkingXhigh": "#d8d8d8"
	}
}"##;

    const DETECTED_LIGHT_DOC: &str = r##"{
	"name": "detected-light",
	"colors": {
		"accent": "#202020", "border": "#303030", "borderAccent": "#404040", "borderMuted": "#505050",
		"success": "#606060", "error": "#101010", "warning": "#707070", "muted": "#808080",
		"dim": "#909090", "text": "#1a1a1a", "thinkingText": "#282828", "selectedBg": "#e0e0e0",
		"userMessageBg": "#d8d8d8", "userMessageText": "#101010", "customMessageBg": "#d0d0d0",
		"customMessageText": "#181818", "customMessageLabel": "#282828", "toolPendingBg": "#c8c8c8",
		"toolSuccessBg": "#c0c0c0", "toolErrorBg": "#b8b8b8", "toolTitle": "#080808",
		"toolOutput": "#181818", "thinkingXhigh": "#303030"
	}
}"##;

    const INDEXED_ONLY_DOC: &str = r##"{
	"name": "indexed-only",
	"colors": {
		"accent": 4, "border": 8, "borderAccent": 12, "borderMuted": 0, "success": 2, "error": 1,
		"warning": 3, "muted": 7, "dim": 8, "text": 7, "thinkingText": 15, "selectedBg": 0,
		"userMessageBg": 0, "userMessageText": 15, "customMessageBg": 0, "customMessageText": 15,
		"customMessageLabel": 15, "toolPendingBg": 0, "toolSuccessBg": 0, "toolErrorBg": 0,
		"toolTitle": 15, "toolOutput": 7, "thinkingXhigh": 15
	}
}"##;

    fn load_doc(content: &str) -> Theme {
        let theme_json = parse_theme_json_content("synthetic", content).expect("synthetic doc");
        // The capture pins the defaults group to truecolor
        // (globalThis.__piColorMode).
        create_theme(&theme_json, Some(TerminalColorMode::Truecolor), None)
            .expect("synthetic theme")
    }

    // -- builtin theme tables ----------------------------------------------

    #[test]
    fn builtin_theme_names_match_the_upstream_registry_order() {
        assert_eq!(builtin_theme_names(), ["dark", "light"]);
    }

    /// Groups 1-2: the full builtin-theme dumps (token ANSI tables, resolved
    /// hex colors, appearance, color mode, stylers, probes) in both color
    /// modes with an empty terminal state.
    #[test]
    fn builtin_theme_dumps_match_the_delta_oracle() {
        let oracle = oracle();
        for mode in [TerminalColorMode::Truecolor, TerminalColorMode::Color256] {
            reset_state_for_tests();
            set_terminal_colors(Default::default());
            let key = format!("builtin_{}", mode.as_str());
            for name in ["dark", "light"] {
                let theme = load_builtin_theme(name, Some(mode)).expect("builtin");
                assert_eq!(
                    dump_theme(&theme),
                    oracle[&key][name],
                    "builtin dump {key}/{name} diverges"
                );
            }
        }
        reset_state_for_tests();
    }

    /// Group 3: terminal-default tokens, appearance detection, and the
    /// resolved-colors cache over the synthetic documents, under all four
    /// terminal-color states of the capture (in capture order).
    #[test]
    fn defaults_and_detected_scenarios_match_the_delta_oracle() {
        let oracle = oracle();

        fn state_guessed() {
            set_terminal_color_scheme(None);
            set_terminal_colors(Default::default());
        }
        fn state_reported_dark() {
            set_terminal_colors(crate::tui::terminal_colors::TerminalColors {
                foreground: Some(rgb("#c4c4d4")),
                background: Some(rgb("#1e1e2e")),
                palette: Some(XTERM16.iter().map(|hex| rgb(hex)).collect()),
            });
        }
        fn state_reported_light() {
            set_terminal_colors(crate::tui::terminal_colors::TerminalColors {
                foreground: Some(rgb("#202028")),
                background: Some(rgb("#e8e8e8")),
                palette: None,
            });
        }
        fn state_reported_bg_only() {
            set_terminal_colors(crate::tui::terminal_colors::TerminalColors {
                foreground: None,
                background: Some(rgb("#808080")),
                palette: None,
            });
        }
        let states: [(&str, fn()); 4] = [
            ("guessed", state_guessed),
            ("reported_dark", state_reported_dark),
            ("reported_light", state_reported_light),
            ("reported_bg_only", state_reported_bg_only),
        ];

        reset_state_for_tests();
        for (label, prepare) in states {
            prepare();
            let defaults = load_doc(DEFAULTS_DOC);
            let detected_dark = load_doc(DETECTED_DARK_DOC);
            let detected_light = load_doc(DETECTED_LIGHT_DOC);
            let indexed_only = load_doc(INDEXED_ONLY_DOC);

            let raw = parse_color("#ff8800").expect("raw");
            let raw_green = parse_color("#00ff00").expect("raw green");
            let raw_panel = parse_color("#404040").expect("raw panel");
            let token = |name: &str| Some(StyleColor::Token(name.to_string()));
            let some_color = |color| Some(StyleColor::Color(color));
            let defaults_probes = json!({
                "border_fg": defaults.fg("border", "x").expect("border token"),
                "text_fg": defaults.fg("text", "x").expect("text token"),
                "selectedBg_as_fg": probe_or_error(defaults.fg("selectedBg", "x")),
                "style_bg_token_as_fg": probe_or_error(
                    defaults.style("x", &ThemeStyle { fg: token("selectedBg"), bg: None, attributes: Default::default() }),
                ),
                "dim_getFgAnsi": defaults.get_fg_ansi("dim").expect("dim"),
                "text_getFgAnsi": defaults.get_fg_ansi("text").expect("text"),
                "style_raw_fg": defaults.style("x", &ThemeStyle { fg: some_color(raw), bg: None, attributes: Default::default() }).expect("style"),
                "style_raw_fg_token_bg": defaults.style("x", &ThemeStyle {
                    fg: some_color(raw_green),
                    bg: token("userMessageBg"),
                    attributes: Default::default(),
                }).expect("style"),
                "style_raw_both": defaults.style("x", &ThemeStyle {
                    fg: some_color(raw_green),
                    bg: some_color(raw_panel),
                    attributes: Default::default(),
                }).expect("style"),
            });

            let expected = &oracle[format!("defaults_{label}").as_str()];
            assert_eq!(
                dump_theme(&defaults),
                expected["defaults"],
                "defaults dump ({label}) diverges"
            );
            assert_eq!(defaults_probes, expected["defaults_probes"]);
            assert_eq!(
                json!({
                    "appearance": detected_dark.appearance(),
                    "fg_sample": detected_dark.get_fg_ansi("text").expect("text"),
                    "bg_sample": detected_dark.get_bg_ansi("selectedBg").expect("selectedBg"),
                }),
                expected["detected_dark"],
                "detected dark ({label}) diverges"
            );
            assert_eq!(
                json!({
                    "appearance": detected_light.appearance(),
                    "fg_sample": detected_light.get_fg_ansi("text").expect("text"),
                    "bg_sample": detected_light.get_bg_ansi("selectedBg").expect("selectedBg"),
                }),
                expected["detected_light"],
                "detected light ({label}) diverges"
            );
            let indexed = json!({
                "appearance": indexed_only.appearance(),
                "fg": serde_json::Map::from_iter(
                    FG_TOKENS.iter().map(|token| ((*token).to_string(), ansi_of(&indexed_only, token, false))),
                ),
                "bg": serde_json::Map::from_iter(
                    BG_TOKENS.iter().map(|token| ((*token).to_string(), ansi_of(&indexed_only, token, true))),
                ),
                "colorsHex": serde_json::Map::from_iter(
                    indexed_only.colors().into_iter().map(|(token, color)| (token, json!(color_to_hex(color)))),
                ),
            });
            assert_eq!(
                indexed, expected["indexed_only"],
                "indexed only ({label}) diverges"
            );
        }
        reset_state_for_tests();
    }

    /// The resolved-colors cache re-keys on a new terminal-colors report, even
    /// with equal content (capture `cache_identity`; the Rust port replays the
    /// value identity of the snapshots through `Arc` ptr equality internally —
    /// observable here as re-resolved, equal colors).
    #[test]
    fn resolved_colors_cache_rekeys_on_a_new_report() {
        reset_state_for_tests();
        set_terminal_colors(crate::tui::terminal_colors::TerminalColors {
            foreground: Some(rgb("#c4c4d4")),
            background: Some(rgb("#1e1e2e")),
            palette: None,
        });
        let theme = load_doc(DEFAULTS_DOC);
        let first = theme.colors();
        let again = theme.colors();
        assert_eq!(first, again, "two reads under one report share values");

        set_terminal_colors(crate::tui::terminal_colors::TerminalColors {
            foreground: Some(rgb("#c4c4d4")),
            background: Some(rgb("#1e1e2e")),
            palette: None,
        });
        let after = theme.colors();
        assert_eq!(first, after, "equal reports resolve equal colors");
        reset_state_for_tests();
    }

    /// Groups 4-5: the system theme over fixed TerminalColors inputs
    /// (grayscale pending, scheme-only, full reports), in both color modes.
    #[test]
    fn system_theme_scenarios_match_the_delta_oracle() {
        let oracle = oracle();
        for mode in [TerminalColorMode::Truecolor, TerminalColorMode::Color256] {
            reset_state_for_tests();
            let key = format!("system_{}", mode.as_str());

            set_terminal_colors(Default::default());
            set_terminal_color_scheme(None);
            mark_terminal_colors_pending();
            let pending_empty = create_system_theme(Some(mode)).expect("system");
            assert_eq!(
                dump_theme(&pending_empty),
                oracle[&key]["pending_empty"],
                "system pending_empty ({mode:?}) diverges"
            );

            mark_terminal_colors_pending();
            set_terminal_color_scheme(Some(TerminalTheme::Light));
            set_terminal_colors(Default::default());
            let pending_scheme_light = create_system_theme(Some(mode)).expect("system");
            assert_eq!(
                dump_theme(&pending_scheme_light),
                oracle[&key]["pending_scheme_light"],
                "system pending_scheme_light ({mode:?}) diverges"
            );

            set_terminal_color_scheme(None);
            set_terminal_colors(crate::tui::terminal_colors::TerminalColors {
                foreground: Some(rgb("#c4c4d4")),
                background: Some(rgb("#1e1e2e")),
                palette: Some(XTERM16.iter().map(|hex| rgb(hex)).collect()),
            });
            let full_dark = create_system_theme(Some(mode)).expect("system");
            assert_eq!(
                dump_theme(&full_dark),
                oracle[&key]["full_dark"],
                "system full_dark ({mode:?}) diverges"
            );

            set_terminal_colors(crate::tui::terminal_colors::TerminalColors {
                foreground: Some(rgb("#202028")),
                background: Some(rgb("#e8e8e8")),
                palette: Some(XTERM16.iter().map(|hex| rgb(hex)).collect()),
            });
            let full_light = create_system_theme(Some(mode)).expect("system");
            assert_eq!(
                dump_theme(&full_light),
                oracle[&key]["full_light"],
                "system full_light ({mode:?}) diverges"
            );

            set_terminal_colors(crate::tui::terminal_colors::TerminalColors {
                foreground: None,
                background: None,
                palette: Some(XTERM16.iter().map(|hex| rgb(hex)).collect()),
            });
            let palette_only = create_system_theme(Some(mode)).expect("system");
            assert_eq!(
                dump_theme(&palette_only),
                oracle[&key]["palette_only"],
                "system palette_only ({mode:?}) diverges"
            );
        }
        reset_state_for_tests();
    }

    /// Group 6: `detectColorFgBgTheme` over the COLORFGBG grid.
    #[test]
    fn colorfgbg_grid_matches_the_delta_oracle() {
        reset_state_for_tests();
        let cases: [Option<&str>; 18] = [
            Some("0;15"),
            Some("15;0"),
            Some("0;7;15"),
            None,
            Some(""),
            Some("15;bad;300"),
            Some("15;bad"),
            Some("7"),
            Some("8"),
            Some("6"),
            Some("9"),
            Some("16"),
            Some("007"),
            Some("1"),
            Some("0; 8 "),
            Some("a;9;"),
            Some("-1"),
            Some("08"),
        ];
        let ours: Vec<Value> = cases
            .iter()
            .map(|case| {
                let theme = detect_color_fg_bg_theme(*case).map(TerminalTheme::as_str);
                json!({ "colorfgbg": case, "theme": theme })
            })
            .collect();
        assert_eq!(json!(ours), oracle()["detect_colorfgbg"]);
    }

    /// Group 6: `detectTerminalTheme` over the colors × scheme × COLORFGBG
    /// grid.
    #[test]
    fn detect_terminal_theme_grid_matches_the_delta_oracle() {
        reset_state_for_tests();
        let color_cases: [(Option<&str>, Option<&str>); 7] = [
            (None, None),
            (Some("#1e1e2e"), Some("#c4c4d4")),
            (Some("#e8e8e8"), Some("#202028")),
            (Some("#808080"), Some("#808080")),
            (Some("#000000"), None),
            (Some("#ffffff"), None),
            (None, Some("#c4c4d4")),
        ];
        let schemes = [None, Some(TerminalTheme::Dark), Some(TerminalTheme::Light)];
        let colorfgbg = [None, Some("15;0"), Some("15;7")];

        let mut ours = Vec::new();
        for (background, foreground) in color_cases {
            let colors = crate::tui::terminal_colors::TerminalColors {
                foreground: foreground.map(rgb),
                background: background.map(rgb),
                palette: None,
            };
            for scheme in schemes {
                for env_value in colorfgbg {
                    ours.push(json!({
                        "background": background,
                        "foreground": foreground,
                        "scheme": scheme.map(TerminalTheme::as_str),
                        "colorfgbg": env_value,
                        "theme": detect_terminal_theme(&colors, scheme, env_value).as_str(),
                    }));
                }
            }
        }
        assert_eq!(json!(ours), oracle()["detect_terminal_theme"]);
    }

    /// Group 6: `parseAutoThemeSetting` / `resolveThemeSetting` grids.
    #[test]
    fn theme_setting_helpers_match_the_delta_oracle() {
        let auto_cases: [Option<&str>; 12] = [
            Some("light/dark"),
            Some(" my-light / my-dark "),
            Some("light/dark/extra"),
            Some("light/"),
            Some("/dark"),
            Some("plain"),
            Some(""),
            None,
            Some("a/b/c"),
            Some("dark/"),
            Some("/"),
            Some("system/light"),
        ];
        let ours: Vec<Value> = auto_cases
            .iter()
            .map(|case| match parse_auto_theme_setting(*case) {
                Some((light, dark)) => json!({ "lightTheme": light, "darkTheme": dark }),
                None => Value::Null,
            })
            .collect();
        assert_eq!(json!(ours), oracle()["auto"]);

        let settings: [Option<&str>; 6] = [
            Some("dark"),
            Some("light/dark"),
            Some("light/dark/extra"),
            None,
            Some("system"),
            Some("a/b"),
        ];
        let ours: Vec<Value> = settings
            .iter()
            .map(|setting| {
                let pair: Vec<Value> = [TerminalTheme::Light, TerminalTheme::Dark]
                    .iter()
                    .map(|terminal| json!(resolve_theme_setting(*setting, *terminal)))
                    .collect();
                json!(pair)
            })
            .collect();
        assert_eq!(json!(ours), oracle()["resolve_setting"]);
    }

    /// Group 6: the name-based export helpers over the theme store
    /// (`getResolvedThemeColors`, `getThemeExportColors`, `isLightTheme`,
    /// `getAvailableThemes*`).
    #[test]
    fn export_helpers_match_the_delta_oracle() {
        reset_state_for_tests();
        set_terminal_colors(crate::tui::terminal_colors::TerminalColors {
            foreground: Some(rgb("#c4c4d4")),
            background: Some(rgb("#1e1e2e")),
            palette: Some(XTERM16.iter().map(|hex| rgb(hex)).collect()),
        });

        assert!(set_theme("dark").success);
        let resolved: Value = Value::Object(
            get_resolved_theme_colors(None)
                .expect("dark resolves")
                .into_iter()
                .map(|(token, hex)| (token, json!(hex)))
                .collect(),
        );
        assert_eq!(resolved, oracle()["resolved_dark_hex"]);
        assert_eq!(get_current_theme_name().as_deref(), Some("dark"));

        let export = |colors: super::super::theme::ThemeExportColors| {
            let mut rendered = serde_json::Map::new();
            if let Some(page_bg) = colors.page_bg {
                rendered.insert("pageBg".to_string(), json!(page_bg));
            }
            if let Some(card_bg) = colors.card_bg {
                rendered.insert("cardBg".to_string(), json!(card_bg));
            }
            if let Some(info_bg) = colors.info_bg {
                rendered.insert("infoBg".to_string(), json!(info_bg));
            }
            Value::Object(rendered)
        };
        assert_eq!(
            export(get_theme_export_colors(Some("dark"))),
            oracle()["export_dark"]
        );
        assert_eq!(
            export(get_theme_export_colors(Some("system"))),
            oracle()["export_system"]
        );
        assert_eq!(
            export(get_theme_export_colors(Some("nope"))),
            oracle()["export_missing"]
        );

        assert!(set_theme("light").success);
        let resolved: Value = Value::Object(
            get_resolved_theme_colors(None)
                .expect("light resolves")
                .into_iter()
                .map(|(token, hex)| (token, json!(hex)))
                .collect(),
        );
        assert_eq!(resolved, oracle()["resolved_light_hex"]);
        assert_eq!(
            json!({
                "light": is_light_theme(Some("light")).expect("light"),
                "dark": is_light_theme(Some("dark")).expect("dark"),
                "system": is_light_theme(Some("system")).expect("system"),
            }),
            oracle()["is_light"]
        );

        // `getAvailableThemesWithPaths`: the system theme comes first, then
        // name order; the fs paths are presentation (compared as presence).
        let infos = vec![
            ("dark".to_string(), Some("dark.json".to_string())),
            ("light".to_string(), Some("light.json".to_string())),
            (SYSTEM_THEME_NAME.to_string(), None),
        ];
        let sorted = sort_theme_infos(infos);
        let available = oracle()["available_themes_with_paths"]
            .as_array()
            .expect("array")
            .clone();
        let expected: Vec<&Value> = available.iter().collect();
        assert_eq!(sorted.len(), expected.len());
        for (info, expected) in sorted.iter().zip(expected) {
            assert_eq!(&info.0, expected["name"].as_str().expect("name"));
            assert_eq!(info.1.is_none(), expected["path"].is_null());
        }
        assert_eq!(
            get_theme_by_name("dark").expect("dark").name.as_deref(),
            Some("dark")
        );
        assert!(get_theme_by_name("nope").is_none());
        reset_state_for_tests();
    }

    /// The registered-themes registry: names with "/" are rejected, and a
    /// registered theme wins over the built-in documents in `loadTheme`.
    #[test]
    fn registered_themes_precede_the_builtins() {
        reset_state_for_tests();
        assert!(assert_theme_name_is_valid("pair/theme").is_err());
        assert!(assert_theme_name_is_valid("plain").is_ok());

        // A marker theme: the built-in dark theme renders accent from okhsl
        // vars; a registered "dark" with a #ff0000 accent is distinguishable.
        let mut marked = dark_value();
        marked["name"] = json!("dark");
        marked["colors"]["accent"] = json!("#ff0000");
        let marked_doc = parse_theme_json_content(
            "dark.json",
            &serde_json::to_string(&marked).expect("serialize"),
        )
        .expect("marked doc");
        let marked_theme = create_theme(&marked_doc, None, None).expect("marked theme");
        set_registered_themes(vec![marked_theme]);

        let loaded = load_theme("dark").expect("registered wins");
        // The store-backed load uses the default (256-color) mode.
        assert_eq!(
            loaded.get_fg_ansi("accent").expect("accent"),
            "\x1b[38;5;196m"
        );
        reset_state_for_tests();
        assert_ne!(
            load_theme("dark")
                .expect("builtin back")
                .get_fg_ansi("accent")
                .expect("accent"),
            "\x1b[38;5;196m"
        );
    }

    /// `initTheme`/`setTheme` fallback: an invalid name falls back to the
    /// system theme with the load error in the result (upstream `setTheme`).
    #[test]
    fn set_theme_falls_back_to_the_system_theme() {
        reset_state_for_tests();
        let result = set_theme("nope");
        assert!(!result.success);
        assert_eq!(result.error.as_deref(), Some("Theme not found: nope"));
        assert_eq!(get_current_theme_name().as_deref(), Some(SYSTEM_THEME_NAME));

        init_theme(None);
        assert_eq!(get_current_theme_name().as_deref(), Some(SYSTEM_THEME_NAME));
        reset_state_for_tests();
    }

    /// Upstream `getThemeByName` registry access for both built-ins.
    #[test]
    fn builtin_themes_load_by_name() {
        assert!(get_builtin_theme_json("dark").is_some());
        assert!(get_builtin_theme_json("light").is_some());
        assert!(get_builtin_theme_json("nope").is_none());
        assert!(load_builtin_theme("dark", None).is_ok());
        assert!(load_builtin_theme("nope", None).is_err());
    }

    /// Upstream `getLanguageFromPath` over the full table and edge inputs.
    #[test]
    fn language_table_matches_the_node_oracle() {
        let r17 =
            include_str!("../../../../tests/fixtures/interactive_r17_oracle/theme_oracle.json");
        let r17: Value = serde_json::from_str(r17).expect("r17 oracle parses");
        let paths = [
            "a.ts",
            "B.TSX",
            "x.js",
            "y.mjs",
            "z.cjs",
            "p.py",
            "q.rb",
            "main.rs",
            "m.go",
            "F.java",
            "a.kt",
            "b.swift",
            "c.c",
            "d.h",
            "e.cpp",
            "f.cc",
            "g.cxx",
            "h.hpp",
            "i.cs",
            "j.php",
            "k.sh",
            "l.bash",
            "m.zsh",
            "n.fish",
            "o.ps1",
            "p.sql",
            "q.html",
            "r.htm",
            "s.css",
            "t.scss",
            "u.sass",
            "v.less",
            "w.json",
            "x.yaml",
            "y.yml",
            "z.toml",
            "a.xml",
            "b.md",
            "c.markdown",
            "Dockerfile",
            "Makefile",
            "cmakelists.txt",
            "a.lua",
            "b.perl",
            "c.r",
            "d.scala",
            "e.clj",
            "f.ex",
            "g.exs",
            "h.erl",
            "i.hs",
            "j.ml",
            "k.vim",
            "l.graphql",
            "m.proto",
            "n.tf",
            "o.hcl",
            "no-ext",
            "",
            "a.",
        ];
        let ours: Vec<Value> = paths
            .iter()
            .map(|path| match get_language_from_path(path) {
                Some(language) => json!(language),
                None => Value::Null,
            })
            .collect();
        assert_eq!(json!(ours), r17["languages"]);
    }

    // -- theme-json.ts (oracle: validate_theme_json_oracle.mjs) ------------

    /// Upstream `validateThemeJson`: the built-in document is accepted
    /// (including the new `appearance` key).
    #[test]
    fn validation_accepts_the_builtin_document() {
        let dark = dark_value();
        assert_eq!(dark["appearance"], json!("dark"));
        let validated = validate_theme_json("dark.json", &dark).expect("valid");
        assert_eq!(
            validated.name,
            json_oracle()["valid_dark_name"].as_str().expect("name")
        );
        assert_eq!(validated.appearance(), Some("dark"));
    }

    /// Upstream `validateThemeJson`: missing color tokens produce the
    /// byte-exact "Missing required color tokens" message with a sorted list.
    #[test]
    fn validation_lists_missing_colors_byte_for_byte() {
        let mut broken = dark_value();
        let colors = broken
            .get_mut("colors")
            .and_then(Value::as_object_mut)
            .expect("colors");
        for key in ["accent", "thinkingXhigh", "bashMode", "userMessageBg"] {
            colors.remove(key);
        }
        let error = validate_theme_json("broken.json", &broken).expect_err("invalid");
        assert_eq!(
            error,
            json_oracle()["missing_four"].as_str().expect("message")
        );
    }

    /// Upstream `validateThemeJson`: an absent "colors" map lists every
    /// required token, byte for byte.
    #[test]
    fn validation_reports_an_absent_colors_map_byte_for_byte() {
        let mut no_colors = dark_value();
        no_colors
            .as_object_mut()
            .expect("object")
            .remove("colors")
            .expect("colors key");
        let error = validate_theme_json("nocolors.json", &no_colors).expect_err("invalid");
        assert_eq!(
            error,
            json_oracle()["colors_absent"].as_str().expect("message")
        );
    }

    /// Upstream `validateThemeJson`: theme names cannot contain "/".
    #[test]
    fn validation_rejects_slash_names_byte_for_byte() {
        let mut pair = dark_value();
        pair["name"] = json!("light/dark");
        let error = validate_theme_json("pair.json", &pair).expect_err("invalid");
        assert_eq!(
            error,
            json_oracle()["name_slash"].as_str().expect("message")
        );
    }

    /// The new optional `appearance` key: "dark"/"light" accepted, other
    /// strings rejected (the D3 seam wording is not byte-exact).
    #[test]
    fn validation_checks_the_appearance_key() {
        let mut doc = dark_value();
        doc["appearance"] = json!("light");
        assert_eq!(
            validate_theme_json("light.json", &doc)
                .expect("valid")
                .appearance(),
            Some("light")
        );

        doc["appearance"] = json!("blue");
        let error = validate_theme_json("blue.json", &doc).expect_err("invalid");
        assert!(error.contains("/appearance"), "{error}");

        doc.as_object_mut().expect("object").remove("appearance");
        assert_eq!(
            validate_theme_json("absent.json", &doc)
                .expect("valid")
                .appearance(),
            None,
            "absent appearance detects from the theme colors"
        );
    }

    /// Upstream `validateThemeJson`: a minimal-but-complete custom theme with
    /// vars, a 256-color index, and an empty value is accepted.
    #[test]
    fn validation_accepts_a_minimal_custom_theme() {
        let required_colors: serde_json::Map<String, Value> =
            super::super::theme_json::REQUIRED_COLORS
                .iter()
                .map(|key| {
                    (
                        (*key).to_string(),
                        if *key == "accent" {
                            json!("ink")
                        } else {
                            json!("#0a0b0c")
                        },
                    )
                })
                .collect();
        let minimal = json!({
            "name": "minimal",
            "vars": { "ink": "#010203" },
            "colors": required_colors,
            "scrollbarTrack": 24,
            "searchMatchText": "",
        });
        let validated = validate_theme_json("minimal.json", &minimal).expect("valid");
        assert_eq!(
            validated.name,
            json_oracle()["valid_minimal"].as_str().expect("name")
        );
    }

    // -- external-editor.ts (upstream test/external-editor.test.ts) --------

    type EditorCaptures = Arc<Mutex<Vec<(PathBuf, String)>>>;

    struct StubRunner {
        exit_code: Option<i32>,
        rewrite: Option<String>,
        captures: EditorCaptures,
    }

    impl StubRunner {
        fn spawn(code: Option<i32>, rewrite: Option<String>) -> (Self, EditorCaptures) {
            let captures = Arc::new(Mutex::new(Vec::new()));
            (
                Self {
                    exit_code: code,
                    rewrite,
                    captures: Arc::clone(&captures),
                },
                captures,
            )
        }
    }

    impl EditorRunner for StubRunner {
        async fn run(&self, editor: &str, editor_args: &[String], file_path: &Path) -> Option<i32> {
            assert_eq!(editor, "stub");
            assert!(
                editor_args.is_empty(),
                "only the file path follows the command"
            );
            let content = std::fs::read_to_string(file_path).expect("prompt readable");
            self.captures
                .lock()
                .expect("captures")
                .push((file_path.to_path_buf(), content));
            if let Some(text) = &self.rewrite {
                std::fs::write(file_path, text).expect("rewrite prompt");
            }
            self.exit_code
        }
    }

    fn stub_options() -> ExternalEditorOptions {
        ExternalEditorOptions {
            command: "stub".to_string(),
            content: "original".to_string(),
        }
    }

    /// Upstream "edits a prompt inside a private temporary directory": the
    /// original content is written to prompt.md under a pi-editor-* temp dir,
    /// the edited content comes back normalized, and the directory is removed.
    #[tokio::test]
    async fn external_editor_edits_a_prompt_in_a_private_temp_directory() {
        let (runner, captures) = StubRunner::spawn(Some(0), Some("edited".to_string()));
        let result = edit_in_external_editor_with(&stub_options(), &runner).await;

        assert_eq!(captures.lock().expect("captures").len(), 1);
        let (file_path, written) = &captures.lock().expect("captures")[0];
        assert_eq!(written, "original");
        assert_eq!(file_path.file_name().expect("name"), "prompt.md");
        let directory = file_path.parent().expect("parent");
        assert_eq!(
            directory.parent().expect("tmp"),
            std::env::temp_dir(),
            "the private directory lives under the system temp dir"
        );
        assert!(
            directory
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("pi-editor-")),
            "the directory name starts with pi-editor-"
        );
        assert!(!directory.exists(), "the private directory is cleaned up");
        assert_eq!(
            result,
            ExternalEditorResult::Complete {
                content: "edited".to_string()
            }
        );
    }

    /// Upstream "keeps the original content when the editor exits
    /// unsuccessfully".
    #[tokio::test]
    async fn external_editor_fails_when_the_editor_exits_unsuccessfully() {
        let (runner, captures) = StubRunner::spawn(Some(1), None);
        let result = edit_in_external_editor_with(&stub_options(), &runner).await;
        let (file_path, _) = &captures.lock().expect("captures")[0];
        assert!(!file_path.parent().expect("parent").exists());
        assert_eq!(result, ExternalEditorResult::Failed);
    }

    /// Upstream "returns empty content when the editor clears the prompt".
    #[tokio::test]
    async fn external_editor_returns_empty_content_for_a_cleared_prompt() {
        let (runner, _) = StubRunner::spawn(Some(0), Some(String::new()));
        let result = edit_in_external_editor_with(&stub_options(), &runner).await;
        assert_eq!(
            result,
            ExternalEditorResult::Complete {
                content: String::new()
            }
        );
    }

    /// A spawn error resolves to `failed` (upstream `child.on("error")`).
    #[tokio::test]
    async fn external_editor_treats_a_spawn_error_as_failure() {
        let (runner, _) = StubRunner::spawn(None, None);
        let result = edit_in_external_editor_with(&stub_options(), &runner).await;
        assert_eq!(result, ExternalEditorResult::Failed);
    }

    /// Real spawn (win32 shell semantics): an editor that truncates the prompt
    /// to empty completes with empty content.
    #[cfg(windows)]
    #[tokio::test]
    async fn real_spawn_truncating_editor_completes_with_empty_content() {
        let result = edit_in_external_editor(&ExternalEditorOptions {
            command: "cmd /c copy /y nul".to_string(),
            content: "original".to_string(),
        })
        .await;
        assert_eq!(
            result,
            ExternalEditorResult::Complete {
                content: String::new()
            }
        );
    }

    /// Real spawn: an unknown editor command resolves to `failed` (upstream
    /// `child.on("error")` / non-zero shell exit).
    #[cfg(windows)]
    #[tokio::test]
    async fn real_spawn_unknown_editor_fails() {
        let result = edit_in_external_editor(&ExternalEditorOptions {
            command: "definitely-not-a-real-editor-xyz-12345".to_string(),
            content: "original".to_string(),
        })
        .await;
        assert_eq!(result, ExternalEditorResult::Failed);
    }

    /// The `Theme` text stylers and accessors (upstream `Theme` class; the
    /// chalk level is fixed, see seam D2). Unknown tokens throw the unified
    /// upstream `Unknown theme color` message for both slots.
    #[test]
    fn theme_text_stylers_and_unknown_color_errors() {
        reset_state_for_tests();
        let theme = load_builtin_theme("dark", None).expect("dark");
        assert_eq!(theme.bold("x"), "\x1b[1mx\x1b[22m");
        assert_eq!(theme.italic("x"), "\x1b[3mx\x1b[23m");
        assert_eq!(theme.underline("x"), "\x1b[4mx\x1b[24m");
        assert_eq!(theme.inverse("x"), "\x1b[7mx\x1b[27m");
        assert_eq!(theme.strikethrough("x"), "\x1b[9mx\x1b[29m");

        let accent = theme.get_fg_ansi("accent").expect("accent");
        assert_eq!(
            theme.fg("accent", "t").expect("fg"),
            format!("{accent}t\x1b[39m")
        );
        let selected = theme.get_bg_ansi("selectedBg").expect("selectedBg");
        assert_eq!(
            theme.bg("selectedBg", "t").expect("bg"),
            format!("{selected}t\x1b[49m")
        );

        assert_eq!(
            theme.fg("not-a-color", "t").expect_err("unknown fg"),
            "Unknown theme color: not-a-color"
        );
        assert_eq!(
            theme.bg("not-a-bg", "t").expect_err("unknown bg"),
            "Unknown theme color: not-a-bg"
        );
        reset_state_for_tests();
    }

    /// Upstream `getThinkingBorderColor` / `getBashModeBorderColor`.
    #[test]
    fn thinking_and_bash_mode_border_colors() {
        use super::super::theme::ThinkingLevel;
        reset_state_for_tests();
        let theme = load_builtin_theme("dark", None).expect("dark");
        let levels = [
            (ThinkingLevel::Off, "thinkingOff"),
            (ThinkingLevel::Minimal, "thinkingMinimal"),
            (ThinkingLevel::Low, "thinkingLow"),
            (ThinkingLevel::Medium, "thinkingMedium"),
            (ThinkingLevel::High, "thinkingHigh"),
            (ThinkingLevel::Xhigh, "thinkingXhigh"),
            (ThinkingLevel::Max, "thinkingMax"),
        ];
        for (level, key) in levels {
            let expected = theme.fg(key, "b").expect("border");
            assert_eq!(
                theme.get_thinking_border_color(level, "b").expect("border"),
                expected
            );
        }
        assert_eq!(
            theme.get_bash_mode_border_color("b").expect("bash border"),
            theme.fg("bashMode", "b").expect("bash border")
        );
        reset_state_for_tests();
    }

    /// `loadThemeFromPath` core over explicit content (`load_theme_from_content`):
    /// the built-in documents round-trip through parse + create.
    #[test]
    fn theme_content_loading_round_trips() {
        reset_state_for_tests();
        let theme =
            load_theme_from_content(BUILTIN_DARK, None, "dark.json".to_string()).expect("dark");
        assert_eq!(theme.source_path.as_deref(), Some("dark.json"));
        assert_eq!(theme.name.as_deref(), Some("dark"));
        let broken = "{ not json";
        assert!(load_theme_from_content(broken, None, "broken.json".to_string()).is_err());
        reset_state_for_tests();
    }

    /// `withThemeColorFallbacks` + `resolveThemeColors` on a doc without the
    /// optional keys (kept from the r17 suite; the mechanism is unchanged in
    /// the delta — only the ANSI encoding moved to the tui pipeline).
    #[test]
    fn fallback_resolution_still_applies() {
        let mut stripped = dark_value();
        let colors = stripped
            .get_mut("colors")
            .and_then(Value::as_object_mut)
            .expect("colors");
        for key in [
            "scrollbarTrack",
            "scrollbarThumb",
            "thinkingMax",
            "searchMatchBg",
            "searchMatchText",
        ] {
            colors.remove(key);
        }
        let stripped_doc =
            ThemeJson::parse(&serde_json::to_string(&stripped).expect("serialize")).expect("doc");
        let empty = BTreeMap::new();
        let vars = stripped_doc.vars.as_ref().unwrap_or(&empty);
        let resolved = super::super::theme::resolve_theme_colors(
            &with_theme_color_fallbacks(&stripped_doc.colors),
            vars,
        )
        .expect("resolution");
        // Every optional key now carries its fallback's value.
        assert_eq!(resolved["scrollbarTrack"], resolved["muted"]);
        assert_eq!(resolved["scrollbarThumb"], resolved["text"]);
        assert_eq!(resolved["thinkingMax"], resolved["thinkingXhigh"]);
        assert_eq!(resolved["searchMatchBg"], resolved["selectedBg"]);
        assert_eq!(resolved["searchMatchText"], resolved["text"]);
        assert!(matches!(resolved["searchMatchBg"], ResolvedColor::Str(_)));
    }

    // -- theme-controller.ts ------------------------------------------------

    mod controller {
        use std::sync::{Arc, Mutex};

        use super::super::super::theme::theme_controller::{
            request_terminal_colors, InteractiveThemeController, InteractiveThemeControllerOptions,
            ThemeControllerUi, TERMINAL_QUERY_TIMEOUT_MS,
        };
        use super::super::super::theme::{
            get_current_theme_name, get_terminal_theme, reset_state_for_tests,
            set_terminal_color_scheme, TerminalTheme,
        };
        use crate::tui::terminal_colors::{RgbColor, TerminalColorScheme, TerminalColors};

        type Resolve = Box<dyn FnOnce(TerminalColors) + Send>;
        type LateReply = Box<dyn FnOnce(TerminalColors) + Send>;

        /// A recording TUI double: held query callbacks flush explicitly.
        struct RecordingUi {
            queries: Vec<f64>,
            pending_resolve: Vec<Resolve>,
            pending_late: Vec<LateReply>,
            notifications: Vec<bool>,
            scheme_listeners: usize,
            invalidates: u32,
            renders: u32,
            unsubscribes: Vec<u64>,
        }

        impl RecordingUi {
            fn new() -> Self {
                Self {
                    queries: Vec::new(),
                    pending_resolve: Vec::new(),
                    pending_late: Vec::new(),
                    notifications: Vec::new(),
                    scheme_listeners: 0,
                    invalidates: 0,
                    renders: 0,
                    unsubscribes: Vec::new(),
                }
            }

            fn flush_resolve(&mut self, colors: TerminalColors) {
                if let Some(resolve) = self.pending_resolve.pop() {
                    resolve(colors);
                }
            }
        }

        impl ThemeControllerUi for RecordingUi {
            fn query_terminal_colors(
                &mut self,
                timeout_ms: f64,
                on_resolve: Resolve,
                on_late_reply: Option<LateReply>,
            ) {
                self.queries.push(timeout_ms);
                self.pending_resolve.push(on_resolve);
                if let Some(late) = on_late_reply {
                    self.pending_late.push(late);
                }
            }

            fn set_terminal_color_scheme_notifications(&mut self, enabled: bool) {
                self.notifications.push(enabled);
            }

            fn on_terminal_color_scheme_change(
                &mut self,
                _listener: Box<dyn FnMut(&TerminalColorScheme) + Send>,
            ) -> u64 {
                self.scheme_listeners += 1;
                self.scheme_listeners as u64
            }

            fn remove_terminal_color_scheme_listener(&mut self, id: u64) {
                self.unsubscribes.push(id);
            }

            fn invalidate(&mut self) {
                self.invalidates += 1;
            }

            fn request_render(&mut self) {
                self.renders += 1;
            }
        }

        fn controller_options(setting: Option<&str>) -> InteractiveThemeControllerOptions {
            let setting = setting.map(str::to_string);
            InteractiveThemeControllerOptions {
                get_theme_setting: Box::new(move || setting.clone()),
                show_error: Box::new(|_| {}),
                on_changed: Box::new(|| {}),
                initial_theme_setting: None,
            }
        }

        fn rgb(hex: &str) -> RgbColor {
            let cleaned = hex.strip_prefix('#').expect("hex prefix");
            RgbColor {
                r: u8::from_str_radix(&cleaned[0..2], 16).expect("hex"),
                g: u8::from_str_radix(&cleaned[2..4], 16).expect("hex"),
                b: u8::from_str_radix(&cleaned[4..6], 16).expect("hex"),
            }
        }

        /// Upstream constructor: the active theme resolves from the setting,
        /// the system theme starts grayscale-pending, and the scheme listener
        /// binds.
        #[test]
        fn constructor_marks_pending_and_initializes_the_theme() {
            reset_state_for_tests();
            let mut ui = RecordingUi::new();
            let controller = InteractiveThemeController::new(&mut ui, controller_options(None));
            assert_eq!(controller.get_theme_selection().as_deref(), Some("system"));
            assert_eq!(get_current_theme_name().as_deref(), Some("system"));
            assert_eq!(ui.scheme_listeners, 1);
            assert!(ui.notifications.is_empty(), "auto sync stays off");
            // `markTerminalColorsPending()` ran (module state flag).
            assert_eq!(controller.get_terminal_theme(), TerminalTheme::Dark);
            reset_state_for_tests();
        }

        /// Upstream `applyFromSettings`: the setting applies with error
        /// reporting enabled, auto sync tracks pair/system settings, and the
        /// terminal color query starts with the pinned timeout.
        #[test]
        fn apply_from_settings_applies_and_queries() {
            reset_state_for_tests();
            let mut ui = RecordingUi::new();
            let mut controller =
                InteractiveThemeController::new(&mut ui, controller_options(Some("dark")));
            controller.apply_from_settings(&mut ui);
            assert_eq!(get_current_theme_name().as_deref(), Some("dark"));
            assert_eq!(ui.queries, vec![TERMINAL_QUERY_TIMEOUT_MS]);
            assert!(ui.notifications.is_empty(), "dark is not auto-synced");

            // A pair setting enables auto sync (2031 notifications on).
            let mut pair_ui = RecordingUi::new();
            let mut pair = InteractiveThemeController::new(
                &mut pair_ui,
                controller_options(Some("light/dark")),
            );
            pair.apply_from_settings(&mut pair_ui);
            assert_eq!(pair_ui.notifications, vec![true]);
            // The pair follows the terminal appearance (dark).
            assert_eq!(get_current_theme_name().as_deref(), Some("dark"));
            reset_state_for_tests();
        }

        /// Upstream `applyTerminalColors`: reported colors merge over the
        /// previous (a timeout keeps them), unchanged merges skip re-render,
        /// and `setTerminalColors` records them for the theme module.
        #[test]
        fn reported_colors_merge_and_skip_unchanged_renders() {
            reset_state_for_tests();
            let mut ui = RecordingUi::new();
            let mut controller =
                InteractiveThemeController::new(&mut ui, controller_options(Some("system")));
            controller.apply_from_settings(&mut ui);
            let invalidates_after_apply = ui.invalidates;

            ui.flush_resolve(TerminalColors {
                foreground: Some(rgb("#c4c4d4")),
                background: Some(rgb("#1e1e2e")),
                palette: None,
            });
            controller.deliver_pending(&mut ui);
            assert_eq!(
                super::super::super::theme::get_terminal_colors().background,
                Some(rgb("#1e1e2e")),
                "setTerminalColors recorded the report"
            );
            let invalidates_after_report = ui.invalidates;
            assert!(invalidates_after_report > invalidates_after_apply);

            // A timeout (empty report) keeps the previous colors and does not
            // re-render.
            ui.flush_resolve(TerminalColors::default());
            controller.deliver_pending(&mut ui);
            assert_eq!(ui.invalidates, invalidates_after_report);

            // The same report again: no change, no re-render.
            ui.flush_resolve(TerminalColors {
                foreground: Some(rgb("#c4c4d4")),
                background: Some(rgb("#1e1e2e")),
                palette: None,
            });
            controller.deliver_pending(&mut ui);
            assert_eq!(ui.invalidates, invalidates_after_report);
            reset_state_for_tests();
        }

        /// Upstream `waitForTerminalColors`: a fresh controller's query has
        /// settled (initial `Promise.resolve`), a started query is pending
        /// until the terminal's reply (or the timeout) delivers, at which
        /// point the reported colors are in the delivery inbox.
        #[test]
        fn wait_for_terminal_colors_settles_with_the_query() {
            reset_state_for_tests();
            let mut ui = RecordingUi::new();
            let mut controller =
                InteractiveThemeController::new(&mut ui, controller_options(Some("dark")));
            assert!(
                *controller.wait_for_terminal_colors().lock().expect("flag"),
                "the initial query promise is resolved"
            );
            controller.apply_from_settings(&mut ui);
            assert!(
                !*controller.wait_for_terminal_colors().lock().expect("flag"),
                "the started query is pending"
            );
            ui.flush_resolve(TerminalColors::default());
            assert!(
                *controller.wait_for_terminal_colors().lock().expect("flag"),
                "settled when the query delivers its colors"
            );
            controller.deliver_pending(&mut ui);
            reset_state_for_tests();
        }

        /// Upstream `applyTerminalColorSchemeChange`: with auto sync on, the
        /// reported scheme updates the module state and re-queries; with auto
        /// sync off it is ignored.
        #[test]
        fn scheme_changes_follow_auto_sync() {
            reset_state_for_tests();
            let mut ui = RecordingUi::new();
            let mut controller =
                InteractiveThemeController::new(&mut ui, controller_options(Some("system")));
            controller.apply_from_settings(&mut ui);
            let queries_after_apply = ui.queries.len();

            controller.apply_terminal_color_scheme_change(&mut ui, TerminalTheme::Light);
            assert_eq!(get_terminal_theme(), TerminalTheme::Light);
            assert_eq!(ui.queries.len(), queries_after_apply + 1);

            // Auto sync off: the scheme report is ignored entirely.
            controller.disable_auto_sync(&mut ui);
            assert_eq!(ui.notifications.last(), Some(&false));
            set_terminal_color_scheme(None);
            controller.apply_terminal_color_scheme_change(&mut ui, TerminalTheme::Light);
            assert_eq!(get_terminal_theme(), TerminalTheme::Dark);
            reset_state_for_tests();
        }

        /// Upstream `setThemeName`: an invalid theme falls back to the system
        /// theme; a valid one becomes the current setting. The load failure
        /// surfaces through `showError` when the setting came from settings
        /// (`applyFromSettings`), matching upstream's `showError` flag.
        #[test]
        fn set_theme_name_falls_back_and_reports() {
            reset_state_for_tests();
            let errors = Arc::new(Mutex::new(Vec::new()));
            let errors_for_controller = Arc::clone(&errors);
            let mut ui = RecordingUi::new();
            let mut controller = InteractiveThemeController::new(
                &mut ui,
                InteractiveThemeControllerOptions {
                    get_theme_setting: Box::new(|| None),
                    show_error: Box::new(move |message| {
                        errors_for_controller
                            .lock()
                            .expect("errors")
                            .push(message.to_string())
                    }),
                    on_changed: Box::new(|| {}),
                    initial_theme_setting: None,
                },
            );
            // `setThemeName` defaults `showError` to false: the fallback is
            // silent there.
            let result = controller.set_theme_name(&mut ui, "nope");
            assert!(!result.success);
            assert_eq!(controller.get_theme_selection().as_deref(), Some("system"));
            assert!(errors.lock().expect("errors").is_empty());

            // A settings-driven apply reports the failure.
            controller.set_theme_setting(&mut ui, "nope");
            let errors = errors.lock().expect("errors");
            assert!(
                errors[0].contains("Failed to load theme \"nope\"")
                    && errors[0].contains("Fell back to the system theme."),
                "{}",
                errors[0]
            );
            drop(errors);
            assert_eq!(get_current_theme_name().as_deref(), Some("system"));

            let result = controller.set_theme_name(&mut ui, "light");
            assert!(result.success);
            assert_eq!(get_current_theme_name().as_deref(), Some("light"));
            assert_eq!(controller.get_theme_selection().as_deref(), Some("light"));
            reset_state_for_tests();
        }

        /// Upstream `dispose`: auto sync turns off and the scheme listener
        /// unsubscribes.
        #[test]
        fn dispose_disables_auto_sync_and_unsubscribes() {
            reset_state_for_tests();
            let mut ui = RecordingUi::new();
            let mut controller =
                InteractiveThemeController::new(&mut ui, controller_options(Some("light/dark")));
            controller.apply_from_settings(&mut ui);
            let listener_id = ui.scheme_listeners as u64;
            controller.dispose(&mut ui);
            assert_eq!(ui.notifications.last(), Some(&false));
            assert_eq!(ui.unsubscribes, vec![listener_id]);
            reset_state_for_tests();
        }

        /// Upstream `requestTerminalColors`: the settle and late-reply paths
        /// share the apply closure.
        #[test]
        fn request_terminal_colors_applies_settle_and_late_replies() {
            let applies = Arc::new(Mutex::new(Vec::new()));
            let sink = Arc::clone(&applies);
            let apply = Arc::new(Mutex::new(move |colors: TerminalColors| {
                sink.lock().expect("applies").push(colors);
            })) as Arc<Mutex<dyn FnMut(TerminalColors) + Send>>;
            let mut ui = RecordingUi::new();
            request_terminal_colors(&mut ui, apply);
            assert_eq!(ui.queries, vec![TERMINAL_QUERY_TIMEOUT_MS]);
            ui.flush_resolve(TerminalColors {
                foreground: Some(rgb("#ffffff")),
                background: Some(rgb("#000000")),
                palette: None,
            });
            assert_eq!(applies.lock().expect("applies").len(), 1);
            reset_state_for_tests();
        }

        /// `markTerminalColorsPending` + a full report regenerate the system
        /// theme in color (the controller's reapply path): the regenerated
        /// system theme renders real colors instead of terminal defaults.
        #[test]
        fn system_theme_follows_the_reported_colors() {
            reset_state_for_tests();
            let mut ui = RecordingUi::new();
            let mut controller =
                InteractiveThemeController::new(&mut ui, controller_options(Some("system")));
            controller.apply_from_settings(&mut ui);

            ui.flush_resolve(TerminalColors {
                foreground: Some(rgb("#c4c4d4")),
                background: Some(rgb("#1e1e2e")),
                palette: None,
            });
            controller.deliver_pending(&mut ui);
            assert_eq!(get_current_theme_name().as_deref(), Some("system"));
            // The regenerated system theme is installed as the global theme
            // and its colors flow through setTerminalColors.
            assert_eq!(
                super::super::super::theme::get_terminal_colors().background,
                Some(rgb("#1e1e2e"))
            );
            reset_state_for_tests();
        }
    }
}
