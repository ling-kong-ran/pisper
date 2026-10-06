//! Tests for the ported `coding-agent/src/core/cache-stats.ts`: the upstream
//! `test/cache-stats.test.ts` suite plus byte-level comparisons of the
//! scenario battery captured from the real upstream module
//! (`tests/fixtures/core_oracle/cache_stats.oracle.json`). Floats are compared
//! bit-exactly (both runtimes print shortest-round-trip forms).

use std::sync::Mutex;

use serde_json::Value;

use super::{
    collect_cache_misses, compute_cache_waste, detect_cache_miss, CacheMissHit, ModelPrice,
    ModelPriceSource, SessionEntry, SessionMessage, CACHE_TTL_MS,
};
use crate::ai::types::{AssistantMessage, Usage, UsageCost};
use crate::coding_agent::core::oracle_data;

/// `$0`-cost fixture, matching the upstream test helper.
fn zero_cost() -> UsageCost {
    UsageCost {
        input: 0.0,
        output: 0.0,
        cache_read: 0.0,
        cache_write: 0.0,
        total: 0.0,
    }
}

fn usage(input: u64, cache_read: u64, cache_write: u64, cost: UsageCost) -> Usage {
    Usage {
        input,
        output: 10,
        cache_read,
        cache_write,
        cache_write_1h: None,
        reasoning: None,
        total_tokens: 0,
        cost,
    }
}

fn assistant(options: AssistantOptions) -> AssistantMessage {
    let mut cost = zero_cost();
    if let Some(partial) = options.cost {
        if let Some(input) = partial.input {
            cost.input = input;
        }
        if let Some(cache_read) = partial.cache_read {
            cost.cache_read = cache_read;
        }
        if let Some(cache_write) = partial.cache_write {
            cost.cache_write = cache_write;
        }
    }
    AssistantMessage {
        content: Vec::new(),
        api: "anthropic-messages".to_string(),
        provider: "test".to_string(),
        model: options.model.unwrap_or_else(|| "test-model".to_string()),
        response_model: None,
        response_id: None,
        provider_thinking_level: None,
        diagnostics: None,
        usage: usage(
            options.input.unwrap_or(0),
            options.cache_read.unwrap_or(0),
            options.cache_write.unwrap_or(0),
            cost,
        ),
        stop_reason: crate::ai::types::StopReason::Stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: options.timestamp.unwrap_or(0),
    }
}

#[derive(Default)]
struct AssistantOptions {
    input: Option<u64>,
    cache_read: Option<u64>,
    cache_write: Option<u64>,
    cost: Option<PartialCost>,
    model: Option<String>,
    timestamp: Option<i64>,
}

#[derive(Default, Clone, Copy)]
struct PartialCost {
    input: Option<f64>,
    cache_read: Option<f64>,
    cache_write: Option<f64>,
}

fn entry(message: AssistantMessage) -> SessionEntry {
    SessionEntry::Message(SessionMessage::Assistant(message))
}

/// `$3/M` cache-read fallback (the upstream `models` stub uses $0.3/M).
struct TestModels {
    cache_read: f64,
}

impl ModelPriceSource for TestModels {
    fn get_model(&self, _provider: &str, _model_id: &str) -> Option<ModelPrice> {
        Some(ModelPrice {
            cache_read: self.cache_read,
        })
    }
}

fn models() -> TestModels {
    TestModels { cache_read: 0.3 }
}

// Turn 1: fresh 100k cache write at $3.75/M
fn turn1() -> AssistantMessage {
    assistant(AssistantOptions {
        cache_write: Some(100_000),
        cost: Some(PartialCost {
            cache_write: Some(0.375),
            ..PartialCost::default()
        }),
        timestamp: Some(0),
        ..AssistantOptions::default()
    })
}

// Turn 2: healthy, everything read back at $0.30/M
fn turn2() -> AssistantMessage {
    assistant(AssistantOptions {
        cache_read: Some(100_000),
        cache_write: Some(5_000),
        cost: Some(PartialCost {
            cache_read: Some(0.03),
            cache_write: Some(0.019),
            ..PartialCost::default()
        }),
        timestamp: Some(60_000),
        ..AssistantOptions::default()
    })
}

// ---------------------------------------------------------------------------
// Upstream suite
// ---------------------------------------------------------------------------

#[test]
fn accumulates_missed_tokens_and_cost_across_turns() {
    // Turn 3: full miss, previous 105k prompt re-billed at $3.75/M write
    let turn3 = assistant(AssistantOptions {
        cache_write: Some(110_000),
        cost: Some(PartialCost {
            cache_write: Some(0.4125),
            ..PartialCost::default()
        }),
        timestamp: Some(120_000),
        ..AssistantOptions::default()
    });
    let totals = compute_cache_waste(&[entry(turn1()), entry(turn2()), entry(turn3)], &models());
    assert_eq!(totals.missed_tokens, 105_000);
    // 105k at ($3.75 - $0.30)/M — bit-exact against the oracle float.
    assert_eq!(
        totals.missed_cost.to_bits(),
        0.36224999999999996_f64.to_bits()
    );
}

#[test]
fn counts_nothing_for_healthy_sessions() {
    let totals = compute_cache_waste(&[entry(turn1()), entry(turn2())], &models());
    assert_eq!(totals.missed_tokens, 0);
    assert_eq!(totals.missed_cost, 0.0);
}

#[test]
fn skips_the_turn_after_a_compaction_reset() {
    let after_reset = assistant(AssistantOptions {
        cache_write: Some(20_000),
        cost: Some(PartialCost {
            cache_write: Some(0.075),
            ..PartialCost::default()
        }),
        ..AssistantOptions::default()
    });
    let totals = compute_cache_waste(
        &[entry(turn1()), SessionEntry::Compaction, entry(after_reset)],
        &models(),
    );
    assert_eq!(totals.missed_tokens, 0);
}

#[test]
fn counts_misses_caused_by_model_switches() {
    let other_model = assistant(AssistantOptions {
        cache_write: Some(100_000),
        cost: Some(PartialCost {
            cache_write: Some(0.375),
            ..PartialCost::default()
        }),
        model: Some("other-model".to_string()),
        ..AssistantOptions::default()
    });
    let totals = compute_cache_waste(&[entry(turn1()), entry(other_model)], &models());
    assert_eq!(totals.missed_tokens, 100_000);
    assert_eq!(totals.miss_count, 1);
}

#[test]
fn skips_providers_that_report_no_cache_activity() {
    let a = assistant(AssistantOptions {
        input: Some(100_000),
        ..AssistantOptions::default()
    });
    let b = assistant(AssistantOptions {
        input: Some(110_000),
        ..AssistantOptions::default()
    });
    let totals = compute_cache_waste(&[entry(a), entry(b)], &models());
    assert_eq!(totals.missed_tokens, 0);
}

#[test]
fn maps_counted_misses_to_their_assistant_messages() {
    let miss_turn = assistant(AssistantOptions {
        cache_write: Some(110_000),
        cost: Some(PartialCost {
            cache_write: Some(0.4125),
            ..PartialCost::default()
        }),
        timestamp: Some(120_000),
        ..AssistantOptions::default()
    });
    let misses = collect_cache_misses(
        &[entry(turn1()), entry(turn2()), entry(miss_turn.clone())],
        &models(),
    );
    assert_eq!(misses.len(), 1);
    assert_eq!(misses[0].miss.missed_tokens, 105_000);
    // The hit carries the message it belongs to (identity keyed upstream).
    assert_eq!(misses[0].message, miss_turn);
}

#[test]
fn detects_a_miss_on_a_just_completed_message_with_idle_time() {
    let miss_message = assistant(AssistantOptions {
        cache_write: Some(110_000),
        cost: Some(PartialCost {
            cache_write: Some(0.4125),
            ..PartialCost::default()
        }),
        timestamp: Some(600_000),
        ..AssistantOptions::default()
    });
    let miss = detect_cache_miss(&[entry(turn1()), entry(turn2())], &miss_message, &models())
        .expect("miss expected");
    assert_eq!(miss.missed_tokens, 105_000);
    assert_eq!(
        miss.missed_cost.to_bits(),
        0.36224999999999996_f64.to_bits()
    );
    // 600s - 60s since the previous request
    assert_eq!(miss.idle_ms, 540_000);
    assert!(!miss.model_changed);
}

#[test]
fn flags_model_switches_on_detected_misses() {
    let other_model = assistant(AssistantOptions {
        cache_write: Some(110_000),
        cost: Some(PartialCost {
            cache_write: Some(0.4125),
            ..PartialCost::default()
        }),
        model: Some("other-model".to_string()),
        timestamp: Some(120_000),
        ..AssistantOptions::default()
    });
    let miss =
        detect_cache_miss(&[entry(turn1()), entry(turn2())], &other_model, &models()).unwrap();
    assert_eq!(miss.missed_tokens, 105_000);
    assert!(miss.model_changed);
}

#[test]
fn returns_none_for_healthy_turns() {
    let healthy = assistant(AssistantOptions {
        cache_read: Some(105_000),
        cache_write: Some(2_000),
        cost: Some(PartialCost {
            cache_read: Some(0.0315),
            cache_write: Some(0.0075),
            ..PartialCost::default()
        }),
        timestamp: Some(120_000),
        ..AssistantOptions::default()
    });
    assert!(detect_cache_miss(&[entry(turn1()), entry(turn2())], &healthy, &models()).is_none());
}

#[test]
fn returns_none_for_the_first_turn_of_a_session() {
    let message = turn1();
    assert!(detect_cache_miss(&[], &message, &models()).is_none());
}

// ---------------------------------------------------------------------------
// Oracle scenario battery (bit-exact floats)
// ---------------------------------------------------------------------------

/// Compare two f64s bit-exactly, allowing ≤1 ULP: the oracle JSON is parsed
/// by serde_json, whose (non-`float_roundtrip`) float parser can be off by
/// one ULP; the port's arithmetic itself is strict IEEE and matches node
/// bit-for-bit (the explicit unit tests above pin the std-parsed literals).
fn assert_f64_close(actual: f64, expected: f64, label: &str) {
    let delta = (actual.to_bits() as i64 - expected.to_bits() as i64).unsigned_abs();
    assert!(
        delta <= 1,
        "{label}: {actual} ({:016x}) vs {expected} ({:016x}) differs by {delta} ULP",
        actual.to_bits(),
        expected.to_bits()
    );
}

static ORACLE_CACHE: Mutex<Option<Value>> = Mutex::new(None);

fn oracle_cache() -> Value {
    let mut guard = ORACLE_CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if guard.is_none() {
        *guard = Some(serde_json::from_str(oracle_data::CACHE_STATS).unwrap());
    }
    guard.as_ref().unwrap().clone()
}

fn value_cost(cost: &Value) -> UsageCost {
    UsageCost {
        input: cost["input"].as_f64().unwrap_or(0.0),
        output: cost["output"].as_f64().unwrap_or(0.0),
        cache_read: cost["cacheRead"].as_f64().unwrap_or(0.0),
        cache_write: cost["cacheWrite"].as_f64().unwrap_or(0.0),
        total: cost["total"].as_f64().unwrap_or(0.0),
    }
}

fn value_usage(message: &Value) -> Usage {
    let usage = &message["usage"];
    Usage {
        input: usage["input"].as_u64().unwrap_or(0),
        output: usage["output"].as_u64().unwrap_or(0),
        cache_read: usage["cacheRead"].as_u64().unwrap_or(0),
        cache_write: usage["cacheWrite"].as_u64().unwrap_or(0),
        cache_write_1h: None,
        reasoning: None,
        total_tokens: usage["totalTokens"].as_u64().unwrap_or(0),
        cost: value_cost(&usage["cost"]),
    }
}

fn value_assistant(message: &Value) -> AssistantMessage {
    AssistantMessage {
        content: Vec::new(),
        api: message["api"].as_str().unwrap().to_string(),
        provider: message["provider"].as_str().unwrap().to_string(),
        model: message["model"].as_str().unwrap().to_string(),
        response_model: None,
        response_id: None,
        provider_thinking_level: None,
        diagnostics: None,
        usage: value_usage(message),
        stop_reason: crate::ai::types::StopReason::Stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: message["timestamp"].as_i64().unwrap_or(0),
    }
}

fn value_entry(entry: &Value) -> SessionEntry {
    if entry["type"] == "message" {
        SessionEntry::Message(SessionMessage::Assistant(value_assistant(
            &entry["message"],
        )))
    } else if entry["type"] == "compaction" {
        SessionEntry::Compaction
    } else {
        SessionEntry::BranchSummary
    }
}

fn value_entries(entries: &[Value]) -> Vec<SessionEntry> {
    entries.iter().map(value_entry).collect()
}

#[test]
fn cache_ttl_matches_the_upstream_constant() {
    assert_eq!(CACHE_TTL_MS, 300_000);
    let capture = oracle_cache();
    assert_eq!(CACHE_TTL_MS, capture["cache_ttl_ms"].as_i64().unwrap());
}

#[test]
fn compute_cache_waste_matches_the_oracle_bit_for_bit() {
    let capture = oracle_cache();
    for case in capture["compute"].as_array().unwrap() {
        let entries = value_entries(case["entries"].as_array().unwrap());
        let totals = compute_cache_waste(&entries, &models());
        let expected = &case["totals"];
        assert_eq!(
            totals.missed_tokens,
            expected["missedTokens"].as_u64().unwrap(),
            "{}: missedTokens",
            case["name"].as_str().unwrap()
        );
        assert_f64_close(
            totals.missed_cost,
            expected["missedCost"].as_f64().unwrap(),
            &format!("{}: missedCost", case["name"].as_str().unwrap()),
        );
        assert_eq!(
            totals.miss_count,
            expected["missCount"].as_u64().unwrap(),
            "{}: missCount",
            case["name"].as_str().unwrap()
        );
    }
}

#[test]
fn collect_cache_misses_matches_the_oracle() {
    let capture = oracle_cache();
    for case in capture["collect"].as_array().unwrap() {
        let entries = value_entries(case["entries"].as_array().unwrap());
        let misses: Vec<CacheMissHit> = collect_cache_misses(&entries, &models());
        assert_eq!(
            misses.len(),
            case["size"].as_u64().unwrap() as usize,
            "{}: size",
            case["name"].as_str().unwrap()
        );
        let expected_tokens: Vec<u64> = case["missedTokens"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_u64().unwrap())
            .collect();
        let actual_tokens: Vec<u64> = misses.iter().map(|hit| hit.miss.missed_tokens).collect();
        assert_eq!(
            actual_tokens,
            expected_tokens,
            "{}: missedTokens",
            case["name"].as_str().unwrap()
        );
    }
}

#[test]
fn detect_cache_miss_matches_the_oracle_bit_for_bit() {
    let capture = oracle_cache();
    for case in capture["detect"].as_array().unwrap() {
        let entries = value_entries(case["entries"].as_array().unwrap());
        let message = value_assistant(&case["message"]);
        let miss = detect_cache_miss(&entries, &message, &models());
        let expected = &case["miss"];
        if expected.is_null() {
            assert!(
                miss.is_none(),
                "{}: expected no miss",
                case["name"].as_str().unwrap()
            );
            continue;
        }
        let miss = miss.unwrap();
        assert_eq!(
            miss.missed_tokens,
            expected["missedTokens"].as_u64().unwrap(),
            "{}: missedTokens",
            case["name"].as_str().unwrap()
        );
        assert_f64_close(
            miss.missed_cost,
            expected["missedCost"].as_f64().unwrap(),
            &format!("{}: missedCost", case["name"].as_str().unwrap()),
        );
        assert_eq!(
            miss.idle_ms,
            expected["idleMs"].as_i64().unwrap(),
            "{}: idleMs",
            case["name"].as_str().unwrap()
        );
        assert_eq!(
            miss.model_changed,
            expected["modelChanged"].as_bool().unwrap(),
            "{}: modelChanged",
            case["name"].as_str().unwrap()
        );
    }
}
