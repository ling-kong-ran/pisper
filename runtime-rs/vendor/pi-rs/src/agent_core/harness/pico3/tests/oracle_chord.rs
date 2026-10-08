//! Ports of the pure `chord.ts` surface (`applyTracked`, the published view
//! shape). The bridge/services themselves are M6 + Task 9 (see `chord.rs`
//! module docs), so `chord.test.ts`'s bridge-convergence tests reduce here
//! to op application and round-tripping.

use crate::agent_core::chord_support::delta::{Op, Seg};
use crate::agent_core::harness::pico3::chord::{apply_ops_to_view, PublishedConversationView};

use serde_json::{json, Value};

/// `chord.ts:145-169` op semantics: set, delete, append, splice, string
/// truncate.
#[test]
fn apply_tracked_ops_matches_upstream_shapes() {
    let mut root = json!({
        "entries": [{ "id": 1 }, { "id": 2 }],
        "turn": { "tools": [{ "output": "half" }] },
        "config": { "profile": "default" },
        "scratch": "keep",
    });
    let ops = vec![
        // ["s", ["config", "profile"], "p2"]
        Op::Set {
            path: vec![
                Seg::Key("config".to_owned()),
                Seg::Key("profile".to_owned()),
            ],
            value: json!("p2"),
        },
        // ["d", ["scratch"]]
        Op::Delete {
            path: vec![Seg::Key("scratch".to_owned())],
        },
        // ["a", ["turn", "tools", 0, "output"], " full"]
        Op::Append {
            path: vec![
                Seg::Key("turn".to_owned()),
                Seg::Key("tools".to_owned()),
                Seg::Index(0),
                Seg::Key("output".to_owned()),
            ],
            text: " full".to_owned(),
        },
        // ["p", ["entries"], 0, 1, [{ "id": 9 }]]
        Op::Splice {
            path: vec![Seg::Key("entries".to_owned())],
            index: 0,
            remove: 1,
            items: vec![json!({ "id": 9 })],
        },
    ];
    apply_ops_to_view(&mut root, &ops).expect("ops apply");
    assert_eq!(root["config"]["profile"], json!("p2"));
    assert!(root.get("scratch").is_none());
    assert_eq!(root["turn"]["tools"][0]["output"], json!("half full"));
    assert_eq!(root["entries"], json!([{ "id": 9 }, { "id": 2 }]));

    // ["t", path, count] removes UTF-16 units from the front.
    let ops = vec![Op::Truncate {
        path: vec![
            Seg::Key("turn".to_owned()),
            Seg::Key("tools".to_owned()),
            Seg::Index(0),
            Seg::Key("output".to_owned()),
        ],
        count: 5,
    }];
    apply_ops_to_view(&mut root, &ops).unwrap();
    assert_eq!(root["turn"]["tools"][0]["output"], json!("full"));
}

/// `chord.ts:147` a root replacement is a contract breach with the upstream
/// message.
#[test]
fn root_replacement_is_rejected() {
    let mut root = json!({});
    let error =
        apply_ops_to_view(&mut root, &[Op::Replace(json!({ "replaced": true }))]).unwrap_err();
    assert!(
        format!("{error}").contains("unexpectedly replaced the view root"),
        "{error}"
    );
}

/// `chord.ts:151,156-157` shape guards.
#[test]
fn shape_guards_match_upstream_messages() {
    let mut root = json!({ "notArray": 5 });
    let error = apply_ops_to_view(
        &mut root,
        &[Op::Splice {
            path: vec![Seg::Key("notArray".to_owned())],
            index: 0,
            remove: 0,
            items: vec![],
        }],
    )
    .unwrap_err();
    assert!(
        format!("{error}").contains("splice path is not an array"),
        "{error}"
    );

    let mut root = json!({ "scalar": 7 });
    let error = apply_ops_to_view(
        &mut root,
        &[Op::Set {
            path: vec![Seg::Key("scalar".to_owned()), Seg::Key("deep".to_owned())],
            value: json!(1),
        }],
    )
    .unwrap_err();
    assert!(
        format!("{error}").contains("parent is not an object"),
        "{error}"
    );
}

/// `PublishedConversationView` (`chord.ts:18-20`): the `commit.events`
/// key rides beside the conversation view and round-trips.
#[test]
fn published_view_round_trips_with_commit_events() {
    use crate::agent_core::harness::pico3::types::{Entry, ViewEvent};
    let view = PublishedConversationView {
        view: crate::agent_core::harness::pico3::types::ConversationView {
            conversation: json!({ "id": 1 }),
            entries: vec![Entry {
                id: 2,
                conversation_id: 1,
                kind: "pi.user".to_owned(),
                model: None,
                data: None,
                head: None,
                edits: None,
                by_task_id: None,
            }],
            ..Default::default()
        },
        commit_events: vec![ViewEvent::EntryAdded {
            entry: Entry {
                id: 2,
                conversation_id: 1,
                kind: "pi.user".to_owned(),
                model: None,
                data: None,
                head: None,
                edits: None,
                by_task_id: None,
            },
        }],
    };
    let wire = view.to_value().unwrap();
    assert!(wire
        .get("commit")
        .and_then(|commit| commit.get("events"))
        .is_some());
    let parsed = PublishedConversationView::from_value(&wire).unwrap();
    assert_eq!(parsed, view, "the published shape round-trips");
    // `commit` is not part of the conversation view itself.
    assert!(json!(parsed.view).get("commit").is_none());
}

/// Unused-import guard for Value in this module.
#[allow(dead_code)]
const _: Option<Value> = None;
