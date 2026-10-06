//! Port of upstream `coding-agent/src/core/model-config.ts`: the immutable,
//! credential-blind `models.json` snapshot ([`ModelConfig`]).
//!
//! Upstream validates with typebox (`Type.Object`/`Compile`/`Errors`); no
//! schema-validation crate is available offline, so the validator here is a
//! hand-written, typebox-faithful subset covering exactly the schema
//! combinators this file uses (object/required, string+minLength, number,
//! boolean, literal, null, array+maxItems, record, anyOf-union, unknown).
//! Error *rendering* is byte-pinned against the real typebox (1.3.11, from a
//! local install; upstream pins 1.3.27) in
//! `tests/fixtures/core_oracle/model_config.oracle.json`:
//!
//! - type mismatches: `must be object` / `must be string` / `must be number`
//!   / `must be boolean` / `must be array` / `must be null`
//! - missing required: `must have required properties a, b` (all missing, in
//!   schema order, one error per object)
//! - `minLength`: `must not have fewer than N characters`
//! - literals: `must be equal to constant`
//! - unions: each branch's errors in branch order, then one
//!   `must match a schema in anyOf`
//! - order: schema property order per object, depth-first; array/record
//!   values in document order
//!
//! One behavioral consequence falls out exactly as upstream: because every
//! `compat` union branch is an all-optional object and typebox allows
//! additional properties, *any* object passes the `compat` union (only
//! non-objects fail), so compat contents are never effectively validated.
//!
//! Other disclosures:
//! - JSON is parsed into [`OrderedValue`] to keep document key order
//!   (providers retain stored order). serde_json/preserve_order now also
//!   retains string insertion order; JS integer-index enumeration is a
//!   separate audit item for these model-config APIs.
//! - V8/serde_json JSON parse error texts differ (disclosed); the failure
//!   channel (`Failed to parse models.json: …`) matches.
//! - non-ENOENT read-failure texts come from `std::io::Error` (upstream: node
//!   fs error messages); disclosed.
//! - `deepFreeze(structuredClone(provider))` becomes ownership (Rust data is
//!   immutable behind `&`); no runtime equivalent is needed.
//! - `ModelConfig.load` never rejects upstream; path-normalization failures
//!   ([`crate::coding_agent::utils::paths::PathError`]) are the port's only
//!   rejecting channel, surfaced through `Result`.

use serde::de::{Deserializer, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;
use std::fmt;

use crate::coding_agent::utils::json::strip_json_comments;
use crate::coding_agent::utils::paths::normalize_path;
use crate::coding_agent::utils::text::strip_bom;

// ---------------------------------------------------------------------------
// OrderedValue: document-order JSON
// ---------------------------------------------------------------------------

/// `serde_json::Value` with object key order preserved (JS object semantics:
/// duplicate keys keep the first position, last value wins).
#[derive(Debug, Clone, PartialEq, Default)]
pub enum OrderedValue {
    #[default]
    Null,
    Bool(bool),
    Number(serde_json::Number),
    String(String),
    Array(Vec<OrderedValue>),
    Object(Vec<(String, OrderedValue)>),
}

impl OrderedValue {
    /// JS `typeof value === "object" && value !== null`.
    pub fn is_object(&self) -> bool {
        matches!(self, Self::Object(_))
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(text) => Some(text),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Number(number) => number.as_f64(),
            _ => None,
        }
    }

    pub fn get(&self, key: &str) -> Option<&OrderedValue> {
        match self {
            Self::Object(entries) => entries
                .iter()
                .find(|(existing, _)| existing == key)
                .map(|(_, value)| value),
            _ => None,
        }
    }

    /// Convert to `serde_json::Value`, retaining the stored object entry order
    /// via serde_json/preserve_order.
    pub fn to_serde(&self) -> serde_json::Value {
        match self {
            Self::Null => serde_json::Value::Null,
            Self::Bool(value) => serde_json::Value::Bool(*value),
            Self::Number(number) => serde_json::Value::Number(number.clone()),
            Self::String(text) => serde_json::Value::String(text.clone()),
            Self::Array(items) => {
                serde_json::Value::Array(items.iter().map(Self::to_serde).collect())
            }
            Self::Object(entries) => entries
                .iter()
                .map(|(key, value)| (key.clone(), value.to_serde()))
                .collect::<serde_json::Map<String, serde_json::Value>>()
                .into(),
        }
    }

    /// Render compact JSON in stored order, with JS binary64 Number formatting.
    /// JS integer-index enumeration must still be applied by the caller.
    pub fn to_json_string(&self) -> String {
        let mut out = String::new();
        self.write_json(&mut out);
        out
    }

    fn write_json(&self, out: &mut String) {
        match self {
            Self::Null => out.push_str("null"),
            Self::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
            Self::Number(number) => out.push_str(&crate::serde_support::js_number_string(
                number.as_f64().expect("JSON numbers fit in binary64"),
            )),
            Self::String(text) => {
                let json = serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string());
                out.push_str(&json);
            }
            Self::Array(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    item.write_json(out);
                }
                out.push(']');
            }
            Self::Object(entries) => {
                out.push('{');
                for (index, (key, value)) in entries.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    let key_json =
                        serde_json::to_string(key).unwrap_or_else(|_| "\"\"".to_string());
                    out.push_str(&key_json);
                    out.push(':');
                    value.write_json(out);
                }
                out.push('}');
            }
        }
    }
}

impl<'de> Deserialize<'de> for OrderedValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct OrderedValueVisitor;

        impl<'de> Visitor<'de> for OrderedValueVisitor {
            type Value = OrderedValue;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("any JSON value")
            }

            fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
                Ok(OrderedValue::Bool(value))
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
                Ok(OrderedValue::Number(value.into()))
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
                Ok(OrderedValue::Number(value.into()))
            }

            fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(OrderedValue::Number(
                    serde_json::Number::from_f64(value)
                        .ok_or_else(|| serde::de::Error::custom("non-finite number"))?,
                ))
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
                Ok(OrderedValue::String(value.to_string()))
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(OrderedValue::Null)
            }

            fn visit_none<E>(self) -> Result<Self::Value, E> {
                Ok(OrderedValue::Null)
            }

            fn visit_some<D: Deserializer<'de>>(
                self,
                deserializer: D,
            ) -> Result<Self::Value, D::Error> {
                OrderedValue::deserialize(deserializer)
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
                let mut items = Vec::new();
                while let Some(item) = access.next_element::<OrderedValue>()? {
                    items.push(item);
                }
                Ok(OrderedValue::Array(items))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
                let mut entries: Vec<(String, OrderedValue)> = Vec::new();
                while let Some((key, value)) = access.next_entry::<String, OrderedValue>()? {
                    match entries.iter_mut().find(|(existing, _)| *existing == key) {
                        // JS JSON.parse semantics: first position, last value.
                        Some(entry) => entry.1 = value,
                        None => entries.push((key, value)),
                    }
                }
                Ok(OrderedValue::Object(entries))
            }
        }

        deserializer.deserialize_any(OrderedValueVisitor)
    }
}

// ---------------------------------------------------------------------------
// The hand-written typebox-faithful validator
// ---------------------------------------------------------------------------

/// A validation error, mirroring typebox's `TLocalizedValidationError` fields
/// the port reads: `keyword`, `instancePath` (slash-joined), the
/// `params.requiredProperties` list, and the default-locale message.
#[derive(Debug, Clone, PartialEq)]
struct SchemaError {
    instance_path: String,
    message: String,
    required_properties: Vec<String>,
    keyword_is_required: bool,
}

type SchemaResult = Result<(), Vec<SchemaError>>;

/// The schema combinator subset model-config.ts uses.
#[derive(Debug, Clone)]
enum Schema {
    Object {
        properties: Vec<(&'static str, Schema)>,
        required: &'static [&'static str],
    },
    String {
        min_length: Option<u64>,
    },
    Number,
    Boolean,
    Literal(&'static str),
    Null,
    Array {
        items: Box<Schema>,
        max_items: Option<usize>,
    },
    Record(Box<Schema>),
    AnyOf(Vec<Schema>),
    Unknown,
}

fn join_path(base: &str, key: &str) -> String {
    if base.is_empty() {
        format!("/{key}")
    } else {
        format!("{base}/{key}")
    }
}

fn type_error(path: &str, expected: &str) -> SchemaError {
    SchemaError {
        instance_path: path.to_string(),
        message: format!("must be {expected}"),
        required_properties: Vec::new(),
        keyword_is_required: false,
    }
}

fn validate(value: &OrderedValue, schema: &Schema, path: &str) -> SchemaResult {
    match schema {
        Schema::Unknown => Ok(()),
        Schema::Number => {
            if matches!(value, OrderedValue::Number(_)) {
                Ok(())
            } else {
                Err(vec![type_error(path, "number")])
            }
        }
        Schema::Boolean => {
            if matches!(value, OrderedValue::Bool(_)) {
                Ok(())
            } else {
                Err(vec![type_error(path, "boolean")])
            }
        }
        Schema::Null => {
            if matches!(value, OrderedValue::Null) {
                Ok(())
            } else {
                Err(vec![type_error(path, "null")])
            }
        }
        Schema::Literal(expected) => {
            if value.as_str() == Some(expected) {
                Ok(())
            } else {
                Err(vec![SchemaError {
                    instance_path: path.to_string(),
                    message: "must be equal to constant".to_string(),
                    required_properties: Vec::new(),
                    keyword_is_required: false,
                }])
            }
        }
        Schema::String { min_length } => {
            let Some(text) = value.as_str() else {
                return Err(vec![type_error(path, "string")]);
            };
            if let Some(min_length) = min_length {
                // JS string length: UTF-16 code units.
                let length = text.encode_utf16().count() as u64;
                if length < *min_length {
                    return Err(vec![SchemaError {
                        instance_path: path.to_string(),
                        message: format!("must not have fewer than {min_length} characters"),
                        required_properties: Vec::new(),
                        keyword_is_required: false,
                    }]);
                }
            }
            Ok(())
        }
        Schema::Object {
            properties,
            required,
        } => {
            let entries = match value {
                OrderedValue::Object(entries) => entries,
                _ => return Err(vec![type_error(path, "object")]),
            };
            let mut errors = Vec::new();
            let missing: Vec<&str> = required
                .iter()
                .copied()
                .filter(|key| !entries.iter().any(|(existing, _)| existing == *key))
                .collect();
            if !missing.is_empty() {
                let list = missing
                    .iter()
                    .map(|key| (*key).to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                errors.push(SchemaError {
                    instance_path: path.to_string(),
                    message: format!("must have required properties {list}"),
                    required_properties: missing.iter().map(|key| key.to_string()).collect(),
                    keyword_is_required: true,
                });
            }
            // Property validation follows schema order, depth-first.
            for (key, property_schema) in properties {
                if let Some(property_value) = entries
                    .iter()
                    .find(|(existing, _)| existing == key)
                    .map(|(_, value)| value)
                {
                    if let Err(property_errors) =
                        validate(property_value, property_schema, &join_path(path, key))
                    {
                        errors.extend(property_errors);
                    }
                }
            }
            if errors.is_empty() {
                Ok(())
            } else {
                Err(errors)
            }
        }
        Schema::Array { items, max_items } => {
            let values = match value {
                OrderedValue::Array(values) => values,
                _ => return Err(vec![type_error(path, "array")]),
            };
            let mut errors = Vec::new();
            for (index, item) in values.iter().enumerate() {
                if let Err(item_errors) = validate(item, items, &format!("{path}/{index}")) {
                    errors.extend(item_errors);
                }
            }
            if let Some(max_items) = max_items {
                if values.len() > *max_items {
                    errors.push(SchemaError {
                        instance_path: path.to_string(),
                        message: format!("must not have more than {max_items} elements"),
                        required_properties: Vec::new(),
                        keyword_is_required: false,
                    });
                }
            }
            if errors.is_empty() {
                Ok(())
            } else {
                Err(errors)
            }
        }
        Schema::Record(value_schema) => {
            let entries = match value {
                OrderedValue::Object(entries) => entries,
                _ => return Err(vec![type_error(path, "object")]),
            };
            let mut errors = Vec::new();
            for (key, entry_value) in entries {
                if let Err(entry_errors) =
                    validate(entry_value, value_schema, &join_path(path, key))
                {
                    errors.extend(entry_errors);
                }
            }
            if errors.is_empty() {
                Ok(())
            } else {
                Err(errors)
            }
        }
        Schema::AnyOf(branches) => {
            let mut all_errors = Vec::new();
            for branch in branches {
                match validate(value, branch, path) {
                    Ok(()) => return Ok(()),
                    Err(errors) => all_errors.extend(errors),
                }
            }
            all_errors.push(SchemaError {
                instance_path: path.to_string(),
                message: "must match a schema in anyOf".to_string(),
                required_properties: Vec::new(),
                keyword_is_required: false,
            });
            Err(all_errors)
        }
    }
}

/// Upstream `formatValidationPath(error)`.
fn format_validation_path(error: &SchemaError) -> String {
    if error.keyword_is_required {
        if let Some(required_property) = error.required_properties.first() {
            let base_path = error
                .instance_path
                .strip_prefix('/')
                .unwrap_or(&error.instance_path)
                .replace('/', ".");
            return if base_path.is_empty() {
                required_property.clone()
            } else {
                format!("{base_path}.{required_property}")
            };
        }
    }
    let path = error
        .instance_path
        .strip_prefix('/')
        .unwrap_or(&error.instance_path)
        .replace('/', ".");
    if path.is_empty() {
        "root".to_string()
    } else {
        path
    }
}

// ---------------------------------------------------------------------------
// The models.json schema (upstream Type.* definitions)
// ---------------------------------------------------------------------------

fn percentile_cutoffs_schema() -> Schema {
    Schema::Object {
        properties: vec![
            ("p50", Schema::Number),
            ("p75", Schema::Number),
            ("p90", Schema::Number),
            ("p99", Schema::Number),
        ],
        required: &[],
    }
}

fn number_or_string() -> Schema {
    Schema::AnyOf(vec![Schema::Number, Schema::String { min_length: None }])
}

fn open_router_routing_schema() -> Schema {
    Schema::Object {
        properties: vec![
            ("allow_fallbacks", Schema::Boolean),
            ("require_parameters", Schema::Boolean),
            (
                "data_collection",
                Schema::AnyOf(vec![Schema::Literal("deny"), Schema::Literal("allow")]),
            ),
            ("zdr", Schema::Boolean),
            ("enforce_distillable_text", Schema::Boolean),
            (
                "order",
                Schema::Array {
                    items: Box::new(Schema::String { min_length: None }),
                    max_items: None,
                },
            ),
            (
                "only",
                Schema::Array {
                    items: Box::new(Schema::String { min_length: None }),
                    max_items: None,
                },
            ),
            (
                "ignore",
                Schema::Array {
                    items: Box::new(Schema::String { min_length: None }),
                    max_items: None,
                },
            ),
            (
                "quantizations",
                Schema::Array {
                    items: Box::new(Schema::String { min_length: None }),
                    max_items: None,
                },
            ),
            (
                "sort",
                Schema::AnyOf(vec![
                    Schema::String { min_length: None },
                    Schema::Object {
                        properties: vec![
                            ("by", Schema::String { min_length: None }),
                            (
                                "partition",
                                Schema::AnyOf(vec![
                                    Schema::String { min_length: None },
                                    Schema::Null,
                                ]),
                            ),
                        ],
                        required: &[],
                    },
                ]),
            ),
            (
                "max_price",
                Schema::Object {
                    properties: vec![
                        ("prompt", number_or_string()),
                        ("completion", number_or_string()),
                        ("image", number_or_string()),
                        ("audio", number_or_string()),
                        ("request", number_or_string()),
                    ],
                    required: &[],
                },
            ),
            (
                "preferred_min_throughput",
                Schema::AnyOf(vec![Schema::Number, percentile_cutoffs_schema()]),
            ),
            (
                "preferred_max_latency",
                Schema::AnyOf(vec![Schema::Number, percentile_cutoffs_schema()]),
            ),
        ],
        required: &[],
    }
}

fn vercel_gateway_routing_schema() -> Schema {
    Schema::Object {
        properties: vec![
            (
                "only",
                Schema::Array {
                    items: Box::new(Schema::String { min_length: None }),
                    max_items: None,
                },
            ),
            (
                "order",
                Schema::Array {
                    items: Box::new(Schema::String { min_length: None }),
                    max_items: None,
                },
            ),
        ],
        required: &[],
    }
}

fn thinking_level_map_schema() -> Schema {
    let value = || Schema::AnyOf(vec![Schema::String { min_length: None }, Schema::Null]);
    Schema::Object {
        properties: vec![
            ("off", value()),
            ("minimal", value()),
            ("low", value()),
            ("medium", value()),
            ("high", value()),
            ("xhigh", value()),
            ("max", value()),
        ],
        required: &[],
    }
}

/// Upstream `SamplingParamsSchema` (`Type.Record(Type.String(), Type.Unknown())`).
fn sampling_params_schema() -> Schema {
    Schema::Record(Box::new(Schema::Unknown))
}

/// Upstream `SamplingParamsByThinkingLevelSchema`: the seven optional
/// `ModelThinkingLevel` keys, each a [`sampling_params_schema`] record.
fn sampling_params_by_thinking_level_schema() -> Schema {
    let value = || sampling_params_schema();
    Schema::Object {
        properties: vec![
            ("off", value()),
            ("minimal", value()),
            ("low", value()),
            ("medium", value()),
            ("high", value()),
            ("xhigh", value()),
            ("max", value()),
        ],
        required: &[],
    }
}

fn chat_template_kwarg_schema() -> Schema {
    // Union([scalar union, { $var: Union([...]), omitWhenOff? }])
    Schema::AnyOf(vec![
        Schema::AnyOf(vec![
            Schema::String { min_length: None },
            Schema::Number,
            Schema::Boolean,
            Schema::Null,
        ]),
        Schema::Object {
            properties: vec![
                (
                    "$var",
                    Schema::AnyOf(vec![
                        Schema::Literal("thinking.enabled"),
                        Schema::Literal("thinking.effort"),
                    ]),
                ),
                ("omitWhenOff", Schema::Boolean),
            ],
            required: &["$var"],
        },
    ])
}

fn non_empty_string() -> Schema {
    Schema::String {
        min_length: Some(1),
    }
}

fn thinking_format_schema() -> Schema {
    Schema::AnyOf(vec![
        Schema::Literal("openai"),
        Schema::Literal("openrouter"),
        Schema::Literal("together"),
        Schema::Literal("baseten"),
        Schema::Literal("deepseek"),
        Schema::Literal("zai"),
        Schema::Literal("qwen"),
        Schema::Literal("chat-template"),
        Schema::Literal("qwen-chat-template"),
        Schema::Literal("string-thinking"),
        Schema::Literal("ant-ling"),
    ])
}

fn session_affinity_format_schema() -> Schema {
    Schema::AnyOf(vec![
        Schema::Literal("openai"),
        Schema::Literal("openai-nosession"),
        Schema::Literal("openrouter"),
    ])
}

fn chat_template_kwargs_properties() -> Vec<(&'static str, Schema)> {
    vec![
        (
            "chatTemplateKwargs",
            Schema::Record(Box::new(chat_template_kwarg_schema())),
        ),
        (
            "chatTemplateArgs",
            Schema::Record(Box::new(chat_template_kwarg_schema())),
        ),
    ]
}

fn open_ai_completions_compat_schema() -> Schema {
    let mut properties: Vec<(&'static str, Schema)> = vec![
        ("supportsStore", Schema::Boolean),
        ("supportsDeveloperRole", Schema::Boolean),
        ("supportsReasoningEffort", Schema::Boolean),
        ("supportsUsageInStreaming", Schema::Boolean),
        ("supportsFinishReason", Schema::Boolean),
        (
            "maxTokensField",
            Schema::AnyOf(vec![
                Schema::Literal("max_completion_tokens"),
                Schema::Literal("max_tokens"),
            ]),
        ),
        ("requiresToolResultName", Schema::Boolean),
        ("requiresAssistantAfterToolResult", Schema::Boolean),
        ("requiresThinkingAsText", Schema::Boolean),
        (
            "requiresReasoningContentOnAssistantMessages",
            Schema::Boolean,
        ),
        ("thinkingFormat", thinking_format_schema()),
    ];
    properties.extend(chat_template_kwargs_properties());
    properties.extend(vec![
        ("cacheControlFormat", Schema::Literal("anthropic")),
        ("openRouterRouting", open_router_routing_schema()),
        ("vercelGatewayRouting", vercel_gateway_routing_schema()),
        ("supportsOpenAIGrammarTools", Schema::Boolean),
        ("supportsStrictMode", Schema::Boolean),
        ("sendSessionAffinityHeaders", Schema::Boolean),
        ("sessionAffinityFormat", session_affinity_format_schema()),
        ("supportsLongCacheRetention", Schema::Boolean),
        ("vllmPriority", Schema::Number),
    ]);
    Schema::Object {
        properties,
        required: &[],
    }
}

fn open_ai_responses_compat_schema() -> Schema {
    Schema::Object {
        properties: vec![
            ("supportsDeveloperRole", Schema::Boolean),
            ("sessionAffinityFormat", session_affinity_format_schema()),
            ("supportsLongCacheRetention", Schema::Boolean),
            ("supportsStrictMode", Schema::Boolean),
            ("supportsOpenAIGrammarTools", Schema::Boolean),
            ("supportsMaxOutputTokens", Schema::Boolean),
        ],
        required: &[],
    }
}

fn model_cost_schema() -> Schema {
    Schema::Object {
        properties: vec![
            ("input", Schema::Number),
            ("output", Schema::Number),
            ("cacheRead", Schema::Number),
            ("cacheWrite", Schema::Number),
            (
                "tiers",
                Schema::Array {
                    items: Box::new(Schema::Object {
                        properties: vec![
                            ("inputTokensAbove", Schema::Number),
                            ("input", Schema::Number),
                            ("output", Schema::Number),
                            ("cacheRead", Schema::Number),
                            ("cacheWrite", Schema::Number),
                        ],
                        required: &[
                            "inputTokensAbove",
                            "input",
                            "output",
                            "cacheRead",
                            "cacheWrite",
                        ],
                    }),
                    max_items: None,
                },
            ),
        ],
        required: &["input", "output", "cacheRead", "cacheWrite"],
    }
}

fn anthropic_messages_compat_schema() -> Schema {
    Schema::Object {
        properties: vec![
            ("supportsEagerToolInputStreaming", Schema::Boolean),
            ("supportsLongCacheRetention", Schema::Boolean),
            ("sendSessionAffinityHeaders", Schema::Boolean),
            ("supportsCacheControlOnTools", Schema::Boolean),
            ("supportsTemperature", Schema::Boolean),
            ("forceAdaptiveThinking", Schema::Boolean),
            ("allowEmptySignature", Schema::Boolean),
            ("supportsStrictTools", Schema::Boolean),
            ("supportsMidConvoEffort", Schema::Boolean),
            (
                "allowedFallbackModels",
                Schema::Array {
                    items: Box::new(Schema::Object {
                        properties: vec![
                            ("provider", non_empty_string()),
                            ("model", non_empty_string()),
                            ("cost", model_cost_schema()),
                        ],
                        required: &["provider", "model", "cost"],
                    }),
                    max_items: Some(3),
                },
            ),
        ],
        required: &[],
    }
}

fn provider_compat_schema() -> Schema {
    Schema::AnyOf(vec![
        open_ai_completions_compat_schema(),
        open_ai_responses_compat_schema(),
        anthropic_messages_compat_schema(),
    ])
}

fn model_definition_schema() -> Schema {
    Schema::Object {
        properties: vec![
            ("id", non_empty_string()),
            ("name", non_empty_string()),
            ("api", non_empty_string()),
            ("baseUrl", non_empty_string()),
            ("reasoning", Schema::Boolean),
            ("thinkingLevelMap", thinking_level_map_schema()),
            (
                "input",
                Schema::Array {
                    items: Box::new(Schema::AnyOf(vec![
                        Schema::Literal("text"),
                        Schema::Literal("image"),
                    ])),
                    max_items: None,
                },
            ),
            ("cost", model_cost_schema()),
            ("contextWindow", Schema::Number),
            ("maxTokens", Schema::Number),
            ("samplingParams", sampling_params_schema()),
            (
                "samplingParamsByThinkingLevel",
                sampling_params_by_thinking_level_schema(),
            ),
            ("headers", Schema::Record(Box::new(non_empty_string()))),
            ("compat", provider_compat_schema()),
        ],
        required: &["id"],
    }
}

/// Upstream `ModelOverrideSchema.cost`: all-optional rates (unlike
/// `ModelDefinitionSchema.cost`, which requires them).
fn model_override_cost_schema() -> Schema {
    Schema::Object {
        properties: vec![
            ("input", Schema::Number),
            ("output", Schema::Number),
            ("cacheRead", Schema::Number),
            ("cacheWrite", Schema::Number),
            (
                "tiers",
                Schema::Array {
                    items: Box::new(Schema::Object {
                        properties: vec![
                            ("inputTokensAbove", Schema::Number),
                            ("input", Schema::Number),
                            ("output", Schema::Number),
                            ("cacheRead", Schema::Number),
                            ("cacheWrite", Schema::Number),
                        ],
                        required: &[
                            "inputTokensAbove",
                            "input",
                            "output",
                            "cacheRead",
                            "cacheWrite",
                        ],
                    }),
                    max_items: None,
                },
            ),
        ],
        required: &[],
    }
}

fn model_override_schema() -> Schema {
    Schema::Object {
        properties: vec![
            ("name", non_empty_string()),
            ("reasoning", Schema::Boolean),
            ("thinkingLevelMap", thinking_level_map_schema()),
            (
                "input",
                Schema::Array {
                    items: Box::new(Schema::AnyOf(vec![
                        Schema::Literal("text"),
                        Schema::Literal("image"),
                    ])),
                    max_items: None,
                },
            ),
            ("cost", model_override_cost_schema()),
            ("contextWindow", Schema::Number),
            ("maxTokens", Schema::Number),
            ("samplingParams", sampling_params_schema()),
            (
                "samplingParamsByThinkingLevel",
                sampling_params_by_thinking_level_schema(),
            ),
            ("headers", Schema::Record(Box::new(non_empty_string()))),
            ("compat", provider_compat_schema()),
        ],
        required: &[],
    }
}

fn provider_config_schema() -> Schema {
    Schema::Object {
        properties: vec![
            ("name", non_empty_string()),
            ("baseUrl", non_empty_string()),
            ("apiKey", non_empty_string()),
            ("api", non_empty_string()),
            ("oauth", Schema::Literal("radius")),
            ("headers", Schema::Record(Box::new(non_empty_string()))),
            ("compat", provider_compat_schema()),
            ("authHeader", Schema::Boolean),
            (
                "models",
                Schema::Array {
                    items: Box::new(model_definition_schema()),
                    max_items: None,
                },
            ),
            (
                "modelOverrides",
                Schema::Record(Box::new(model_override_schema())),
            ),
        ],
        required: &[],
    }
}

fn models_config_schema() -> Schema {
    Schema::Object {
        properties: vec![(
            "providers",
            Schema::Record(Box::new(provider_config_schema())),
        )],
        required: &["providers"],
    }
}

// ---------------------------------------------------------------------------
// Typed snapshot (upstream `Static<typeof …>` exports)
// ---------------------------------------------------------------------------

/// Upstream `ModelsJsonProvider` (`Static<typeof ProviderConfigSchema>`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModelsJsonProvider {
    pub name: Option<String>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub api: Option<String>,
    /// Upstream `oauth?: "radius"`.
    pub oauth: Option<String>,
    /// Ordered `Record<string, string>`.
    pub headers: Option<Vec<(String, String)>>,
    /// Raw compat object (union data; contents are not effectively validated
    /// upstream — see the module docs).
    pub compat: Option<OrderedValue>,
    pub auth_header: Option<bool>,
    pub models: Option<Vec<ModelsJsonModel>>,
    /// Ordered `Record<string, ModelsJsonModelOverride>`.
    pub model_overrides: Option<Vec<(String, ModelsJsonModelOverride)>>,
}

/// Upstream `ModelsJsonModel` (`Static<typeof ModelDefinitionSchema>`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModelsJsonModel {
    pub id: String,
    pub name: Option<String>,
    pub api: Option<String>,
    pub base_url: Option<String>,
    pub reasoning: Option<bool>,
    pub thinking_level_map: Option<OrderedValue>,
    /// Ordered `("text" | "image")[]`.
    pub input: Option<Vec<String>>,
    pub cost: Option<OrderedValue>,
    pub context_window: Option<f64>,
    pub max_tokens: Option<f64>,
    pub sampling_params: Option<OrderedValue>,
    /// Ordered `samplingParamsByThinkingLevel` record.
    pub sampling_params_by_thinking_level: Option<OrderedValue>,
    pub headers: Option<Vec<(String, String)>>,
    pub compat: Option<OrderedValue>,
}

/// Upstream `ModelsJsonModelOverride` (`Static<typeof ModelOverrideSchema>`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModelsJsonModelOverride {
    pub name: Option<String>,
    pub reasoning: Option<bool>,
    pub thinking_level_map: Option<OrderedValue>,
    pub input: Option<Vec<String>>,
    pub cost: Option<OrderedValue>,
    pub context_window: Option<f64>,
    pub max_tokens: Option<f64>,
    pub sampling_params: Option<OrderedValue>,
    /// Ordered `samplingParamsByThinkingLevel` record.
    pub sampling_params_by_thinking_level: Option<OrderedValue>,
    pub headers: Option<Vec<(String, String)>>,
    pub compat: Option<OrderedValue>,
}

fn optional_string(value: &OrderedValue, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(OrderedValue::as_str)
        .map(str::to_string)
}

fn optional_bool(value: &OrderedValue, key: &str) -> Option<bool> {
    value.get(key).and_then(OrderedValue::as_bool)
}

fn optional_f64(value: &OrderedValue, key: &str) -> Option<f64> {
    value.get(key).and_then(OrderedValue::as_f64)
}

fn optional_ordered(value: &OrderedValue, key: &str) -> Option<OrderedValue> {
    value.get(key).cloned()
}

fn optional_string_record(value: &OrderedValue, key: &str) -> Option<Vec<(String, String)>> {
    match value.get(key) {
        Some(OrderedValue::Object(entries)) => Some(
            entries
                .iter()
                .map(|(name, item)| (name.clone(), item.as_str().unwrap_or_default().to_string()))
                .collect(),
        ),
        _ => None,
    }
}

fn optional_input_list(value: &OrderedValue, key: &str) -> Option<Vec<String>> {
    match value.get(key) {
        Some(OrderedValue::Array(items)) => Some(
            items
                .iter()
                .map(|item| item.as_str().unwrap_or_default().to_string())
                .collect(),
        ),
        _ => None,
    }
}

impl ModelsJsonModel {
    fn from_ordered(value: &OrderedValue) -> Self {
        Self {
            id: optional_string(value, "id").unwrap_or_default(),
            name: optional_string(value, "name"),
            api: optional_string(value, "api"),
            base_url: optional_string(value, "baseUrl"),
            reasoning: optional_bool(value, "reasoning"),
            thinking_level_map: optional_ordered(value, "thinkingLevelMap"),
            input: optional_input_list(value, "input"),
            cost: optional_ordered(value, "cost"),
            context_window: optional_f64(value, "contextWindow"),
            max_tokens: optional_f64(value, "maxTokens"),
            sampling_params: optional_ordered(value, "samplingParams"),
            sampling_params_by_thinking_level: optional_ordered(
                value,
                "samplingParamsByThinkingLevel",
            ),
            headers: optional_string_record(value, "headers"),
            compat: optional_ordered(value, "compat"),
        }
    }
}

impl ModelsJsonModelOverride {
    fn from_ordered(value: &OrderedValue) -> Self {
        Self {
            name: optional_string(value, "name"),
            reasoning: optional_bool(value, "reasoning"),
            thinking_level_map: optional_ordered(value, "thinkingLevelMap"),
            input: optional_input_list(value, "input"),
            cost: optional_ordered(value, "cost"),
            context_window: optional_f64(value, "contextWindow"),
            max_tokens: optional_f64(value, "maxTokens"),
            sampling_params: optional_ordered(value, "samplingParams"),
            sampling_params_by_thinking_level: optional_ordered(
                value,
                "samplingParamsByThinkingLevel",
            ),
            headers: optional_string_record(value, "headers"),
            compat: optional_ordered(value, "compat"),
        }
    }
}

impl ModelsJsonProvider {
    fn from_ordered(value: &OrderedValue) -> Self {
        let models = match value.get("models") {
            Some(OrderedValue::Array(items)) => {
                Some(items.iter().map(ModelsJsonModel::from_ordered).collect())
            }
            _ => None,
        };
        let model_overrides = match value.get("modelOverrides") {
            Some(OrderedValue::Object(entries)) => Some(
                entries
                    .iter()
                    .map(|(id, override_value)| {
                        (
                            id.clone(),
                            ModelsJsonModelOverride::from_ordered(override_value),
                        )
                    })
                    .collect(),
            ),
            _ => None,
        };
        Self {
            name: optional_string(value, "name"),
            base_url: optional_string(value, "baseUrl"),
            api_key: optional_string(value, "apiKey"),
            api: optional_string(value, "api"),
            oauth: optional_string(value, "oauth"),
            headers: optional_string_record(value, "headers"),
            compat: optional_ordered(value, "compat"),
            auth_header: optional_bool(value, "authHeader"),
            models,
            model_overrides,
        }
    }
}

// ---------------------------------------------------------------------------
// ModelConfig
// ---------------------------------------------------------------------------

/// One immutable load of models.json (upstream `ModelConfig`). Never holds
/// credentials; `load` failures are reported through
/// [`ModelConfig::get_error`] with an empty provider set.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModelConfig {
    providers: Vec<(String, ModelsJsonProvider)>,
    /// Raw document-order providers (the deep-frozen JS objects), for raw
    /// byte-level rendering.
    raw_providers: Vec<(String, OrderedValue)>,
    error: Option<String>,
}

impl ModelConfig {
    /// Upstream `ModelConfig.load(modelsJsonPath)`. `None` path → an empty
    /// config. Never fails except for path-normalization errors (upstream
    /// rejects the promise there; the port propagates [`PathError`]).
    pub async fn load(models_json_path: Option<&str>) -> Result<Self, PathLoadError> {
        let Some(models_json_path) = models_json_path else {
            return Ok(Self::default());
        };
        let path = normalize_path(models_json_path).map_err(PathLoadError)?;

        let content = match tokio::fs::read_to_string(&path).await {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => {
                return Ok(Self::with_error(format!(
                    "Failed to load models.json: {error}\n\nFile: {path}"
                )));
            }
        };

        let stripped = strip_json_comments(strip_bom(&content));
        let parsed: OrderedValue = match serde_json::from_str(&stripped) {
            Ok(parsed) => parsed,
            Err(error) => {
                return Ok(Self::with_error(format!(
                    "Failed to parse models.json: {error}\n\nFile: {path}"
                )));
            }
        };

        if let Err(errors) = validate(&parsed, &models_config_schema(), "") {
            let rendered = errors
                .iter()
                .map(|error| format!("  - {}: {}", format_validation_path(error), error.message))
                .collect::<Vec<_>>()
                .join("\n");
            let rendered = if rendered.is_empty() {
                "Unknown schema error".to_string()
            } else {
                rendered
            };
            return Ok(Self::with_error(format!(
                "Invalid models.json schema:\n{rendered}\n\nFile: {path}"
            )));
        }

        // `config.providers` is guaranteed to be an object here.
        let (providers, raw_providers) = match parsed.get("providers") {
            Some(OrderedValue::Object(entries)) => (
                entries
                    .iter()
                    .map(|(id, value)| (id.clone(), ModelsJsonProvider::from_ordered(value)))
                    .collect(),
                entries.clone(),
            ),
            _ => (Vec::new(), Vec::new()),
        };
        Ok(Self {
            providers,
            raw_providers,
            error: None,
        })
    }

    fn with_error(error: String) -> Self {
        Self {
            providers: Vec::new(),
            raw_providers: Vec::new(),
            error: Some(error),
        }
    }

    /// Upstream `getProvider(providerId)`.
    pub fn get_provider(&self, provider_id: &str) -> Option<&ModelsJsonProvider> {
        self.providers
            .iter()
            .find(|(id, _)| id == provider_id)
            .map(|(_, provider)| provider)
    }

    /// The raw document-order provider object (the deep-frozen JS value),
    /// rendered in stored order with JS Number formatting via
    /// [`OrderedValue::to_json_string`]. Integer-index enumeration for this
    /// model-config API remains a separate audit item.
    pub fn get_provider_raw(&self, provider_id: &str) -> Option<&OrderedValue> {
        self.raw_providers
            .iter()
            .find(|(id, _)| id == provider_id)
            .map(|(_, value)| value)
    }

    /// Upstream `getProviderIds()` (document order, like `Object.entries`).
    pub fn get_provider_ids(&self) -> Vec<&str> {
        self.providers.iter().map(|(id, _)| id.as_str()).collect()
    }

    /// Upstream `getError()`.
    pub fn get_error(&self) -> Option<&str> {
        self.error.as_deref()
    }
}

/// The only rejecting channel of [`ModelConfig::load`]: a path that node's
/// `normalizePath` would throw on (upstream rejects the promise).
#[derive(Debug, Clone, PartialEq)]
pub struct PathLoadError(pub crate::coding_agent::utils::paths::PathError);

impl fmt::Display for PathLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

impl std::error::Error for PathLoadError {}

#[cfg(test)]
#[path = "model_config_tests.rs"]
mod tests;
