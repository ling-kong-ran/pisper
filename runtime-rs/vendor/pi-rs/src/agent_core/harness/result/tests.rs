//! Tests for the `result.ts` port: tag values, `toJSON` payloads, and the
//! fault/closed errors. Derived from the upstream source (`result.ts:53-118`)
//! and its call sites (`test/harness/runtime/accept.test.ts:560`).

use super::*;

#[test]
fn tagged_error_tags_match_upstream_class_names() {
    let cases: [(TaggedError, &str); 13] = [
        (
            TaggedError::LaneBusy {
                lane: "main".into(),
                operation_id: "run".into(),
                operation_kind: OperationKind::Run,
                message: "busy".into(),
            },
            "LaneBusy",
        ),
        (
            TaggedError::OperationMismatch {
                lane: "main".into(),
                expected_operation_id: "a".into(),
                current_operation_id: Some("b".into()),
                last_operation_id: None,
                message: "mismatch".into(),
            },
            "OperationMismatch",
        ),
        (
            TaggedError::NoActiveRun {
                lane: "main".into(),
                message: "no run".into(),
            },
            "NoActiveRun",
        ),
        (
            TaggedError::NoActiveOperation {
                lane: "main".into(),
                message: "no op".into(),
            },
            "NoActiveOperation",
        ),
        (
            TaggedError::NothingToResume {
                lane: "main".into(),
                message: "nothing".into(),
            },
            "NothingToResume",
        ),
        (
            TaggedError::NothingToCompact {
                lane: "main".into(),
                message: "nothing".into(),
            },
            "NothingToCompact",
        ),
        (
            TaggedError::InvalidMessage {
                lane: "main".into(),
                reason: "empty".into(),
                message: "bad".into(),
            },
            "InvalidMessage",
        ),
        (
            TaggedError::InvalidNavigation {
                lane: "main".into(),
                reason: "unknown".into(),
                message: "bad".into(),
            },
            "InvalidNavigation",
        ),
        (
            TaggedError::UnknownSkill {
                name: "nope".into(),
                message: "unknown".into(),
            },
            "UnknownSkill",
        ),
        (
            TaggedError::UnknownTemplate {
                name: "nope".into(),
                message: "unknown".into(),
            },
            "UnknownTemplate",
        ),
        (
            TaggedError::UnknownTarget {
                target_id: "t".into(),
                message: "unknown".into(),
            },
            "UnknownTarget",
        ),
        (
            TaggedError::InvalidLane {
                lane: "nope".into(),
                reason: "reserved".into(),
                message: "bad".into(),
            },
            "InvalidLane",
        ),
        (
            TaggedError::Closed {
                message: "closed".into(),
            },
            "Closed",
        ),
    ];
    for (error, tag) in cases {
        assert_eq!(error.tag(), tag);
        assert!(!error.message().is_empty());
    }
}

#[test]
fn to_json_matches_upstream_tojson_shape() {
    // result.ts:53-58: LaneBusy { lane, operationId, operationKind, message }.
    let error = TaggedError::LaneBusy {
        lane: "main".into(),
        operation_id: "run-1".into(),
        operation_kind: OperationKind::Compaction,
        message: "busy".into(),
    };
    assert_eq!(
        error.to_json(),
        serde_json::json!({
            "_tag": "LaneBusy",
            "message": "busy",
            "lane": "main",
            "operationId": "run-1",
            "operationKind": "compaction",
        })
    );

    // result.ts:88: Closed has only { _tag, message }.
    let closed = TaggedError::Closed {
        message: "harness closed".into(),
    };
    assert_eq!(
        closed.to_json(),
        serde_json::json!({"_tag": "Closed", "message": "harness closed"})
    );

    // result.ts:70-74: InvalidMessage { lane, reason, message }.
    let invalid = TaggedError::InvalidMessage {
        lane: "worker".into(),
        reason: "no content".into(),
        message: "invalid".into(),
    };
    assert_eq!(
        invalid.to_json(),
        serde_json::json!({
            "_tag": "InvalidMessage",
            "message": "invalid",
            "lane": "worker",
            "reason": "no content",
        })
    );
}

#[test]
fn tagged_error_serializes_as_its_tojson_payload() {
    let error = TaggedError::UnknownSkill {
        name: "search".into(),
        message: "no such skill".into(),
    };
    let json = serde_json::to_value(&error).unwrap();
    assert_eq!(json["_tag"], "UnknownSkill");
    assert_eq!(json["message"], "no such skill");
    assert_eq!(json["name"], "search");
}

#[test]
fn display_includes_tag_and_message() {
    let error = TaggedError::LaneBusy {
        lane: "main".into(),
        operation_id: "run".into(),
        operation_kind: OperationKind::Run,
        message: "lane busy".into(),
    };
    assert_eq!(error.to_string(), "LaneBusy: lane busy");
}

#[test]
fn harness_fault_carries_message_and_cause() {
    let cause = anyhow::anyhow!("disk on fire");
    let fault = HarnessFault::new("drive failed", cause);
    assert_eq!(fault.message, "drive failed");
    assert_eq!(fault.cause.to_string(), "disk on fire");
    assert_eq!(fault.to_string(), "drive failed: disk on fire");
    let source = std::error::Error::source(&fault).expect("fault has a cause");
    assert_eq!(source.to_string(), "disk on fire");
}

#[test]
fn harness_closed_uses_the_fixed_upstream_message() {
    assert_eq!(
        HarnessClosed.to_string(),
        "AgentHarness was closed while the operation was active"
    );
    assert_eq!(HarnessClosed, HarnessClosed);
}

#[test]
fn match_dispatch_replaces_match_error() {
    // result.ts:107-117 `matchError` maps tags to handlers; a plain `match`
    // over the enum variants is the port's equivalent.
    let error = TaggedError::NoActiveRun {
        lane: "main".into(),
        message: "no run".into(),
    };
    let value = match &error {
        TaggedError::NoActiveRun { lane, .. } => format!("lane:{lane}"),
        _ => panic!("unexpected variant"),
    };
    assert_eq!(value, "lane:main");
}
