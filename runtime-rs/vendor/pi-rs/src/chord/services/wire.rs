//! Service wire protocol: control calls, parsing and validation of untrusted
//! service values. Port of `packages/chord/src/services/wire.ts` (upstream
//! sha256 `84ec6e3362ae3c5239b27b63be8e8377e1cf45c97088bb8d628c5e55458614b8`).
//!
//! Parsing operates on untyped [`JsonValue`] inputs, exactly like upstream:
//! callers hand over `unknown` values decoded from a transport, and these
//! functions validate shape strictly — required keys present, no unknown
//! keys, well-formed ids/addresses/modes — before handing back typed values.
//! Ops are validated with the delta vocabulary validators
//! ([`op_from_json`]/[`wire_op_from_json`]).

use serde_json::json;

use crate::chord::delta::{op_from_json, wire_op_from_json, DeltaError, Op, WireOp};
use crate::chord::types::{
    JsonValue, ServiceCall, ServiceCatalogueEntry, ServiceInstanceAddress, ServiceInstanceSnapshot,
    ServiceMemberSnapshot, ServiceMode, ServiceProviderUpdate, ServiceSubscriptionSnapshot,
    WireServiceInstanceSnapshot, WireServiceMemberSnapshot, WireServiceProviderUpdate,
    WireServiceSubscriptionSnapshot,
};

use super::errors::ChordError;

const SERVICE_CONTROL_ID: &str = "$chord.service";
const SERVICE_CATALOGUE_MEMBER: &str = "catalogue";
const SERVICE_SUBSCRIBE_MEMBER: &str = "subscribe";
const SERVICE_UNSUBSCRIBE_MEMBER: &str = "unsubscribe";

fn invalid(description: &str) -> ChordError {
    ChordError::Type(format!("Invalid {description}"))
}

/// Upstream `ServiceControlCall` (`wire.ts:44-52`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServiceControlCall {
    Catalogue,
    Subscribe {
        subscription_id: String,
        service_id: String,
        mode: ServiceMode,
    },
    Unsubscribe {
        subscription_id: String,
    },
}

/// `createServiceCatalogueCall` (`wire.ts:54-56`).
pub fn create_service_catalogue_call() -> ServiceCall {
    ServiceCall {
        service_id: SERVICE_CONTROL_ID.to_owned(),
        instance: None,
        member: SERVICE_CATALOGUE_MEMBER.to_owned(),
        args: Vec::new(),
    }
}

/// `createServiceSubscribeCall` (`wire.ts:58-60`).
pub fn create_service_subscribe_call(
    subscription_id: &str,
    service_id: &str,
    mode: ServiceMode,
) -> ServiceCall {
    ServiceCall {
        service_id: SERVICE_CONTROL_ID.to_owned(),
        instance: None,
        member: SERVICE_SUBSCRIBE_MEMBER.to_owned(),
        args: vec![json!(subscription_id), json!(service_id), mode.to_json()],
    }
}

/// `createServiceUnsubscribeCall` (`wire.ts:62-64`).
pub fn create_service_unsubscribe_call(subscription_id: &str) -> ServiceCall {
    ServiceCall {
        service_id: SERVICE_CONTROL_ID.to_owned(),
        instance: None,
        member: SERVICE_UNSUBSCRIBE_MEMBER.to_owned(),
        args: vec![json!(subscription_id)],
    }
}

/// `decodeServiceControlCall` (`wire.ts:66-87`).
pub fn decode_service_control_call(call: &ServiceCall) -> Option<ServiceControlCall> {
    if call.service_id != SERVICE_CONTROL_ID || call.instance.is_some() {
        return None;
    }
    if call.member == SERVICE_CATALOGUE_MEMBER && call.args.is_empty() {
        return Some(ServiceControlCall::Catalogue);
    }
    if call.member == SERVICE_SUBSCRIBE_MEMBER
        && call.args.len() == 3
        && is_id(&call.args[0])
        && is_id(&call.args[1])
        && (call.args[2] == json!("singleton") || call.args[2] == json!("keyed"))
    {
        let mode = if call.args[2] == json!("keyed") {
            ServiceMode::Keyed
        } else {
            ServiceMode::Singleton
        };
        return Some(ServiceControlCall::Subscribe {
            subscription_id: call.args[0].as_str().expect("is_id").to_owned(),
            service_id: call.args[1].as_str().expect("is_id").to_owned(),
            mode,
        });
    }
    if call.member == SERVICE_UNSUBSCRIBE_MEMBER && call.args.len() == 1 && is_id(&call.args[0]) {
        return Some(ServiceControlCall::Unsubscribe {
            subscription_id: call.args[0].as_str().expect("is_id").to_owned(),
        });
    }
    None
}

/// `parseServiceCall` (`wire.ts:89-97`).
pub fn parse_service_call(value: &JsonValue) -> Result<ServiceCall, ChordError> {
    let object = record(value, "service call")?;
    assert_keys(
        object,
        &["serviceId", "member", "args"],
        &["instance"],
        "service call",
    )?;
    if !is_id(&value["serviceId"]) || !is_id(&value["member"]) || !value["args"].is_array() {
        return Err(invalid("service call"));
    }
    let instance = match value.get("instance") {
        Some(instance) if !instance.is_null() => Some(assert_address(instance)?),
        _ => None,
    };
    Ok(ServiceCall {
        service_id: value["serviceId"].as_str().expect("is_id").to_owned(),
        instance,
        member: value["member"].as_str().expect("is_id").to_owned(),
        args: value["args"].as_array().expect("checked above").clone(),
    })
}

/// `parseServiceCatalogue` (`wire.ts:99-111`).
pub fn parse_service_catalogue(
    value: &JsonValue,
) -> Result<Vec<ServiceCatalogueEntry>, ChordError> {
    let Some(entries) = value.as_array() else {
        return Err(invalid("service catalogue"));
    };
    let mut seen: Vec<&str> = Vec::new();
    for candidate in entries {
        let entry = record(candidate, "service catalogue entry")?;
        assert_keys(
            entry,
            &["serviceId", "mode"],
            &[],
            "service catalogue entry",
        )?;
        let service_id = entry["serviceId"].as_str().unwrap_or_default();
        let mode_ok = entry["mode"] == json!("singleton") || entry["mode"] == json!("keyed");
        if service_id.is_empty() || !mode_ok || seen.contains(&service_id) {
            return Err(invalid("service catalogue"));
        }
        seen.push(service_id);
    }
    Ok(entries
        .iter()
        .map(|entry| ServiceCatalogueEntry {
            service_id: entry["serviceId"]
                .as_str()
                .expect("checked above")
                .to_owned(),
            mode: if entry["mode"] == json!("keyed") {
                ServiceMode::Keyed
            } else {
                ServiceMode::Singleton
            },
        })
        .collect())
}

/// `parseServiceSubscriptionSnapshot` (`wire.ts:113-116`): validates decoded
/// ops.
pub fn parse_service_subscription_snapshot(
    value: &JsonValue,
) -> Result<ServiceSubscriptionSnapshot, ChordError> {
    assert_subscription_snapshot(value, |op: &serde_json::Value| {
        op_from_json(op).map(|_: Op| ())
    })?;
    Ok(serde_json::from_value(value.clone())
        .map(|raw: RawSubscriptionSnapshot| {
            raw.into_snapshot(|ops| {
                ops.iter()
                    .map(op_from_json)
                    .collect::<Result<Vec<Op>, DeltaError>>()
            })
        })
        .expect("validated above"))
}

/// `parseWireServiceSubscriptionSnapshot` (`wire.ts:118-121`): validates wire
/// ops.
pub fn parse_wire_service_subscription_snapshot(
    value: &JsonValue,
) -> Result<WireServiceSubscriptionSnapshot, ChordError> {
    assert_subscription_snapshot(value, |op: &serde_json::Value| {
        wire_op_from_json(op).map(|_: WireOp| ())
    })?;
    Ok(serde_json::from_value(value.clone())
        .map(|raw: RawSubscriptionSnapshot| {
            raw.into_wire_snapshot(|ops| {
                ops.iter()
                    .map(wire_op_from_json)
                    .collect::<Result<Vec<WireOp>, DeltaError>>()
            })
        })
        .expect("validated above"))
}

/// `parseServiceProviderUpdate` (`wire.ts:123-126`).
pub fn parse_service_provider_update(
    value: &JsonValue,
) -> Result<ServiceProviderUpdate, ChordError> {
    assert_provider_update(value, |op| op_from_json(op).map(|_: Op| ()))?;
    let raw: RawProviderUpdate =
        serde_json::from_value(value.clone()).map_err(|_| invalid("service provider update"))?;
    raw.into_update(|ops| {
        ops.iter()
            .map(op_from_json)
            .collect::<Result<Vec<Op>, DeltaError>>()
    })
}

/// `parseWireServiceProviderUpdate` (`wire.ts:128-131`).
pub fn parse_wire_service_provider_update(
    value: &JsonValue,
) -> Result<WireServiceProviderUpdate, ChordError> {
    assert_provider_update(value, |op| wire_op_from_json(op).map(|_: WireOp| ()))?;
    let raw: RawProviderUpdate =
        serde_json::from_value(value.clone()).map_err(|_| invalid("service provider update"))?;
    raw.into_wire_update(|ops| {
        ops.iter()
            .map(wire_op_from_json)
            .collect::<Result<Vec<WireOp>, DeltaError>>()
    })
}

fn assert_subscription_snapshot(
    value: &JsonValue,
    assert_op: impl Fn(&JsonValue) -> Result<(), DeltaError>,
) -> Result<(), ChordError> {
    let snapshot = record(value, "service subscription snapshot")?;
    assert_keys(
        snapshot,
        &["serviceId", "mode", "instances"],
        &[],
        "service subscription snapshot",
    )?;
    if !is_id(&snapshot["serviceId"])
        || !is_mode(&snapshot["mode"])
        || !snapshot["instances"].is_array()
    {
        return Err(invalid("service subscription snapshot"));
    }
    for instance in snapshot["instances"].as_array().expect("checked above") {
        assert_instance(instance, &assert_op)?;
    }
    Ok(())
}

fn assert_provider_update(
    value: &JsonValue,
    assert_op: impl Fn(&JsonValue) -> Result<(), DeltaError>,
) -> Result<(), ChordError> {
    let update = record(value, "service provider update")?;
    match value.get("type").and_then(JsonValue::as_str) {
        Some("state") => {
            assert_keys(
                update,
                &["type", "member", "sequence", "ops"],
                &["instance"],
                "state update",
            )?;
            if !is_id(&update["member"])
                || !is_integer(&update["sequence"], 1)
                || !update["ops"].is_array()
            {
                return Err(invalid("service state update"));
            }
            if update.get("instance").is_some_and(|i| !i.is_null()) {
                assert_address(&update["instance"])?;
            }
            for op in update["ops"].as_array().expect("checked above") {
                assert_op(op)?;
            }
            Ok(())
        }
        Some("reset") => {
            assert_keys(update, &["type", "snapshot"], &[], "reset update")?;
            assert_subscription_snapshot(&update["snapshot"], &assert_op)?;
            // Service reset must contain full root replacements
            // (`wire.ts:154-166`).
            let snapshot = update["snapshot"]["instances"]
                .as_array()
                .expect("validated above");
            for instance in snapshot {
                for member in instance["members"].as_array().expect("validated above") {
                    if member["kind"] == json!("state") {
                        let ops = member["ops"].as_array().expect("validated above");
                        let ok = ops.len() == 1
                            && ops[0].is_array()
                            && ops[0].get(0).and_then(JsonValue::as_str) == Some("r");
                        if !ok {
                            return Err(ChordError::Type(
                                "Service reset must contain full root replacements".to_owned(),
                            ));
                        }
                    }
                }
            }
            Ok(())
        }
        Some("unavailable") => {
            assert_keys(update, &["type"], &[], "unavailable update")?;
            Ok(())
        }
        Some("replaced") => {
            assert_keys(update, &["type", "snapshot"], &[], "replacement update")?;
            assert_instance(&update["snapshot"], &assert_op)?;
            Ok(())
        }
        Some("spawned") => {
            assert_keys(update, &["type", "instance"], &[], "spawn update")?;
            assert_instance(&update["instance"], &assert_op)?;
            Ok(())
        }
        Some("closed") => {
            assert_keys(update, &["type", "instance"], &[], "close update")?;
            assert_address(&update["instance"])?;
            Ok(())
        }
        _ => Err(invalid("service provider update")),
    }
}

fn assert_instance(
    value: &JsonValue,
    assert_op: &impl Fn(&JsonValue) -> Result<(), DeltaError>,
) -> Result<(), ChordError> {
    let instance = record(value, "service instance snapshot")?;
    assert_keys(
        instance,
        &["members"],
        &["instance"],
        "service instance snapshot",
    )?;
    if instance.get("instance").is_some_and(|i| !i.is_null()) {
        assert_address(&instance["instance"])?;
    }
    if !instance["members"].is_array() {
        return Err(invalid("service instance snapshot"));
    }
    for candidate in instance["members"].as_array().expect("checked above") {
        let member = record(candidate, "service member snapshot")?;
        if member["kind"] == json!("method") {
            assert_keys(member, &["name", "kind"], &[], "service method snapshot")?;
            if !is_id(&member["name"]) {
                return Err(invalid("service method snapshot"));
            }
            continue;
        }
        if member["kind"] == json!("state") {
            assert_keys(
                member,
                &["name", "kind", "sequence", "ops"],
                &[],
                "service state snapshot",
            )?;
            if !is_id(&member["name"])
                || !is_integer(&member["sequence"], 0)
                || !member["ops"].is_array()
            {
                return Err(invalid("service state snapshot"));
            }
            for op in member["ops"].as_array().expect("checked above") {
                assert_op(op)?;
            }
            continue;
        }
        return Err(invalid("service member snapshot"));
    }
    Ok(())
}

fn assert_address(value: &JsonValue) -> Result<ServiceInstanceAddress, ChordError> {
    let address = record(value, "service instance address")?;
    assert_keys(
        address,
        &["key", "generation"],
        &[],
        "service instance address",
    )?;
    if !is_id(&address["key"]) || !is_integer(&address["generation"], 1) {
        return Err(invalid("service instance address"));
    }
    Ok(ServiceInstanceAddress {
        key: address["key"].as_str().expect("is_id").to_owned(),
        generation: address["generation"].as_u64().expect("is_integer"),
    })
}

fn record<'a>(
    value: &'a JsonValue,
    description: &str,
) -> Result<&'a serde_json::Map<String, JsonValue>, ChordError> {
    if !value.is_object() {
        return Err(invalid(description));
    }
    Ok(value.as_object().expect("checked above"))
}

fn assert_keys(
    value: &serde_json::Map<String, JsonValue>,
    required: &[&str],
    optional: &[&str],
    description: &str,
) -> Result<(), ChordError> {
    for key in required {
        if !value.contains_key(*key) {
            return Err(invalid(description));
        }
    }
    for key in value.keys() {
        if !required.contains(&key.as_str()) && !optional.contains(&key.as_str()) {
            return Err(invalid(description));
        }
    }
    Ok(())
}

fn is_id(value: &JsonValue) -> bool {
    value.as_str().is_some_and(|text| !text.is_empty())
}

fn is_mode(value: &JsonValue) -> bool {
    value
        .as_str()
        .map(|mode| mode == "singleton" || mode == "keyed")
        == Some(true)
}

fn is_integer(value: &JsonValue, minimum: i64) -> bool {
    value
        .as_f64()
        .is_some_and(|number| number.fract() == 0.0 && number >= minimum as f64)
}

// ── serde adapters ───────────────────────────────────────────────────────────
//
// The typed snapshots/updates reuse serde_json's derived shapes with the ops
// left raw, so validation (strict, upstream-ordered) happens above and the
// conversion below only maps already-checked tuples.

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawSubscriptionSnapshot {
    service_id: String,
    mode: ServiceModeDef,
    instances: Vec<RawInstanceSnapshot>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawInstanceSnapshot {
    instance: Option<ServiceInstanceAddress>,
    members: Vec<RawMemberSnapshot>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawMemberSnapshot {
    name: String,
    kind: String,
    sequence: Option<u64>,
    ops: Option<Vec<JsonValue>>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawProviderUpdate {
    #[serde(rename = "type")]
    kind: String,
    instance: Option<ServiceInstanceAddress>,
    member: Option<String>,
    sequence: Option<u64>,
    ops: Option<Vec<JsonValue>>,
    /// Raw: `replaced`/`spawned` carry an instance snapshot, `reset` a
    /// whole subscription snapshot (decoded per kind below).
    snapshot: Option<JsonValue>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
enum ServiceModeDef {
    Singleton,
    Keyed,
}

impl From<ServiceModeDef> for ServiceMode {
    fn from(mode: ServiceModeDef) -> ServiceMode {
        match mode {
            ServiceModeDef::Singleton => ServiceMode::Singleton,
            ServiceModeDef::Keyed => ServiceMode::Keyed,
        }
    }
}

impl RawSubscriptionSnapshot {
    fn into_snapshot(
        self,
        decode_ops: impl Fn(&[JsonValue]) -> Result<Vec<Op>, DeltaError>,
    ) -> ServiceSubscriptionSnapshot {
        ServiceSubscriptionSnapshot {
            service_id: self.service_id,
            mode: self.mode.into(),
            instances: self
                .instances
                .into_iter()
                .map(|instance| instance.into_snapshot(&decode_ops))
                .collect(),
        }
    }

    fn into_wire_snapshot(
        self,
        decode_ops: impl Fn(&[JsonValue]) -> Result<Vec<WireOp>, DeltaError>,
    ) -> WireServiceSubscriptionSnapshot {
        WireServiceSubscriptionSnapshot {
            service_id: self.service_id,
            mode: self.mode.into(),
            instances: self
                .instances
                .into_iter()
                .map(|instance| instance.into_wire_snapshot(&decode_ops))
                .collect(),
        }
    }
}

impl RawInstanceSnapshot {
    fn into_snapshot(
        self,
        decode_ops: impl Fn(&[JsonValue]) -> Result<Vec<Op>, DeltaError>,
    ) -> ServiceInstanceSnapshot {
        ServiceInstanceSnapshot {
            instance: self.instance,
            members: self
                .members
                .into_iter()
                .map(|member| member.into_snapshot(&decode_ops))
                .collect(),
        }
    }

    fn into_wire_snapshot(
        self,
        decode_ops: impl Fn(&[JsonValue]) -> Result<Vec<WireOp>, DeltaError>,
    ) -> WireServiceInstanceSnapshot {
        WireServiceInstanceSnapshot {
            instance: self.instance,
            members: self
                .members
                .into_iter()
                .map(|member| member.into_wire_snapshot(&decode_ops))
                .collect(),
        }
    }
}

impl RawMemberSnapshot {
    fn into_snapshot(
        self,
        decode_ops: impl Fn(&[JsonValue]) -> Result<Vec<Op>, DeltaError>,
    ) -> ServiceMemberSnapshot {
        let ops = self.ops.unwrap_or_default();
        match (self.kind.as_str(), self.sequence) {
            ("state", Some(sequence)) => ServiceMemberSnapshot::State {
                name: self.name,
                sequence,
                ops: decode_ops(&ops).expect("validated above"),
            },
            _ => ServiceMemberSnapshot::Method { name: self.name },
        }
    }

    fn into_wire_snapshot(
        self,
        decode_ops: impl Fn(&[JsonValue]) -> Result<Vec<WireOp>, DeltaError>,
    ) -> WireServiceMemberSnapshot {
        let ops = self.ops.unwrap_or_default();
        match (self.kind.as_str(), self.sequence) {
            ("state", Some(sequence)) => WireServiceMemberSnapshot::State {
                name: self.name,
                sequence,
                ops: decode_ops(&ops).expect("validated above"),
            },
            _ => WireServiceMemberSnapshot::Method { name: self.name },
        }
    }
}

impl RawProviderUpdate {
    fn instance_snapshot(&self) -> Result<RawInstanceSnapshot, ChordError> {
        serde_json::from_value(self.snapshot.clone().unwrap_or(JsonValue::Null))
            .map_err(|_| invalid("service instance snapshot"))
    }

    fn subscription_snapshot(&self) -> Result<RawSubscriptionSnapshot, ChordError> {
        serde_json::from_value(self.snapshot.clone().unwrap_or(JsonValue::Null))
            .map_err(|_| invalid("service subscription snapshot"))
    }

    fn into_update(
        self,
        decode_ops: impl Fn(&[JsonValue]) -> Result<Vec<Op>, DeltaError>,
    ) -> Result<ServiceProviderUpdate, ChordError> {
        match self.kind.as_str() {
            "state" => Ok(ServiceProviderUpdate::State {
                instance: self.instance,
                member: self.member.unwrap_or_default(),
                sequence: self.sequence.unwrap_or_default(),
                ops: decode_ops(&self.ops.unwrap_or_default()).expect("validated above"),
            }),
            "reset" => Ok(ServiceProviderUpdate::Reset {
                snapshot: self.subscription_snapshot()?.into_snapshot(&decode_ops),
            }),
            "replaced" => Ok(ServiceProviderUpdate::Replaced {
                snapshot: self.instance_snapshot()?.into_snapshot(&decode_ops),
            }),
            "spawned" => Ok(ServiceProviderUpdate::Spawned {
                instance: self.instance_snapshot()?.into_snapshot(&decode_ops),
            }),
            "closed" => Ok(ServiceProviderUpdate::Closed {
                instance: self.instance.expect("validated above"),
            }),
            _ => Ok(ServiceProviderUpdate::Unavailable),
        }
    }

    fn into_wire_update(
        self,
        decode_ops: impl Fn(&[JsonValue]) -> Result<Vec<WireOp>, DeltaError>,
    ) -> Result<WireServiceProviderUpdate, ChordError> {
        match self.kind.as_str() {
            "state" => Ok(WireServiceProviderUpdate::State {
                instance: self.instance,
                member: self.member.unwrap_or_default(),
                sequence: self.sequence.unwrap_or_default(),
                ops: decode_ops(&self.ops.unwrap_or_default()).expect("validated above"),
            }),
            "reset" => Ok(WireServiceProviderUpdate::Reset {
                snapshot: self
                    .subscription_snapshot()?
                    .into_wire_snapshot(&decode_ops),
            }),
            "replaced" => Ok(WireServiceProviderUpdate::Replaced {
                snapshot: self.instance_snapshot()?.into_wire_snapshot(&decode_ops),
            }),
            "spawned" => Ok(WireServiceProviderUpdate::Spawned {
                instance: self.instance_snapshot()?.into_wire_snapshot(&decode_ops),
            }),
            "closed" => Ok(WireServiceProviderUpdate::Closed {
                instance: self.instance.expect("validated above"),
            }),
            _ => Ok(WireServiceProviderUpdate::Unavailable),
        }
    }
}
