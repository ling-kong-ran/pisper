//! Tests for the chord-face seam (S1): the wire-grammar validators, the path
//! interning decoder, the per-state codec registry, and the service call
//! faces. Error texts and decoded values are pinned against the node oracle
//! (`tests/fixtures/client_oracle/oracle.out.txt` runs the real upstream chord
//! code); the wire-grammar edges below follow `chord/src/delta/index.ts`
//! assertions directly.

use std::sync::Arc;

use crate::client::service::{
    assert_valid_wire_op, create_service_catalogue_call, create_service_subscribe_call,
    create_service_unsubscribe_call, parse_service_call, parse_service_catalogue,
    parse_wire_service_provider_update, parse_wire_service_subscription_snapshot, wire_op_decoder,
    ChordServiceStateDecoder, SeamError, ServiceMode, ServiceStateDecoder,
};
use crate::client::support::parse_ordered_json;
use crate::protocol::json::JsonValue;

fn wire_op(text: &str) -> JsonValue {
    parse_ordered_json(text)
}

#[test]
fn service_call_constructors_match_upstream_key_order() {
    let catalogue = create_service_catalogue_call();
    assert_eq!(
        catalogue,
        parse_ordered_json(
            "{\"serviceId\":\"$chord.service\",\"member\":\"catalogue\",\"args\":[]}"
        )
    );
    let subscribe = create_service_subscribe_call("service-1", "pi.models", ServiceMode::Singleton);
    assert_eq!(
        subscribe,
        parse_ordered_json(
            "{\"serviceId\":\"$chord.service\",\"member\":\"subscribe\",\"args\":[\"service-1\",\"pi.models\",\"singleton\"]}"
        )
    );
    let unsubscribe = create_service_unsubscribe_call("service-1");
    assert_eq!(
        unsubscribe,
        parse_ordered_json(
            "{\"serviceId\":\"$chord.service\",\"member\":\"unsubscribe\",\"args\":[\"service-1\"]}"
        )
    );
}

#[test]
fn parse_service_call_accepts_valid_and_optional_instance() {
    let call = parse_ordered_json(
        "{\"serviceId\":\"test\",\"member\":\"run\",\"args\":[1],\"instance\":{\"key\":\"k\",\"generation\":2}}",
    );
    assert_eq!(parse_service_call(&call).expect("valid"), call);

    let plain = parse_ordered_json("{\"serviceId\":\"test\",\"member\":\"run\",\"args\":[]}");
    assert_eq!(parse_service_call(&plain).expect("valid"), plain);
}

#[test]
fn parse_service_call_rejects_malformed_calls_with_upstream_texts() {
    let invalid = parse_ordered_json("{\"serviceId\":\"test\",\"member\":\"run\"}");
    assert_eq!(
        parse_service_call(&invalid)
            .expect_err("missing args")
            .message(),
        "Invalid service call"
    );

    let extra =
        parse_ordered_json("{\"serviceId\":\"test\",\"member\":\"run\",\"args\":[],\"x\":1}");
    assert_eq!(
        parse_service_call(&extra).expect_err("extra key").message(),
        "Invalid service call"
    );

    let empty_id = parse_ordered_json("{\"serviceId\":\"\",\"member\":\"run\",\"args\":[]}");
    assert_eq!(
        parse_service_call(&empty_id)
            .expect_err("empty id")
            .message(),
        "Invalid service call"
    );

    let array = parse_ordered_json("[1,2]");
    assert_eq!(
        parse_service_call(&array).expect_err("array").message(),
        "Invalid service call"
    );

    let bad_address = parse_ordered_json(
        "{\"serviceId\":\"test\",\"member\":\"run\",\"args\":[],\"instance\":{\"key\":\"k\"}}",
    );
    assert_eq!(
        parse_service_call(&bad_address)
            .expect_err("address")
            .message(),
        "Invalid service instance address"
    );
}

#[test]
fn parse_service_catalogue_validates_entries() {
    let entries = parse_ordered_json("[{\"serviceId\":\"pi.models\",\"mode\":\"singleton\"}]");
    assert_eq!(
        parse_service_catalogue(&entries).expect("valid"),
        entries.as_array().unwrap().to_vec()
    );

    let not_array = parse_ordered_json("{\"not\":\"an array\"}");
    assert_eq!(
        parse_service_catalogue(&not_array)
            .expect_err("not array")
            .message(),
        "Invalid service catalogue"
    );

    let entry_error = parse_ordered_json("[{\"serviceId\":\"pi.models\",\"mode\":\"bogus\"}]");
    assert_eq!(
        parse_service_catalogue(&entry_error)
            .expect_err("mode")
            .message(),
        "Invalid service catalogue"
    );

    let duplicate = parse_ordered_json(
        "[{\"serviceId\":\"a\",\"mode\":\"singleton\"},{\"serviceId\":\"a\",\"mode\":\"keyed\"}]",
    );
    assert_eq!(
        parse_service_catalogue(&duplicate)
            .expect_err("duplicate")
            .message(),
        "Invalid service catalogue"
    );
}

#[test]
fn wire_grammar_rejects_invalid_ops() {
    let cases: Vec<(&str, &str)> = vec![
        ("5", "op is not a tuple"),
        ("[]", "op is not a tuple"),
        ("[\"r\",1,2]", "r arity"),
        ("[\"s\"]", "s arity"),
        ("[\"s\",\"path\",1]", "path is not an array"),
        ("[\"s\",-1,1]", "bad path id"),
        ("[\"d\",\"a\",\"b\"]", "d arity"),
        ("[\"t\",[],-1]", "t count"),
        ("[\"p\",0,-1,[]]", "p remove"),
        ("[\"p\",0,0]", "p arity"),
        ("[\"#\",-1,[]]", "# shape"),
        ("[\"z\",1]", "unknown op verb: z"),
        (
            "[\"s\",[\"__proto__\"],1]",
            "unsafe path segment: __proto__",
        ),
    ];
    for (input, expected) in cases {
        let op = wire_op(input);
        let error = assert_valid_wire_op(&op).expect_err(input);
        assert_eq!(error.message(), expected, "case {input}");
    }
}

#[test]
fn wire_decoder_resolves_interned_paths_and_short_forms() {
    let mut decoder = wire_op_decoder();
    let decoded = decoder
        .decode(&[
            wire_op("[\"r\",{\"revision\":0}]"),
            wire_op("[\"s\",[\"revision\"],1]"),
            wire_op("[\"#\",0,[\"revision\"]]"),
            wire_op("[\"s\",0,3]"),
            wire_op("[\"d\"]"),
        ])
        .expect("decodes");
    assert_eq!(
        decoded,
        vec![
            wire_op("[\"r\",{\"revision\":0}]"),
            wire_op("[\"s\",[\"revision\"],1]"),
            wire_op("[\"s\",[\"revision\"],3]"),
            wire_op("[\"d\",[\"revision\"]]"),
        ]
    );
}

#[test]
fn wire_decoder_reports_unresolvable_paths_with_upstream_texts() {
    let mut decoder = wire_op_decoder();
    let error = decoder
        .decode(&[wire_op("[\"s\",0,1]")])
        .expect_err("unresolvable id");
    assert_eq!(error.message(), "unresolvable path: 0");

    // A short form with no previous path.
    let mut decoder = wire_op_decoder();
    let error = decoder.decode(&[wire_op("[\"d\"]")]).expect_err("short");
    assert_eq!(error.message(), "unresolvable path: []");

    // An empty resolved path is only legal for `p`.
    let mut decoder = wire_op_decoder();
    let error = decoder
        .decode(&[wire_op("[\"s\",[],1]"), wire_op("[\"s\",2,3]")])
        .expect_err("empty path");
    assert_eq!(error.message(), "unresolvable path: []");

    // Ids reset at a base batch.
    let mut decoder = wire_op_decoder();
    let error = decoder
        .decode(&[
            wire_op("[\"#\",0,[\"a\"]]"),
            wire_op("[\"r\",0]"),
            wire_op("[\"s\",0,1]"),
        ])
        .expect_err("id cleared by base");
    assert_eq!(error.message(), "unresolvable path: 0");
}

#[test]
fn state_decoder_decodes_snapshot_members_per_codec() {
    let mut decoder = ChordServiceStateDecoder::default();
    let snapshot = parse_ordered_json(
        "{\"serviceId\":\"pi.models\",\"mode\":\"singleton\",\"instances\":[{\"members\":[{\"name\":\"state\",\"kind\":\"state\",\"sequence\":0,\"ops\":[[\"r\",{\"revision\":0}]]},{\"name\":\"run\",\"kind\":\"method\"}]}]}",
    );
    let decoded = decoder.decode_snapshot(snapshot).expect("decodes");
    let expected = parse_ordered_json(
        "{\"serviceId\":\"pi.models\",\"mode\":\"singleton\",\"instances\":[{\"members\":[{\"name\":\"state\",\"kind\":\"state\",\"sequence\":0,\"ops\":[[\"r\",{\"revision\":0}]]},{\"name\":\"run\",\"kind\":\"method\"}]}]}",
    );
    assert_eq!(decoded, expected);

    // Updates decode through the same per-member codec, so interned ids
    // resolve across updates (this is the oracle `updates-final` behavior).
    let update = parse_ordered_json(
        "{\"type\":\"state\",\"member\":\"state\",\"sequence\":2,\"ops\":[[\"#\",7,[\"revision\"]],[\"s\",7,2]]}",
    );
    let decoded = decoder.decode_update(update).expect("decodes");
    assert_eq!(
        decoded,
        parse_ordered_json(
            "{\"type\":\"state\",\"member\":\"state\",\"sequence\":2,\"ops\":[[\"s\",[\"revision\"],2]]}"
        )
    );
}

#[test]
fn state_decoder_registry_reports_duplicate_and_unknown_states() {
    let mut decoder = ChordServiceStateDecoder::default();
    let snapshot = parse_ordered_json(
        "{\"serviceId\":\"s\",\"mode\":\"singleton\",\"instances\":[{\"members\":[{\"name\":\"state\",\"kind\":\"state\",\"sequence\":0,\"ops\":[[\"r\",0]]},{\"name\":\"state\",\"kind\":\"state\",\"sequence\":0,\"ops\":[[\"r\",1]]}]}]}",
    );
    assert_eq!(
        decoder
            .decode_snapshot(snapshot)
            .expect_err("duplicate")
            .message(),
        "Duplicate service state state"
    );

    let unknown =
        parse_ordered_json("{\"type\":\"state\",\"member\":\"missing\",\"sequence\":1,\"ops\":[]}");
    assert_eq!(
        decoder
            .decode_update(unknown)
            .expect_err("unknown")
            .message(),
        "Unknown service state missing"
    );

    // Addressed states key on the instance address.
    let addressed_unknown = parse_ordered_json(
        "{\"type\":\"state\",\"instance\":{\"key\":\"k\",\"generation\":1},\"member\":\"state\",\"sequence\":1,\"ops\":[]}",
    );
    assert_eq!(
        decoder
            .decode_update(addressed_unknown)
            .expect_err("addressed")
            .message(),
        "Unknown service state k@1.state"
    );

    // `closed` removes the addressed codecs; a later state update re-fails.
    let mut decoder = ChordServiceStateDecoder::default();
    let base = parse_ordered_json(
        "{\"serviceId\":\"s\",\"mode\":\"singleton\",\"instances\":[{\"instance\":{\"key\":\"k\",\"generation\":1},\"members\":[{\"name\":\"state\",\"kind\":\"state\",\"sequence\":0,\"ops\":[[\"r\",0]]}]}]}",
    );
    decoder.decode_snapshot(base).expect("base");
    let closed =
        parse_ordered_json("{\"type\":\"closed\",\"instance\":{\"key\":\"k\",\"generation\":1}}");
    decoder.decode_update(closed).expect("closed");
    let stale = parse_ordered_json(
        "{\"type\":\"state\",\"instance\":{\"key\":\"k\",\"generation\":1},\"member\":\"state\",\"sequence\":1,\"ops\":[]}",
    );
    assert_eq!(
        decoder.decode_update(stale).expect_err("removed").message(),
        "Unknown service state k@1.state"
    );
}

#[test]
fn state_decoder_handles_replaced_spawned_unavailable_updates() {
    let mut decoder = ChordServiceStateDecoder::default();
    let base = parse_ordered_json(
        "{\"serviceId\":\"s\",\"mode\":\"singleton\",\"instances\":[{\"members\":[{\"name\":\"state\",\"kind\":\"state\",\"sequence\":0,\"ops\":[[\"r\",0]]}]}]}",
    );
    decoder.decode_snapshot(base).expect("base");

    let replaced = parse_ordered_json(
        "{\"type\":\"replaced\",\"snapshot\":{\"members\":[{\"name\":\"state\",\"kind\":\"state\",\"sequence\":0,\"ops\":[[\"r\",9]]}]}}",
    );
    let decoded = decoder.decode_update(replaced).expect("replaced");
    assert_eq!(
        decoded,
        parse_ordered_json(
            "{\"type\":\"replaced\",\"snapshot\":{\"members\":[{\"name\":\"state\",\"kind\":\"state\",\"sequence\":0,\"ops\":[[\"r\",9]]}]}}"
        )
    );

    let spawned = parse_ordered_json(
        "{\"type\":\"spawned\",\"instance\":{\"members\":[{\"name\":\"count\",\"kind\":\"state\",\"sequence\":0,\"ops\":[[\"r\",[]]]}]}}",
    );
    let decoded = decoder.decode_update(spawned).expect("spawned");
    assert_eq!(
        decoded,
        parse_ordered_json(
            "{\"type\":\"spawned\",\"instance\":{\"members\":[{\"name\":\"count\",\"kind\":\"state\",\"sequence\":0,\"ops\":[[\"r\",[]]]}]}}"
        )
    );

    let unavailable = parse_ordered_json("{\"type\":\"unavailable\"}");
    assert_eq!(
        decoder.decode_update(unavailable).expect("unavailable"),
        parse_ordered_json("{\"type\":\"unavailable\"}")
    );
}

#[test]
fn parse_wire_snapshot_rejects_malformed_payloads_with_upstream_texts() {
    let invalid = parse_ordered_json("{\"nope\":true}");
    assert_eq!(
        parse_wire_service_subscription_snapshot(&invalid)
            .expect_err("keys")
            .message(),
        "Invalid service subscription snapshot"
    );

    let bad_instance = parse_ordered_json(
        "{\"serviceId\":\"s\",\"mode\":\"singleton\",\"instances\":[{\"members\":\"nope\"}]}",
    );
    assert_eq!(
        parse_wire_service_subscription_snapshot(&bad_instance)
            .expect_err("members")
            .message(),
        "Invalid service instance snapshot"
    );

    let bad_member = parse_ordered_json(
        "{\"serviceId\":\"s\",\"mode\":\"singleton\",\"instances\":[{\"members\":[{\"name\":\"x\",\"kind\":\"other\"}]}]}",
    );
    assert_eq!(
        parse_wire_service_subscription_snapshot(&bad_member)
            .expect_err("kind")
            .message(),
        "Invalid service member snapshot"
    );

    let bad_update =
        parse_ordered_json("{\"type\":\"state\",\"member\":\"\",\"sequence\":0,\"ops\":[]}");
    assert_eq!(
        parse_wire_service_provider_update(&bad_update)
            .expect_err("state")
            .message(),
        "Invalid service state update"
    );

    let unknown_type = parse_ordered_json("{\"type\":\"wat\"}");
    assert_eq!(
        parse_wire_service_provider_update(&unknown_type)
            .expect_err("type")
            .message(),
        "Invalid service provider update"
    );
}

#[test]
fn default_factory_produces_independent_decoders() {
    let factory = crate::client::service::default_service_state_decoder_factory();
    let first = factory();
    let second = factory();
    let _ = (first, second);
    // The trait object boundary stays usable from the client side.
    let mut any_decoder: Box<dyn ServiceStateDecoder + Send> = factory();
    let snapshot = parse_ordered_json("{\"serviceId\":\"s\",\"mode\":\"keyed\",\"instances\":[]}");
    assert_eq!(
        any_decoder.decode_snapshot(snapshot).expect("decodes"),
        parse_ordered_json("{\"serviceId\":\"s\",\"mode\":\"keyed\",\"instances\":[]}")
    );
}

#[test]
fn seam_error_is_display_and_error() {
    let error = SeamError::new("Invalid service call");
    assert_eq!(error.to_string(), "Invalid service call");
    let _: &dyn std::error::Error = &error;
}

#[test]
fn remote_transport_listener_types_are_shareable() {
    // Compile-level witness: the seam listener type is an Arc'd shared
    // closure over (update, context).
    let calls: Arc<std::sync::Mutex<Vec<JsonValue>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let listener: crate::client::service::RemoteServiceListener = {
        let calls = calls.clone();
        Arc::new(move |update, _context| {
            let calls = calls.clone();
            Box::pin(async move {
                calls.lock().unwrap().push(update);
            })
        })
    };
    let witness: crate::client::service::RemoteServiceListener = listener;
    let _ = witness;
}
