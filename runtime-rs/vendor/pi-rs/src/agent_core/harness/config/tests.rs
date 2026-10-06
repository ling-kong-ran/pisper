//! Tests for the `config.ts` port, derived from the upstream source
//! (`config.ts:4-42`).

use super::*;

#[test]
fn default_retry_policy_matches_upstream_values() {
    // config.ts:4-9 — enabled, 3 retries, 1s base delay, the ai layer's
    // default agent delay cap (60s).
    assert_eq!(
        DEFAULT_RETRY_POLICY,
        RetryPolicy {
            enabled: true,
            max_retries: 3,
            base_delay_ms: 1_000,
            max_agent_delay_ms: Some(DEFAULT_MAX_AGENT_RETRY_DELAY_MS),
        }
    );
}

#[test]
fn default_compaction_settings_match_upstream_values() {
    // compaction/compaction.ts:157-161.
    assert_eq!(
        DEFAULT_COMPACTION_SETTINGS,
        CompactionSettings {
            enabled: true,
            reserve_tokens: 16_384,
            keep_recent_tokens: 20_000,
        }
    );
}

#[test]
fn compaction_settings_round_trips_upstream_wire_shape() {
    let json = serde_json::json!({
        "enabled": true,
        "reserveTokens": 1000,
        "keepRecentTokens": 2000
    });
    let settings: CompactionSettings = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(settings.reserve_tokens, 1000);
    assert_eq!(settings.keep_recent_tokens, 2000);
    assert_eq!(serde_json::to_value(settings).unwrap(), json);

    // `enabled` defaults to true like the settings docs promise.
    let minimal: CompactionSettings =
        serde_json::from_value(serde_json::json!({"reserveTokens": 1, "keepRecentTokens": 2}))
            .unwrap();
    assert!(minimal.enabled);
}

#[test]
fn validate_tool_names_accepts_unique_names() {
    validate_tool_names(["read", "edit", "bash"]).unwrap();
    validate_tool_names(Vec::<&str>::new()).unwrap();
}

#[test]
fn validate_tool_names_rejects_duplicates_with_upstream_message() {
    // config.ts:14: `Duplicate tool name: ${JSON.stringify(tool.name)}`.
    let error = validate_tool_names(["read", "edit", "read"]).unwrap_err();
    assert_eq!(error.to_string(), "Duplicate tool name: \"read\"");
}

#[test]
fn validate_retry_policy_accepts_the_default_policy() {
    validate_retry_policy(&DEFAULT_RETRY_POLICY).unwrap();
    validate_retry_policy(&RetryPolicy {
        enabled: false,
        max_retries: 0,
        base_delay_ms: 0,
        max_agent_delay_ms: None,
    })
    .unwrap();
}

#[test]
fn validate_compaction_settings_accepts_the_defaults() {
    validate_compaction_settings(&DEFAULT_COMPACTION_SETTINGS).unwrap();
    validate_compaction_settings(&CompactionSettings {
        enabled: false,
        reserve_tokens: 0,
        keep_recent_tokens: 0,
    })
    .unwrap();
}
