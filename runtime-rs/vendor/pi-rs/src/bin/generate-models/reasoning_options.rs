//! Ports of `scripts/models-dev-reasoning-options.ts` and
//! `scripts/openrouter-reasoning-options.ts`.

use std::collections::HashMap;

use serde_json::Value as Json;

use crate::json::{obj, JsObj, Jv};

/// `ModelsDevReasoningOption` reduced to what `getEffortThinkingLevelMap`
/// reads: `toggle`, `effort` (with its value list), or `budget_tokens`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ReasoningOption {
    Toggle,
    Effort(Vec<Option<String>>),
    BudgetTokens,
}

/// Parse a models.dev `reasoning_options` array (`m.reasoning_options ?? []`
/// upstream passes `Option` straight through; absent arrays are empty).
pub(crate) fn parse_reasoning_options(value: Option<&Json>) -> Vec<ReasoningOption> {
    let Some(entries) = value.and_then(Json::as_array) else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| match entry.get("type").and_then(Json::as_str) {
            Some("toggle") => Some(ReasoningOption::Toggle),
            Some("effort") => {
                let values = entry
                    .get("values")
                    .and_then(Json::as_array)
                    .map(|values| {
                        values
                            .iter()
                            .map(|value| value.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                Some(ReasoningOption::Effort(values))
            }
            Some("budget_tokens") => Some(ReasoningOption::BudgetTokens),
            _ => None,
        })
        .collect()
}

const THINKING_LEVELS: [&str; 6] = ["minimal", "low", "medium", "high", "xhigh", "max"];

/// `getEffortThinkingLevelMap` (models-dev-reasoning-options.ts:18-30).
/// Values without a pi equivalent (`default`, JSON `null`) are intentionally
/// omitted. Key order is the canonical `off, minimal, low, medium, high,
/// xhigh, max` insertion order.
pub(crate) fn effort_thinking_level_map(options: &[ReasoningOption]) -> Option<JsObj> {
    let supported: Vec<Option<&str>> = options
        .iter()
        .filter_map(|option| match option {
            ReasoningOption::Effort(values) => Some(values),
            _ => None,
        })
        .flatten()
        .map(|value| value.as_deref())
        .collect();
    if supported.is_empty() {
        return None;
    }
    let has = |level: &str| supported.contains(&Some(level));
    if !THINKING_LEVELS.iter().any(|level| has(level)) && !has("none") {
        return None;
    }
    let mut map = JsObj::new();
    map.set("off", if has("none") { Jv::s("none") } else { Jv::Null });
    for level in THINKING_LEVELS {
        map.set(level, if has(level) { Jv::s(level) } else { Jv::Null });
    }
    Some(map)
}

/// `getOpenRouterThinkingLevelMap` (openrouter-reasoning-options.ts:12-23).
pub(crate) fn openrouter_thinking_level_map(reasoning: Option<&Json>) -> Option<JsObj> {
    let reasoning = reasoning?;
    let supported_efforts: Vec<Option<String>> = reasoning
        .get("supported_efforts")
        .and_then(Json::as_array)
        .map(|values| {
            values
                .iter()
                .map(|value| value.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let mandatory = reasoning.get("mandatory").and_then(Json::as_bool) == Some(true);
    if supported_efforts.is_empty() {
        return mandatory.then(|| obj! { "off" => Jv::Null });
    }
    // OpenRouter's supported_efforts uses the same effort values as models.dev
    // reasoning_options, so both sources share the same conversion.
    let map = effort_thinking_level_map(&[ReasoningOption::Effort(supported_efforts)])?;
    let mut map = map;
    map.set("off", if mandatory { Jv::Null } else { Jv::s("none") });
    Some(map)
}

/// Upstream `recordModelsDevReasoningOptions`: the `provider:id` → options map
/// the pipeline reads back in `applyModelsDevReasoningOptionMetadata`.
pub(crate) fn record_reasoning_options(
    recorded: &mut HashMap<String, Vec<ReasoningOption>>,
    provider: &str,
    id: &str,
    source: &Json,
) {
    if let Some(options) = source.get("reasoning_options") {
        if !options.is_null() {
            recorded.insert(
                format!("{provider}:{id}"),
                parse_reasoning_options(Some(options)),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn effort_map_uses_canonical_key_order() {
        let options = parse_reasoning_options(Some(&json!([
            {"type": "toggle"},
            {"type": "effort", "values": ["none", "low", "medium", "high", "xhigh", "default", null]},
        ])));
        let map = effort_thinking_level_map(&options).expect("map");
        assert_eq!(
            crate::json::serialize_json(&Jv::Obj(map)),
            "{\"off\":\"none\",\"minimal\":null,\"low\":\"low\",\"medium\":\"medium\",\"high\":\"high\",\"xhigh\":\"xhigh\",\"max\":null}\n"
        );
    }

    #[test]
    fn effort_map_requires_a_usable_value() {
        let default_only = parse_reasoning_options(Some(&json!([
            {"type": "effort", "values": ["default"]},
        ])));
        assert_eq!(effort_thinking_level_map(&default_only), None);
        let empty: Vec<ReasoningOption> = Vec::new();
        assert_eq!(effort_thinking_level_map(&empty), None);
    }

    #[test]
    fn openrouter_map_overrides_off_in_place() {
        let map = openrouter_thinking_level_map(Some(&json!({
            "supported_efforts": ["low", "medium", "high"],
        })))
        .expect("map");
        assert_eq!(
            crate::json::serialize_json(&Jv::Obj(map)),
            "{\"off\":\"none\",\"minimal\":null,\"low\":\"low\",\"medium\":\"medium\",\"high\":\"high\",\"xhigh\":null,\"max\":null}\n"
        );
        // mandatory flips `off` to null without moving the key.
        let map = openrouter_thinking_level_map(Some(&json!({
            "mandatory": true,
            "supported_efforts": ["low", "medium", "high"],
        })))
        .expect("map");
        assert!(map.get("off") == Some(&Jv::Null));
        assert_eq!(map.get("low"), Some(&Jv::s("low")));
    }

    #[test]
    fn openrouter_without_efforts_needs_mandatory() {
        assert_eq!(
            openrouter_thinking_level_map(Some(&json!({"mandatory": true}))),
            None.or(Some(obj! { "off" => Jv::Null }))
        );
        assert_eq!(openrouter_thinking_level_map(Some(&json!({}))), None);
        assert_eq!(openrouter_thinking_level_map(None), None);
    }
}
