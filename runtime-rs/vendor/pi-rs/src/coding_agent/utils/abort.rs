//! Port of upstream `coding-agent/src/utils/abort.ts`.
//!
//! Abort-signal helpers. The port's `AbortSignal` equivalent is
//! [`tokio_util::sync::CancellationToken`] (the same mapping used across this
//! crate, e.g. `ai::retry`), and the abort reason is the fixed
//! [`AbortError`] ("The operation was aborted", JS `name: "AbortError"`):
//! cancellation tokens carry no `signal.reason`, so the `abortReason`
//! fallback branch is the only observable one (disclosed divergence, same
//! convention as the `REQUEST_ABORTED` errors in `ai::api`).
//!
//! Divergence: upstream `raceWithAbortSignal` detaches the abandoned promise
//! (`void operation.catch(() => {})`) — a JS promise cannot be cancelled, so
//! the abandoned operation keeps running. Rust futures are cancellable, so
//! when the abort wins the abandoned future is dropped at the select point.

use std::fmt;
use std::future::Future;

use tokio_util::sync::CancellationToken;

/// Message of the abort error produced when a signal has no reason
/// (upstream `new Error("The operation was aborted")` with
/// `name = "AbortError"`).
pub const OPERATION_ABORTED_MESSAGE: &str = "The operation was aborted";

/// The upstream `AbortError` fallback reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbortError;

impl fmt::Display for AbortError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(OPERATION_ABORTED_MESSAGE)
    }
}

impl std::error::Error for AbortError {}

/// Normalize an optional public signal without imposing a deadline
/// (upstream `operationSignal`): an absent signal becomes a fresh,
/// never-cancelled token (upstream `new AbortController().signal`).
pub fn operation_signal(signal: Option<&CancellationToken>) -> CancellationToken {
    signal.cloned().unwrap_or_default()
}

/// Stop waiting on abort while observing the abandoned operation through
/// settlement (upstream `raceWithAbortSignal`).
///
/// - No signal: the operation result passes through.
/// - Already-cancelled signal: rejects with [`AbortError`] immediately.
/// - Otherwise: first settlement wins — the operation value on success, the
///   abort error if the token is cancelled first.
pub async fn race_with_abort_signal<T, F>(
    operation: F,
    signal: Option<&CancellationToken>,
) -> Result<T, AbortError>
where
    F: Future<Output = T>,
{
    let Some(signal) = signal else {
        return Ok(operation.await);
    };
    if signal.is_cancelled() {
        // upstream: `void operation.catch(() => {}); return
        // Promise.reject(abortReason(signal))`.
        return Err(AbortError);
    }

    tokio::select! {
        value = operation => Ok(value),
        _ = signal.cancelled() => Err(AbortError),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn operation_signal_returns_a_fresh_token_when_absent() {
        let token = operation_signal(None);
        assert!(!token.is_cancelled());
    }

    #[tokio::test]
    async fn operation_signal_passes_the_given_token_through() {
        let given = CancellationToken::new();
        given.cancel();
        let token = operation_signal(Some(&given));
        assert!(token.is_cancelled());
        // Cloned tokens share cancellation state with the original.
        assert!(!CancellationToken::new().is_cancelled());
        let shared = CancellationToken::new();
        let mirrored = operation_signal(Some(&shared));
        shared.cancel();
        assert!(mirrored.is_cancelled());
    }

    #[tokio::test]
    async fn race_passes_through_without_a_signal() {
        let value = race_with_abort_signal(async { 41 }, None).await;
        assert_eq!(value, Ok(41));
    }

    #[tokio::test]
    async fn race_rejects_immediately_when_the_signal_is_already_cancelled() {
        let signal = CancellationToken::new();
        signal.cancel();
        let result = race_with_abort_signal(async { 1 }, Some(&signal)).await;
        assert_eq!(result.unwrap_err().to_string(), OPERATION_ABORTED_MESSAGE);
    }

    #[tokio::test]
    async fn race_resolves_when_the_operation_wins() {
        let signal = CancellationToken::new();
        let value = race_with_abort_signal(async { "value" }, Some(&signal)).await;
        assert_eq!(value, Ok("value"));
        assert!(!signal.is_cancelled());
    }

    #[tokio::test(start_paused = true)]
    async fn race_rejects_when_the_signal_wins_mid_flight() {
        let signal = CancellationToken::new();
        let raced =
            race_with_abort_signal(tokio::time::sleep(Duration::from_secs(60)), Some(&signal));
        tokio::pin!(raced);
        // Advance the paused clock, then abort while the operation is parked.
        tokio::time::sleep(Duration::from_secs(1)).await;
        signal.cancel();
        let result = raced.as_mut().await;
        assert_eq!(result.unwrap_err(), AbortError);
        assert!(signal.is_cancelled());
    }

    #[tokio::test]
    async fn abort_error_display_matches_upstream_message() {
        assert_eq!(AbortError.to_string(), "The operation was aborted");
    }
}
