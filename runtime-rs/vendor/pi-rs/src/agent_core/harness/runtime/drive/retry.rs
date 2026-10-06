//! Port of `packages/agent/src/harness/runtime/drive/retry.ts` (36 lines):
//! the agent retry-backoff vocabulary — one delay computation, one
//! retry-earliest timestamp, and one abort-aware wait.
//!
//! Substitutions: `retryDelayMs` (upstream `pi-ai utils/retry.ts:111-117`)
//! lands here as a private helper with the literal
//! `DEFAULT_MAX_AGENT_RETRY_DELAY_MS` until the pi-ai retry module ports to
//! `src/ai`; `waitUntil`'s upstream rejection value `signal.reason` is
//! supplied by the caller beside the cancellation token (the drive_pass
//! token/reason separation); `retry_not_before` takes `now_ms` explicitly
//! instead of the `Date.now()` default parameter.

use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

/// JavaScript `Number.MAX_SAFE_INTEGER` — both clamps below are exact
/// upstream literals (`retry.ts:9`, `retry.ts` via `retryDelayMs`).
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

/// Upstream `DEFAULT_MAX_AGENT_RETRY_DELAY_MS` (`pi-ai utils/retry.ts:111`),
/// now delegated to the ported `crate::ai::retry` copy.
pub use crate::ai::retry::DEFAULT_MAX_AGENT_RETRY_DELAY_MS;

/// Upstream `Pick<RetryPolicy, "baseDelayMs" | "maxAgentDelayMs">` — the
/// only retry-policy fields the drive delay math reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryDelayPolicy {
    /// Per-attempt delay is `baseDelayMs * 2^(attempt-1)` before the cap.
    pub base_delay_ms: i64,
    /// Caps each computed delay; `None` uses
    /// [`DEFAULT_MAX_AGENT_RETRY_DELAY_MS`].
    pub max_agent_delay_ms: Option<i64>,
}

/// Upstream `retryDelayMs` (`pi-ai utils/retry.ts:113-117`), including the
/// `Number.isSafeInteger` clamp: a non-integer or out-of-range product
/// becomes `MAX_SAFE_INTEGER` before the cap narrows it.
fn retry_delay_ms(policy: &RetryDelayPolicy, attempt: i64) -> i64 {
    let full = crate::ai::retry::RetryPolicy {
        enabled: true,
        max_retries: u32::MAX,
        base_delay_ms: policy.base_delay_ms.max(0) as u64,
        max_agent_delay_ms: policy.max_agent_delay_ms.map(|cap| cap.max(0) as u64),
    };
    let attempt = attempt.clamp(0, u32::MAX as i64) as u32;
    crate::ai::retry::retry_delay_ms(&full, attempt) as i64
}

/// Upstream `retryNotBefore` (`retry.ts:3-10`): the earliest wall-clock
/// timestamp the next attempt may start. `now_ms` is explicit (upstream
/// default `Date.now()`).
pub fn retry_not_before(policy: &RetryDelayPolicy, attempt: i64, now_ms: i64) -> i64 {
    let sum = now_ms as f64 + retry_delay_ms(policy, attempt) as f64;
    if sum.is_finite() && sum.fract() == 0.0 && sum.abs() <= MAX_SAFE_INTEGER {
        sum as i64
    } else {
        MAX_SAFE_INTEGER as i64
    }
}

/// Upstream `waitUntil` (`retry.ts:12-36`): resolve at `not_before_ms`,
/// reject early on abort. The upstream timer re-arms with the remaining
/// delay capped at `2_147_483_647` ms; the sleep/select loop keeps that
/// re-check and cap. An already-aborted signal rejects before any time
/// check, matching the upstream `if (signal.aborted) onAbort()` order.
pub async fn wait_until(
    not_before_ms: i64,
    signal: CancellationToken,
    abort_reason: Arc<dyn Error + Send + Sync>,
) -> anyhow::Result<()> {
    if signal.is_cancelled() {
        anyhow::bail!("{}", abort_reason);
    }
    loop {
        let remaining = not_before_ms - crate::ai::now_ms();
        if remaining <= 0 {
            return Ok(());
        }
        let capped = remaining.min(2_147_483_647) as u64;
        tokio::select! {
            () = signal.cancelled() => {
                anyhow::bail!("{}", abort_reason);
            }
            () = tokio::time::sleep(Duration::from_millis(capped)) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(base: i64, cap: Option<i64>) -> RetryDelayPolicy {
        RetryDelayPolicy {
            base_delay_ms: base,
            max_agent_delay_ms: cap,
        }
    }

    fn reason() -> Arc<dyn Error + Send + Sync> {
        Arc::new(std::io::Error::other("aborted"))
    }

    #[test]
    fn retry_delay_matches_exponential_backoff() {
        let base = policy(500, None);
        assert_eq!(retry_delay_ms(&base, 1), 500);
        assert_eq!(retry_delay_ms(&base, 2), 1_000);
        assert_eq!(retry_delay_ms(&base, 3), 2_000);
        // attempt 0 clamps the exponent to 0 (Math.max(0, attempt-1)).
        assert_eq!(retry_delay_ms(&base, 0), 500);
        assert_eq!(retry_delay_ms(&base, -5), 500);
        // Default cap applies (60s) even when the product explodes.
        assert_eq!(retry_delay_ms(&base, 40), 60_000);
    }

    #[test]
    fn retry_delay_caps_and_clamps_like_safe_integers() {
        // Explicit cap wins under the default.
        assert_eq!(retry_delay_ms(&policy(500, Some(700)), 3), 700);
        // Non-safe-integer product clamps to MAX_SAFE_INTEGER, then the cap.
        let huge = policy(9_007_199_254_740_991, Some(60_000));
        assert_eq!(retry_delay_ms(&huge, 2), 60_000);
        // No explicit cap: the clamped product still meets the default 60s
        // cap; MAX_SAFE_INTEGER only survives under a cap that large.
        let uncapped = policy(9_007_199_254_740_991, Some(9_007_199_254_740_991));
        assert_eq!(retry_delay_ms(&uncapped, 2), 9_007_199_254_740_991);
        // Zero base stays zero under any cap.
        assert_eq!(retry_delay_ms(&policy(0, None), 7), 0);
    }

    #[test]
    fn retry_not_before_sums_and_clamps() {
        let base = policy(1_000, None);
        // attempt 2 → delay 2000 (base * 2^1).
        assert_eq!(retry_not_before(&base, 2, 50_000), 52_000);
        // now + delay overflowing the safe range clamps the SUM (retry.ts:9),
        // not the delay.
        assert_eq!(
            retry_not_before(&base, 2, 9_007_199_254_740_991),
            9_007_199_254_740_991
        );
    }

    #[tokio::test]
    async fn wait_until_resolves_when_already_past() {
        wait_until(0, CancellationToken::new(), reason())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn wait_until_sleeps_then_resolves() {
        let start = std::time::Instant::now();
        wait_until(crate::ai::now_ms() + 30, CancellationToken::new(), reason())
            .await
            .unwrap();
        assert!(start.elapsed() >= Duration::from_millis(25));
    }

    #[tokio::test]
    async fn wait_until_rejects_on_abort_during_sleep() {
        let token = CancellationToken::new();
        let waiter = tokio::spawn(wait_until(
            crate::ai::now_ms() + 60_000,
            token.clone(),
            reason(),
        ));
        tokio::time::sleep(Duration::from_millis(10)).await;
        token.cancel();
        let error = waiter.await.unwrap().unwrap_err();
        assert!(error.to_string().contains("aborted"));
    }

    #[tokio::test]
    async fn wait_until_rejects_immediately_when_already_aborted() {
        // Even a past notBefore must lose to the abort check (retry.ts:32-34).
        let token = CancellationToken::new();
        token.cancel();
        assert!(wait_until(0, token, reason()).await.is_err());
    }
}
