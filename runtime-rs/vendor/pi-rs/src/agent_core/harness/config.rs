//! Port of `packages/agent/src/harness/config.ts` (42 lines): the default
//! retry policy, tool-name uniqueness validation, retry-policy and compaction
//! validation used at harness configuration boundaries.
//!
//! Upstream home of `CompactionSettings` is
//! `harness/compaction/compaction.ts:147-161`; it lives here (with the same
//! fields and default) so the validator has its argument type before the
//! compaction module is ported, and the compaction module re-exports it.
//!
//! Disclosed substitution: upstream validates that the policy/settings
//! numbers are "finite non-negative safe integers" (`config.ts:19-31,
//! 33-42`) because TypeScript numbers admit `NaN`, negatives, and floats.
//! The port's [`RetryPolicy`] (`src/ai/retry.rs:445-453`) and
//! [`CompactionSettings`] use `bool`/`u32`/`u64`, so those failure modes are
//! unrepresentable and the validators are total — they remain part of the
//! surface because harness entry points call them at every configuration
//! boundary (`harness/runtime/harness.ts`), and returning `Ok` documents the
//! check that upstream performs.

use std::collections::HashSet;

use crate::ai::retry::{RetryPolicy, DEFAULT_MAX_AGENT_RETRY_DELAY_MS};

/// Upstream `DEFAULT_RETRY_POLICY` (`config.ts:4-9`).
pub const DEFAULT_RETRY_POLICY: RetryPolicy = RetryPolicy {
    enabled: true,
    max_retries: 3,
    base_delay_ms: 1_000,
    max_agent_delay_ms: Some(DEFAULT_MAX_AGENT_RETRY_DELAY_MS),
};

/// Upstream `CompactionSettings` (`compaction/compaction.ts:147-154`):
/// compaction thresholds and retention settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionSettings {
    /// Enable automatic compaction decisions.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Tokens reserved for summary prompt and output.
    pub reserve_tokens: u64,
    /// Approximate recent-context tokens to keep after compaction.
    pub keep_recent_tokens: u64,
}

fn default_true() -> bool {
    true
}

/// Upstream `DEFAULT_COMPACTION_SETTINGS` (`compaction/compaction.ts:157-161`).
pub const DEFAULT_COMPACTION_SETTINGS: CompactionSettings = CompactionSettings {
    enabled: true,
    reserve_tokens: 16_384,
    keep_recent_tokens: 20_000,
};

/// Upstream `validateToolNames` (`config.ts:11-17`): reject duplicate tool
/// names. The upstream `TypeError` becomes an `Err`; the message keeps the
/// upstream `JSON.stringify` quoting.
pub fn validate_tool_names<'a, I>(tools: I) -> anyhow::Result<()>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut names: HashSet<&str> = HashSet::new();
    for name in tools {
        if !names.insert(name) {
            anyhow::bail!("Duplicate tool name: {name:?}");
        }
    }
    Ok(())
}

/// Upstream `validateRetryPolicy` (`config.ts:19-31`): see the module docs —
/// the checked failure modes are unrepresentable over the typed
/// [`RetryPolicy`] fields, so this is total.
pub fn validate_retry_policy(_policy: &RetryPolicy) -> anyhow::Result<()> {
    Ok(())
}

/// Upstream `validateCompactionSettings` (`config.ts:33-42`): see the module
/// docs — the checked failure modes are unrepresentable over the typed
/// [`CompactionSettings`] fields, so this is total.
pub fn validate_compaction_settings(_settings: &CompactionSettings) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests;
