//! Oracle-driven tests for the chord services surface. Expected values were
//! captured from the read-only upstream TypeScript sources
//! (`pi/packages/chord/src`) run under `node --experimental-strip-types` by
//! `tests/fixtures/chord_oracle/capture_services.mjs` and stored in
//! `src/chord/testdata/services_oracle.json`. Comparison is byte-identical
//! over the canonical serialization (compact JSON with explicitly sorted object keys).

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::chord::context::Context;
use crate::chord::delta::{Op, Seg};
use crate::chord::services::errors::ChordError;
use crate::chord::services::provider::{
    create_remote_service_endpoint, Implementation, InstanceMember, ProviderEntry,
    RemoteServiceEndpoint, RemoteServiceProvider,
};
use crate::chord::services::state::MutableReplicatedState;
use crate::chord::services::state_codec::{ServiceStateDecoder, ServiceStateEncoder};
use crate::chord::services::wire::{
    create_service_catalogue_call, create_service_subscribe_call, create_service_unsubscribe_call,
    decode_service_control_call, parse_service_call, parse_service_catalogue,
    parse_service_provider_update, parse_service_subscription_snapshot,
    parse_wire_service_provider_update, parse_wire_service_subscription_snapshot,
    ServiceControlCall,
};
use crate::chord::types::{
    JsonValue, ServiceCall, ServiceInstanceAddress, ServiceMode, ServiceProviderUpdate,
};

const ORACLE: &str = include_str!("../testdata/services_oracle.json");

fn k(key: &str) -> Seg {
    Seg::Key(key.to_owned())
}

#[test]
fn canonical_comparison_preserves_original_value_order() {
    let raw = r#"{"z":[{"y":1,"a":2}],"a":3}"#;
    let value: JsonValue = serde_json::from_str(raw).unwrap();
    assert_eq!(canon(&value), r#"{"a":3,"z":[{"a":2,"y":1}]}"#);
    assert_eq!(serde_json::to_string(&value).unwrap(), raw);
}

fn canon(value: &JsonValue) -> String {
    // Both Node capture scripts explicitly canonicalize comparison values.
    // Sort a clone only; production state and wire retain insertion order.
    let mut sorted = value.clone();
    sorted.sort_all_objects();
    serde_json::to_string(&sorted).expect("canonical serialization cannot fail")
}

fn control_call_json(call: &ServiceControlCall) -> JsonValue {
    match call {
        ServiceControlCall::Catalogue => json!({ "type": "catalogue" }),
        ServiceControlCall::Subscribe {
            subscription_id,
            service_id,
            mode,
        } => json!({
            "type": "subscribe",
            "subscriptionId": subscription_id,
            "serviceId": service_id,
            "mode": mode.as_str(),
        }),
        ServiceControlCall::Unsubscribe { subscription_id } => {
            json!({ "type": "unsubscribe", "subscriptionId": subscription_id })
        }
    }
}

fn members_json(snapshot: &crate::chord::types::ServiceSubscriptionSnapshot) -> JsonValue {
    json!(snapshot
        .instances
        .iter()
        .flat_map(|instance| instance.members.iter())
        .map(|member| member.to_json())
        .collect::<Vec<_>>())
}

/// Compare one oracle row against the port outcome: `Err` for error rows,
/// `Ok(Some(value))` for value rows, `Ok(None)` for plain-ok rows.
fn check_row(row: &Value, outcome: RowOutcome) {
    let name = row["name"].as_str().expect("row name");
    if let Some(expected_error) = row.get("error").and_then(Value::as_str) {
        let error = outcome
            .err()
            .unwrap_or_else(|| panic!("{name}: expected an error"));
        assert_eq!(error.message(), expected_error, "{name}: error message");
        return;
    }
    let value = outcome.unwrap_or_else(|error| panic!("{name}: unexpected error {error}"));
    if let Some(expected) = row.get("value").and_then(Value::as_str) {
        assert_eq!(
            canon(&value.expect("{name}: value")),
            expected,
            "{name}: value"
        );
    }
}

type RowOutcome = Result<Option<JsonValue>, ChordError>;

fn row(name: &'static str, outcome: RowOutcome) -> (&'static str, RowOutcome) {
    (name, outcome)
}

#[test]
fn wire_rows_match_oracle() {
    let oracle: Value = serde_json::from_str(ORACLE).expect("oracle fixture parses");
    let mut cases: Vec<(&'static str, RowOutcome)> = Vec::new();

    cases.push(row(
        "catalogue_call",
        Ok(Some(create_service_catalogue_call().to_json())),
    ));
    cases.push(row(
        "subscribe_call",
        Ok(Some(
            create_service_subscribe_call("subscription-1", "pi.models", ServiceMode::Singleton)
                .to_json(),
        )),
    ));
    cases.push(row(
        "unsubscribe_call",
        Ok(Some(
            create_service_unsubscribe_call("subscription-1").to_json(),
        )),
    ));
    let no_call = || ChordError::Type("no control call".to_owned());
    cases.push(row(
        "decode_catalogue",
        decode_service_control_call(&create_service_catalogue_call())
            .map(|call| Some(control_call_json(&call)))
            .ok_or_else(no_call),
    ));
    cases.push(row(
        "decode_subscribe",
        decode_service_control_call(&create_service_subscribe_call(
            "subscription-1",
            "pi.models",
            ServiceMode::Singleton,
        ))
        .map(|call| Some(control_call_json(&call)))
        .ok_or_else(no_call),
    ));
    cases.push(row(
        "decode_unsubscribe",
        decode_service_control_call(&create_service_unsubscribe_call("subscription-1"))
            .map(|call| Some(control_call_json(&call)))
            .ok_or_else(no_call),
    ));
    cases.push(row(
        "decode_with_instance",
        Ok(decode_service_control_call(&ServiceCall {
            service_id: "$chord.service".to_owned(),
            instance: Some(ServiceInstanceAddress {
                key: "k".to_owned(),
                generation: 1,
            }),
            member: "catalogue".to_owned(),
            args: vec![],
        })
        .map(|call| control_call_json(&call))),
    ));
    cases.push(row(
        "decode_subscribe_bad_mode",
        Ok(decode_service_control_call(&ServiceCall {
            service_id: "$chord.service".to_owned(),
            instance: None,
            member: "subscribe".to_owned(),
            args: vec![json!("s1"), json!("svc"), json!("other")],
        })
        .map(|call| control_call_json(&call))),
    ));
    cases.push(row(
        "parse_call_ok",
        parse_service_call(&json!({
            "serviceId": "pi.question-dialog",
            "instance": { "key": "invocation-1", "generation": 2 },
            "member": "submit",
            "args": [{ "outcome": "selected", "index": 0 }],
        }))
        .map(|call| Some(call.to_json())),
    ));
    cases.push(row(
        "parse_call_extra_key",
        parse_service_call(&json!({
            "serviceId": "pi.models", "member": "list", "args": [], "extra": true
        }))
        .map(|call| Some(call.to_json())),
    ));
    cases.push(row(
        "parse_call_empty_id",
        parse_service_call(&json!({ "serviceId": "", "member": "list", "args": [] }))
            .map(|call| Some(call.to_json())),
    ));
    let catalogue_json = |entries: Vec<crate::chord::types::ServiceCatalogueEntry>| {
        Some(json!(entries
            .iter()
            .map(|entry| entry.to_json())
            .collect::<Vec<_>>()))
    };
    cases.push(row(
        "parse_catalogue_ok",
        parse_service_catalogue(&json!([
            { "serviceId": "pi.models", "mode": "singleton" },
            { "serviceId": "pi.dialogs", "mode": "keyed" },
        ]))
        .map(catalogue_json),
    ));
    cases.push(row(
        "parse_catalogue_bad_mode",
        parse_service_catalogue(&json!([{ "serviceId": "pi.models", "mode": "unknown" }]))
            .map(catalogue_json),
    ));
    cases.push(row(
        "parse_catalogue_duplicate",
        parse_service_catalogue(&json!([
            { "serviceId": "pi.models", "mode": "singleton" },
            { "serviceId": "pi.models", "mode": "singleton" },
        ]))
        .map(catalogue_json),
    ));
    cases.push(row(
        "parse_update_sequence_zero",
        parse_service_provider_update(&json!({
            "type": "state", "member": "state", "sequence": 0, "ops": []
        }))
        .map(|update| Some(update.to_json())),
    ));
    cases.push(row(
        "parse_wire_update_bad_op",
        crate::chord::services::wire::parse_wire_service_provider_update(&json!({
            "type": "state", "member": "state", "sequence": 1, "ops": [["?", 0]]
        }))
        .map(|update| Some(update.to_json())),
    ));
    cases.push(row(
        "parse_update_unavailable",
        parse_service_provider_update(&json!({ "type": "unavailable" }))
            .map(|update| Some(update.to_json())),
    ));
    cases.push(row(
        "parse_update_unavailable_extra",
        parse_service_provider_update(&json!({ "type": "unavailable", "extra": 1 }))
            .map(|update| Some(update.to_json())),
    ));
    cases.push(row(
        "parse_update_bad_type",
        parse_service_provider_update(&json!({ "type": "other" }))
            .map(|update| Some(update.to_json())),
    ));
    cases.push(row(
        "parse_address_generation_zero",
        parse_service_call(&json!({
            "serviceId": "svc", "member": "m", "args": [],
            "instance": { "key": "k", "generation": 0 }
        }))
        .map(|call| Some(call.to_json())),
    ));

    let rows = oracle["wire"].as_array().expect("wire rows");
    assert_eq!(cases.len(), rows.len(), "wire row coverage");
    for row in rows {
        let name = row["name"].as_str().expect("row name");
        let (_, outcome) = cases
            .iter()
            .find(|(case_name, _)| *case_name == name)
            .unwrap_or_else(|| panic!("missing case {name}"));
        check_row(row, outcome.clone());
    }
}

#[test]
fn codec_rows_match_oracle() {
    let oracle: Value = serde_json::from_str(ORACLE).expect("oracle fixture parses");
    let rows = oracle["codec"].as_array().expect("codec rows");
    let value_str = |row: &Value| -> String {
        row.get("value")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("row {:?} has no value", row["name"]))
            .to_owned()
    };
    let error_str = |row: &Value| -> String {
        row.get("error")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("row {:?} has no error", row["name"]))
            .to_owned()
    };
    let find = |name: &str| {
        rows.iter()
            .find(|row| row["name"] == json!(name))
            .expect("row")
            .clone()
    };

    let snapshot_json = json!({
        "serviceId": "pi.models",
        "mode": "singleton",
        "instances": [
            {
                "members": [
                    { "name": "state", "kind": "state", "sequence": 0,
                      "ops": [["r", { "revision": 1 }]] },
                ],
            },
        ],
    });
    let parsed = parse_service_subscription_snapshot(&snapshot_json).unwrap();
    assert_eq!(canon(&parsed.to_json()), value_str(&find("parse_snapshot")));
    let mut encoder = ServiceStateEncoder::new();
    let wire = encoder.encode_snapshot(&parsed).unwrap();
    parse_wire_service_subscription_snapshot(&wire.to_json()).expect("wire snapshot valid");
    assert_eq!(canon(&wire.to_json()), value_str(&find("encode_snapshot")));

    let update_json = json!({
        "type": "state", "member": "state", "sequence": 1,
        "ops": [["s", ["revision"], 2]]
    });
    let parsed_update = parse_service_provider_update(&update_json).unwrap();
    assert_eq!(
        canon(&parsed_update.to_json()),
        value_str(&find("parse_update"))
    );
    let update = ServiceProviderUpdate::State {
        instance: None,
        member: "state".to_owned(),
        sequence: 1,
        ops: vec![Op::Set {
            path: vec![k("revision")],
            value: json!(2),
        }],
    };
    let wire_update = encoder.encode_update(&update).unwrap();
    parse_wire_service_provider_update(&wire_update.to_json()).expect("wire update valid");
    assert_eq!(
        canon(&wire_update.to_json()),
        value_str(&find("encode_update"))
    );

    // one codec pair per subscription state
    {
        let snap = parse_service_subscription_snapshot(&json!({
            "serviceId": "pi.models",
            "mode": "singleton",
            "instances": [
                {
                    "members": [
                        { "name": "state", "kind": "state", "sequence": 0,
                          "ops": [["r", { "revision": 0 }]] },
                    ],
                },
            ],
        }))
        .unwrap();
        let mut enc = ServiceStateEncoder::new();
        let mut dec = ServiceStateDecoder::new();
        let decoded = dec
            .decode_snapshot(&enc.encode_snapshot(&snap).unwrap())
            .unwrap();
        assert_eq!(
            canon(&decoded.to_json()),
            value_str(&find("pair_snapshot_roundtrip"))
        );
        let first = ServiceProviderUpdate::State {
            instance: None,
            member: "state".to_owned(),
            sequence: 1,
            ops: vec![Op::Set {
                path: vec![k("revision")],
                value: json!(1),
            }],
        };
        let second = ServiceProviderUpdate::State {
            instance: None,
            member: "state".to_owned(),
            sequence: 2,
            ops: vec![Op::Set {
                path: vec![k("revision")],
                value: json!(2),
            }],
        };
        let first_wire = enc.encode_update(&first).unwrap();
        let second_wire = enc.encode_update(&second).unwrap();
        assert_eq!(
            canon(&first_wire.to_json()),
            value_str(&find("pair_first_wire"))
        );
        assert_eq!(
            canon(&second_wire.to_json()),
            value_str(&find("pair_second_wire"))
        );
        assert_eq!(
            canon(&dec.decode_update(&first_wire).unwrap().to_json()),
            value_str(&find("pair_first_decoded"))
        );
        assert_eq!(
            canon(&dec.decode_update(&second_wire).unwrap().to_json()),
            value_str(&find("pair_second_decoded"))
        );
    }

    // codec dictionaries are isolated per (instance, member) and per pair
    {
        let snapshot = parse_service_subscription_snapshot(&json!({
            "serviceId": "pi.states",
            "mode": "singleton",
            "instances": [
                {
                    "members": [
                        { "name": "left", "kind": "state", "sequence": 0,
                          "ops": [["r", { "revision": 0 }]] },
                        { "name": "right", "kind": "state", "sequence": 0,
                          "ops": [["r", { "revision": 0 }]] },
                    ],
                },
            ],
        }))
        .unwrap();
        let mut first_encoder = ServiceStateEncoder::new();
        let mut first_decoder = ServiceStateDecoder::new();
        let mut second_encoder = ServiceStateEncoder::new();
        let mut second_decoder = ServiceStateDecoder::new();
        first_decoder
            .decode_snapshot(&first_encoder.encode_snapshot(&snapshot).unwrap())
            .unwrap();
        second_decoder
            .decode_snapshot(&second_encoder.encode_snapshot(&snapshot).unwrap())
            .unwrap();
        let update = |member: &str, sequence: u64, revision: u64| ServiceProviderUpdate::State {
            instance: None,
            member: member.to_owned(),
            sequence,
            ops: vec![Op::Set {
                path: vec![k("revision")],
                value: json!(revision),
            }],
        };
        let first_left = first_encoder.encode_update(&update("left", 1, 1)).unwrap();
        let first_right = first_encoder.encode_update(&update("right", 1, 1)).unwrap();
        let second_left = first_encoder.encode_update(&update("left", 2, 2)).unwrap();
        let second_right = first_encoder.encode_update(&update("right", 2, 2)).unwrap();
        assert_eq!(
            canon(&first_left.to_json()),
            value_str(&find("isolate_first_left"))
        );
        assert_eq!(
            canon(&first_right.to_json()),
            value_str(&find("isolate_first_right"))
        );
        assert_eq!(
            canon(&second_left.to_json()),
            value_str(&find("isolate_second_left"))
        );
        assert_eq!(
            canon(&second_right.to_json()),
            value_str(&find("isolate_second_right"))
        );
        assert_eq!(
            canon(&first_decoder.decode_update(&first_left).unwrap().to_json()),
            value_str(&find("isolate_first_left_dec"))
        );
        assert_eq!(
            canon(&first_decoder.decode_update(&first_right).unwrap().to_json()),
            value_str(&find("isolate_first_right_dec"))
        );
        assert_eq!(
            canon(&first_decoder.decode_update(&second_left).unwrap().to_json()),
            value_str(&find("isolate_second_left_dec"))
        );
        assert_eq!(
            canon(
                &first_decoder
                    .decode_update(&second_right)
                    .unwrap()
                    .to_json()
            ),
            value_str(&find("isolate_second_right_dec"))
        );
        let independent_left = second_encoder.encode_update(&update("left", 1, 1)).unwrap();
        assert_eq!(
            canon(&independent_left.to_json()),
            value_str(&find("isolate_independent_left"))
        );
        assert_eq!(
            canon(
                &second_decoder
                    .decode_update(&independent_left)
                    .unwrap()
                    .to_json()
            ),
            value_str(&find("isolate_independent_left_dec"))
        );
        let left_base = ServiceProviderUpdate::State {
            instance: None,
            member: "left".to_owned(),
            sequence: 3,
            ops: vec![Op::Replace(json!({ "revision": 3 }))],
        };
        assert_eq!(
            canon(
                &first_decoder
                    .decode_update(&first_encoder.encode_update(&left_base).unwrap())
                    .unwrap()
                    .to_json()
            ),
            value_str(&find("isolate_left_base_dec"))
        );
        let third_right = first_encoder.encode_update(&update("right", 3, 3)).unwrap();
        assert_eq!(
            canon(&third_right.to_json()),
            value_str(&find("isolate_third_right"))
        );
        assert_eq!(
            canon(&first_decoder.decode_update(&third_right).unwrap().to_json()),
            value_str(&find("isolate_third_right_dec"))
        );
    }

    // keyed instance codecs follow the spawn/close lifecycle
    {
        let mut enc = ServiceStateEncoder::new();
        let mut dec = ServiceStateDecoder::new();
        let snapshot = parse_service_subscription_snapshot(&json!({
            "serviceId": "pi.dialogs", "mode": "keyed", "instances": []
        }))
        .unwrap();
        assert_eq!(
            canon(
                &dec.decode_snapshot(&enc.encode_snapshot(&snapshot).unwrap())
                    .unwrap()
                    .to_json()
            ),
            value_str(&find("keyed_snapshot_roundtrip"))
        );
        let address = ServiceInstanceAddress {
            key: "dialog-1".to_owned(),
            generation: 1,
        };
        let spawned = ServiceProviderUpdate::Spawned {
            instance: crate::chord::types::ServiceInstanceSnapshot {
                instance: Some(address.clone()),
                members: vec![crate::chord::types::ServiceMemberSnapshot::State {
                    name: "request".to_owned(),
                    sequence: 0,
                    ops: vec![Op::Replace(json!({ "value": 0 }))],
                }],
            },
        };
        assert_eq!(
            canon(
                &dec.decode_update(&enc.encode_update(&spawned).unwrap())
                    .unwrap()
                    .to_json()
            ),
            value_str(&find("keyed_spawned_dec"))
        );
        let update = ServiceProviderUpdate::State {
            instance: Some(address.clone()),
            member: "request".to_owned(),
            sequence: 1,
            ops: vec![Op::Set {
                path: vec![k("value")],
                value: json!(1),
            }],
        };
        assert_eq!(
            canon(
                &dec.decode_update(&enc.encode_update(&update).unwrap())
                    .unwrap()
                    .to_json()
            ),
            value_str(&find("keyed_update_dec"))
        );
        let closed = ServiceProviderUpdate::Closed {
            instance: address.clone(),
        };
        assert_eq!(
            canon(
                &dec.decode_update(&enc.encode_update(&closed).unwrap())
                    .unwrap()
                    .to_json()
            ),
            value_str(&find("keyed_closed_dec"))
        );
        let error = enc
            .encode_update(&ServiceProviderUpdate::State {
                instance: Some(address),
                member: "request".to_owned(),
                sequence: 2,
                ops: vec![Op::Set {
                    path: vec![k("value")],
                    value: json!(1),
                }],
            })
            .expect_err("must fail");
        assert_eq!(
            error.message(),
            error_str(&find("keyed_unknown_after_close"))
        );
    }
}

#[test]
fn service_rows_match_oracle() {
    let oracle: Value = serde_json::from_str(ORACLE).expect("oracle fixture parses");
    let rows = oracle["services"].as_array().expect("service rows");
    for row in rows {
        let name = row["name"].as_str().unwrap();
        match name {
            "models_local" => assert_eq!(row["result"], json!(false)),
            "local_flag" => assert_eq!(row["result"], json!(true)),
            "models_canonical" => assert_eq!(
                canon(
                    &crate::chord::api::define_service("test.models")
                        .unwrap()
                        .to_json()
                ),
                row["value"].as_str().unwrap()
            ),
            "local_canonical" => assert_eq!(
                canon(
                    &crate::chord::api::define_service_local("test.local")
                        .unwrap()
                        .to_json()
                ),
                row["value"].as_str().unwrap()
            ),
            "reserved_id" => assert_eq!(
                crate::chord::api::define_service_local("$chord.internal")
                    .expect_err("reserved")
                    .message(),
                row["error"].as_str().unwrap()
            ),
            "empty_id" => assert_eq!(
                crate::chord::api::define_service("")
                    .expect_err("empty")
                    .message(),
                row["error"].as_str().unwrap()
            ),
            other => panic!("unhandled service row {other}"),
        }
    }
}

// ── delta-slice oracle (chord_delta_oracle.json) ─────────────────────────────

const DELTA_ORACLE: &str =
    include_str!("../../../tests/fixtures/chord_delta_oracle/chord_delta_oracle.json");

fn canon_value(value: &JsonValue) -> String {
    let mut sorted = value.clone();
    sorted.sort_all_objects();
    serde_json::to_string(&sorted).expect("canonical serialization cannot fail")
}

/// Replicated state publication order: subscribe hydrate, one delivery per
/// publication (empty changes and no-op replacements consume the revision
/// silently), unsubscribe stops deliveries, replace publishes atomically.
#[test]
fn replicated_state_matches_oracle() {
    let oracle: Value = serde_json::from_str(DELTA_ORACLE).expect("delta oracle parses");
    let expected = &oracle["services"]["state"];
    let state = crate::chord::api::replicated_state(json!({ "count": 0, "log": [] }));
    let events = Arc::new(Mutex::new(Vec::<Value>::new()));
    let unsubscribe = {
        let events = Arc::clone(&events);
        state.subscribe(move |value, _context, delivery| {
            events.lock().unwrap().push(json!({
                "kind": "listener",
                "delivery": delivery.to_json(),
                "value": serde_json::from_str::<Value>(
                    &{
                        let mut sorted = value.clone();
                        sorted.sort_all_objects();
                        serde_json::to_string(&sorted).unwrap()
                    },
                )
                .unwrap(),
            }));
        })
    };
    let change_n = |state: &Arc<MutableReplicatedState>, n: i64| {
        state
            .change(&Context::background(), |draft| {
                draft
                    .set(&[k("count")], json!(n))
                    .map_err(|error| ChordError::Type(error.message()))
            })
            .expect("change publishes");
    };
    // change 1: one draft mutation touching two members.
    state
        .change(&Context::background(), |draft| {
            draft
                .set(&[k("count")], json!(1))
                .map_err(|error| ChordError::Type(error.message()))?;
            draft
                .push(&[k("log")], vec![json!("one")])
                .map_err(|error| ChordError::Type(error.message()))
                .map(|_| ())
        })
        .expect("change publishes");
    change_n(&state, 2);
    // An empty change consumes a revision without publishing.
    state
        .change(&Context::background(), |_| Ok(()))
        .expect("empty change");
    // Whole-value replacement, then a no-op replacement (revision only).
    state
        .replace(
            &Context::background(),
            json!({ "count": 2, "log": ["one"], "extra": true }),
        )
        .expect("replace publishes");
    state
        .replace(
            &Context::background(),
            json!({ "count": 2, "log": ["one"], "extra": true }),
        )
        .expect("noop replace");
    unsubscribe();
    change_n(&state, 3);

    assert!(expected["error"].is_null(), "capture reported an error");
    assert_eq!(
        events.lock().unwrap().len(),
        expected["events"].as_array().expect("events").len(),
        "event count"
    );
    for (at, (actual, want)) in events
        .lock()
        .unwrap()
        .iter()
        .zip(expected["events"].as_array().unwrap().iter())
        .enumerate()
    {
        assert_eq!(
            canon_value(&actual["value"]),
            want["value"].as_str().unwrap(),
            "event {at}"
        );
    }
    assert_eq!(
        canon_value(&state.value()),
        expected["final"].as_str().unwrap(),
        "final value"
    );
    assert_eq!(
        expected["final_sequence"],
        json!(expected["events"]
            .as_array()
            .unwrap()
            .last()
            .map(|at| at["delivery"]["sequence"].clone())
            .unwrap_or(Value::Null)),
        "fixture sanity"
    );
    // The post-unsubscribe change still advanced the publication sequence
    // (4 = hydrate 0 + three delivered publications + one silent one).
    assert_eq!(state.sequence(), 4, "sequence advances on publications");
}

/// Provider buffer overflow rebaselines a cold subscriber with a full reset
/// snapshot; active subscribers stream; snapshot sequences suppress
/// already-covered updates.
#[test]
fn provider_reset_matches_oracle() {
    let oracle: Value = serde_json::from_str(DELTA_ORACLE).expect("delta oracle parses");
    let expected = &oracle["services"]["provider_reset"];
    assert!(expected["error"].is_null(), "capture reported an error");

    let provider = RemoteServiceProvider::new(&[ProviderEntry {
        id: "svc".to_owned(),
        mode: ServiceMode::Singleton,
        local: false,
    }])
    .expect("provider");
    let state = crate::chord::api::replicated_state(json!({ "n": 0 }));
    let mut implementation = Implementation::new();
    implementation.insert("state".to_owned(), state_member(&state));
    provider
        .provide(
            &crate::chord::api::define_service("svc").unwrap(),
            implementation,
        )
        .expect("provide");

    let updates = Arc::new(Mutex::new(Vec::<String>::new()));
    let subscription = {
        let updates = Arc::clone(&updates);
        provider
            .subscribe("svc", ServiceMode::Singleton, move |update, _context| {
                updates.lock().unwrap().push(canon_value(&update.to_json()));
                Ok(())
            })
            .expect("subscribe")
    };
    subscription.activate().expect("activate");
    for n in 1..=120i64 {
        state
            .change(&Context::background(), |draft| {
                draft
                    .set(&[k("n")], json!(n))
                    .map_err(|error| ChordError::Type(error.message()))
            })
            .expect("change publishes");
    }
    assert_eq!(
        updates.lock().unwrap().len(),
        120,
        "every active-subscriber update delivers inline"
    );

    let buffered = Arc::new(Mutex::new(Vec::<String>::new()));
    let late_subscription = {
        let buffered = Arc::clone(&buffered);
        provider
            .subscribe("svc", ServiceMode::Singleton, move |update, _context| {
                buffered
                    .lock()
                    .unwrap()
                    .push(canon_value(&update.to_json()));
                Ok(())
            })
            .expect("subscribe")
    };
    for n in 121..=240i64 {
        state
            .change(&Context::background(), |draft| {
                draft
                    .set(&[k("n")], json!(n))
                    .map_err(|error| ChordError::Type(error.message()))
            })
            .expect("change publishes");
    }
    late_subscription.activate().expect("activate");
    let buffered_after = buffered.lock().unwrap().clone();
    let resets = buffered_after
        .iter()
        .filter(|at| {
            serde_json::from_str::<Value>(at)
                .expect("canonical json")
                .get("type")
                .and_then(Value::as_str)
                == Some("reset")
        })
        .count();
    assert_eq!(
        resets,
        expected["resets"].as_u64().unwrap_or(0) as usize + 1,
        "exactly one reset rebaseline"
    );
    let first: Value = serde_json::from_str(&buffered_after[0]).unwrap();
    assert_eq!(first["type"], json!("reset"));
    assert_eq!(
        first["snapshot"]["instances"][0]["members"][0]["ops"][0][0],
        json!("r"),
        "reset carries full root replacements"
    );
    assert_eq!(
        first["snapshot"]["instances"][0]["members"][0]["sequence"],
        json!(221),
        "reset snapshot sequence"
    );
    provider.dispose().expect("dispose");
    assert!(
        updates
            .lock()
            .unwrap()
            .last()
            .map(|at| at.contains("\"unavailable\""))
            .unwrap_or(false),
        "dispose closes active subscribers with unavailable"
    );
}

/// Replica hydration, update sequencing, gap clearing and base-batch
/// validation.
#[test]
fn replica_matches_oracle() {
    let oracle: Value = serde_json::from_str(DELTA_ORACLE).expect("delta oracle parses");
    let expected = &oracle["services"]["replica"];
    let replica = crate::chord::services::state::ReplicatedStateReplica::new();
    let events = Arc::new(Mutex::new(Vec::<Value>::new()));
    {
        let events = Arc::clone(&events);
        let _unsubscribe = replica.subscribe(move |value, _context, delivery| {
            let mut sorted = value.clone();
            sorted.sort_all_objects();
            events.lock().unwrap().push(json!({
                "delivery": delivery.to_json(),
                "value": serde_json::to_string(&sorted).unwrap(),
            }));
        });
    }
    replica
        .hydrate(1, &[Op::Replace(json!({ "v": 1 }))], &Context::background())
        .expect("hydrate");
    replica
        .update(
            2,
            &[Op::Set {
                path: vec![k("v")],
                value: json!(2),
            }],
            &Context::background(),
        )
        .expect("update");
    let gap = replica
        .update(
            5,
            &[Op::Set {
                path: vec![k("v")],
                value: json!(5),
            }],
            &Context::background(),
        )
        .expect_err("gap fails");
    assert_eq!(gap.message(), "Replicated state update sequence has a gap");
    assert!(replica.value().is_none(), "a gap clears the replica");
    let non_base = replica
        .hydrate(
            9,
            &[Op::Set {
                path: vec![k("v")],
                value: json!(9),
            }],
            &Context::background(),
        )
        .expect_err("non-base snapshot fails");
    assert_eq!(
        non_base.message(),
        "Replicated state snapshot is not a base operation batch"
    );
    assert!(expected["error"].is_null(), "capture reported an error");
    assert_eq!(
        events.lock().unwrap().len(),
        expected["events"].as_array().expect("events").len()
    );
    for (at, (actual, want)) in events
        .lock()
        .unwrap()
        .iter()
        .zip(expected["events"].as_array().unwrap().iter())
        .enumerate()
    {
        assert_eq!(canon_value(actual), canon_value(want), "replica event {at}");
    }
}

/// The replica-side revision validator accepts strict JSON trees (every
/// upstream rejection is unrepresentable over owned values; see the
/// validator module docs).
#[test]
fn validator_and_json_guards() {
    let oracle: Value = serde_json::from_str(DELTA_ORACLE).expect("delta oracle parses");
    let expected = &oracle["services"]["validator"];
    let validator = crate::chord::delta::JsonRevisionValidator;
    let validated = validator.validate(&json!({ "a": [1, { "b": null }] }));
    assert_eq!(
        canon_value(&validated),
        expected["plain"].as_str().unwrap(),
        "validate pass-through"
    );
    // isJsonValue over owned trees is constant true (the Map case in the
    // fixture is JS-only).
    assert!(crate::chord::json::is_json_value(&json!({ "a": 1 })));
    assert_eq!(expected["is_json"], json!([true, false]), "fixture sanity");
}

type Publisher = Arc<dyn Fn(&str, &ServiceProviderUpdate, &Context) + Send + Sync>;

fn method<F>(f: F) -> InstanceMember
where
    F: Fn(&[JsonValue], &Context) -> Result<Option<JsonValue>, ChordError> + Send + Sync + 'static,
{
    InstanceMember::Method(Arc::new(f))
}

fn state_member(state: &Arc<MutableReplicatedState>) -> InstanceMember {
    InstanceMember::State(Arc::clone(state))
}

/// One draft mutation published synchronously (the new-API equivalent of
/// the old `set` + `publish` pair in these rows).
fn publish_value(state: &Arc<MutableReplicatedState>, value: JsonValue) {
    state
        .change(&Context::background(), |draft| {
            draft
                .set(&[k("value")], value)
                .map_err(|error| ChordError::Type(error.message()))
        })
        .expect("change publishes");
}

fn publish_revision(state: &Arc<MutableReplicatedState>, revision: i64) {
    state
        .change(&Context::background(), |draft| {
            draft
                .set(&[k("revision")], json!(revision))
                .map_err(|error| ChordError::Type(error.message()))
        })
        .expect("change publishes");
}

fn implementation<const N: usize>(members: [(&str, InstanceMember); N]) -> Implementation {
    members
        .into_iter()
        .map(|(name, member)| (name.to_owned(), member))
        .collect()
}

#[test]
fn provider_rows_match_oracle() {
    let oracle: Value = serde_json::from_str(ORACLE).expect("oracle fixture parses");
    let rows = oracle["provider"].as_array().expect("provider rows");
    let find = |name: &str| -> Value {
        rows.iter()
            .find(|row| row["name"] == json!(name))
            .unwrap_or_else(|| panic!("missing provider row {name}"))
            .clone()
    };
    let updates_json = |updates: &[String]| canon(&json!(updates.to_vec()));

    // endpoint: catalogue, subscribe, publish, dispose
    {
        let counter = crate::chord::api::define_service("test.counter").unwrap();
        let provider =
            RemoteServiceProvider::new(&[ProviderEntry::singleton(&counter.id)]).unwrap();
        let state = crate::chord::api::replicated_state(json!({ "value": 0 }));
        provider
            .provide(&counter, implementation([("state", state_member(&state))]))
            .unwrap();
        let endpoint: RemoteServiceEndpoint = create_remote_service_endpoint(Arc::clone(&provider));
        let updates = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink = Arc::clone(&updates);
        let publish: Publisher = Arc::new(move |id, update, _context| {
            let _ = id;
            sink.lock().unwrap().push(canon(&update.to_json()));
        });
        let catalogue = endpoint
            .invoke(
                &create_service_catalogue_call(),
                Arc::clone(&publish),
                &Context::background(),
            )
            .unwrap();
        assert_eq!(
            canon(&catalogue.expect("catalogue")),
            find("endpoint_catalogue")["value"].as_str().unwrap()
        );
        let snapshot = endpoint
            .invoke(
                &create_service_subscribe_call(
                    "subscription-1",
                    &counter.id,
                    ServiceMode::Singleton,
                ),
                Arc::clone(&publish),
                &Context::background(),
            )
            .unwrap();
        assert_eq!(
            canon(&snapshot.expect("snapshot")),
            find("endpoint_subscribe_snapshot")["value"]
                .as_str()
                .unwrap()
        );
        publish_value(&state, json!(1));
        assert_eq!(
            updates_json(&updates.lock().unwrap()),
            canon(&find("endpoint_updates_after_publish")["result"])
        );
        endpoint.dispose();
        publish_value(&state, json!(2));
        assert_eq!(
            updates.lock().unwrap().len(),
            find("endpoint_updates_after_dispose_len")["result"]
                .as_u64()
                .unwrap() as usize
        );
        provider.dispose().unwrap();
    }

    // provider catalogue, local rejection, raw subscription, late snapshot
    {
        let models = crate::chord::api::define_service("test.models").unwrap();
        let provider = RemoteServiceProvider::new(&[ProviderEntry::singleton(&models.id)]).unwrap();
        assert_eq!(
            canon(&json!(provider
                .catalogue()
                .iter()
                .map(|entry| entry.to_json())
                .collect::<Vec<_>>())),
            find("provider_catalogue")["value"].as_str().unwrap()
        );
        let local = crate::chord::api::define_service_local("test.local").unwrap();
        let error = RemoteServiceProvider::new(&[ProviderEntry::from_service(
            &local,
            ServiceMode::Singleton,
        )])
        .expect_err("local services cannot be published");
        assert_eq!(
            error.message(),
            find("provider_rejects_local")["error"].as_str().unwrap()
        );

        let state =
            crate::chord::api::replicated_state(json!({ "revision": 0, "selected": Value::Null }));
        let method_state = Arc::clone(&state);
        provider
            .provide(
                &models,
                implementation([
                    (
                        "select",
                        method(move |args, context| {
                            let _ = args;
                            method_state.change(context, |draft| {
                                draft
                                    .set(&[k("revision")], json!(1))
                                    .map_err(|error| ChordError::Type(error.message()))
                            })?;
                            Ok(None)
                        }),
                    ),
                    ("state", state_member(&state)),
                ]),
            )
            .unwrap();
        let updates = Arc::new(Mutex::new(Vec::<String>::new()));
        let raw = provider
            .subscribe(&models.id, ServiceMode::Singleton, {
                let updates = Arc::clone(&updates);
                move |update, _context| {
                    updates.lock().unwrap().push(canon(&update.to_json()));
                    Ok(())
                }
            })
            .unwrap();
        assert_eq!(
            canon(&members_json(raw.snapshot())),
            find("raw_snapshot_members")["value"].as_str().unwrap()
        );
        raw.activate().unwrap();
        provider
            .invoke(
                &ServiceCall {
                    service_id: models.id.clone(),
                    instance: None,
                    member: "select".to_owned(),
                    args: vec![json!({ "modelId": "one", "provider": "test" })],
                },
                &Context::background(),
            )
            .unwrap();
        assert_eq!(
            json!(*updates.lock().unwrap()),
            find("provider_updates_after_invoke")["result"],
            "provider updates after invoke"
        );
        let late = provider
            .subscribe(&models.id, ServiceMode::Singleton, |_update, _context| {
                Ok(())
            })
            .unwrap();
        assert_eq!(
            canon(&members_json(late.snapshot())),
            find("late_snapshot_members")["value"].as_str().unwrap()
        );
        late.close();
        raw.close();
        provider.dispose().unwrap();
    }

    // withdraw, replace, and the shape preservation checks
    {
        let models = crate::chord::api::define_service("test.models").unwrap();
        let provider = RemoteServiceProvider::new(&[ProviderEntry::singleton(&models.id)]).unwrap();
        provider
            .provide(
                &models,
                implementation([
                    ("select", method(|_args, _context| Ok(None))),
                    (
                        "state",
                        state_member(&crate::chord::api::replicated_state(
                            json!({ "revision": 1, "selected": Value::Null }),
                        )),
                    ),
                ]),
            )
            .unwrap();
        provider.withdraw(&models).unwrap();
        let error = provider
            .invoke(
                &ServiceCall {
                    service_id: models.id.clone(),
                    instance: None,
                    member: "select".to_owned(),
                    args: vec![],
                },
                &Context::background(),
            )
            .expect_err("withdrawn");
        assert_eq!(
            error.message(),
            find("withdraw_then_invoke")["error"].as_str().unwrap()
        );
        provider
            .replace(
                &models,
                implementation([
                    ("select", method(|_args, _context| Ok(None))),
                    (
                        "state",
                        state_member(&crate::chord::api::replicated_state(
                            json!({ "revision": 2, "selected": Value::Null }),
                        )),
                    ),
                ]),
            )
            .unwrap();
        let error = provider
            .replace(
                &models,
                implementation([("select", method(|_args, _context| Ok(None)))]),
            )
            .expect_err("shape mismatch");
        assert_eq!(
            error.message(),
            find("replace_shape_mismatch_missing_member")["error"]
                .as_str()
                .unwrap()
        );
        let error = provider
            .replace(
                &models,
                implementation([
                    ("select", method(|_args, _context| Ok(None))),
                    ("state", method(|_args, _context| Ok(None))),
                ]),
            )
            .expect_err("shape mismatch");
        assert_eq!(
            error.message(),
            find("replace_shape_mismatch_kind_change")["error"]
                .as_str()
                .unwrap()
        );
        provider.dispose().unwrap();
    }

    // listener failures deliver to everyone before being reported
    {
        let models = crate::chord::api::define_service("test.models").unwrap();
        let provider = RemoteServiceProvider::new(&[ProviderEntry::singleton(&models.id)]).unwrap();
        provider
            .provide(
                &models,
                implementation([
                    ("select", method(|_args, _context| Ok(None))),
                    (
                        "state",
                        state_member(&crate::chord::api::replicated_state(
                            json!({ "revision": 1, "selected": Value::Null }),
                        )),
                    ),
                ]),
            )
            .unwrap();
        let delivered = Arc::new(Mutex::new(0u64));
        let failing = provider
            .subscribe(&models.id, ServiceMode::Singleton, |_update, _context| {
                Err(ChordError::Type("listener failed".to_owned()))
            })
            .unwrap();
        let delivered_sink = Arc::clone(&delivered);
        let succeeding = provider
            .subscribe(
                &models.id,
                ServiceMode::Singleton,
                move |_update, _context| {
                    *delivered_sink.lock().unwrap() += 1;
                    Ok(())
                },
            )
            .unwrap();
        failing.activate().unwrap();
        succeeding.activate().unwrap();
        let replace_error = provider
            .replace(
                &models,
                implementation([
                    ("select", method(|_args, _context| Ok(None))),
                    (
                        "state",
                        state_member(&crate::chord::api::replicated_state(
                            json!({ "revision": 2, "selected": Value::Null }),
                        )),
                    ),
                ]),
            )
            .expect_err("listener failure propagates");
        assert_eq!(
            replace_error.message(),
            find("listener_failure_message")["result"].as_str().unwrap()
        );
        assert_eq!(
            *delivered.lock().unwrap(),
            find("listener_failure_delivered")["result"]
                .as_u64()
                .unwrap()
        );
        failing.close();
        succeeding.close();
        provider.dispose().unwrap();
    }

    // buffered updates replay on activation before failures are reported
    {
        let models = crate::chord::api::define_service("test.models").unwrap();
        let provider = RemoteServiceProvider::new(&[ProviderEntry::singleton(&models.id)]).unwrap();
        let state =
            crate::chord::api::replicated_state(json!({ "revision": 0, "selected": Value::Null }));
        provider
            .provide(
                &models,
                implementation([
                    ("select", method(|_args, _context| Ok(None))),
                    ("state", state_member(&state)),
                ]),
            )
            .unwrap();
        let delivered = Arc::new(Mutex::new(0u64));
        let subscription = provider
            .subscribe(&models.id, ServiceMode::Singleton, {
                let delivered = Arc::clone(&delivered);
                move |_update, _context| {
                    *delivered.lock().unwrap() += 1;
                    Err(ChordError::Type("listener failed".to_owned()))
                }
            })
            .unwrap();
        publish_revision(&state, 1);
        publish_revision(&state, 2);
        let error = subscription.activate().expect_err("activation fails");
        assert_eq!(
            error.message(),
            find("buffered_replay_message")["result"].as_str().unwrap()
        );
        assert_eq!(
            *delivered.lock().unwrap(),
            find("buffered_replay_delivered")["result"]
                .as_u64()
                .unwrap()
        );
        subscription.close();
        provider.dispose().unwrap();
    }

    // keyed instances: spawn, invoke, stale generations, close, disposal
    {
        let models = crate::chord::api::define_service("test.models").unwrap();
        let dialogs = crate::chord::api::define_service("test.question-dialogs").unwrap();
        let provider = RemoteServiceProvider::new(&[
            ProviderEntry::singleton(&models.id),
            ProviderEntry::keyed(&dialogs.id),
        ])
        .unwrap();
        provider
            .provide(
                &models,
                implementation([
                    ("select", method(|_args, _context| Ok(None))),
                    (
                        "state",
                        state_member(&crate::chord::api::replicated_state(
                            json!({ "revision": 0, "selected": Value::Null }),
                        )),
                    ),
                ]),
            )
            .unwrap();
        let error = provider
            .spawn(
                &models,
                "wrong",
                implementation([("select", method(|_args, _context| Ok(None)))]),
            )
            .expect_err("spawn on singleton");
        assert_eq!(
            error.message(),
            find("spawn_on_singleton")["error"].as_str().unwrap()
        );

        let keyed_updates = Arc::new(Mutex::new(Vec::<String>::new()));
        let close_first = provider
            .spawn(
                &dialogs,
                "invocation-1",
                implementation([
                    (
                        "request",
                        state_member(&crate::chord::api::replicated_state(
                            json!({ "question": "First?" }),
                        )),
                    ),
                    (
                        "submit",
                        method(|_args, _context| Ok(Some(json!({ "accepted": true })))),
                    ),
                ]),
            )
            .unwrap();
        let keyed_subscription = provider
            .subscribe(&dialogs.id, ServiceMode::Keyed, {
                let keyed_updates = Arc::clone(&keyed_updates);
                move |update, _context| {
                    keyed_updates.lock().unwrap().push(canon(&update.to_json()));
                    Ok(())
                }
            })
            .unwrap();
        keyed_subscription.activate().unwrap();
        let invoke = provider
            .invoke(
                &ServiceCall {
                    service_id: dialogs.id.clone(),
                    instance: Some(ServiceInstanceAddress {
                        key: "invocation-1".to_owned(),
                        generation: 1,
                    }),
                    member: "submit".to_owned(),
                    args: vec![json!("yes")],
                },
                &Context::background(),
            )
            .unwrap();
        assert_eq!(
            canon(&invoke.expect("invoke result")),
            find("keyed_invoke_result")["value"].as_str().unwrap()
        );
        let error = provider
            .invoke(
                &ServiceCall {
                    service_id: dialogs.id.clone(),
                    instance: Some(ServiceInstanceAddress {
                        key: "invocation-1".to_owned(),
                        generation: 2,
                    }),
                    member: "submit".to_owned(),
                    args: vec![json!("yes")],
                },
                &Context::background(),
            )
            .expect_err("stale generation");
        assert_eq!(
            error.message(),
            find("keyed_invoke_stale_generation")["error"]
                .as_str()
                .unwrap()
        );
        close_first.close().unwrap();
        assert_eq!(
            updates_json(&keyed_updates.lock().unwrap()),
            canon(&find("keyed_closed_updates")["result"])
        );
        let error = provider
            .invoke(
                &ServiceCall {
                    service_id: dialogs.id.clone(),
                    instance: Some(ServiceInstanceAddress {
                        key: "invocation-1".to_owned(),
                        generation: 1,
                    }),
                    member: "submit".to_owned(),
                    args: vec![json!("yes")],
                },
                &Context::background(),
            )
            .expect_err("instance closed");
        assert_eq!(
            error.message(),
            find("keyed_invoke_after_close")["error"].as_str().unwrap()
        );
        let error = provider
            .invoke(
                &ServiceCall {
                    service_id: models.id.clone(),
                    instance: None,
                    member: "missing".to_owned(),
                    args: vec![],
                },
                &Context::background(),
            )
            .expect_err("unknown member");
        assert_eq!(
            error.message(),
            find("invoke_unknown_member")["error"].as_str().unwrap()
        );
        let error = provider
            .invoke(
                &ServiceCall {
                    service_id: "nope".to_owned(),
                    instance: None,
                    member: "m".to_owned(),
                    args: vec![],
                },
                &Context::background(),
            )
            .expect_err("unknown service");
        assert_eq!(
            error.message(),
            find("invoke_unknown_service")["error"].as_str().unwrap()
        );
        let error = provider
            .invoke(
                &ServiceCall {
                    service_id: models.id.clone(),
                    instance: None,
                    member: "state".to_owned(),
                    args: vec![],
                },
                &Context::background(),
            )
            .expect_err("state member is not a method");
        assert_eq!(
            error.message(),
            find("invoke_state_member")["error"].as_str().unwrap()
        );
        let error = provider
            .spawn(
                &dialogs,
                "dupe",
                implementation([
                    (
                        "request",
                        state_member(&crate::chord::api::replicated_state(
                            json!({ "question": "?" }),
                        )),
                    ),
                    (
                        "submit",
                        method(|_args, _context| Ok(Some(json!({ "accepted": false })))),
                    ),
                ]),
            )
            .and_then(|first| {
                provider
                    .spawn(
                        &dialogs,
                        "dupe",
                        implementation([
                            (
                                "request",
                                state_member(&crate::chord::api::replicated_state(
                                    json!({ "question": "?" }),
                                )),
                            ),
                            (
                                "submit",
                                method(|_args, _context| Ok(Some(json!({ "accepted": false })))),
                            ),
                        ]),
                    )
                    .map(|_| first)
            })
            .expect_err("duplicate key");
        assert_eq!(
            error.message(),
            find("spawn_duplicate_key")["error"].as_str().unwrap()
        );
        provider.dispose().unwrap();
        let error = provider
            .invoke(
                &ServiceCall {
                    service_id: models.id.clone(),
                    instance: None,
                    member: "select".to_owned(),
                    args: vec![],
                },
                &Context::background(),
            )
            .expect_err("disposed");
        assert_eq!(
            error.message(),
            find("invoke_after_dispose")["error"].as_str().unwrap()
        );
    }
}

/// Port of the upstream tracker performance guard (the old delta.test.ts
/// "pending operation coalescing" check, kept as a local guard now that the
/// upstream benchmark moved to worker processes): one wide change must stay
/// roughly linear, not quadratic, in the number of dirty nodes.
#[test]
fn emission_is_linear_in_the_number_of_dirty_nodes() {
    fn wide(n: usize) -> u128 {
        let mut root = serde_json::Map::new();
        for i in 0..n {
            root.insert(format!("f{i}"), json!(i));
        }
        let tracker = crate::chord::delta::track(JsonValue::Object(root));
        let change = tracker.begin_change();
        for i in 0..n {
            change.set(&[k(&format!("f{i}"))], json!(i + 1)).unwrap();
        }
        let started = std::time::Instant::now();
        let prepared = change.prepare().expect("wide change prepares");
        prepared.abort();
        started.elapsed().as_millis().max(1)
    }
    wide(200);
    let small = wide(250);
    let large = wide(2500);
    // The small case measures ~1ms on quiet machines, where scheduler noise
    // alone can inflate the large-run ratio far beyond the scaling factor;
    // clamp the denominator to a floor that keeps the check meaningful (a
    // quadratic large would still blow far past 40x20ms).
    let floor = small.max(20);
    assert!(
        large / floor < 40,
        "emission should be near-linear: large={large}ms small={small}ms floor={floor}ms"
    );
}
