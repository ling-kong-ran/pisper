//! Port of `packages/ai/src/utils/overflow.ts` (186 lines): context-overflow
//! detection across provider error-message grammars, silent z.ai style
//! overflow, and Xiaomi MiMo style filled-context length stops.

use regex::Regex;
use std::sync::LazyLock;

use crate::ai::types::message::AssistantMessage;
use crate::ai::types::primitives::StopReason;

struct CaseInsensitive(&'static str);

impl CaseInsensitive {
    fn regex(&self) -> Regex {
        Regex::new(&format!("(?i){}", self.0)).expect("valid overflow pattern")
    }
}

/// Upstream `OVERFLOW_PATTERNS` (`overflow.ts:37-67`), order preserved.
/// The z.ai delta widens the Anthropic pattern to `prompt (?:is )?too long`
/// and adds the CN-endpoint `prompt exceeds max length` grammar.
static OVERFLOW_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"prompt (?:is )?too long",
        r"prompt exceeds max length",
        r"request_too_large",
        r"input is too long for requested model",
        r"exceeds the context window",
        r"exceeds (?:the )?(?:model'?s )?maximum context length(?: of [\d,]+ tokens?|\s*\([\d,]+\))",
        r"input token count.*exceeds the maximum",
        r"maximum prompt length is \d+",
        r"reduce the length of the messages",
        r"maximum context length is \d+ tokens",
        r"exceeds (?:the )?maximum allowed input length of [\d,]+ tokens?",
        r"input \(\d+ tokens\) is longer than the model'?s context length \(\d+ tokens\)",
        r"exceeds the limit of \d+",
        r"exceeds the available context size",
        r"greater than the context length",
        r"context window exceeds limit",
        r"exceeded model token limit",
        r"too large for model with \d+ maximum context length",
        r"prompt has [\d,]+ tokens?, but the configured context size is [\d,]+ tokens?",
        r"model_context_window_exceeded",
        r"prompt too long; exceeded (?:max )?context length",
        r"range of input length should be",
        r"context[_ ]length[_ ]exceeded",
        r"too many tokens",
        r"token limit exceeded",
    ]
    .iter()
    .map(|pattern| CaseInsensitive(pattern).regex())
    .collect()
});

/// Upstream `CEREBRAS_BODYLESS_OVERFLOW_PATTERN` (`overflow.ts:68`).
static CEREBRAS_BODYLESS_OVERFLOW_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| CaseInsensitive(r"^4(?:00|13)\s*(?:status code)?\s*\(no body\)").regex());

/// Upstream `NON_OVERFLOW_PATTERNS` (`overflow.ts:78-82`): throttling and
/// rate-limit errors are excluded even when they match an overflow pattern.
static NON_OVERFLOW_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"^(Throttling error|Service unavailable):",
        r"rate limit",
        r"too many requests",
    ]
    .iter()
    .map(|pattern| CaseInsensitive(pattern).regex())
    .collect()
});

/// Upstream `isContextOverflow` (`overflow.ts:145-182`): error-message
/// patterns, silent z.ai style usage overflow, and Xiaomi MiMo style
/// filled-context length stops.
pub fn is_context_overflow(message: &AssistantMessage, context_window: Option<u64>) -> bool {
    // Case 1: known error-message grammars, minus throttling/rate-limit.
    if message.stop_reason == StopReason::Error {
        if let Some(error_message) = &message.error_message {
            let is_non_overflow = NON_OVERFLOW_PATTERNS
                .iter()
                .any(|pattern| pattern.is_match(error_message));
            if !is_non_overflow {
                if OVERFLOW_PATTERNS
                    .iter()
                    .any(|pattern| pattern.is_match(error_message))
                {
                    return true;
                }
                if message.provider == "cerebras"
                    && CEREBRAS_BODYLESS_OVERFLOW_PATTERN.is_match(error_message)
                {
                    return true;
                }
            }
        }
    }
    let Some(context_window) = context_window else {
        return false;
    };
    let input_tokens = message.usage.input + message.usage.cache_read;
    // Case 2: silent overflow (z.ai style) — success but usage exceeds the
    // context window.
    if message.stop_reason == StopReason::Stop && input_tokens > context_window {
        return true;
    }
    // Case 3: filled context + zero output (Xiaomi MiMo style).
    if message.stop_reason == StopReason::Length
        && message.usage.output == 0
        && input_tokens as f64 >= context_window as f64 * 0.99
    {
        return true;
    }
    false
}

/// Upstream `isRecoverableLength` (`overflow.ts:184-186`): a length stop that
/// ended below the intended output limit may retry once after compaction.
pub fn is_recoverable_length(message: &AssistantMessage, desired_max_output: u64) -> bool {
    message.stop_reason == StopReason::Length
        && desired_max_output > 0
        && message.usage.output < desired_max_output
}

/// Upstream `getOverflowPatterns` (`overflow.ts:188-190`): the raw patterns
/// for tooling.
pub fn get_overflow_patterns() -> Vec<String> {
    OVERFLOW_PATTERNS
        .iter()
        .map(|pattern| pattern.as_str().to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::types::primitives::{Usage, UsageCost};

    fn message(
        provider: &str,
        stop_reason: StopReason,
        error_message: Option<&str>,
    ) -> AssistantMessage {
        serde_json::from_value(serde_json::json!({
            "role": "assistant", "content": [], "api": "faux", "provider": provider,
            "model": "faux-1", "stopReason": stop_reason,
            "usage": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0,
                "totalTokens": 0, "cost": {"input": 0, "output": 0, "cacheRead": 0,
                "cacheWrite": 0, "total": 0}},
            "timestamp": 1,
            "errorMessage": error_message
        }))
        .unwrap()
    }

    fn usage_with(input: u64, output: u64, cache_read: u64) -> Usage {
        Usage {
            input,
            output,
            cache_read,
            cache_write: 0,
            cache_write_1h: None,
            reasoning: None,
            total_tokens: input + output,
            cost: UsageCost::default(),
        }
    }

    #[test]
    fn detects_provider_error_grammars() {
        let cases = [
            ("anthropic", "prompt is too long: 213462 tokens > 200000 maximum"),
            ("anthropic", r#"413 {"error":{"type":"request_too_large"}}"#),
            ("openai", "Your input exceeds the context window of this model"),
            ("google", "The input token count (1196265) exceeds the maximum number of tokens allowed (1048575)"),
            ("groq", "Please reduce the length of the messages or completion"),
            ("ollama", "prompt too long; exceeded max context length by 1200 tokens"),
        ];
        for (provider, error_message) in cases {
            let message = message(provider, StopReason::Error, Some(error_message));
            assert!(
                is_context_overflow(&message, None),
                "{provider} overflow not detected"
            );
        }
    }

    #[test]
    fn excludes_rate_limit_and_keeps_non_error_stops() {
        let throttled = message(
            "bedrock",
            StopReason::Error,
            Some("Throttling error: Too many tokens, please wait before trying again."),
        );
        assert!(!is_context_overflow(&throttled, None));
        let stopped = message("openai", StopReason::Stop, None);
        assert!(!is_context_overflow(&stopped, None));
    }

    #[test]
    fn silent_and_length_overflow_use_the_context_window() {
        // z.ai style silent overflow: success with usage above the window.
        let mut silent = message("z.ai", StopReason::Stop, None);
        silent.usage = usage_with(110_000, 10, 0);
        assert!(is_context_overflow(&silent, Some(100_000)));
        assert!(!is_context_overflow(&silent, None));
        // Xiaomi MiMo style: length stop with zero output and a filled window
        // (input >= 99% of the window).
        let mut filled = message("mimo", StopReason::Length, None);
        filled.usage = usage_with(99_500, 0, 0);
        assert!(is_context_overflow(&filled, Some(100_000)));
        // Below the 99% line it does not fire.
        filled.usage = usage_with(90_000, 0, 0);
        assert!(!is_context_overflow(&filled, Some(100_000)));
    }

    #[test]
    fn recoverable_length_requires_headroom() {
        let mut message = message("openai", StopReason::Length, None);
        message.usage = usage_with(10, 50, 0);
        assert!(is_recoverable_length(&message, 100));
        assert!(!is_recoverable_length(&message, 0));
        assert!(!is_recoverable_length(&message, 50));
        message.stop_reason = StopReason::Stop;
        assert!(!is_recoverable_length(&message, 100));
    }
}
