//! Tests for the ported `coding-agent/src/core/messages.ts`: byte-level
//! constants, `bashExecutionToText` over the oracle scenario battery,
//! constructor behavior (including ISO timestamp parsing), and `convertToLlm`
//! producing byte-identical serialization against the real upstream module
//! (`tests/fixtures/core_oracle/messages.oracle.json`).

use serde_json::Value;

use super::{
    bash_execution_to_text, convert_to_llm, create_branch_summary_message,
    create_compaction_summary_message, create_custom_message, parse_epoch_millis,
    BashExecutionMessage, BranchSummaryMessage, CustomMessageContent, BRANCH_SUMMARY_PREFIX,
    BRANCH_SUMMARY_SUFFIX, COMPACTION_SUMMARY_PREFIX, COMPACTION_SUMMARY_SUFFIX,
};
use crate::agent_core::types::{AgentMessage, CustomAgentMessage};
use crate::coding_agent::core::oracle_data;

fn oracle() -> Value {
    serde_json::from_str(oracle_data::MESSAGES).unwrap()
}

fn bash_message(case: &Value) -> BashExecutionMessage {
    let msg = &case["msg"];
    BashExecutionMessage {
        command: msg["command"].as_str().unwrap().to_string(),
        output: msg["output"].as_str().unwrap().to_string(),
        exit_code: msg["exitCode"].as_i64(),
        cancelled: msg["cancelled"].as_bool().unwrap(),
        truncated: msg["truncated"].as_bool().unwrap(),
        full_output_path: msg["fullOutputPath"].as_str().map(str::to_string),
        timestamp: msg["timestamp"].as_i64().unwrap(),
        exclude_from_context: msg["excludeFromContext"].as_bool(),
    }
}

#[test]
fn summary_prefixes_and_suffixes_are_byte_exact() {
    let capture = oracle();
    assert_eq!(
        COMPACTION_SUMMARY_PREFIX,
        capture["constants"]["COMPACTION_SUMMARY_PREFIX"]
            .as_str()
            .unwrap()
    );
    assert_eq!(
        COMPACTION_SUMMARY_SUFFIX,
        capture["constants"]["COMPACTION_SUMMARY_SUFFIX"]
            .as_str()
            .unwrap()
    );
    assert_eq!(
        BRANCH_SUMMARY_PREFIX,
        capture["constants"]["BRANCH_SUMMARY_PREFIX"]
            .as_str()
            .unwrap()
    );
    assert_eq!(
        BRANCH_SUMMARY_SUFFIX,
        capture["constants"]["BRANCH_SUMMARY_SUFFIX"]
            .as_str()
            .unwrap()
    );
}

#[test]
fn bash_execution_to_text_matches_the_oracle_battery() {
    let capture = oracle();
    for case in capture["bashTexts"].as_array().unwrap() {
        let message = bash_message(case);
        assert_eq!(
            bash_execution_to_text(&message),
            case["text"].as_str().unwrap(),
            "case {}",
            case["name"].as_str().unwrap()
        );
    }
}

#[test]
fn constructors_match_the_oracle_records() {
    let capture = oracle();
    let created = &capture["created"];

    let branch = create_branch_summary_message(
        created["branchMessage"]["summary"].as_str().unwrap(),
        created["branchMessage"]["fromId"].as_str().unwrap(),
        "2026-01-02T03:04:05.678Z",
    )
    .unwrap();
    assert_eq!(
        branch,
        serde_json::from_value::<BranchSummaryMessage>(serde_json::json!({
            "summary": "We came back.",
            "fromId": "node-42",
            "timestamp": 1767323045678_i64,
        }))
        .unwrap()
    );

    let compaction = create_compaction_summary_message(
        created["compactionMessage"]["summary"].as_str().unwrap(),
        created["compactionMessage"]["tokensBefore"]
            .as_i64()
            .unwrap(),
        "2026-01-02T03:04:05.678Z",
    )
    .unwrap();
    assert_eq!(compaction.summary, "Earlier stuff.");
    assert_eq!(compaction.tokens_before, 12345);
    assert_eq!(compaction.timestamp, 1767323045678);

    let custom = create_custom_message(
        created["customString"]["customType"].as_str().unwrap(),
        CustomMessageContent::Text(
            created["customString"]["content"]
                .as_str()
                .unwrap()
                .to_string(),
        ),
        created["customString"]["display"].as_bool().unwrap(),
        None,
        "2026-01-02T03:04:05.678Z",
    )
    .unwrap();
    assert_eq!(custom.custom_type, "my-plugin/status");
    assert!(custom.display);
    assert_eq!(custom.details, None);

    let rich = create_custom_message(
        created["customBlocks"]["customType"].as_str().unwrap(),
        CustomMessageContent::Blocks(
            serde_json::from_value(created["customBlocks"]["content"].clone()).unwrap(),
        ),
        created["customBlocks"]["display"].as_bool().unwrap(),
        Some(serde_json::json!({ "attempt": 2 })),
        "2026-01-02T03:04:05.678Z",
    )
    .unwrap();
    assert_eq!(
        rich.timestamp,
        created["customBlocks"]["timestamp"].as_i64().unwrap()
    );
    assert!(!rich.display);
    assert_eq!(rich.details, Some(serde_json::json!({ "attempt": 2 })));
}

#[test]
fn epoch_millis_parses_iso_timestamps_like_date_get_time() {
    assert_eq!(
        parse_epoch_millis("2026-01-02T03:04:05.678Z"),
        Some(1767323045678)
    );
    // Fractional seconds truncate to millis like Date.parse.
    assert_eq!(parse_epoch_millis("1970-01-01T00:00:00.000Z"), Some(0));
    assert_eq!(
        parse_epoch_millis("2026-01-02T03:04:05Z"),
        Some(1767323045000)
    );
    // Explicit offsets subtract from UTC.
    assert_eq!(
        parse_epoch_millis("2026-01-02T03:04:05+02:00"),
        Some(1767323045000 - 2 * 3_600_000)
    );
    assert_eq!(
        parse_epoch_millis("2026-01-02T03:04:05-0130"),
        Some(1767323045000 + 90 * 60_000)
    );
    // Garbage is the NaN case (the port's None).
    assert_eq!(parse_epoch_millis("not-a-date"), None);
}

fn custom_message(role: &str, data: serde_json::Value) -> AgentMessage {
    let object = data.as_object().cloned().unwrap();
    AgentMessage::Custom(CustomAgentMessage {
        role: role.to_string(),
        data: object,
    })
}

#[test]
fn convert_to_llm_serializes_byte_identically_to_the_oracle() {
    let capture = oracle();
    let converted_json = capture["convertedJson"].as_str().unwrap();

    let bash_excluded = custom_message(
        "bashExecution",
        serde_json::json!({
            "command": "hidden", "output": "x", "exitCode": 0,
            "cancelled": false, "truncated": false, "timestamp": 11,
            "excludeFromContext": true,
        }),
    );
    let bash_shown = custom_message(
        "bashExecution",
        serde_json::json!({
            "command": "shown", "output": "", "exitCode": 2,
            "cancelled": false, "truncated": false, "timestamp": 12,
        }),
    );
    let custom_string = custom_message(
        "custom",
        serde_json::json!({
            "customType": "my-plugin/status", "content": "hello custom",
            "display": true, "timestamp": 1767323045678_i64,
        }),
    );
    let custom_blocks = custom_message(
        "custom",
        serde_json::json!({
            "customType": "my-plugin/rich",
            "content": [{ "type": "text", "text": "rich" }],
            "display": false, "details": { "attempt": 2 },
            "timestamp": 1767323045678_i64,
        }),
    );
    let branch = custom_message(
        "branchSummary",
        serde_json::json!({
            "summary": "We came back.", "fromId": "node-42",
            "timestamp": 1767323045678_i64,
        }),
    );
    let compaction = custom_message(
        "compactionSummary",
        serde_json::json!({
            "summary": "Earlier stuff.", "tokensBefore": 12345,
            "timestamp": 1767323045678_i64,
        }),
    );
    let unknown_custom = custom_message(
        "custom",
        serde_json::json!({
            "customType": "unknown-kind", "content": "mystery",
            "display": true, "timestamp": 13,
        }),
    );

    // Same message battery as the oracle script, in the same order.
    let messages = vec![
        bash_excluded,
        bash_shown,
        custom_string,
        custom_blocks,
        branch,
        compaction,
        AgentMessage::User(crate::ai::types::UserMessage {
            content: crate::ai::types::StringOrBlocks::Text("plain question".to_string()),
            timestamp: 1767337445000_i64,
        }),
        assistant_fixture(),
        AgentMessage::System(crate::ai::types::SystemMessage {
            content: crate::ai::types::StringOrBlocks::Text("be helpful".to_string()),
            sections: None,
            tools_added: None,
            tools_removed: None,
            timestamp: 1767337444000,
        }),
        serde_json::from_value::<AgentMessage>(serde_json::json!({
            "role": "toolResult", "toolCallId": "call_1", "toolName": "bash",
            "content": [{ "type": "text", "text": "out" }],
            "isError": false, "timestamp": 1767337446000_i64,
        }))
        .unwrap(),
        unknown_custom,
    ];

    let converted = convert_to_llm(&messages);
    let actual = serde_json::to_string(&converted).unwrap();
    assert_eq!(actual, converted_json);

    let expected_roles: Vec<&str> = capture["convertedRoles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|role| role.as_str().unwrap())
        .collect();
    let actual_roles: Vec<&str> = converted
        .iter()
        .map(|message| match message {
            crate::ai::types::Message::System(_) => "system",
            crate::ai::types::Message::User(_) => "user",
            crate::ai::types::Message::Assistant(_) => "assistant",
            crate::ai::types::Message::ToolResult(_) => "toolResult",
        })
        .collect();
    assert_eq!(actual_roles, expected_roles);
}

/// The assistant fixture from the oracle script (the passthrough input).
fn assistant_fixture() -> AgentMessage {
    AgentMessage::Assistant(crate::ai::types::AssistantMessage {
        content: Vec::new(),
        api: "anthropic-messages".to_string(),
        provider: "anthropic".to_string(),
        model: "claude".to_string(),
        response_model: None,
        response_id: None,
        provider_thinking_level: None,
        diagnostics: None,
        usage: crate::ai::types::Usage {
            input: 3,
            output: 4,
            cache_read: 0,
            cache_write: 0,
            cache_write_1h: None,
            reasoning: None,
            total_tokens: 7,
            cost: crate::ai::types::UsageCost {
                input: 0.1,
                output: 0.2,
                cache_read: 0.0,
                cache_write: 0.0,
                total: 0.3,
            },
        },
        stop_reason: crate::ai::types::StopReason::Stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 1767337445678,
    })
}

/// Standard roles pass through unchanged; excluded bash executions and
/// unknown custom roles are dropped.
#[test]
fn convert_to_llm_filters_and_passes_through() {
    let messages = vec![
        AgentMessage::User(crate::ai::types::UserMessage {
            content: crate::ai::types::StringOrBlocks::Text("hi".to_string()),
            timestamp: 1,
        }),
        custom_message(
            "bashExecution",
            serde_json::json!({
                "command": "x", "output": "", "exitCode": 0,
                "cancelled": false, "truncated": false, "timestamp": 2,
                "excludeFromContext": true,
            }),
        ),
        AgentMessage::Custom(CustomAgentMessage::new("mystery-role")),
    ];
    let converted = convert_to_llm(&messages);
    assert_eq!(converted.len(), 1);
    match &converted[0] {
        crate::ai::types::Message::User(user) => {
            assert_eq!(
                user.content,
                crate::ai::types::StringOrBlocks::Text("hi".to_string())
            );
        }
        other => panic!("expected user message, got {other:?}"),
    }
}

/// The compaction/branch summaries wrap the payload with the exact
/// prefix/suffix strings.
#[test]
fn summaries_wrap_payload_with_prefix_and_suffix() {
    let messages = vec![
        custom_message(
            "compactionSummary",
            serde_json::json!({
                "summary": "S", "tokensBefore": 10, "timestamp": 5,
            }),
        ),
        custom_message(
            "branchSummary",
            serde_json::json!({
                "summary": "B", "fromId": null, "timestamp": 6,
            }),
        ),
    ];
    let converted = convert_to_llm(&messages);
    assert_eq!(converted.len(), 2);
    let text = |message: &crate::ai::types::Message| match message {
        crate::ai::types::Message::User(user) => match &user.content {
            crate::ai::types::StringOrBlocks::Blocks(blocks) => match &blocks[0] {
                crate::ai::types::TextOrImageBlock::Text(text) => text.text.clone(),
                other => panic!("expected text block, got {other:?}"),
            },
            other => panic!("expected blocks, got {other:?}"),
        },
        other => panic!("expected user message, got {other:?}"),
    };
    assert_eq!(
        text(&converted[0]),
        format!("{COMPACTION_SUMMARY_PREFIX}S{COMPACTION_SUMMARY_SUFFIX}")
    );
    assert_eq!(
        text(&converted[1]),
        format!("{BRANCH_SUMMARY_PREFIX}B{BRANCH_SUMMARY_SUFFIX}")
    );
}
