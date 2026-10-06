//! Port of upstream `coding-agent/src/core/defaults.ts`.
//!
//! Two constants only. Upstream types them with the pi-agent-core
//! `ThinkingLevel` union (`"off" | "minimal" | "low" | "medium" | "high" |
//! "xhigh" | "max"`), which is `agent_core::types::ThinkingLevel`
//! (= `ai::types::ModelThinkingLevel`) in this crate.

use crate::agent_core::types::ThinkingLevel;

/// Upstream `DEFAULT_THINKING_LEVEL` (`"medium"`).
pub const DEFAULT_THINKING_LEVEL: ThinkingLevel = ThinkingLevel::Medium;

/// Upstream `THINKING_LEVEL_OPTIONS`, in the same order.
pub const THINKING_LEVEL_OPTIONS: &[ThinkingLevel] = &[
    ThinkingLevel::Off,
    ThinkingLevel::Minimal,
    ThinkingLevel::Low,
    ThinkingLevel::Medium,
    ThinkingLevel::High,
    ThinkingLevel::Xhigh,
    ThinkingLevel::Max,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_upstream_table() {
        assert_eq!(DEFAULT_THINKING_LEVEL, ThinkingLevel::Medium);
        assert_eq!(
            THINKING_LEVEL_OPTIONS,
            &[
                ThinkingLevel::Off,
                ThinkingLevel::Minimal,
                ThinkingLevel::Low,
                ThinkingLevel::Medium,
                ThinkingLevel::High,
                ThinkingLevel::Xhigh,
                ThinkingLevel::Max,
            ]
        );
        // Every option round-trips through the serde wire spelling upstream
        // uses in settings files.
        let spellings: Vec<String> = THINKING_LEVEL_OPTIONS
            .iter()
            .map(|level| {
                serde_json::to_string(level)
                    .unwrap()
                    .trim_matches('"')
                    .to_string()
            })
            .collect();
        assert_eq!(
            spellings,
            ["off", "minimal", "low", "medium", "high", "xhigh", "max"]
        );
    }
}
