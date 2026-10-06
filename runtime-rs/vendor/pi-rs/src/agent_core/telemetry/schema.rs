//! Port of the schema *data* model of `packages/telemetry/src/index.ts`
//! (`index.ts:26-74` plus the runtime-relevant pieces of the typed layer).
//!
//! Upstream schemas are `as const` object literals consumed by a large
//! compile-time inference machinery (`index.ts:76-354`) that has no runtime
//! behavior; the data itself is a compatibility surface — the serialized
//! schema must byte-match `JSON.stringify` of the upstream literals, whose
//! key order is each literal's insertion order. Because that order varies
//! per entry (e.g. `"pi.ai.response.id"` interleaves `cardinality` between
//! `type` and `description`), an attribute definition is modeled as an
//! ordered field list ([`AttrField`]) and serialized by streaming entries in
//! list order — `serde_json` map streaming preserves call order, unlike
//! `serde_json::Value` maps which sort.

use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};

/// A scalar entry of a schema `values`/`elementValues` array.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttrScalar {
    Str(&'static str),
    Num(i64),
    Bool(bool),
}

impl Serialize for AttrScalar {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            AttrScalar::Str(value) => serializer.serialize_str(value),
            AttrScalar::Num(value) => serializer.serialize_i64(*value),
            AttrScalar::Bool(value) => serializer.serialize_bool(*value),
        }
    }
}

/// One field of an attribute definition, serialized under its own key in
/// list order. The variant set covers every key the upstream literals use
/// (`type`, `required`, `values`, `elementValues`, `examples`, `cardinality`,
/// `description`).
#[derive(Debug, Clone)]
pub enum AttrField {
    Type(&'static str),
    Required(bool),
    Values(&'static [AttrScalar]),
    ElementValues(&'static [AttrScalar]),
    Cardinality(&'static str),
    Description(&'static str),
}

impl AttrField {
    /// The JSON key this field streams under inside the flat attribute
    /// definition object.
    fn key(&self) -> &'static str {
        match self {
            AttrField::Type(_) => "type",
            AttrField::Required(_) => "required",
            AttrField::Values(_) => "values",
            AttrField::ElementValues(_) => "elementValues",
            AttrField::Cardinality(_) => "cardinality",
            AttrField::Description(_) => "description",
        }
    }
}

impl Serialize for AttrField {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            AttrField::Type(value) => serializer.serialize_str(value),
            AttrField::Required(value) => serializer.serialize_bool(*value),
            AttrField::Values(values) => values.serialize(serializer),
            AttrField::ElementValues(values) => values.serialize(serializer),
            AttrField::Cardinality(value) => serializer.serialize_str(value),
            AttrField::Description(value) => serializer.serialize_str(value),
        }
    }
}

/// One attribute definition serialized as a single flat object whose keys
/// appear in upstream literal insertion order.
struct FlatAttrDef(&'static [AttrField]);

impl Serialize for FlatAttrDef {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for field in self.0 {
            map.serialize_entry(field.key(), field)?;
        }
        map.end()
    }
}

/// Upstream `TelemetryStartAttributeDefinition`/`TelemetryAttributeDefinition`
/// runtime shape: an ordered field list serialized as one object.
pub type AttributeDefinition = &'static [AttrField];

fn serialize_attribute_table<S: Serializer>(
    attributes: &[(&'static str, AttributeDefinition)],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let mut map = serializer.serialize_map(Some(attributes.len()))?;
    for (name, definition) in attributes {
        map.serialize_entry(name, &FlatAttrDef(definition))?;
    }
    map.end()
}

/// Upstream `TelemetryParentDefinition` (`index.ts:52-55`).
#[derive(Debug, Clone, Copy)]
pub enum ParentsKind {
    Any,
    RootOrExternal,
    Spans(&'static [&'static str]),
}

impl Serialize for ParentsKind {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(2))?;
        match self {
            ParentsKind::Any => {
                map.serialize_entry("kind", "any")?;
                map.end()
            }
            ParentsKind::RootOrExternal => {
                map.serialize_entry("kind", "root_or_external")?;
                map.end()
            }
            ParentsKind::Spans(spans) => {
                map.serialize_entry("kind", "spans")?;
                map.serialize_entry("spans", spans)?;
                map.end()
            }
        }
    }
}

/// Upstream `TelemetrySpanDefinition["status"]` (`index.ts:63`); `default`
/// is always `"ok"` in both shipped schemas, so only `errorWhen` is carried.
#[derive(Debug, Clone, Copy)]
pub struct StatusDefinition {
    pub error_when: &'static str,
}

impl Serialize for StatusDefinition {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(2))?;
        map.serialize_entry("default", "ok")?;
        map.serialize_entry("errorWhen", self.error_when)?;
        map.end()
    }
}

/// Upstream `TelemetrySpanDefinition` (`index.ts:57-64`). No shipped span
/// carries `events`, so the optional table is omitted (upstream serializes
/// the key only when present).
#[derive(Debug, Clone, Copy)]
pub struct SpanDefinition {
    pub description: &'static str,
    pub parents: ParentsKind,
    pub start_attributes: &'static [(&'static str, AttributeDefinition)],
    pub end_attributes: &'static [(&'static str, AttributeDefinition)],
    pub status: StatusDefinition,
}

impl Serialize for SpanDefinition {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(5))?;
        map.serialize_entry("description", self.description)?;
        map.serialize_entry("parents", &self.parents)?;
        map.serialize_entry("startAttributes", &SerializeTable(self.start_attributes))?;
        map.serialize_entry("endAttributes", &SerializeTable(self.end_attributes))?;
        map.serialize_entry("status", &self.status)?;
        map.end()
    }
}

struct SerializeTable(&'static [(&'static str, AttributeDefinition)]);

impl Serialize for SerializeTable {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serialize_attribute_table(self.0, serializer)
    }
}

/// Upstream `TelemetrySchemaDefinition` (`index.ts:66-69`).
#[derive(Debug, Clone, Copy)]
pub struct TelemetrySchemaDefinition {
    pub version: u64,
    pub spans: &'static [(&'static str, SpanDefinition)],
}

impl Serialize for TelemetrySchemaDefinition {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(2))?;
        map.serialize_entry("version", &self.version)?;
        map.serialize_entry("spans", &SerializeSpans(self.spans))?;
        map.end()
    }
}

struct SerializeSpans(&'static [(&'static str, SpanDefinition)]);

impl Serialize for SerializeSpans {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (name, definition) in self.0 {
            map.serialize_entry(name, definition)?;
        }
        map.end()
    }
}

/// Upstream `defineTelemetrySchema` (`index.ts:72-74`): a typed identity
/// helper; schema values are data, no runtime validation is performed.
pub fn define_telemetry_schema(schema: TelemetrySchemaDefinition) -> TelemetrySchemaDefinition {
    schema
}

/// Serialize a string-array constant (`HOOK_NAMES`/`EVENT_TYPES`) as a JSON
/// array, matching `JSON.stringify` of the upstream `as const` tuples.
pub struct JsonStringArray(pub &'static [&'static str]);

impl Serialize for JsonStringArray {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
        for item in self.0 {
            seq.serialize_element(item)?;
        }
        seq.end()
    }
}
