//! Port of upstream `coding-agent/src/core/usage-totals.ts` (the v0.99.x
//! extraction of the shared usage helpers): the running totals accumulator,
//! the sum of two usages, and the per-model cost breakdown behind the
//! `/usage` report.
//!
//! `combineUsage` is the single implementation for the crate — compaction's
//! private copy was the baseline duplicate and now delegates here.

use std::collections::HashMap;

use crate::ai::types::primitives::Usage;
use crate::coding_agent::session_manager::SessionEntry;

/// Upstream `UsageTotals`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct UsageTotals {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub cost: f64,
}

pub fn create_usage_totals() -> UsageTotals {
    UsageTotals::default()
}

pub fn add_usage_to_totals(totals: &mut UsageTotals, usage: &Usage) {
    totals.input += usage.input;
    totals.output += usage.output;
    totals.cache_read += usage.cache_read;
    totals.cache_write += usage.cache_write;
    totals.cost += usage.cost.total;
}

/// Sum of two usages, keeping the optional token splits when either side
/// reports them.
pub fn combine_usage(first: Usage, second: Usage) -> Usage {
    Usage {
        input: first.input.saturating_add(second.input),
        output: first.output.saturating_add(second.output),
        cache_read: first.cache_read.saturating_add(second.cache_read),
        cache_write: first.cache_write.saturating_add(second.cache_write),
        cache_write_1h: match (first.cache_write_1h, second.cache_write_1h) {
            (None, None) => None,
            (left, right) => Some(left.unwrap_or(0).saturating_add(right.unwrap_or(0))),
        },
        reasoning: match (first.reasoning, second.reasoning) {
            (None, None) => None,
            (left, right) => Some(left.unwrap_or(0).saturating_add(right.unwrap_or(0))),
        },
        total_tokens: first.total_tokens.saturating_add(second.total_tokens),
        cost: crate::ai::types::primitives::UsageCost {
            input: first.cost.input + second.cost.input,
            output: first.cost.output + second.cost.output,
            cache_read: first.cost.cache_read + second.cost.cache_read,
            cache_write: first.cost.cache_write + second.cost.cache_write,
            total: first.cost.total + second.cost.total,
        },
    }
}

/// Upstream `UsageCostBreakdownEntry`.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageCostBreakdownEntry {
    pub key: String,
    pub cost: f64,
    pub tokens: u64,
}

/// Group model-attributed usage by model and all other usage into a separate
/// bucket. Ties keep insertion order (stable sort upstream), descending by
/// cost.
pub fn get_usage_cost_breakdown(entries: &[SessionEntry]) -> Vec<UsageCostBreakdownEntry> {
    // Ordered accumulation: insertion order of first appearance.
    let mut order: Vec<String> = Vec::new();
    let mut totals_by_key: HashMap<String, UsageTotals> = HashMap::new();

    for entry in entries {
        let (key, usage): (String, Option<&Usage>) = match entry {
            SessionEntry::Message(message) => match &message.message {
                crate::agent_core::types::AgentMessage::Assistant(assistant) => (
                    format!(
                        "{}/{}",
                        assistant.provider,
                        assistant
                            .response_model
                            .as_deref()
                            .unwrap_or(&assistant.model)
                    ),
                    Some(&assistant.usage),
                ),
                crate::agent_core::types::AgentMessage::ToolResult(tool_result) => {
                    match &tool_result.usage {
                        Some(usage) => ("Tools/summaries".to_string(), Some(usage)),
                        None => continue,
                    }
                }
                _ => continue,
            },
            // Upstream: `entry.type === "usage"` attributes the entry to
            // `provider/model` directly.
            SessionEntry::Usage(usage_entry) => (
                format!("{}/{}", usage_entry.provider, usage_entry.model),
                Some(&usage_entry.usage),
            ),
            SessionEntry::BranchSummary(summary) => match &summary.usage {
                Some(usage) => ("Tools/summaries".to_string(), Some(usage)),
                None => continue,
            },
            SessionEntry::Compaction(compaction) => match &compaction.usage {
                Some(usage) => ("Tools/summaries".to_string(), Some(usage)),
                None => continue,
            },
            _ => continue,
        };
        let Some(usage) = usage else { continue };
        if !totals_by_key.contains_key(&key) {
            order.push(key.clone());
            totals_by_key.insert(key.clone(), create_usage_totals());
        }
        add_usage_to_totals(totals_by_key.get_mut(&key).expect("inserted above"), usage);
    }

    let mut result: Vec<UsageCostBreakdownEntry> = order
        .into_iter()
        .filter_map(|key| {
            let totals = totals_by_key.get(&key)?;
            Some(UsageCostBreakdownEntry {
                tokens: totals.input + totals.output + totals.cache_read + totals.cache_write,
                key,
                cost: totals.cost,
            })
        })
        .filter(|entry| entry.cost > 0.0 || entry.tokens > 0)
        .collect();
    // V8's Array.prototype.sort is stable; `sort_by` in Rust is stable too.
    result.sort_by(|a, b| b.cost.total_cmp(&a.cost));
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_core::types::AgentMessage;
    use crate::ai::types::message::AssistantMessage;
    use crate::ai::types::primitives::{Usage, UsageCost};
    use crate::coding_agent::session_manager::{BranchSummaryEntry, MessageEntry};

    fn usage(input: u64, output: u64, cost_total: f64) -> Usage {
        Usage {
            input,
            output,
            cache_read: 0,
            cache_write: 0,
            cache_write_1h: None,
            reasoning: None,
            total_tokens: input + output,
            cost: UsageCost {
                input: cost_total / 2.0,
                output: 0.0,
                cache_read: 0.0,
                cache_write: 0.0,
                total: cost_total,
            },
        }
    }

    fn assistant(
        provider: &str,
        model: &str,
        response_model: Option<&str>,
        usage: Usage,
    ) -> SessionEntry {
        SessionEntry::Message(MessageEntry {
            id: format!("m-{provider}-{model}"),
            parent_id: None,
            timestamp: "2026-09-30T00:00:00.000Z".to_string(),
            message: AgentMessage::Assistant(AssistantMessage {
                content: Vec::new(),
                api: "anthropic-messages".to_string(),
                provider: provider.to_string(),
                model: model.to_string(),
                response_model: response_model.map(str::to_string),
                response_id: None,
                provider_thinking_level: None,
                diagnostics: None,
                usage,
                stop_reason: crate::ai::types::primitives::StopReason::Stop,
                deferred: None,
                error_message: None,
                raw_stop_reason: None,
                end_turn: None,
                timestamp: 0,
            }),
        })
    }

    #[test]
    fn combine_usage_sums_and_keeps_optional_splits() {
        let base = usage(10, 5, 0.5);
        let merged = combine_usage(base, base);
        assert_eq!(merged.input, 20);
        assert_eq!(merged.output, 10);
        assert_eq!(merged.total_tokens, 30);
        assert_eq!(merged.cost.total, 1.0);
        assert_eq!(merged.cache_write_1h, None);
        assert_eq!(merged.reasoning, None);

        let mut with_optional = base;
        with_optional.cache_write_1h = Some(3);
        let merged = combine_usage(with_optional, base);
        assert_eq!(merged.cache_write_1h, Some(3));
        let merged = combine_usage(with_optional, with_optional);
        assert_eq!(merged.cache_write_1h, Some(6));
    }

    #[test]
    fn breakdown_groups_by_model_and_buckets_tools() {
        let entries = vec![
            assistant("anthropic", "claude", None, usage(100, 10, 1.0)),
            assistant("anthropic", "claude", None, usage(50, 5, 0.5)),
            assistant("openai", "gpt", Some("gpt-real"), usage(10, 1, 2.0)),
            SessionEntry::BranchSummary(BranchSummaryEntry {
                id: "b1".to_string(),
                parent_id: None,
                timestamp: String::new(),
                from_id: "a1".to_string(),
                summary: "summary".to_string(),
                details: None,
                usage: Some(usage(1, 1, 0.25)),
                from_hook: None,
            }),
        ];
        let breakdown = get_usage_cost_breakdown(&entries);
        assert_eq!(breakdown.len(), 3);
        // Descending by cost: gpt-real (2.0), anthropic/claude (1.5), tools (0.25).
        assert_eq!(breakdown[0].key, "openai/gpt-real");
        assert_eq!(breakdown[1].key, "anthropic/claude");
        assert_eq!(breakdown[2].key, "Tools/summaries");
        assert_eq!(breakdown[1].tokens, 165);
        // Zero-cost zero-token entries drop out.
        let only_zero = vec![assistant("p", "m", None, usage(0, 0, 0.0))];
        assert!(get_usage_cost_breakdown(&only_zero).is_empty());
    }
}
