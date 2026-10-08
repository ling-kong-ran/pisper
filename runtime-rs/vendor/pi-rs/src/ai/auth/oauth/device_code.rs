//! The generic OAuth device-code poll engine ported from upstream
//! `packages/ai/src/auth/oauth/device-code.ts`: [`poll_device_code_flow`]
//! drives a caller-supplied poll closure on the RFC 8628 polling schedule —
//! immediate first poll (or a deliberate first-poll delay), `slow_down`
//! backoff, an expiry deadline and abortable waits — and
//! [`abortable_sleep`] is the shared cancellation-aware sleep (the GitHub
//! Copilot flow reuses it for its rate-limit retry backoff).
//!
//! Consumed by the OpenAI Codex (`openai_codex`), GitHub Copilot
//! (`github_copilot`) and xAI (`xai`) flows; T4/T5 initially carried private
//! copies of this engine, absorbed here once the plan's generic device-code
//! task landed (their tests stay green against the shared engine).
//!
//! Port notes (disclosed divergences):
//! - Poll deadlines and schedules are measured on tokio's clock
//!   ([`tokio::time::Instant`]): wall-clock in production, pause-able in
//!   tests. Upstream uses `Date.now()`.
//! - Cancellation maps to [`AuthError::Cancelled`] everywhere upstream throws
//!   `Error("Login cancelled")` (port contract: interaction-signal aborts are
//!   never wrapped); poll failures surface as
//!   [`AuthError::Operation`] with the upstream message.
//! - The upstream options bag (`intervalSeconds?`, `expiresInSeconds?`,
//!   `waitBeforeFirstPoll?`) becomes three leading parameters; `None`
//!   interval falls back to the RFC 8628 5-second default, `None` expiry
//!   means no deadline.
//! - A `NaN` interval would produce `NaN` timings in JS (`setTimeout(NaN)`
//!   fires immediately); the port floors it through the 1-second minimum
//!   like any non-positive interval. No port caller can pass one (every
//!   flow validates its response fields first).

use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::ai::auth::types::AuthError;

/// device-code.ts `MINIMUM_INTERVAL_MS`.
pub(crate) const MINIMUM_INTERVAL_MS: u64 = 1000;

/// device-code.ts `DEFAULT_POLL_INTERVAL_SECONDS` (RFC 8628 section 3.2: if
/// the authorization server omits `interval`, the client must use 5 seconds).
pub(crate) const DEFAULT_POLL_INTERVAL_SECONDS: f64 = 5.0;

/// device-code.ts `SLOW_DOWN_INTERVAL_INCREMENT_MS` (RFC 8628 section 3.5:
/// `slow_down` means the polling interval must increase by 5 seconds).
pub(crate) const SLOW_DOWN_INTERVAL_INCREMENT_MS: u64 = 5000;

/// device-code.ts `TIMEOUT_MESSAGE`.
pub(crate) const TIMEOUT_MESSAGE: &str = "Device flow timed out";

/// device-code.ts `SLOW_DOWN_TIMEOUT_MESSAGE`.
pub(crate) const SLOW_DOWN_TIMEOUT_MESSAGE: &str = "Device flow timed out after one or more \
     slow_down responses. This is often caused by clock drift in WSL or VM environments. Please \
     sync or restart the VM clock and try again.";

/// Incomplete poll results (device-code.ts
/// `OAuthDeviceCodeIncompletePollResult`); [`PollOutcome::Complete`] carries
/// the value (upstream `{ status: "complete", value }`).
pub(crate) enum PollOutcome<T> {
    Complete(T),
    Pending,
    /// RFC 8628 section 3.5; the server may supply its own new interval.
    SlowDown(Option<f64>),
    Failed(String),
}

/// `Math.max(MINIMUM_INTERVAL_MS, Math.floor(seconds * 1000))`.
fn interval_duration(seconds: f64) -> Duration {
    Duration::from_millis(((seconds * 1000.0).floor() as u64).max(MINIMUM_INTERVAL_MS))
}

/// device-code.ts `abortableSleep`: resolves after `duration`, or
/// `Err(Cancelled)` when the signal fires first (or already has).
pub(crate) async fn abortable_sleep(
    duration: Duration,
    signal: &CancellationToken,
) -> Result<(), AuthError> {
    if signal.is_cancelled() {
        return Err(AuthError::Cancelled);
    }
    tokio::select! {
        biased;
        _ = signal.cancelled() => Err(AuthError::Cancelled),
        _ = tokio::time::sleep(duration) => Ok(()),
    }
}

/// device-code.ts `pollOAuthDeviceCodeFlow`: optionally wait before the first
/// poll, then poll every `interval_seconds` (default 5, minimum 1s);
/// `slow_down` bumps the interval by 5s or adopts a finite positive
/// server-provided interval, until the deadline expires (`Device flow timed
/// out`, or the slow-down clock-drift message after any slow_down) or the
/// signal fires. `None` `expires_in_seconds` polls without a deadline.
pub(crate) async fn poll_device_code_flow<T, F, Fut>(
    interval_seconds: Option<f64>,
    expires_in_seconds: Option<f64>,
    wait_before_first_poll: bool,
    signal: &CancellationToken,
    mut poll: F,
) -> Result<T, AuthError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<PollOutcome<T>, AuthError>>,
{
    let deadline = expires_in_seconds.map(|seconds| {
        tokio::time::Instant::now() + Duration::from_millis((seconds * 1000.0) as u64)
    });
    // `Math.max(1000, Math.floor((intervalSeconds ?? 5) * 1000))`.
    let mut interval_ms =
        interval_duration(interval_seconds.unwrap_or(DEFAULT_POLL_INTERVAL_SECONDS));
    let mut slow_down_responses = 0u32;

    if wait_before_first_poll {
        match deadline {
            Some(deadline) => {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                if !remaining.is_zero() {
                    abortable_sleep(interval_ms.min(remaining), signal).await?;
                }
            }
            None => abortable_sleep(interval_ms, signal).await?,
        }
    }

    loop {
        if let Some(deadline) = deadline {
            if tokio::time::Instant::now() >= deadline {
                break;
            }
        }
        if signal.is_cancelled() {
            return Err(AuthError::Cancelled);
        }

        match poll().await? {
            PollOutcome::Complete(value) => return Ok(value),
            PollOutcome::Failed(message) => return Err(AuthError::Operation(message)),
            PollOutcome::SlowDown(server_interval) => {
                slow_down_responses += 1;
                // Use the server-provided interval when given (GitHub reports
                // the new required minimum in `interval`); trusting only a
                // client-tracked value risks polling early forever under
                // WSL/VM clock drift. Otherwise apply RFC 8628 section 3.5:
                // increase by 5 seconds.
                interval_ms = match server_interval {
                    Some(seconds) if seconds.is_finite() && seconds > 0.0 => {
                        interval_duration(seconds)
                    }
                    _ => Duration::from_millis(
                        (interval_ms.as_millis() as u64 + SLOW_DOWN_INTERVAL_INCREMENT_MS)
                            .max(MINIMUM_INTERVAL_MS),
                    ),
                };
            }
            PollOutcome::Pending => {}
        }

        let sleep_for = match deadline {
            Some(deadline) => {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                if remaining.is_zero() {
                    break;
                }
                interval_ms.min(remaining)
            }
            None => interval_ms,
        };
        abortable_sleep(sleep_for, signal).await?;
    }

    Err(AuthError::Operation(
        if slow_down_responses > 0 {
            SLOW_DOWN_TIMEOUT_MESSAGE
        } else {
            TIMEOUT_MESSAGE
        }
        .to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use tokio_util::sync::CancellationToken;

    use super::*;

    type EngineOutcome = PollOutcome<()>;

    /// Engine poll closure serving the queued outcomes and recording the poll
    /// instants (the oracle tests' `pollTimes` recorder).
    fn recording_poll(
        times: Arc<Mutex<Vec<tokio::time::Instant>>>,
        outcomes: Arc<Mutex<VecDeque<EngineOutcome>>>,
    ) -> impl FnMut() -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<EngineOutcome, AuthError>> + Send>,
    > {
        move || {
            let times = Arc::clone(&times);
            let outcomes = Arc::clone(&outcomes);
            Box::pin(async move {
                times.lock().unwrap().push(tokio::time::Instant::now());
                Ok(outcomes
                    .lock()
                    .unwrap()
                    .pop_front()
                    .unwrap_or(EngineOutcome::Pending))
            })
        }
    }

    fn queue(outcomes: Vec<EngineOutcome>) -> Arc<Mutex<VecDeque<EngineOutcome>>> {
        Arc::new(Mutex::new(VecDeque::from(outcomes)))
    }

    /// Oracle: "polls immediately and returns the completed value"
    /// (oauth-device-code.test.ts, interval 2s, first poll pending).
    #[tokio::test(start_paused = true)]
    async fn polls_immediately_then_at_each_interval() {
        let token = CancellationToken::new();
        let times = Arc::new(Mutex::new(Vec::new()));
        let outcomes = queue(vec![
            PollOutcome::<()>::Pending,
            PollOutcome::<()>::Complete(()),
        ]);
        let start = tokio::time::Instant::now();

        poll_device_code_flow(
            Some(2.0),
            Some(30.0),
            false,
            &token,
            recording_poll(Arc::clone(&times), outcomes),
        )
        .await
        .unwrap();

        assert_eq!(
            times.lock().unwrap().clone(),
            vec![start, start + Duration::from_secs(2)]
        );
    }

    /// Oracle: "can wait before the first poll".
    #[tokio::test(start_paused = true)]
    async fn waits_before_the_first_poll_when_asked() {
        let token = CancellationToken::new();
        let times = Arc::new(Mutex::new(Vec::new()));
        let outcomes = queue(vec![PollOutcome::<()>::Complete(())]);
        let start = tokio::time::Instant::now();

        poll_device_code_flow(
            Some(2.0),
            Some(30.0),
            true,
            &token,
            recording_poll(Arc::clone(&times), outcomes),
        )
        .await
        .unwrap();

        assert_eq!(
            times.lock().unwrap().clone(),
            vec![start + Duration::from_secs(2)]
        );
    }

    /// Oracle: "increases the interval by 5 seconds after slow_down without a
    /// server interval" (2s → 7s).
    #[tokio::test(start_paused = true)]
    async fn slow_down_without_a_server_interval_adds_five_seconds() {
        let token = CancellationToken::new();
        let times = Arc::new(Mutex::new(Vec::new()));
        let outcomes = queue(vec![
            PollOutcome::<()>::SlowDown(None),
            PollOutcome::<()>::Complete(()),
        ]);
        let start = tokio::time::Instant::now();

        poll_device_code_flow(
            Some(2.0),
            Some(900.0),
            false,
            &token,
            recording_poll(Arc::clone(&times), outcomes),
        )
        .await
        .unwrap();

        assert_eq!(
            times.lock().unwrap().clone(),
            vec![start, start + Duration::from_secs(7)]
        );
    }

    /// Oracle: "honors a server-provided slow_down interval".
    #[tokio::test(start_paused = true)]
    async fn slow_down_adopts_the_server_interval() {
        let token = CancellationToken::new();
        let times = Arc::new(Mutex::new(Vec::new()));
        let outcomes = queue(vec![
            PollOutcome::<()>::SlowDown(Some(30.0)),
            PollOutcome::<()>::Complete(()),
        ]);
        let start = tokio::time::Instant::now();

        poll_device_code_flow(
            Some(2.0),
            Some(900.0),
            false,
            &token,
            recording_poll(Arc::clone(&times), outcomes),
        )
        .await
        .unwrap();

        assert_eq!(
            times.lock().unwrap().clone(),
            vec![start, start + Duration::from_secs(30)]
        );
    }

    /// Oracle: "cancels an in-flight wait" (upstream rejects "Login
    /// cancelled"; the port surfaces [`AuthError::Cancelled`]). The first
    /// poll runs immediately; the cancellation lands in the wait after it.
    #[tokio::test]
    async fn cancels_an_in_flight_wait() {
        let token = CancellationToken::new();
        let times = Arc::new(Mutex::new(Vec::new()));

        let wait = poll_device_code_flow(
            Some(5.0),
            Some(30.0),
            false,
            &token,
            recording_poll(Arc::clone(&times), queue(vec![])),
        );
        tokio::pin!(wait);
        // Cancel while the engine sleeps before the second poll.
        let driver = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            driver.cancel();
        });

        let error = tokio::time::timeout(std::time::Duration::from_secs(5), wait)
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(error, AuthError::Cancelled);
        assert_eq!(times.lock().unwrap().len(), 1);
    }

    /// Oracle semantics: a pre-cancelled signal never polls (upstream checks
    /// `signal.aborted` at the top of the loop).
    #[tokio::test]
    async fn pre_cancelled_signal_never_polls() {
        let token = CancellationToken::new();
        token.cancel();
        let times = Arc::new(Mutex::new(Vec::new()));

        let error = poll_device_code_flow(
            Some(5.0),
            Some(30.0),
            false,
            &token,
            recording_poll(Arc::clone(&times), queue(vec![])),
        )
        .await
        .unwrap_err();

        assert_eq!(error, AuthError::Cancelled);
        assert!(times.lock().unwrap().is_empty());
    }

    /// Oracle semantics: without `expiresInSeconds` the engine polls until
    /// completion (no deadline break).
    #[tokio::test(start_paused = true)]
    async fn without_an_expiry_the_engine_polls_until_complete() {
        let token = CancellationToken::new();
        let times = Arc::new(Mutex::new(Vec::new()));
        let outcomes = queue(vec![
            PollOutcome::<()>::Pending,
            PollOutcome::<()>::Complete(()),
        ]);
        let start = tokio::time::Instant::now();

        poll_device_code_flow(
            Some(5.0),
            None,
            false,
            &token,
            recording_poll(Arc::clone(&times), outcomes),
        )
        .await
        .unwrap();

        assert_eq!(
            times.lock().unwrap().clone(),
            vec![start, start + Duration::from_secs(5)]
        );
    }

    /// Oracle semantics: the deadline ends the flow with the timeout message
    /// (the slow-down variant after any slow_down response).
    #[tokio::test(start_paused = true)]
    async fn expiry_reports_the_timeout_message() {
        let token = CancellationToken::new();
        let times = Arc::new(Mutex::new(Vec::new()));

        let error = poll_device_code_flow(
            Some(60.0),
            Some(30.0),
            false,
            &token,
            recording_poll(Arc::clone(&times), queue(vec![])),
        )
        .await
        .unwrap_err();
        assert_eq!(error, AuthError::Operation(TIMEOUT_MESSAGE.to_string()));
        // Polls at 0 and (min(60s, remaining 30s) → break) — only the first
        // poll lands before the deadline check ends the flow.
        assert_eq!(times.lock().unwrap().len(), 1);

        let token = CancellationToken::new();
        let times = Arc::new(Mutex::new(Vec::new()));
        let error = poll_device_code_flow(
            Some(60.0),
            Some(30.0),
            false,
            &token,
            recording_poll(
                Arc::clone(&times),
                queue(vec![PollOutcome::<()>::SlowDown(None)]),
            ),
        )
        .await
        .unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation(SLOW_DOWN_TIMEOUT_MESSAGE.to_string())
        );
    }

    /// Oracle semantics: a failed poll surfaces its message immediately.
    #[tokio::test(start_paused = true)]
    async fn failed_poll_surfaces_the_message() {
        let token = CancellationToken::new();
        let times = Arc::new(Mutex::new(Vec::new()));

        let error = poll_device_code_flow(
            Some(5.0),
            None,
            false,
            &token,
            recording_poll(
                Arc::clone(&times),
                queue(vec![PollOutcome::<()>::Failed("boom".to_string())]),
            ),
        )
        .await
        .unwrap_err();

        assert_eq!(error, AuthError::Operation("boom".to_string()));
        assert_eq!(times.lock().unwrap().len(), 1);
    }

    #[test]
    fn timing_constants_match_upstream() {
        assert_eq!(MINIMUM_INTERVAL_MS, 1000);
        assert_eq!(DEFAULT_POLL_INTERVAL_SECONDS, 5.0);
        assert_eq!(SLOW_DOWN_INTERVAL_INCREMENT_MS, 5000);
        assert_eq!(TIMEOUT_MESSAGE, "Device flow timed out");
        assert_eq!(
            SLOW_DOWN_TIMEOUT_MESSAGE,
            "Device flow timed out after one or more slow_down responses. This is often caused \
             by clock drift in WSL or VM environments. Please sync or restart the VM clock and \
             try again."
        );
    }
}
