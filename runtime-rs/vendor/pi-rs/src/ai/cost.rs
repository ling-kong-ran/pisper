//! Upstream `calculateCost` (`packages/ai/src/models.ts:900-919`): request
//! dollar cost from the model's rates, assigned into `usage.cost` in place
//! like upstream mutates the passed `Usage`.
//!
//! Rates are dollars per million tokens. When the model declares pricing
//! tiers, the tier-selection token basis is `input + cacheRead + cacheWrite`
//! and the highest tier whose `inputTokensAbove` is STRICTLY below the basis
//! (upstream comparison is `>`, not `>=`) applies its rates to the ENTIRE
//! request. The 1h cache-write portion (`cacheWrite1h`, Anthropic only) is
//! priced at 2x the input rate; the rest at the `cacheWrite` rate.

use crate::ai::types::{AnyModel, Model, ModelCostTier, Usage};

/// Upstream `calculateCost` (models.ts:900-919): fill `usage.cost` in place
/// from `model.cost`. Operation order matches upstream exactly so dollar
/// amounts are bit-identical: rate/1M x tokens per bucket, then the four
/// buckets are summed for `total`. Upstream types the model as `AnyModel`
/// since the unified catalog; the chat-typed entry point is kept for the
/// existing call sites and both delegate to [`calculate_cost_with_rates`].
pub fn calculate_cost(model: &Model, usage: &mut Usage) {
    calculate_cost_with_rates(&model.cost, usage);
}

/// [`calculate_cost`] over any catalog entry (upstream
/// `calculateCost(model: AnyModel, usage)`).
pub fn calculate_cost_any(model: &AnyModel, usage: &mut Usage) {
    calculate_cost_with_rates(model.cost(), usage);
}

fn calculate_cost_with_rates(cost: &crate::ai::types::ModelCost, usage: &mut Usage) {
    // Upstream: inputTokens = usage.input + usage.cacheRead + usage.cacheWrite.
    // Saturating: hostile usage payloads must not panic the debug overflow check.
    let input_tokens = usage
        .input
        .saturating_add(usage.cache_read)
        .saturating_add(usage.cache_write);
    let mut matched: Option<&ModelCostTier> = None;
    for tier in cost.tiers.iter().flatten() {
        if input_tokens > tier.input_tokens_above
            && matched.is_none_or(|m| tier.input_tokens_above > m.input_tokens_above)
        {
            matched = Some(tier);
        }
    }
    let (input_rate, output_rate, cache_read_rate, cache_write_rate) = match matched {
        Some(tier) => (tier.input, tier.output, tier.cache_read, tier.cache_write),
        None => (cost.input, cost.output, cost.cache_read, cost.cache_write),
    };

    // Anthropic charges 2x base input for 1h cache writes (upstream comment).
    // `cacheWrite1h` is a subset of `cacheWrite` by definition; saturate the
    // subtraction rather than panic if a server ever reports it larger.
    let long_write = usage.cache_write_1h.unwrap_or(0);
    let short_write = usage.cache_write.saturating_sub(long_write);
    usage.cost.input = (input_rate / 1_000_000.0) * usage.input as f64;
    usage.cost.output = (output_rate / 1_000_000.0) * usage.output as f64;
    usage.cost.cache_read = (cache_read_rate / 1_000_000.0) * usage.cache_read as f64;
    usage.cost.cache_write = (cache_write_rate * short_write as f64
        + input_rate * 2.0 * long_write as f64)
        / 1_000_000.0;
    usage.cost.total =
        usage.cost.input + usage.cost.output + usage.cost.cache_read + usage.cost.cache_write;
}

#[cfg(test)]
mod tests {
    use super::calculate_cost;
    use crate::ai::types::{Model, ModelCost, ModelCostTier, ModelInput, Usage, UsageCost};

    fn model_with_cost(cost: ModelCost) -> Model {
        Model {
            r#type: None,
            prompt_cache: None,
            input_limits: None,
            id: "test-model".to_string(),
            name: "Test Model".to_string(),
            api: "openai-completions".to_string(),
            provider: "openai".to_string(),
            base_url: "https://api.example.com".to_string(),
            reasoning: false,
            thinking_level_map: None,
            input: vec![ModelInput::Text],
            cost,
            context_window: 100000,
            max_tokens: 4096,
            sampling_params: None,
            sampling_params_by_thinking_level: None,
            headers: None,
            compat: None,
        }
    }

    fn cost(input: f64, output: f64, cache_read: f64, cache_write: f64) -> ModelCost {
        ModelCost {
            input,
            output,
            cache_read,
            cache_write,
            tiers: None,
        }
    }

    fn tier(
        input: f64,
        output: f64,
        cache_read: f64,
        cache_write: f64,
        above: u64,
    ) -> ModelCostTier {
        ModelCostTier {
            input,
            output,
            cache_read,
            cache_write,
            input_tokens_above: above,
        }
    }

    fn make_usage(input: u64, output: u64, cache_read: u64, cache_write: u64) -> Usage {
        Usage {
            input,
            output,
            cache_read,
            cache_write,
            cache_write_1h: None,
            reasoning: None,
            total_tokens: input + output + cache_read + cache_write,
            cost: UsageCost::default(),
        }
    }

    /// Upstream models-runtime.test.ts "applies request-wide pricing tiers"
    /// short case: flat rates price each bucket at rate x tokens / 1M.
    #[test]
    fn flat_rates_cost_rate_times_tokens_per_million() {
        let model = model_with_cost(cost(5.0, 30.0, 0.5, 6.25));
        let mut usage = make_usage(200000, 100000, 72000, 0);

        calculate_cost(&model, &mut usage);

        assert_eq!(usage.cost.input, 1.0);
        assert_eq!(usage.cost.output, 3.0);
        assert_eq!(usage.cost.cache_read, 0.036);
        assert_eq!(usage.cost.cache_write, 0.0);
        assert_eq!(usage.cost.total, 1.0 + 3.0 + 0.036 + 0.0);
    }

    /// Upstream models-runtime.test.ts long case: once
    /// `input + cacheRead + cacheWrite` is strictly above `inputTokensAbove`,
    /// the tier's rates apply to the ENTIRE request.
    #[test]
    fn tier_applies_when_input_tokens_strictly_exceed_threshold() {
        let mut model_cost = cost(5.0, 30.0, 0.5, 6.25);
        model_cost.tiers = Some(vec![tier(10.0, 45.0, 1.0, 12.5, 272000)]);
        let model = model_with_cost(model_cost);
        // 200000 + 72000 + 1 = 272001 > 272000.
        let mut usage = make_usage(200000, 100000, 72000, 1);

        calculate_cost(&model, &mut usage);

        assert_eq!(usage.cost.input, 2.0);
        assert_eq!(usage.cost.output, 4.5);
        assert_eq!(usage.cost.cache_read, 0.072);
        assert_eq!(usage.cost.cache_write, 0.0000125);
    }

    /// Upstream models-runtime.test.ts short case: at exactly the threshold
    /// the base rates still apply — the comparison is `>`, never `>=`.
    #[test]
    fn tier_boundary_is_strictly_above_threshold() {
        let mut model_cost = cost(5.0, 30.0, 0.5, 6.25);
        model_cost.tiers = Some(vec![tier(10.0, 45.0, 1.0, 12.5, 272000)]);
        let model = model_with_cost(model_cost);
        // 200000 + 72000 + 0 = 272000 == threshold.
        let mut usage = make_usage(200000, 100000, 72000, 0);

        calculate_cost(&model, &mut usage);

        assert_eq!(usage.cost.input, 1.0);
        assert_eq!(usage.cost.output, 3.0);
        assert_eq!(usage.cost.cache_read, 0.036);
    }

    /// Among qualifying tiers the one with the highest threshold wins,
    /// regardless of declaration order; thresholds below the basis but not
    /// maximal are ignored. (Rate x token products for these invented
    /// numbers are not exactly representable, so compare with a tolerance —
    /// the exactness of upstream's own numbers is pinned by the tier tests
    /// above.)
    #[test]
    fn highest_matching_tier_wins() {
        const EPS: f64 = 1e-12;
        let mut model_cost = cost(5.0, 30.0, 0.5, 6.25);
        model_cost.tiers = Some(vec![
            tier(10.0, 45.0, 1.0, 12.5, 272000),
            tier(8.0, 40.0, 0.9, 11.0, 200000),
        ]);
        let model = model_with_cost(model_cost);

        // 300000 > 272000: the higher tier applies even though it is first.
        let mut usage = make_usage(300000, 1000, 0, 0);
        calculate_cost(&model, &mut usage);
        assert!((usage.cost.input - 3.0).abs() < EPS, "{:?}", usage.cost);
        assert!((usage.cost.output - 0.045).abs() < EPS, "{:?}", usage.cost);

        // 250000 > 200000 but not > 272000: the middle tier applies.
        let mut usage = make_usage(250000, 1000, 0, 0);
        calculate_cost(&model, &mut usage);
        assert!((usage.cost.input - 2.0).abs() < EPS, "{:?}", usage.cost);
        assert!((usage.cost.output - 0.04).abs() < EPS, "{:?}", usage.cost);
    }

    /// Zero rates (unset catalog cost) produce zero cost on every bucket.
    #[test]
    fn zero_rates_cost_zero() {
        let model = model_with_cost(ModelCost::default());
        let mut usage = make_usage(1000, 500, 200, 300);

        calculate_cost(&model, &mut usage);

        assert_eq!(usage.cost.input, 0.0);
        assert_eq!(usage.cost.output, 0.0);
        assert_eq!(usage.cost.cache_read, 0.0);
        assert_eq!(usage.cost.cache_write, 0.0);
        assert_eq!(usage.cost.total, 0.0);
    }

    /// The 1h cache-write portion costs 2x the input rate; the 5m portion
    /// costs the cacheWrite rate (upstream anthropic-cache-write-1h-cost
    /// test: 600k at 6.25/Mtok + 400k at 10/Mtok = 7.75).
    #[test]
    fn cache_write_1h_portion_costs_twice_the_input_rate() {
        let model = model_with_cost(cost(5.0, 30.0, 0.5, 6.25));

        let mut usage = make_usage(100, 5, 0, 1000000);
        usage.cache_write_1h = Some(400000);
        calculate_cost(&model, &mut usage);
        assert_eq!(usage.cost.cache_write, 7.75);

        // No breakdown reported: the whole cacheWrite is priced at the 5m rate.
        let mut usage = make_usage(100, 5, 0, 1000000);
        calculate_cost(&model, &mut usage);
        assert_eq!(usage.cost.cache_write, 6.25);
    }
}
