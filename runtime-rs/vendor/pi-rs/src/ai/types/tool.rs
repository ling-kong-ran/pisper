//! Tool declarations from upstream `packages/ai/src/types.ts:571-602`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Upstream `Strict` (types.ts:586): how badly a caller wants JSON-schema
/// constrained sampling. Wire values are `"prefer"` / `"require"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Strict {
    Prefer,
    Require,
}

/// Upstream `GrammarFormat` (types.ts:572): OpenAI grammar variant keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum GrammarFormat {
    #[serde(rename = "openai_lark")]
    Lark,
    #[serde(rename = "openai_regex")]
    Regex,
}

/// Upstream `GrammarVariants` (types.ts:574): provider-specific encodings of
/// the same intended language, keyed by grammar format.
pub type GrammarVariants = BTreeMap<GrammarFormat, String>;

/// JSON `false` — the explicit constrained-sampling opt-out marker. Rejects
/// every other JSON value so the untagged `ConstrainedSampling` union can
/// distinguish `false` from the config objects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Disabled;

impl Serialize for Disabled {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bool(false)
    }
}

impl<'de> Deserialize<'de> for Disabled {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct FalseOnly;
        impl serde::de::Visitor<'_> for FalseOnly {
            type Value = Disabled;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("false")
            }
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Self::Value, E> {
                if v {
                    Err(E::invalid_value(
                        serde::de::Unexpected::Bool(true),
                        &"false",
                    ))
                } else {
                    Ok(Disabled)
                }
            }
        }
        deserializer.deserialize_bool(FalseOnly)
    }
}

/// `{"type":"json_schema","strict":...}` arm of `ConstrainedSamplingConfig`
/// (types.ts:584-587). The `type` discriminant is fixed by the serde impl.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonSchemaSampling {
    pub strict: Strict,
}

impl Serialize for JsonSchemaSampling {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(2))?;
        map.serialize_entry("type", "json_schema")?;
        map.serialize_entry("strict", &self.strict)?;
        map.end()
    }
}

impl<'de> Deserialize<'de> for JsonSchemaSampling {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        const FIELDS: &[&str] = &["type", "strict"];
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = JsonSchemaSampling;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("{\"type\":\"json_schema\",\"strict\":...}")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut access: A,
            ) -> Result<Self::Value, A::Error> {
                let mut kind: Option<String> = None;
                let mut strict: Option<Strict> = None;
                while let Some(key) = access.next_key::<String>()? {
                    match key.as_str() {
                        "type" => kind = Some(access.next_value()?),
                        "strict" => strict = Some(access.next_value()?),
                        _ => {
                            let _ = access.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                match kind.as_deref() {
                    Some("json_schema") => {}
                    other => {
                        return Err(serde::de::Error::custom(format_args!(
                            "expected type \"json_schema\", got {:?}",
                            other
                        )))
                    }
                }
                let strict = strict.ok_or_else(|| serde::de::Error::missing_field("strict"))?;
                Ok(JsonSchemaSampling { strict })
            }
        }
        deserializer.deserialize_struct("JsonSchemaSampling", FIELDS, Visitor)
    }
}

/// `{"type":"grammar","variants":{...}}` arm of `ConstrainedSamplingConfig`
/// (types.ts:588-591). The `type` discriminant is fixed by the serde impl.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrammarSampling {
    pub variants: GrammarVariants,
}

impl Serialize for GrammarSampling {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(2))?;
        map.serialize_entry("type", "grammar")?;
        map.serialize_entry("variants", &self.variants)?;
        map.end()
    }
}

impl<'de> Deserialize<'de> for GrammarSampling {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        const FIELDS: &[&str] = &["type", "variants"];
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = GrammarSampling;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("{\"type\":\"grammar\",\"variants\":{...}}")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut access: A,
            ) -> Result<Self::Value, A::Error> {
                let mut kind: Option<String> = None;
                let mut variants: Option<GrammarVariants> = None;
                while let Some(key) = access.next_key::<String>()? {
                    match key.as_str() {
                        "type" => kind = Some(access.next_value()?),
                        "variants" => variants = Some(access.next_value()?),
                        _ => {
                            let _ = access.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                match kind.as_deref() {
                    Some("grammar") => {}
                    other => {
                        return Err(serde::de::Error::custom(format_args!(
                            "expected type \"grammar\", got {:?}",
                            other
                        )))
                    }
                }
                let variants =
                    variants.ok_or_else(|| serde::de::Error::missing_field("variants"))?;
                Ok(GrammarSampling { variants })
            }
        }
        deserializer.deserialize_struct("GrammarSampling", FIELDS, Visitor)
    }
}

/// Upstream `ConstrainedSamplingConfig` (types.ts:583-591) plus the bare
/// `false` opt-out of `constrainedSampling?: false | ConstrainedSamplingConfig`
/// (types.ts:597). Untagged: `false` maps to `Disabled`, objects dispatch on
/// their `type` discriminant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ConstrainedSampling {
    Disabled(Disabled),
    JsonSchema(JsonSchemaSampling),
    Grammar(GrammarSampling),
}

/// Upstream `Tool` (types.ts:593-598): a tool declaration sent to providers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tool {
    pub name: String,
    pub description: String,
    /// JSON Schema object describing the tool parameters (upstream TypeBox
    /// `TSchema`).
    pub parameters: serde_json::Value,
    /// Upstream `constrainedSampling?: false | ConstrainedSamplingConfig`
    /// (types.ts:597); omitted when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub constrained_sampling: Option<ConstrainedSampling>,
}

/// Upstream `ToolReference` (types.ts:600-602): minimal name-only identifier
/// used by `SystemMessage.toolsRemoved`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolReference {
    pub name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_round_trips_without_constrained_sampling() {
        let fixture = r#"{"name":"bash","description":"Run a shell command","parameters":{"properties":{"command":{"type":"string"}},"type":"object"}}"#;
        let tool: Tool = serde_json::from_str(fixture).unwrap();
        assert_eq!(tool.name, "bash");
        assert_eq!(tool.description, "Run a shell command");
        assert_eq!(tool.constrained_sampling, None);
        assert_eq!(serde_json::to_string(&tool).unwrap(), fixture);
    }

    #[test]
    fn tool_constrained_sampling_false_round_trips() {
        let fixture = r#"{"name":"bash","description":"Run a shell command","parameters":{"type":"object"},"constrainedSampling":false}"#;
        let tool: Tool = serde_json::from_str(fixture).unwrap();
        assert_eq!(
            tool.constrained_sampling,
            Some(ConstrainedSampling::Disabled(Disabled))
        );
        assert_eq!(serde_json::to_string(&tool).unwrap(), fixture);
    }

    #[test]
    fn tool_constrained_sampling_json_schema_prefer_round_trips() {
        let fixture = r#"{"name":"bash","description":"Run a shell command","parameters":{"type":"object"},"constrainedSampling":{"type":"json_schema","strict":"prefer"}}"#;
        let tool: Tool = serde_json::from_str(fixture).unwrap();
        assert_eq!(
            tool.constrained_sampling,
            Some(ConstrainedSampling::JsonSchema(JsonSchemaSampling {
                strict: Strict::Prefer
            }))
        );
        assert_eq!(serde_json::to_string(&tool).unwrap(), fixture);
    }

    #[test]
    fn tool_constrained_sampling_json_schema_require_round_trips() {
        let fixture = r#"{"name":"bash","description":"Run a shell command","parameters":{"type":"object"},"constrainedSampling":{"type":"json_schema","strict":"require"}}"#;
        let tool: Tool = serde_json::from_str(fixture).unwrap();
        assert_eq!(
            tool.constrained_sampling,
            Some(ConstrainedSampling::JsonSchema(JsonSchemaSampling {
                strict: Strict::Require
            }))
        );
        assert_eq!(serde_json::to_string(&tool).unwrap(), fixture);
    }

    #[test]
    fn tool_constrained_sampling_grammar_round_trips_both_variants() {
        let fixture = r#"{"name":"bash","description":"Run a shell command","parameters":{"type":"object"},"constrainedSampling":{"type":"grammar","variants":{"openai_lark":"start: WORD","openai_regex":"[a-z]+"}}}"#;
        let tool: Tool = serde_json::from_str(fixture).unwrap();
        assert_eq!(
            tool.constrained_sampling,
            Some(ConstrainedSampling::Grammar(GrammarSampling {
                variants: GrammarVariants::from([
                    (GrammarFormat::Lark, "start: WORD".to_string()),
                    (GrammarFormat::Regex, "[a-z]+".to_string()),
                ]),
            }))
        );
        assert_eq!(serde_json::to_string(&tool).unwrap(), fixture);
    }

    #[test]
    fn tool_constrained_sampling_grammar_empty_variants_round_trips() {
        let fixture = r#"{"name":"bash","description":"Run a shell command","parameters":{"type":"object"},"constrainedSampling":{"type":"grammar","variants":{}}}"#;
        let tool: Tool = serde_json::from_str(fixture).unwrap();
        assert_eq!(
            tool.constrained_sampling,
            Some(ConstrainedSampling::Grammar(GrammarSampling {
                variants: GrammarVariants::new()
            }))
        );
        assert_eq!(serde_json::to_string(&tool).unwrap(), fixture);
    }

    #[test]
    fn tool_constrained_sampling_rejects_true() {
        let fixture = r#"{"name":"bash","description":"Run a shell command","parameters":{"type":"object"},"constrainedSampling":true}"#;
        assert!(serde_json::from_str::<Tool>(fixture).is_err());
    }

    #[test]
    fn tool_constrained_sampling_rejects_unknown_discriminant() {
        let fixture = r#"{"name":"bash","description":"Run a shell command","parameters":{"type":"object"},"constrainedSampling":{"type":"regex","strict":"prefer"}}"#;
        assert!(serde_json::from_str::<Tool>(fixture).is_err());
    }

    #[test]
    fn tool_constrained_sampling_rejects_unknown_strict() {
        let fixture = r#"{"name":"bash","description":"Run a shell command","parameters":{"type":"object"},"constrainedSampling":{"type":"json_schema","strict":"sometimes"}}"#;
        assert!(serde_json::from_str::<Tool>(fixture).is_err());
    }

    #[test]
    fn tool_reference_round_trips() {
        let fixture = r#"{"name":"weather"}"#;
        let reference: ToolReference = serde_json::from_str(fixture).unwrap();
        assert_eq!(reference.name, "weather");
        assert_eq!(serde_json::to_string(&reference).unwrap(), fixture);
    }
}
