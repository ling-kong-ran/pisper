//! Tests for the compaction utils: conversation serialization (oracle
//! "serializes conversation with truncated tool results",
//! compaction.test.ts:423-438), file-operation extraction and list
//! computation (exercised end-to-end by the compaction result oracle, pinned
//! here at unit level).

use super::*;
use crate::agent_core::harness::messages::convert_to_llm;
use crate::agent_core::types::AgentMessage;
use crate::ai::types::content::ToolCall;
use crate::ai::types::message::{
    AssistantBlock, AssistantMessage, Message, StringOrBlocks, TextOrImageBlock, ToolResultMessage,
    UserMessage,
};
use crate::ai::types::primitives::{StopReason, Usage, UsageCost};
use crate::ai::types::{TextContent, ThinkingContent};

fn tool_result_message(text: &str) -> AgentMessage {
    AgentMessage::ToolResult(ToolResultMessage {
        tool_call_id: "tc1".to_string(),
        tool_name: "read".to_string(),
        content: vec![TextOrImageBlock::Text(TextContent {
            text: text.to_string(),
            text_signature: None,
        })],
        details: None,
        usage: None,
        is_error: false,
        timestamp: 1,
    })
}

/// Oracle "serializes conversation with truncated tool results".
#[test]
fn serializes_conversation_with_truncated_tool_results() {
    let long_content = "x".repeat(5000);
    let messages = convert_to_llm(&[tool_result_message(&long_content)]);
    let result = serialize_conversation(&messages);
    assert!(result.contains("[Tool result]:"));
    assert!(result.contains("[... 3000 more characters truncated]"));
}

/// The user/assistant/toolResult sectioning of `serializeConversation`
/// (utils.ts:91-132).
#[test]
fn serialize_conversation_sections_roles() {
    let messages = vec![
        Message::User(UserMessage {
            content: StringOrBlocks::Text("hello there".to_string()),
            timestamp: 1,
        }),
        Message::Assistant(AssistantMessage {
            content: vec![
                AssistantBlock::Thinking(ThinkingContent {
                    thinking: "pondering".to_string(),
                    thinking_signature: None,
                    redacted: None,
                }),
                AssistantBlock::Text(TextContent {
                    text: "answer".to_string(),
                    text_signature: None,
                }),
                AssistantBlock::ToolCall(ToolCall {
                    id: "call".to_string(),
                    name: "read".to_string(),
                    arguments: serde_json::json!({"path": "a.ts", "n": 2}),
                    thought_signature: None,
                    namespace: None,
                }),
            ],
            api: "faux".to_string(),
            provider: "faux".to_string(),
            model: "faux-1".to_string(),
            response_model: None,
            response_id: None,
            provider_thinking_level: None,
            diagnostics: None,
            usage: Usage {
                input: 0,
                output: 0,
                cache_read: 0,
                cache_write: 0,
                cache_write_1h: None,
                reasoning: None,
                total_tokens: 0,
                cost: UsageCost::default(),
            },
            stop_reason: StopReason::Stop,
            deferred: None,
            error_message: None,
            raw_stop_reason: None,
            end_turn: None,
            timestamp: 2,
        }),
    ];
    let text = serialize_conversation(&messages);
    // Upstream Object.entries(args) retains the caller's parameter order.
    assert_eq!(
        text,
        "[User]: hello there\n\n\
         [Assistant thinking]: pondering\n\n\
         [Assistant]: answer\n\n\
         [Assistant tool calls]: read(path=\"a.ts\", n=2)"
    );

    // Empty user content is skipped (`if (content)` upstream); an assistant
    // message with no text blocks contributes no `[Assistant]:` line.
    let empty = serialize_conversation(&[Message::User(UserMessage {
        content: StringOrBlocks::Text(String::new()),
        timestamp: 1,
    })]);
    assert_eq!(empty, "");
}

/// File-operation extraction from assistant tool calls and the sorted list
/// computation (utils.ts:24-58): read-only files exclude modified ones and
/// both lists sort.
#[test]
fn extracts_and_computes_file_operations() {
    let mut file_ops = FileOperations::new();
    file_ops.read.insert("c.ts".to_string());
    file_ops.written.insert("b.ts".to_string());
    file_ops.edited.insert("a.ts".to_string());

    let lists = compute_file_lists(&file_ops);
    assert_eq!(lists.read_files, vec!["c.ts".to_string()]);
    assert_eq!(
        lists.modified_files,
        vec!["a.ts".to_string(), "b.ts".to_string()]
    );

    // A file both read and edited reports only under modified.
    file_ops.edited.insert("c.ts".to_string());
    let lists = compute_file_lists(&file_ops);
    assert!(lists.read_files.is_empty());
    assert_eq!(
        lists.modified_files,
        vec!["a.ts".to_string(), "b.ts".to_string(), "c.ts".to_string()]
    );
}

/// Tool-call extraction only counts `read`/`write`/`edit` calls carrying a
/// string `path` (utils.ts:24-51).
#[test]
fn extracts_file_ops_from_assistant_tool_calls() {
    let mut file_ops = FileOperations::new();
    let assistant = AgentMessage::Assistant(AssistantMessage {
        content: vec![
            AssistantBlock::ToolCall(ToolCall {
                id: "1".to_string(),
                name: "read".to_string(),
                arguments: serde_json::json!({"path": "read.ts"}),
                thought_signature: None,
                namespace: None,
            }),
            AssistantBlock::ToolCall(ToolCall {
                id: "2".to_string(),
                name: "write".to_string(),
                arguments: serde_json::json!({"path": "write.ts"}),
                thought_signature: None,
                namespace: None,
            }),
            AssistantBlock::ToolCall(ToolCall {
                id: "3".to_string(),
                name: "edit".to_string(),
                arguments: serde_json::json!({"path": "edit.ts"}),
                thought_signature: None,
                namespace: None,
            }),
            AssistantBlock::ToolCall(ToolCall {
                id: "4".to_string(),
                name: "bash".to_string(),
                arguments: serde_json::json!({"path": "ignored.ts"}),
                thought_signature: None,
                namespace: None,
            }),
            AssistantBlock::ToolCall(ToolCall {
                id: "5".to_string(),
                name: "read".to_string(),
                arguments: serde_json::json!({"other": true}),
                thought_signature: None,
                namespace: None,
            }),
            AssistantBlock::Text(TextContent {
                text: "note".to_string(),
                text_signature: None,
            }),
        ],
        ..assistant_shell()
    });
    extract_file_ops_from_message(&assistant, &mut file_ops);
    assert_eq!(file_ops.read, ["read.ts".to_string()].into_iter().collect());
    assert_eq!(
        file_ops.written,
        ["write.ts".to_string()].into_iter().collect()
    );
    assert_eq!(
        file_ops.edited,
        ["edit.ts".to_string()].into_iter().collect()
    );

    // Non-assistant messages contribute nothing.
    extract_file_ops_from_message(
        &AgentMessage::User(UserMessage {
            content: StringOrBlocks::Text("hi".to_string()),
            timestamp: 1,
        }),
        &mut file_ops,
    );
    assert_eq!(file_ops.read, ["read.ts".to_string()].into_iter().collect());
}

fn assistant_shell() -> AssistantMessage {
    AssistantMessage {
        content: vec![],
        api: "faux".to_string(),
        provider: "faux".to_string(),
        model: "faux-1".to_string(),
        response_model: None,
        response_id: None,
        provider_thinking_level: None,
        diagnostics: None,
        usage: Usage {
            input: 0,
            output: 0,
            cache_read: 0,
            cache_write: 0,
            cache_write_1h: None,
            reasoning: None,
            total_tokens: 0,
            cost: UsageCost::default(),
        },
        stop_reason: StopReason::Stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 1,
    }
}

/// `formatFileOperations` (utils.ts:62-72): sections in read/modified order,
/// prefixed by a blank line, empty when nothing to report.
#[test]
fn formats_file_operations() {
    assert_eq!(format_file_operations(&[], &[]), "");
    assert_eq!(
        format_file_operations(&["r.ts".to_string()], &[]),
        "\n\n<read-files>\nr.ts\n</read-files>"
    );
    assert_eq!(
        format_file_operations(&[], &["m.ts".to_string()]),
        "\n\n<modified-files>\nm.ts\n</modified-files>"
    );
    assert_eq!(
        format_file_operations(&["r.ts".to_string()], &["m.ts".to_string()]),
        "\n\n<read-files>\nr.ts\n</read-files>\n\n<modified-files>\nm.ts\n</modified-files>"
    );
}

/// `addUsage` (upstream harness/utils/usage.ts:18-43): componentwise sums;
/// `cacheWrite1h`/`reasoning` stay absent only while both sides are absent.
#[test]
fn add_usage_sums_components_and_optional_fields() {
    fn usage(
        input: u64,
        output: u64,
        cache_read: u64,
        cache_write: u64,
        total_tokens: u64,
        cache_write_1h: Option<u64>,
        reasoning: Option<u64>,
    ) -> Usage {
        Usage {
            input,
            output,
            cache_read,
            cache_write,
            cache_write_1h,
            reasoning,
            total_tokens,
            cost: UsageCost::default(),
        }
    }
    let left = usage(1, 2, 3, 4, 10, None, Some(1));
    let right = usage(5, 6, 7, 8, 26, Some(2), None);
    let summed = add_usage(left, right);
    assert_eq!(summed.input, 6);
    assert_eq!(summed.output, 8);
    assert_eq!(summed.cache_read, 10);
    assert_eq!(summed.cache_write, 12);
    assert_eq!(summed.total_tokens, 36);
    assert_eq!(summed.cache_write_1h, Some(2));
    assert_eq!(summed.reasoning, Some(1));

    let both_absent = add_usage(
        usage(0, 0, 0, 0, 0, None, None),
        usage(0, 0, 0, 0, 0, None, None),
    );
    assert_eq!(both_absent.cache_write_1h, None);
    assert_eq!(both_absent.reasoning, None);
}
