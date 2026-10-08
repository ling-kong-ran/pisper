//! Ports of the registry/config-facade oracle surface (`kinds.test.ts`'s
//! defineTask/config tests, `reads.test.ts`'s collision tests, and the
//! `types.compile.ts` runtime half). See the parent module's mapping table
//! for what remains scheduler-bound.

use crate::agent_core::harness::pico3::session::Defaults;
use crate::agent_core::harness::pico3::tests::support::*;
use crate::agent_core::harness::pico3::types::{
    define_entry, define_task, memo_once, BasicKind, ViewEvent,
};

use futures::FutureExt;
use serde_json::{json, Value};

/// `authority.test.ts` "a plugin kind named pi.* is rejected by defineTask"
/// and `types.ts:109-112`/`484-497`.
#[test]
fn define_task_rejects_reserved_names() {
    let error = define_task(BasicKind::new("pi.generation")).unwrap_err();
    assert!(format!("{error}").contains("reserved"), "{error}");
    let entry_error = define_entry("pi.mine").unwrap_err();
    assert!(
        format!("{entry_error}").contains("reserved"),
        "{entry_error}"
    );
    // An ordinary name registers fine.
    assert!(define_task(BasicKind::new("plan")).is_ok());
    assert!(define_entry("note").is_ok());
}

/// `kinds.test.ts` "c.config ... disjointness enforced at open" and
/// `reads.test.ts` "config: key collisions across kinds are rejected at
/// installation": `session.ts:118-121`.
#[test]
#[should_panic(expected = "declared by more than one kind")]
fn config_key_collisions_are_rejected() {
    let first: std::sync::Arc<dyn crate::agent_core::harness::pico3::types::AnyKind> =
        std::sync::Arc::new(BasicKind::new("plan").config(
            crate::agent_core::harness::pico3::types::KindConfig {
                rewindable: json!({ "planMode": false }).as_object().cloned().unwrap(),
                sticky: Default::default(),
                declared_absent: Default::default(),
            },
        ));
    let second: std::sync::Arc<dyn crate::agent_core::harness::pico3::types::AnyKind> =
        std::sync::Arc::new(BasicKind::new("clash").config(
            crate::agent_core::harness::pico3::types::KindConfig {
                rewindable: Default::default(),
                sticky: json!({ "planMode": "" }).as_object().cloned().unwrap(),
                declared_absent: Default::default(),
            },
        ));
    let _ = Defaults::new([first, second].into_iter());
}

/// `session.ts:144-153`: `validateSeed` rejects keys not routed to the doc
/// (`chord.test.ts` "invalid rewindable config value").
#[test]
fn validate_seed_rejects_foreign_or_unknown_keys() {
    let kind: std::sync::Arc<dyn crate::agent_core::harness::pico3::types::AnyKind> =
        std::sync::Arc::new(BasicKind::new("plan").config(
            crate::agent_core::harness::pico3::types::KindConfig {
                rewindable: json!({ "planMode": false }).as_object().cloned().unwrap(),
                sticky: Default::default(),
                declared_absent: Default::default(),
            },
        ));
    let defaults = Defaults::new([kind].into_iter());
    let seed = json!({ "planMode": true }).as_object().cloned().unwrap();
    assert!(defaults.validate_seed("rewindable", &seed).is_ok());
    // Routed to rewindable: a sticky seed for it is invalid.
    assert!(defaults.validate_seed("sticky", &seed).is_err());
    // Unknown key: invalid.
    let unknown = json!({ "bogus": 1 }).as_object().cloned().unwrap();
    assert!(defaults.validate_seed("rewindable", &unknown).is_err());
}

/// `chord.test.ts` invalid config patches drive `session.ts:67-85` core
/// validators through `Defaults#validate`.
#[test]
fn core_config_validators_match_upstream() {
    let defaults = Defaults::new(std::iter::empty());
    let good = [
        ("model", json!({ "provider": "p", "modelId": "m" })),
        ("thinkingLevel", json!("high")),
        ("selectedTools", json!(["a", "b"])),
        ("profile", json!("default")),
        (
            "retry",
            json!({ "enabled": true, "maxRetries": 3, "baseDelayMs": 100.0 }),
        ),
        ("threshold", json!(0.9)),
        ("keepRecent", json!(20000)),
        ("steeringMode", json!("all")),
        ("followUpMode", json!("one-at-a-time")),
    ];
    for (key, value) in good {
        assert!(defaults.validate(key, &value), "{key} should validate");
    }
    let bad = [
        ("model", json!({ "provider": "p" })), // missing modelId
        ("model", json!({ "provider": "", "modelId": "m" })), // empty provider
        ("model", json!(5)),                   // not an object
        ("thinkingLevel", json!("sometimes")),
        ("selectedTools", json!("not-an-array")),
        ("profile", json!(5)),
        ("retry", json!(5)),
        (
            "retry",
            json!({ "enabled": true, "maxRetries": -1, "baseDelayMs": 1 }),
        ),
        (
            "retry",
            json!({ "enabled": true, "maxRetries": 1, "baseDelayMs": 1, "extra": 2 }),
        ),
        ("threshold", json!("not-a-number")),
        ("keepRecent", json!(-1)),
        ("steeringMode", json!("sometimes")),
    ];
    for (key, value) in bad {
        assert!(
            !defaults.validate(key, &value),
            "{key} should not validate: {value}"
        );
    }
}

/// `types.ts:241-247`: `memoOnce` — first writer wins, including when the
/// stored winner is null.
#[test]
fn memo_once_first_writer_wins() {
    let mut slot = serde_json::Map::new();
    let first = memo_once(&mut slot, "memo", json!("first"));
    assert_eq!(first, json!("first"));
    // Stored null is a value: the candidate loses.
    slot.get_mut("memos")
        .unwrap()
        .as_object_mut()
        .unwrap()
        .insert("cached".to_owned(), json!(null));
    let winner = memo_once(&mut slot, "cached", json!("candidate"));
    assert_eq!(winner, json!(null));
    let second = memo_once(&mut slot, "memo", json!("second"));
    assert_eq!(second, json!("first"));
}

/// Wire compatibility of the event vocabulary the oracle suite matches on:
/// every `ViewEvent` literal round-trips, including the two `turn.ended`
/// shapes and the dynamic `plugin.<ns>.<name>` tag (`types.ts:650-698`).
#[test]
fn view_events_round_trip_through_wire_shapes() {
    use crate::agent_core::harness::pico3::types::Entry;
    let entry = Entry {
        id: 7,
        conversation_id: 1,
        kind: "pi.user".to_owned(),
        model: None,
        data: None,
        head: None,
        edits: None,
        by_task_id: None,
    };
    let events = vec![
        ViewEvent::EntryAdded {
            entry: entry.clone(),
        },
        ViewEvent::HeadMoved {
            entry: entry.clone(),
        },
        ViewEvent::TurnStarted { inputs: vec![3, 4] },
        ViewEvent::TurnEndedDone {
            inputs: vec![3],
            answer: 9,
        },
        ViewEvent::TurnEndedUnanswered {
            inputs: vec![3],
            reason: "aborted".to_owned(),
            detail: Some("late".to_owned()),
        },
        ViewEvent::InputQueued {
            input: 5,
            mode: "steer".to_owned(),
        },
        ViewEvent::InputPlaced { input: 5, entry: 6 },
        ViewEvent::InputAborted { input: 5 },
        ViewEvent::GenerationStarted {
            task_id: 8,
            attempt: 1,
        },
        ViewEvent::GenerationRetrying {
            task_id: 8,
            attempt: 2,
            retry_at: 1234,
            error: "overloaded".to_owned(),
        },
        ViewEvent::GenerationDeferred {
            task_id: 8,
            poll_at: 99,
        },
        ViewEvent::GenerationCompleted {
            task_id: 8,
            entry: 9,
            tool_calls: 2,
        },
        ViewEvent::GenerationFailed {
            task_id: 8,
            reason: "provider".to_owned(),
            detail: "boom".to_owned(),
            entry: Some(9),
        },
        ViewEvent::TaskStarted {
            task_id: 8,
            kind: "pi.plugin".to_owned(),
            background: Some(true),
        },
        ViewEvent::TaskEnded {
            task_id: 8,
            kind: "pi.plugin".to_owned(),
            outcome: "completed".to_owned(),
        },
        ViewEvent::ConfigChanged {
            keys: vec!["profile".to_owned()],
        },
        ViewEvent::Warning {
            source: "test".to_owned(),
            message: "careful".to_owned(),
        },
        ViewEvent::Plugin {
            namespace: "spec".to_owned(),
            name: "ping".to_owned(),
            data: json!({ "ok": true }),
        },
    ];
    for event in events {
        let wire = event.to_value();
        assert_eq!(
            wire.get("type").and_then(Value::as_str),
            Some(event.event_type().as_str()),
            "{wire}"
        );
        let parsed = ViewEvent::from_value(&wire).expect("parses");
        assert_eq!(parsed, event);
    }
    // The `turn.ended` wire shapes distinguish by `status`
    // (`types.ts:653-661`).
    let unanswered = ViewEvent::TurnEndedUnanswered {
        inputs: vec![1],
        reason: "stale".to_owned(),
        detail: None,
    }
    .to_value();
    assert_eq!(
        unanswered.get("status").and_then(Value::as_str),
        Some("unanswered")
    );
    assert!(
        unanswered.get("detail").is_none(),
        "optional detail omitted"
    );
}

/// The `types.compile.ts` runtime half: ordinary host invokers get
/// `Forbidden` from core operations even though the port cannot express the
/// negative type assertions (disclosed in the module docs).
#[tokio::test]
async fn host_tx_core_operations_reject_at_runtime() {
    let env = Env::open_memory().await.unwrap();
    let error = env
        .commit_host(|tx, _ctx| {
            async move {
                tx.append_entry(
                    1,
                    crate::agent_core::harness::pico3::types::NewEntry::new("pi.notice"),
                )
                .map(|_| ())
            }
            .boxed()
        })
        .await
        .unwrap_err();
    assert_named(&error, "Forbidden");
    let error = env
        .commit_host(|tx, _ctx| {
            async move { tx.boundary(1, "final", None).await.map(|_| ()) }.boxed()
        })
        .await
        .unwrap_err();
    assert_named(&error, "Forbidden");
    let error = env
        .commit_host(|tx, _ctx| async move { tx.mark_task(1).map(|_| ()) }.boxed())
        .await
        .unwrap_err();
    assert_named(&error, "Forbidden");
}
