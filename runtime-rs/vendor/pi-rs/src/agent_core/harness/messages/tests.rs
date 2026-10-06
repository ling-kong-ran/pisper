//! Tests for the `messages.ts` port: exact text rendering, wire round-trips
//! through the custom-message capture, and the `convertToLlm` projection.
//! Derived from the upstream source (`messages.ts:4-169`) and its oracle
//! usages (`test/agent-loop.test.ts` bash-execution rendering,
//! `test/harness/runtime/lane.test.ts`).

use super::*;
use crate::ai::types::content::{ImageContent, TextContent};
use crate::ai::types::message::{StringOrBlocks, TextOrImageBlock};

const TS: i64 = 1_758_240_000_000;

fn bash_msg() -> BashExecutionMessage {
    BashExecutionMessage {
        command: "ls -la".into(),
        output: String::new(),
        exit_code: None,
        cancelled: false,
        truncated: false,
        full_output_path: None,
        timestamp: TS,
        exclude_from_context: None,
    }
}

#[test]
fn summary_constants_match_upstream_strings() {
    // messages.ts:4-17 — exact literals including the suffix asymmetry
    // (compaction has a leading newline, branch does not).
    assert_eq!(
        COMPACTION_SUMMARY_PREFIX,
        "The conversation history before this point was compacted into the following summary:\n\n<summary>\n"
    );
    assert_eq!(COMPACTION_SUMMARY_SUFFIX, "\n</summary>");
    assert_eq!(
        BRANCH_SUMMARY_PREFIX,
        "The following is a summary of a branch that this conversation came back from:\n\n<summary>\n"
    );
    assert_eq!(BRANCH_SUMMARY_SUFFIX, "</summary>");
}

#[test]
fn bash_execution_to_text_renders_the_upstream_variants() {
    // messages.ts:63-79, branch by branch.
    let mut msg = bash_msg();
    assert_eq!(bash_execution_to_text(&msg), "Ran `ls -la`\n(no output)");

    msg.output = "total 0".into();
    assert_eq!(
        bash_execution_to_text(&msg),
        "Ran `ls -la`\n```\ntotal 0\n```"
    );

    msg.cancelled = true;
    assert_eq!(
        bash_execution_to_text(&msg),
        "Ran `ls -la`\n```\ntotal 0\n```\n\n(command cancelled)"
    );
    msg.cancelled = false;
    msg.exit_code = Some(0);
    assert_eq!(
        bash_execution_to_text(&msg),
        "Ran `ls -la`\n```\ntotal 0\n```"
    );
    msg.exit_code = Some(2);
    assert_eq!(
        bash_execution_to_text(&msg),
        "Ran `ls -la`\n```\ntotal 0\n```\n\nCommand exited with code 2"
    );

    msg.truncated = true;
    assert_eq!(
        bash_execution_to_text(&msg),
        "Ran `ls -la`\n```\ntotal 0\n```\n\nCommand exited with code 2"
    );
    msg.full_output_path = Some("/tmp/pi-out".into());
    assert_eq!(
        bash_execution_to_text(&msg),
        "Ran `ls -la`\n```\ntotal 0\n```\n\nCommand exited with code 2\n\n[Output truncated. Full output: /tmp/pi-out]"
    );

    // Cancelled wins over a non-zero exit code (messages.ts:70-74).
    let mut cancelled = bash_msg();
    cancelled.exit_code = Some(1);
    cancelled.cancelled = true;
    assert_eq!(
        bash_execution_to_text(&cancelled),
        "Ran `ls -la`\n(no output)\n\n(command cancelled)"
    );
}

#[test]
fn bash_execution_message_round_trips_the_upstream_wire_shape() {
    let msg = bash_msg();
    let custom = msg.to_custom();
    assert_eq!(custom.role, "bashExecution");
    let json = serde_json::to_value(&custom).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "role": "bashExecution",
            "command": "ls -la",
            "output": "",
            "cancelled": false,
            "truncated": false,
            "timestamp": TS,
        })
    );
    let parsed = BashExecutionMessage::from_custom(&custom).unwrap();
    assert_eq!(parsed, msg);

    // Every populated field round-trips, including the optional ones.
    let full = BashExecutionMessage {
        exit_code: Some(3),
        full_output_path: Some("/tmp/full".into()),
        exclude_from_context: Some(true),
        ..bash_msg()
    };
    let parsed = BashExecutionMessage::from_custom(&full.to_custom()).unwrap();
    assert_eq!(parsed, full);
}

#[test]
fn branch_and_compaction_creators_and_round_trips() {
    // messages.ts:81-92.
    let branch = create_branch_summary_message("did things".to_string(), None, TS);
    assert_eq!(branch.summary, "did things");
    assert_eq!(branch.from_id, None);
    assert_eq!(branch.timestamp, TS);
    let branch_json = serde_json::to_value(branch.to_custom()).unwrap();
    assert_eq!(
        branch_json,
        serde_json::json!({
            "role": "branchSummary",
            "summary": "did things",
            "fromId": serde_json::Value::Null,
            "timestamp": TS,
        })
    );
    assert_eq!(
        BranchSummaryMessage::from_custom(&branch.to_custom()).unwrap(),
        branch
    );

    // messages.ts:94-105.
    let compaction = create_compaction_summary_message("old history".to_string(), 4_321, TS);
    assert_eq!(compaction.tokens_before, 4_321);
    let compaction_json = serde_json::to_value(compaction.to_custom()).unwrap();
    assert_eq!(
        compaction_json,
        serde_json::json!({
            "role": "compactionSummary",
            "summary": "old history",
            "tokensBefore": 4321,
            "timestamp": TS,
        })
    );
    assert_eq!(
        CompactionSummaryMessage::from_custom(&compaction.to_custom()).unwrap(),
        compaction
    );
}

#[test]
fn custom_message_creator_and_round_trip() {
    // messages.ts:107-122, string content with details.
    let message = create_custom_message(
        "notification",
        StringOrBlocks::Text("Info".into()),
        true,
        Some(serde_json::json!({"priority": 1})),
        TS,
    );
    assert_eq!(message.custom_type, "notification");
    assert!(message.display);
    let json = serde_json::to_value(message.to_custom()).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "role": "custom",
            "customType": "notification",
            "content": "Info",
            "display": true,
            "details": {"priority": 1},
            "timestamp": TS,
        })
    );
    assert_eq!(
        CustomMessage::from_custom(&message.to_custom()).unwrap(),
        message
    );

    // Block content round-trips through the same union as user messages.
    let blocks = StringOrBlocks::Blocks(vec![
        TextOrImageBlock::Text(TextContent {
            text: "see attachment".into(),
            text_signature: None,
        }),
        TextOrImageBlock::Image(ImageContent {
            data: "aGVsbG8=".into(),
            mime_type: "image/png".into(),
        }),
    ]);
    let block_message = create_custom_message("gallery", blocks, false, None, TS);
    let parsed = CustomMessage::from_custom(&block_message.to_custom()).unwrap();
    assert_eq!(parsed.content, block_message.content);
    assert_eq!(parsed.details, None);
}

#[test]
fn convert_to_llm_passes_standard_roles_through() {
    let user: AgentMessage = serde_json::from_value(
        serde_json::json!({"role": "user", "content": "hi", "timestamp": TS}),
    )
    .unwrap();
    let converted = convert_to_llm(std::slice::from_ref(&user));
    assert_eq!(converted.len(), 1);
    assert_eq!(
        converted[0],
        Message::User(UserMessage {
            content: StringOrBlocks::Text("hi".into()),
            timestamp: TS,
        })
    );
    // Assistant and toolResult pass through unchanged too.
    let assistant: AgentMessage = serde_json::from_value(serde_json::json!({
        "role": "assistant", "content": [], "api": "anthropic-messages",
        "provider": "anthropic", "model": "m", "usage": {"input": 0, "output": 0,
        "cacheRead": 0, "cacheWrite": 0, "totalTokens": 0, "cost": {"input": 0,
        "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0}},
        "stopReason": "stop", "timestamp": TS
    }))
    .unwrap();
    let converted = convert_to_llm(&[assistant]);
    assert!(matches!(converted[0], Message::Assistant(_)));
}

#[test]
fn convert_to_llm_renders_bash_executions_as_user_messages() {
    // messages.ts:128-137.
    let mut msg = bash_msg();
    msg.output = "done".into();
    msg.exit_code = Some(0);
    let converted = convert_to_llm(&[AgentMessage::Custom(msg.to_custom())]);
    assert_eq!(converted.len(), 1);
    let Message::User(user) = &converted[0] else {
        panic!("expected user message, got {:?}", converted[0]);
    };
    assert_eq!(
        user.content,
        StringOrBlocks::Text("Ran `ls -la`\n```\ndone\n```".into())
    );
    assert_eq!(user.timestamp, TS);

    // messages.ts:128-130: excludeFromContext drops the message entirely.
    msg.exclude_from_context = Some(true);
    let converted = convert_to_llm(&[AgentMessage::Custom(msg.to_custom())]);
    assert!(converted.is_empty());
}

#[test]
fn convert_to_llm_projects_custom_content_and_summaries() {
    // messages.ts:137-145: string content becomes a single text block;
    // block content passes through as-is.
    let message =
        create_custom_message("note", StringOrBlocks::Text("hello".into()), true, None, TS);
    let converted = convert_to_llm(&[AgentMessage::Custom(message.to_custom())]);
    let Message::User(user) = &converted[0] else {
        panic!("expected user message");
    };
    assert_eq!(user.content, StringOrBlocks::Text("hello".into()));

    let blocks = StringOrBlocks::Blocks(vec![TextOrImageBlock::Text(TextContent {
        text: "block".into(),
        text_signature: None,
    })]);
    let block_message = create_custom_message("note", blocks.clone(), true, None, TS);
    let converted = convert_to_llm(&[AgentMessage::Custom(block_message.to_custom())]);
    let Message::User(user) = &converted[0] else {
        panic!("expected user message");
    };
    assert_eq!(user.content, blocks);

    // messages.ts:145-158: summaries are wrapped in their XML envelopes.
    let branch = create_branch_summary_message("the branch".to_string(), Some("b1".into()), TS);
    let converted = convert_to_llm(&[AgentMessage::Custom(branch.to_custom())]);
    let Message::User(user) = &converted[0] else {
        panic!("expected user message");
    };
    assert_eq!(
        user.content,
        StringOrBlocks::Text(format!(
            "{BRANCH_SUMMARY_PREFIX}the branch{BRANCH_SUMMARY_SUFFIX}"
        ))
    );

    let compaction = create_compaction_summary_message("old".to_string(), 10, TS);
    let converted = convert_to_llm(&[AgentMessage::Custom(compaction.to_custom())]);
    let Message::User(user) = &converted[0] else {
        panic!("expected user message");
    };
    assert_eq!(
        user.content,
        StringOrBlocks::Text(format!(
            "{COMPACTION_SUMMARY_PREFIX}old{COMPACTION_SUMMARY_SUFFIX}"
        ))
    );
}

#[test]
fn convert_to_llm_drops_unknown_custom_roles() {
    // messages.ts:164-165: the `default` branch returns undefined.
    let unknown = CustomAgentMessage {
        role: "notification".into(),
        data: serde_json::json!({"text": "Info"})
            .as_object()
            .cloned()
            .unwrap(),
    };
    let converted = convert_to_llm(&[AgentMessage::Custom(unknown)]);
    assert!(converted.is_empty());
}

#[test]
fn convert_to_llm_preserves_order_across_roles() {
    let messages = vec![
        AgentMessage::Custom(
            create_custom_message("note", StringOrBlocks::Text("a".into()), true, None, TS)
                .to_custom(),
        ),
        AgentMessage::Custom(create_branch_summary_message("b".to_string(), None, TS).to_custom()),
        AgentMessage::Custom(bash_msg().to_custom()),
    ];
    let converted = convert_to_llm(&messages);
    assert_eq!(converted.len(), 3);
    for message in &converted {
        assert!(matches!(message, Message::User(_)));
    }
}
