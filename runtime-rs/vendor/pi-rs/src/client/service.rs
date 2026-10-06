//! SEAM S1 — the faces of the un-ported `@earendil-works/chord` package that
//! `packages/client` imports. The full chord port (services, provider,
//! consumer, remote bindings) remains its own M6 slice; until it lands, this
//! module carries exactly the surface `client.ts` / `connection.ts` touch,
//! ported behavior-for-behavior from the cited upstream sources:
//!
//! | Upstream (chord) | Port |
//! |---|---|
//! | `services/wire.ts` `createServiceCatalogueCall` / `createServiceSubscribeCall` / `createServiceUnsubscribeCall` | [`create_service_catalogue_call`], [`create_service_subscribe_call`], [`create_service_unsubscribe_call`] |
//! | `services/wire.ts` `parseServiceCall` / `parseServiceCatalogue` | [`parse_service_call`], [`parse_service_catalogue`] |
//! | `services/wire.ts` `parseWireServiceSubscriptionSnapshot` / `parseWireServiceProviderUpdate` | [`parse_wire_service_subscription_snapshot`], [`parse_wire_service_provider_update`] |
//! | `delta/index.ts` `assertValidWireOp` + `decoder()` (wire grammar) | [`assert_valid_wire_op`], [`WireOpDecoder`] |
//! | `services/state-codec.ts` `createServiceStateDecoder` | [`ChordServiceStateDecoder`] behind the [`ServiceStateDecoder`] trait |
//! | `index.ts` `RemoteServiceTransport` (structural type) | [`RemoteServiceTransport`] |
//!
//! Two properties make this a seam rather than a re-export:
//!
//! 1. **Trait boundary.** The client takes its [`ServiceStateDecoder`] from a
//!    factory (`ClientOptions::service_state_decoder_factory`, upstream:
//!    the hard import of `createServiceStateDecoder`). The default factory
//!    returns [`ChordServiceStateDecoder`], a faithful port; the future
//!    chord slice can swap the internals without touching the client.
//! 2. **Opaque payloads.** Upstream hands the client opaque strict-JSON
//!    values (`ServiceCall`, snapshots, updates). The port keeps them as the
//!    protocol package's insertion-ordered [`JsonValue`], so delivered
//!    snapshots/updates preserve received object key order exactly (JS
//!    insertion order), pinned against the node oracle.
//!
//! Errors are [`SeamError`] — the upstream `TypeError`/`Error` message text,
//! which `client.ts` embeds verbatim into `ProtocolValidationError`s. All
//! message texts are pinned against `tests/fixtures/client_oracle/`.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use crate::agent_core::chord_support::Context;
use crate::protocol::json::{JsonValue, Number};

use super::errors::ClientError;

/// The upstream chord error text (a `TypeError`/`Error` `message`) that the
/// client embeds into its protocol failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeamError(pub String);

impl SeamError {
    pub fn new(message: impl Into<String>) -> SeamError {
        SeamError(message.into())
    }

    pub fn message(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SeamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SeamError {}

/// Upstream `ServiceMode` (`types.ts`; `wire.ts:228-230`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceMode {
    Singleton,
    Keyed,
}

impl ServiceMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ServiceMode::Singleton => "singleton",
            ServiceMode::Keyed => "keyed",
        }
    }

    /// Upstream `isMode` (`wire.ts:228-230`).
    pub fn parse(value: &JsonValue) -> Option<ServiceMode> {
        match value.as_str()? {
            "singleton" => Some(ServiceMode::Singleton),
            "keyed" => Some(ServiceMode::Keyed),
            _ => None,
        }
    }
}

/// `wire.ts:39-42`.
const SERVICE_CONTROL_ID: &str = "$chord.service";
const SERVICE_CATALOGUE_MEMBER: &str = "catalogue";
const SERVICE_SUBSCRIBE_MEMBER: &str = "subscribe";
const SERVICE_UNSUBSCRIBE_MEMBER: &str = "unsubscribe";

/// Builds a call object in upstream construction order
/// (`{serviceId, member, args}`).
pub fn service_call(service_id: &str, member: &str, args: Vec<JsonValue>) -> JsonValue {
    JsonValue::object(vec![
        ("serviceId".to_string(), JsonValue::string(service_id)),
        ("member".to_string(), JsonValue::string(member)),
        ("args".to_string(), JsonValue::Array(args)),
    ])
}

/// `wire.ts:54-56`.
pub fn create_service_catalogue_call() -> JsonValue {
    service_call(SERVICE_CONTROL_ID, SERVICE_CATALOGUE_MEMBER, vec![])
}

/// `wire.ts:58-60`.
pub fn create_service_subscribe_call(
    subscription_id: &str,
    service_id: &str,
    mode: ServiceMode,
) -> JsonValue {
    service_call(
        SERVICE_CONTROL_ID,
        SERVICE_SUBSCRIBE_MEMBER,
        vec![
            JsonValue::string(subscription_id),
            JsonValue::string(service_id),
            JsonValue::string(mode.as_str()),
        ],
    )
}

/// `wire.ts:62-64`.
pub fn create_service_unsubscribe_call(subscription_id: &str) -> JsonValue {
    service_call(
        SERVICE_CONTROL_ID,
        SERVICE_UNSUBSCRIBE_MEMBER,
        vec![JsonValue::string(subscription_id)],
    )
}

// ---------------------------------------------------------------------------
// wire.ts validation faces
// ---------------------------------------------------------------------------

fn invalid(description: &str) -> SeamError {
    SeamError(format!("Invalid {description}"))
}

fn is_id(value: &JsonValue) -> bool {
    value.as_str().is_some_and(|value| !value.is_empty())
}

/// `wire.ts:205-210`.
fn record<'a>(
    value: &'a JsonValue,
    description: &str,
) -> Result<&'a [(String, JsonValue)], SeamError> {
    value.as_object().ok_or_else(|| invalid(description))
}

/// `wire.ts:212-222`: every required key present, no key outside the allowed
/// set.
fn assert_keys(
    value: &[(String, JsonValue)],
    required: &[&str],
    optional: &[&str],
    description: &str,
) -> Result<(), SeamError> {
    let missing = required
        .iter()
        .any(|key| !value.iter().any(|(entry, _)| entry == key));
    if missing
        || value
            .iter()
            .any(|(key, _)| !required.contains(&key.as_str()) && !optional.contains(&key.as_str()))
    {
        return Err(invalid(description));
    }
    Ok(())
}

fn optional_integer(value: &JsonValue, minimum: i128) -> bool {
    value.as_number().is_some_and(|number| {
        number
            .as_integer()
            .is_some_and(|integer| integer >= minimum)
    })
}

/// `wire.ts:197-203`.
fn assert_address(value: &JsonValue) -> Result<(), SeamError> {
    let entries = record(value, "service instance address")?;
    assert_keys(
        entries,
        &["key", "generation"],
        &[],
        "service instance address",
    )?;
    let key = value.get("key").unwrap_or(&JsonValue::Null);
    let generation = value.get("generation").unwrap_or(&JsonValue::Null);
    if !is_id(key) || !optional_integer(generation, 1) {
        return Err(invalid("service instance address"));
    }
    Ok(())
}

/// The optional `instance` address of a call/instance/update object.
fn address_of(value: &JsonValue) -> Result<Option<(&str, i128)>, SeamError> {
    match value.get("instance") {
        Some(JsonValue::Null) | None => Ok(None),
        Some(address) => {
            assert_address(address)?;
            let key = address
                .get("key")
                .and_then(JsonValue::as_str)
                .unwrap_or_default();
            let generation = address
                .get("generation")
                .and_then(|value| value.as_number())
                .and_then(|value| value.as_integer())
                .unwrap_or_default();
            Ok(Some((key, generation)))
        }
    }
}

/// Port of `parseServiceCall` (`wire.ts:89-97`). Returns the validated call
/// (upstream returns the same object) for encoding.
pub fn parse_service_call(value: &JsonValue) -> Result<JsonValue, SeamError> {
    let entries = record(value, "service call")?;
    assert_keys(
        entries,
        &["serviceId", "member", "args"],
        &["instance"],
        "service call",
    )?;
    let service_id = value.get("serviceId").unwrap_or(&JsonValue::Null);
    let member = value.get("member").unwrap_or(&JsonValue::Null);
    let args = value.get("args").unwrap_or(&JsonValue::Null);
    if !is_id(service_id) || !is_id(member) || !matches!(args, JsonValue::Array(_)) {
        return Err(invalid("service call"));
    }
    if let Some(instance) = value.get("instance") {
        assert_address(instance)?;
    }
    Ok(value.clone())
}

/// Port of `parseServiceCatalogue` (`wire.ts:99-111`).
pub fn parse_service_catalogue(value: &JsonValue) -> Result<Vec<JsonValue>, SeamError> {
    let entries = value
        .as_array()
        .ok_or_else(|| invalid("service catalogue"))?;
    let mut seen: Vec<String> = Vec::new();
    for entry in entries {
        let fields = record(entry, "service catalogue entry")?;
        assert_keys(
            fields,
            &["serviceId", "mode"],
            &[],
            "service catalogue entry",
        )?;
        let service_id = entry.get("serviceId").unwrap_or(&JsonValue::Null);
        let mode = entry.get("mode").unwrap_or(&JsonValue::Null);
        if !is_id(service_id)
            || ServiceMode::parse(mode).is_none()
            || seen.contains(&service_id.as_str().unwrap_or_default().to_string())
        {
            return Err(invalid("service catalogue"));
        }
        seen.push(service_id.as_str().unwrap_or_default().to_string());
    }
    Ok(entries.to_vec())
}

/// `wire.ts:173-195`.
fn assert_instance(value: &JsonValue) -> Result<(), SeamError> {
    let entries = record(value, "service instance snapshot")?;
    assert_keys(
        entries,
        &["members"],
        &["instance"],
        "service instance snapshot",
    )?;
    address_of(value)?;
    let members = value.get("members").unwrap_or(&JsonValue::Null);
    let JsonValue::Array(members) = members else {
        return Err(invalid("service instance snapshot"));
    };
    for member in members {
        let fields = record(member, "service member snapshot")?;
        let kind = member
            .get("kind")
            .and_then(JsonValue::as_str)
            .unwrap_or_default();
        if kind == "method" {
            assert_keys(fields, &["name", "kind"], &[], "service method snapshot")?;
            if !is_id(member.get("name").unwrap_or(&JsonValue::Null)) {
                return Err(invalid("service method snapshot"));
            }
            continue;
        }
        if kind == "state" {
            assert_keys(
                fields,
                &["name", "kind", "sequence", "ops"],
                &[],
                "service state snapshot",
            )?;
            let sequence = member.get("sequence").unwrap_or(&JsonValue::Null);
            if !is_id(member.get("name").unwrap_or(&JsonValue::Null))
                || !optional_integer(sequence, 0)
                || !matches!(member.get("ops"), Some(JsonValue::Array(_)))
            {
                return Err(invalid("service state snapshot"));
            }
            for op in member
                .get("ops")
                .and_then(JsonValue::as_array)
                .unwrap_or(&[])
            {
                assert_valid_wire_op(op)?;
            }
            continue;
        }
        return Err(invalid("service member snapshot"));
    }
    Ok(())
}

/// `wire.ts:133-140`.
pub fn parse_wire_service_subscription_snapshot(value: &JsonValue) -> Result<JsonValue, SeamError> {
    let entries = record(value, "service subscription snapshot")?;
    assert_keys(
        entries,
        &["serviceId", "mode", "instances"],
        &[],
        "service subscription snapshot",
    )?;
    let service_id = value.get("serviceId").unwrap_or(&JsonValue::Null);
    let mode = value.get("mode").unwrap_or(&JsonValue::Null);
    if !is_id(service_id)
        || ServiceMode::parse(mode).is_none()
        || !matches!(value.get("instances"), Some(JsonValue::Array(_)))
    {
        return Err(invalid("service subscription snapshot"));
    }
    for instance in value
        .get("instances")
        .and_then(JsonValue::as_array)
        .unwrap_or(&[])
    {
        assert_instance(instance)?;
    }
    Ok(value.clone())
}

/// `wire.ts:142-171`.
pub fn parse_wire_service_provider_update(value: &JsonValue) -> Result<JsonValue, SeamError> {
    let entries = record(value, "service provider update")?;
    let update_type = value
        .get("type")
        .and_then(JsonValue::as_str)
        .unwrap_or_default();
    match update_type {
        "state" => {
            assert_keys(
                entries,
                &["type", "member", "sequence", "ops"],
                &["instance"],
                "state update",
            )?;
            let member = value.get("member").unwrap_or(&JsonValue::Null);
            let sequence = value.get("sequence").unwrap_or(&JsonValue::Null);
            if !is_id(member)
                || !optional_integer(sequence, 1)
                || !matches!(value.get("ops"), Some(JsonValue::Array(_)))
            {
                return Err(invalid("service state update"));
            }
            address_of(value)?;
            for op in value
                .get("ops")
                .and_then(JsonValue::as_array)
                .unwrap_or(&[])
            {
                assert_valid_wire_op(op)?;
            }
        }
        "unavailable" => {
            assert_keys(entries, &["type"], &[], "unavailable update")?;
        }
        "replaced" => {
            assert_keys(entries, &["type", "snapshot"], &[], "replacement update")?;
            assert_instance(value.get("snapshot").unwrap_or(&JsonValue::Null))?;
        }
        "spawned" => {
            assert_keys(entries, &["type", "instance"], &[], "spawn update")?;
            assert_instance(value.get("instance").unwrap_or(&JsonValue::Null))?;
        }
        "closed" => {
            assert_keys(entries, &["type", "instance"], &[], "close update")?;
            assert_address(value.get("instance").unwrap_or(&JsonValue::Null))?;
        }
        _ => return Err(invalid("service provider update")),
    }
    Ok(value.clone())
}

// ---------------------------------------------------------------------------
// delta wire grammar (delta/index.ts:1258-1320, 1352-1359, 1194-1206)
// ---------------------------------------------------------------------------

/// `delta/index.ts:1194`: reserved object keys that may not appear as path
/// segments.
const RESERVED_SEGMENTS: [&str; 3] = ["__proto__", "constructor", "prototype"];

fn js_number_display(number: Number) -> String {
    match number {
        Number::Uint(value) => value.to_string(),
        Number::Int(value) => value.to_string(),
        Number::Float(value) => {
            if value.is_finite() {
                format!("{value}")
            } else {
                "null".to_string()
            }
        }
    }
}

/// JS `String(value)` for the messages that embed a raw JSON scalar.
fn js_display(value: &JsonValue) -> String {
    match value {
        JsonValue::Null => "null".to_string(),
        JsonValue::Bool(value) => value.to_string(),
        JsonValue::Number(number) => js_number_display(*number),
        JsonValue::String(value) => value.clone(),
        JsonValue::Array(_) => String::new(),
        JsonValue::Object(_) => "[object Object]".to_string(),
    }
}

fn js_escape(string: &str) -> String {
    let mut out = String::with_capacity(string.len() + 2);
    for character in string.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            character if (character as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => out.push(character),
        }
    }
    out
}

/// `JSON.stringify` over the ordered [`JsonValue`] tree (used for the
/// `PathError` texts and the per-state codec keys).
pub fn js_json_stringify(value: &JsonValue) -> String {
    match value {
        JsonValue::Null => "null".to_string(),
        JsonValue::Bool(value) => value.to_string(),
        JsonValue::Number(number) => js_number_display(*number),
        JsonValue::String(value) => format!("\"{}\"", js_escape(value)),
        JsonValue::Array(items) => {
            let rendered: Vec<String> = items.iter().map(js_json_stringify).collect();
            format!("[{}]", rendered.join(","))
        }
        JsonValue::Object(entries) => {
            let rendered: Vec<String> = entries
                .iter()
                .map(|(key, value)| format!("\"{}\":{}", js_escape(key), js_json_stringify(value)))
                .collect();
            format!("{{{}}}", rendered.join(","))
        }
    }
}

/// `delta/index.ts:1321-1329` (`assertSafePath`): every segment is a
/// non-reserved string or a non-negative integer.
fn assert_safe_path(path: &[JsonValue]) -> Result<(), SeamError> {
    for segment in path {
        match segment {
            JsonValue::String(key) => {
                if RESERVED_SEGMENTS.contains(&key.as_str()) {
                    return Err(SeamError(format!("unsafe path segment: {key}")));
                }
            }
            JsonValue::Number(number) => {
                let integral = number.as_integer().is_some_and(|integer| integer >= 0);
                if !integral {
                    return Err(SeamError(format!(
                        "unsafe path segment: {}",
                        js_display(segment)
                    )));
                }
            }
            other => {
                return Err(SeamError(format!(
                    "unsafe path segment: {}",
                    js_display(other)
                )));
            }
        }
    }
    Ok(())
}

/// `delta/index.ts:1264-1274` (`okRef`): a path reference is a non-negative
/// integer id or an inline path array.
fn assert_path_ref(reference: &JsonValue) -> Result<(), SeamError> {
    if matches!(reference, JsonValue::Number(_)) {
        let integral = reference
            .as_number()
            .and_then(|value| value.as_integer())
            .is_some_and(|integer| integer >= 0);
        if !integral {
            return Err(SeamError::new("bad path id"));
        }
        return Ok(());
    }
    match reference {
        JsonValue::Array(path) => assert_safe_path(path),
        _ => Err(SeamError::new("path is not an array")),
    }
}

fn tuple(value: &JsonValue) -> Result<&[JsonValue], SeamError> {
    match value.as_array() {
        // `delta/index.ts:1259`: an empty tuple is rejected like a non-tuple.
        Some(items) if !items.is_empty() => Ok(items),
        _ => Err(SeamError::new("op is not a tuple")),
    }
}

/// Port of `assertValidWireOp` (`delta/index.ts:1258-1320`): ids, short
/// forms, and inline paths are all legal in the wire grammar.
pub fn assert_valid_wire_op(op: &JsonValue) -> Result<(), SeamError> {
    let items = tuple(op)?;
    let verb = items.first().cloned().unwrap_or(JsonValue::Null);
    let verb = js_display(&verb);
    let arity = items.len();
    match verb.as_str() {
        "r" => {
            if arity != 2 {
                return Err(SeamError::new("r arity"));
            }
        }
        "s" => {
            if arity == 3 {
                assert_path_ref(&items[1])?;
            } else if arity != 2 {
                return Err(SeamError::new("s arity"));
            }
        }
        "d" => {
            if arity == 2 {
                assert_path_ref(&items[1])?;
            } else if arity != 1 {
                return Err(SeamError::new("d arity"));
            }
        }
        "a" => {
            if arity == 3 {
                assert_path_ref(&items[1])?;
                if !matches!(items[2], JsonValue::String(_)) {
                    return Err(SeamError::new("a value"));
                }
            } else if arity == 2 {
                if !matches!(items[1], JsonValue::String(_)) {
                    return Err(SeamError::new("a value"));
                }
            } else {
                return Err(SeamError::new("a arity"));
            }
        }
        "t" => {
            if arity == 3 {
                assert_path_ref(&items[1])?;
                if !optional_integer(&items[2], 0) {
                    return Err(SeamError::new("t count"));
                }
            } else if arity == 2 {
                if !optional_integer(&items[1], 0) {
                    return Err(SeamError::new("t count"));
                }
            } else {
                return Err(SeamError::new("t arity"));
            }
        }
        "p" => {
            if arity != 5 && arity != 4 {
                return Err(SeamError::new("p arity"));
            }
            if arity == 5 {
                assert_path_ref(&items[1])?;
                if !optional_integer(&items[2], 0) {
                    return Err(SeamError::new("p index"));
                }
                if !optional_integer(&items[3], 0) {
                    return Err(SeamError::new("p remove"));
                }
                if !matches!(items[4], JsonValue::Array(_)) {
                    return Err(SeamError::new("p items"));
                }
            } else {
                if !optional_integer(&items[1], 0) {
                    return Err(SeamError::new("p index"));
                }
                if !optional_integer(&items[2], 0) {
                    return Err(SeamError::new("p remove"));
                }
                if !matches!(items[3], JsonValue::Array(_)) {
                    return Err(SeamError::new("p items"));
                }
            }
        }
        "#" => {
            if arity != 3
                || !optional_integer(&items[1], 0)
                || !matches!(items[2], JsonValue::Array(_))
            {
                return Err(SeamError::new("# shape"));
            }
            assert_safe_path(items[2].as_array().unwrap())?;
        }
        other => {
            return Err(SeamError(format!("unknown op verb: {other}")));
        }
    }
    Ok(())
}

/// One stateful wire-op decoder for a single replicated state stream (upstream
/// `delta/index.ts` `decoder()` interface). Decode failures carry the upstream
/// `PathError` / grammar messages.
pub trait WireOpDecoder: Send {
    fn decode(&mut self, wire: &[JsonValue]) -> Result<Vec<JsonValue>, SeamError>;
}

/// Port of `decoder()` (`delta/index.ts:1628-1695`): resolves interned path
/// ids (`["#", id, path]`) and arity-shortened ops into the plain op
/// vocabulary. Ids persist across batches; both ids and the previous-op short
/// form reset at a base (`r`) op.
#[derive(Default)]
pub struct WireGrammarDecoder {
    paths: HashMap<i128, Vec<JsonValue>>,
}

impl WireGrammarDecoder {
    fn unresolved(message: &JsonValue) -> SeamError {
        SeamError(format!("unresolvable path: {}", js_json_stringify(message)))
    }
}

impl WireOpDecoder for WireGrammarDecoder {
    fn decode(&mut self, wire: &[JsonValue]) -> Result<Vec<JsonValue>, SeamError> {
        let mut previous: Option<Vec<JsonValue>> = None;
        let mut out: Vec<JsonValue> = Vec::with_capacity(wire.len());
        for op in wire {
            assert_valid_wire_op(op)?;
            let items = op.as_array().unwrap();
            let verb = items[0].as_str().unwrap_or_default();
            if verb == "#" {
                let id = items[1]
                    .as_number()
                    .and_then(|value| value.as_integer())
                    .unwrap_or_default();
                self.paths.insert(id, items[2].as_array().unwrap().to_vec());
                continue;
            }
            if verb == "r" {
                out.push(op.clone());
                self.paths.clear();
                previous = None;
                continue;
            }
            let short = (verb == "d" && items.len() == 1)
                || ((verb != "d" && verb != "p") && items.len() == 2)
                || (verb == "p" && items.len() == 4);
            let path: Vec<JsonValue> = if short {
                previous
                    .clone()
                    .ok_or_else(|| SeamError::new("unresolvable path: []"))?
            } else if matches!(items[1], JsonValue::Number(_)) {
                let id = items[1]
                    .as_number()
                    .and_then(|value| value.as_integer())
                    .unwrap_or_default();
                self.paths
                    .get(&id)
                    .cloned()
                    .ok_or_else(|| Self::unresolved(&items[1]))?
            } else {
                items[1].as_array().unwrap().to_vec()
            };
            if !short {
                previous = Some(path.clone());
            }
            if verb != "p" && path.is_empty() {
                return Err(Self::unresolved(&JsonValue::Array(Vec::new())));
            }
            let path = JsonValue::Array(path);
            let decoded = match verb {
                "s" => vec![
                    items[0].clone(),
                    path,
                    if short {
                        items[1].clone()
                    } else {
                        items[2].clone()
                    },
                ],
                "d" => vec![items[0].clone(), path],
                "a" | "t" => vec![
                    items[0].clone(),
                    path,
                    if short {
                        items[1].clone()
                    } else {
                        items[2].clone()
                    },
                ],
                "p" => {
                    if short {
                        vec![
                            items[0].clone(),
                            path,
                            items[1].clone(),
                            items[2].clone(),
                            items[3].clone(),
                        ]
                    } else {
                        vec![
                            items[0].clone(),
                            path,
                            items[2].clone(),
                            items[3].clone(),
                            items[4].clone(),
                        ]
                    }
                }
                _ => unreachable!("assert_valid_wire_op accepted an unknown verb"),
            };
            out.push(JsonValue::Array(decoded));
        }
        Ok(out)
    }
}

/// A fresh `decoder()` (one pair per independent state stream).
pub fn wire_op_decoder() -> Box<dyn WireOpDecoder + Send> {
    Box::new(WireGrammarDecoder::default())
}

// ---------------------------------------------------------------------------
// services/state-codec.ts
// ---------------------------------------------------------------------------

/// Upstream `ServiceStateDecoder` (`state-codec.ts:17-20`): stateful decoders
/// for every replicated state in one subscription. SEAM S1: the client takes
/// this from a factory so the chord slice can replace the default.
pub trait ServiceStateDecoder: Send {
    fn decode_snapshot(&mut self, snapshot: JsonValue) -> Result<JsonValue, SeamError>;
    fn decode_update(&mut self, update: JsonValue) -> Result<JsonValue, SeamError>;
}

/// `state-codec.ts:148-150`.
fn state_key(instance: Option<(&str, i128)>, member: &str) -> String {
    let key = match instance {
        Some((key, _)) => JsonValue::string(key),
        None => JsonValue::Null,
    };
    let generation = match instance {
        Some((_, generation)) => JsonValue::Number(match i64::try_from(generation) {
            Ok(value) => Number::Int(value),
            Err(_) => Number::Float(generation as f64),
        }),
        None => JsonValue::Null,
    };
    js_json_stringify(&JsonValue::Array(vec![
        key,
        generation,
        JsonValue::string(member),
    ]))
}

/// `state-codec.ts:156-158`.
fn describe_state(instance: Option<(&str, i128)>, member: &str) -> String {
    match instance {
        None => member.to_string(),
        Some((key, generation)) => format!("{key}@{generation}.{member}"),
    }
}

/// Port of `createServiceStateDecoder` (`state-codec.ts:90-118`) with the
/// `StateCodecRegistry` inlined (`state-codec.ts:27-58`). The registry is a
/// vector (upstream iterates it for `removeInstance`), keyed by
/// [`state_key`] like upstream's map.
#[derive(Default)]
pub struct ChordServiceStateDecoder {
    codecs: Vec<CodecEntry>,
}

struct CodecEntry {
    instance: Option<(String, i128)>,
    key: String,
    codec: Box<dyn WireOpDecoder + Send>,
}

impl ChordServiceStateDecoder {
    /// `StateCodecRegistry#add` — duplicates fail with the upstream text.
    /// Returns the registry index of the (fresh) codec; handing out
    /// `&mut dyn` from a `Box<dyn + 'static>` field runs into invariance, so
    /// callers use the index instead.
    fn add(&mut self, instance: Option<(&str, i128)>, member: &str) -> Result<usize, SeamError> {
        let key = state_key(instance, member);
        if self.codecs.iter().any(|entry| entry.key == key) {
            return Err(SeamError(format!(
                "Duplicate service state {}",
                describe_state(instance, member)
            )));
        }
        self.codecs.push(CodecEntry {
            instance: instance.map(|(key, generation)| (key.to_string(), generation)),
            key: key.clone(),
            codec: wire_op_decoder(),
        });
        Ok(self.codecs.len() - 1)
    }

    /// `StateCodecRegistry#get` — unknown states fail with the upstream text.
    fn get(&mut self, instance: Option<(&str, i128)>, member: &str) -> Result<usize, SeamError> {
        let key = state_key(instance, member);
        self.codecs
            .iter()
            .position(|entry| entry.key == key)
            .ok_or_else(|| {
                SeamError(format!(
                    "Unknown service state {}",
                    describe_state(instance, member)
                ))
            })
    }

    /// `StateCodecRegistry#removeInstance` (`state-codec.ts:53-57`): drop
    /// every codec bound to the closed instance address.
    fn remove_instance(&mut self, instance: (&str, i128)) {
        self.codecs
            .retain(|entry| !same_address(entry.instance.as_ref(), &instance));
    }

    /// `decodeInstance` (`state-codec.ts:134-146`): rebuild the instance with
    /// every state member's ops decoded, preserving received key order.
    fn decode_instance(&mut self, instance: &JsonValue) -> Result<JsonValue, SeamError> {
        let address = address_of(instance)?;
        let members = instance
            .get("members")
            .and_then(JsonValue::as_array)
            .ok_or_else(|| invalid("service instance snapshot"))?;
        let mut decoded_members = Vec::with_capacity(members.len());
        for member in members {
            let mut decoded = member.clone();
            if member.get("kind").and_then(JsonValue::as_str) == Some("state") {
                let name = member
                    .get("name")
                    .and_then(JsonValue::as_str)
                    .unwrap_or_default();
                let ops = member
                    .get("ops")
                    .and_then(JsonValue::as_array)
                    .ok_or_else(|| invalid("service state snapshot"))?;
                let index = self.add(address, name)?;
                let decoded_ops = self.codecs[index].codec.decode(ops)?;
                decoded = replace_key(decoded, "ops", JsonValue::Array(decoded_ops));
            }
            decoded_members.push(decoded);
        }
        Ok(replace_key(
            instance.clone(),
            "members",
            JsonValue::Array(decoded_members),
        ))
    }
}

/// `state-codec.ts:152-154`.
fn same_address(left: Option<&(String, i128)>, right: &(&str, i128)) -> bool {
    match left {
        Some((key, generation)) => key.as_str() == right.0 && *generation == right.1,
        None => false,
    }
}

/// Returns `object` with `value` stored under `key`, preserving key order
/// (upstream object spread + property overwrite).
fn replace_key(object: JsonValue, key: &str, value: JsonValue) -> JsonValue {
    let JsonValue::Object(mut entries) = object else {
        return object;
    };
    if let Some(slot) = entries.iter_mut().find(|(entry_key, _)| entry_key == key) {
        slot.1 = value;
    } else {
        entries.push((key.to_string(), value));
    }
    JsonValue::Object(entries)
}

impl ServiceStateDecoder for ChordServiceStateDecoder {
    fn decode_snapshot(&mut self, snapshot: JsonValue) -> Result<JsonValue, SeamError> {
        self.codecs.clear();
        let instances = snapshot
            .get("instances")
            .and_then(JsonValue::as_array)
            .ok_or_else(|| invalid("service subscription snapshot"))?;
        let mut decoded_instances = Vec::with_capacity(instances.len());
        for instance in instances {
            decoded_instances.push(self.decode_instance(instance)?);
        }
        Ok(replace_key(
            snapshot,
            "instances",
            JsonValue::Array(decoded_instances),
        ))
    }

    fn decode_update(&mut self, update: JsonValue) -> Result<JsonValue, SeamError> {
        let update_type = update
            .get("type")
            .and_then(JsonValue::as_str)
            .unwrap_or_default();
        match update_type {
            "state" => {
                let address = address_of(&update)?;
                let member = update
                    .get("member")
                    .and_then(JsonValue::as_str)
                    .unwrap_or_default();
                let ops = update
                    .get("ops")
                    .and_then(JsonValue::as_array)
                    .ok_or_else(|| invalid("service state update"))?;
                let index = self.get(address, member)?;
                let decoded = self.codecs[index].codec.decode(ops)?;
                Ok(replace_key(update, "ops", JsonValue::Array(decoded)))
            }
            "replaced" => {
                self.codecs.clear();
                let snapshot = update.get("snapshot").cloned().unwrap_or(JsonValue::Null);
                let decoded = self.decode_instance(&snapshot)?;
                Ok(replace_key(update, "snapshot", decoded))
            }
            "spawned" => {
                let instance = update.get("instance").cloned().unwrap_or(JsonValue::Null);
                let decoded = self.decode_instance(&instance)?;
                Ok(replace_key(update, "instance", decoded))
            }
            "unavailable" => {
                self.codecs.clear();
                Ok(update)
            }
            "closed" => {
                if let Some((key, generation)) = address_of(&update)? {
                    self.remove_instance((key, generation));
                }
                Ok(update)
            }
            _ => Err(invalid("service provider update")),
        }
    }
}

/// `state-codec.ts:90`. The default [`ServiceStateDecoderFactory`] target.
pub fn create_service_state_decoder() -> Box<dyn ServiceStateDecoder + Send> {
    Box::new(ChordServiceStateDecoder::default())
}

/// The client's decoder seam: upstream `createServiceStateDecoder` hard
/// import (`client.ts:3-18`), expressed as a factory closure so the chord
/// slice can substitute its own implementation.
pub type ServiceStateDecoderFactory =
    Arc<dyn Fn() -> Box<dyn ServiceStateDecoder + Send> + Send + Sync>;

pub fn default_service_state_decoder_factory() -> ServiceStateDecoderFactory {
    Arc::new(create_service_state_decoder)
}

// ---------------------------------------------------------------------------
// RemoteServiceTransport seam (chord index.ts / api.ts structural face)
// ---------------------------------------------------------------------------

/// Upstream chord service listener: `(update, context) => void | Promise`
/// (`client.ts:448-474` wraps it with `BACKGROUND_CONTEXT`).
pub type RemoteServiceListener =
    Arc<dyn Fn(JsonValue, Context) -> futures::future::BoxFuture<'static, ()> + Send + Sync>;

/// Upstream chord subscribe result: `{snapshot, activate, close}`. `close`
/// resolves with the subscription dispose outcome (upstream: the
/// `subscription.dispose()` promise).
#[derive(Clone)]
pub struct RemoteServiceSubscription {
    pub snapshot: JsonValue,
    pub activate: Arc<dyn Fn() + Send + Sync>,
    pub close:
        Arc<dyn Fn() -> futures::future::BoxFuture<'static, Result<(), ClientError>> + Send + Sync>,
}

/// Upstream `RemoteServiceTransport` (chord `services/consumer.ts` structural
/// face used by `createClientServiceTransport`).
pub trait RemoteServiceTransport: Send + Sync + 'static {
    fn invoke(
        &self,
        call: JsonValue,
        context: &Context,
    ) -> futures::future::BoxFuture<'static, Result<JsonValue, ClientError>>;
    fn subscribe(
        &self,
        service_id: &str,
        mode: ServiceMode,
        listener: RemoteServiceListener,
        context: &Context,
    ) -> futures::future::BoxFuture<'static, Result<RemoteServiceSubscription, ClientError>>;
}
