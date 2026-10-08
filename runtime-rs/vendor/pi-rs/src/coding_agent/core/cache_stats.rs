//! Port of upstream `coding-agent/src/core/cache-stats.ts`.
//!
//! Prompt-cache waste accounting over a session's entries: detects turns
//! whose prompt tokens should have been cache reads but were re-billed, and
//! aggregates tokens / extra cost / counts.
//!
//! Seams (upstream imports modules outside this slice):
//! - `SessionEntry` (upstream `./session-manager.ts`, not yet ported): the
//!   scan consumes the local [`SessionEntry`] seam enum carrying exactly the
//!   fields `scan()` reads (entry kind + the assistant message). When the
//!   session-manager slice lands, a `From` bridge can adapt the real type.
//! - `ModelPriceSource` keeps the upstream shape: a minimal pricing lookup
//!   satisfied by ModelRuntime; the port narrows the returned record to the
//!   `cost.cacheRead` field the algorithm reads ([`ModelPrice`]).
//! - Upstream keys `collectCacheMisses` results by assistant-message object
//!   identity (`Map<AssistantMessage, CacheMiss>`); Rust has no object
//!   identity, so [`collect_cache_misses`] returns entries in scan order with
//!   the owning entry index and a clone of the message (disclosed).

use crate::ai::types::AssistantMessage;

/// Prompt-cache TTL: idle gaps longer than this are worth mentioning as the
/// likely cause of a miss. Anthropic's default cache TTL is 5 minutes.
pub const CACHE_TTL_MS: i64 = 5 * 60 * 1000;

/// Per-turn misses at or below this are cache breakpoint granularity noise.
const NOISE_FLOOR_TOKENS: u64 = 1024;

/// A counted cache miss on a single assistant message.
#[derive(Debug, Clone, PartialEq)]
pub struct CacheMiss {
    /// Prompt tokens that were in the previous turn's prompt but not read
    /// from cache.
    pub missed_tokens: u64,
    /// Extra dollars paid vs. a full cache hit; 0 when pricing is unknown.
    pub missed_cost: f64,
    /// Milliseconds since the previous request (which last refreshed the
    /// cache).
    pub idle_ms: i64,
    /// True when the model changed relative to the previous request.
    pub model_changed: bool,
}

/// Cumulative cache waste across a session.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CacheWasteTotals {
    pub missed_tokens: u64,
    pub missed_cost: f64,
    /// Number of counted misses (turns above the noise floor).
    pub miss_count: u64,
}

/// Minimal pricing lookup, satisfied by ModelRuntime. Cost is $/million
/// tokens; only the `cacheRead` rate is consulted.
pub trait ModelPriceSource {
    fn get_model(&self, provider: &str, model_id: &str) -> Option<ModelPrice>;
}

/// The `cost.cacheRead` slice of the upstream pricing record.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPrice {
    pub cache_read: f64,
}

/// The session entry shapes `scan()` distinguishes (upstream `SessionEntry`).
/// The Assistant variant is intrinsically the largest payload (same reason
/// `AgentMessage` carries the same allow); boxing it would add indirection at
/// every scan step for no functional gain.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum SessionEntry {
    /// Upstream `type: "compaction"`.
    Compaction,
    /// Upstream `type: "branch_summary"`.
    BranchSummary,
    /// Upstream `type: "message"`.
    Message(SessionMessage),
}

/// The message half of a `message` entry. Only assistant messages affect the
/// scan; every other role is carried opaquely (same size-difference allow as
/// [`SessionEntry`]).
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum SessionMessage {
    Assistant(AssistantMessage),
    /// user/system/toolResult messages (ignored by the scan).
    Other,
}

/// The last request seen by the scan; everything in its prompt should be
/// cached.
#[derive(Debug, Clone)]
struct PreviousRequest {
    prompt_tokens: u64,
    model_key: String,
    timestamp: i64,
    /// Sticky: some earlier request in this scan segment reported cache
    /// activity. Distinguishes a total miss on a cache-read-only provider
    /// (OpenAI-style, writes unreported) from a provider that never reports
    /// caching at all.
    reported_cache: bool,
}

fn message_key(message: &AssistantMessage) -> String {
    format!("{}/{}", message.provider, message.model)
}

/// Compute the cache miss for one assistant message relative to the previous
/// request. Returns `None` when nothing is counted: first turn, after a
/// reset, no cache activity ever reported (provider without cache support),
/// or miss below the noise floor.
fn detect_miss(
    prev: Option<&PreviousRequest>,
    message: &AssistantMessage,
    models: &dyn ModelPriceSource,
) -> Option<CacheMiss> {
    let usage = &message.usage;
    let prompt_tokens = usage.input + usage.cache_read + usage.cache_write;
    // A zero-cache turn only counts when cache activity was reported before:
    // on cache-read-only providers that is a total miss, while on providers
    // that never report caching it means nothing.
    let prev = prev?;
    if prompt_tokens == 0 || (usage.cache_read + usage.cache_write == 0 && !prev.reported_cache) {
        return None;
    }

    let missed_tokens = prev
        .prompt_tokens
        .min(prompt_tokens)
        .saturating_sub(usage.cache_read);
    if missed_tokens <= NOISE_FLOOR_TOKENS {
        return None;
    }

    // Extra cost = missed tokens billed at the actual paid rate
    // (input/cacheWrite, incl. write premium) instead of the cache-read rate.
    // Missed tokens can only land in the input or cacheWrite buckets, so the
    // paid rate comes straight from this message's own cost breakdown.
    let paid_tokens = usage.input + usage.cache_write;
    let paid_per_token = if paid_tokens > 0 {
        (usage.cost.input + usage.cost.cache_write) / paid_tokens as f64
    } else {
        0.0
    };
    let read_per_token = if usage.cache_read > 0 {
        usage.cost.cache_read / usage.cache_read as f64
    } else {
        models
            .get_model(&message.provider, &message.model)
            .map(|price| price.cache_read)
            .unwrap_or(0.0)
            / 1_000_000.0
    };

    Some(CacheMiss {
        missed_tokens,
        missed_cost: missed_tokens as f64 * (paid_per_token - read_per_token).max(0.0),
        idle_ms: (message.timestamp - prev.timestamp).max(0),
        model_changed: message_key(message) != prev.model_key,
    })
}

fn as_previous_request(
    message: &AssistantMessage,
    reported_cache: bool,
) -> Option<PreviousRequest> {
    let usage = &message.usage;
    let prompt_tokens = usage.input + usage.cache_read + usage.cache_write;
    if prompt_tokens == 0 {
        return None;
    }
    Some(PreviousRequest {
        prompt_tokens,
        model_key: message_key(message),
        timestamp: message.timestamp,
        reported_cache: reported_cache || usage.cache_read + usage.cache_write > 0,
    })
}

struct ScanResult {
    prev: Option<PreviousRequest>,
    totals: CacheWasteTotals,
    /// (entry index, message, miss) in scan order.
    misses: Vec<(usize, AssistantMessage, CacheMiss)>,
}

fn scan(entries: &[SessionEntry], models: &dyn ModelPriceSource) -> ScanResult {
    let mut prev: Option<PreviousRequest> = None;
    let totals = CacheWasteTotals::default();
    let mut misses = Vec::new();

    for (index, entry) in entries.iter().enumerate() {
        match entry {
            SessionEntry::Compaction | SessionEntry::BranchSummary => {
                // The context legitimately changed; the next turn's prompt is
                // new content, not re-billed content. Model switches are NOT
                // exempt: they re-bill the full prompt and should be counted.
                prev = None;
            }
            SessionEntry::Message(SessionMessage::Assistant(message)) => {
                if let Some(miss) = detect_miss(prev.as_ref(), message, models) {
                    misses.push((index, message.clone(), miss));
                }
                // `prev = asPreviousRequest(entry.message, prev?.reportedCache
                // ?? false) ?? prev`: keep the old request when the new one
                // has no prompt tokens.
                let reported = prev.as_ref().map(|p| p.reported_cache).unwrap_or(false);
                prev = as_previous_request(message, reported).or(prev);
            }
            SessionEntry::Message(SessionMessage::Other) => {}
        }
    }

    let totals = misses.iter().fold(totals, |mut totals, (_, _, miss)| {
        totals.missed_tokens += miss.missed_tokens;
        totals.missed_cost += miss.missed_cost;
        totals.miss_count += 1;
        totals
    });
    ScanResult {
        prev,
        totals,
        misses,
    }
}

/// Cumulative cache waste across a session: prompt tokens that should have
/// been cache reads (they were in the previous turn's prompt) but were
/// re-billed.
pub fn compute_cache_waste(
    entries: &[SessionEntry],
    models: &dyn ModelPriceSource,
) -> CacheWasteTotals {
    scan(entries, models).totals
}

/// All counted cache misses across a session, in scan order, with the owning
/// entry index and the assistant message that paid for them. Used to
/// re-derive transcript notices when rebuilding the chat from entries
/// (resume, post-compaction rebuild).
#[derive(Debug, Clone)]
pub struct CacheMissHit {
    /// Index of the `message` entry carrying the assistant message.
    pub entry_index: usize,
    pub message: AssistantMessage,
    pub miss: CacheMiss,
}

pub fn collect_cache_misses(
    entries: &[SessionEntry],
    models: &dyn ModelPriceSource,
) -> Vec<CacheMissHit> {
    scan(entries, models)
        .misses
        .into_iter()
        .map(|(entry_index, message, miss)| CacheMissHit {
            entry_index,
            message,
            miss,
        })
        .collect()
}

/// Detect a cache miss on a just-completed assistant message. `entries` must
/// not yet contain `message` (message_end fires before persistence).
pub fn detect_cache_miss(
    entries: &[SessionEntry],
    message: &AssistantMessage,
    models: &dyn ModelPriceSource,
) -> Option<CacheMiss> {
    detect_miss(scan(entries, models).prev.as_ref(), message, models)
}

#[cfg(test)]
#[path = "cache_stats_tests.rs"]
mod tests;
