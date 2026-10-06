//! Process-local Drive from upstream runtime/types.ts. This owns no Lane and
//! starts no dispatcher or provider work. A cloneable completion handle replaces
//! the shared JS promise; dropping one waiter cannot cancel another waiter.
use crate::agent_core::harness::context::{without_abort_signal, Context};
use crate::agent_core::harness::execution::{create_gate, EffectGate, GateControl};
use crate::agent_core::harness::session::OperationResultRecord;
use crate::ai::types::DeferredHandle;
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

/// The native AgentHarness drive options, before a public harness is available.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveOptions {
    pub operation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_for_retry: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poll_deferred: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum DriveOutcome {
    Settled {
        outcome: OperationResultRecord,
    },
    Waiting {
        operation_id: String,
        #[serde(flatten)]
        reason: WaitingReason,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "reason",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum WaitingReason {
    Retry { not_before: i64 },
    Deferred { deferred: DeferredHandle },
}

/// JS unknown rejection values use the harness's typed shared-error convention.
pub type DriveError = Arc<dyn Error + Send + Sync>;
pub type DriveCompletionResult = Result<Arc<DriveOutcome>, DriveError>;

/// Retains a shared first-wins result, including when no waiter is registered.
/// This handle owns a sender so dropping the Drive does not spuriously reject an
/// unsettled completion (the upstream promise would remain pending).
#[derive(Clone)]
pub struct DriveCompletion {
    result: watch::Sender<Option<DriveCompletionResult>>,
}
impl DriveCompletion {
    fn new() -> Self {
        Self {
            result: watch::channel(None).0,
        }
    }
    fn finish(&self, result: DriveCompletionResult) {
        self.result.send_if_modified(|current| {
            if current.is_some() {
                return false;
            }
            *current = Some(result);
            true
        });
    }
    pub async fn wait(&self) -> DriveCompletionResult {
        let mut receiver = self.result.subscribe();
        let result = receiver
            .wait_for(|value| value.is_some())
            .await
            .expect("completion handle retains the sender");
        result
            .as_ref()
            .expect("wait predicate checked completion")
            .clone()
    }
}

/// One installed drive pass. Lane admission, durable mutation and procedure
/// dispatch remain separate, unported runtime work.
pub struct Drive {
    operation_id: String,
    context: Context,
    wait_for_retry: bool,
    deferred_permits: Arc<AtomicUsize>,
    gate: EffectGate,
    control: GateControl,
    completion: DriveCompletion,
    close_signal: CancellationToken,
    close_error: Mutex<Option<DriveError>>,
}
impl Drive {
    pub fn new(options: &DriveOptions, context: Context) -> Self {
        let (gate, control) = create_gate();
        Self {
            operation_id: options.operation_id.clone(),
            context: without_abort_signal(context),
            wait_for_retry: options.wait_for_retry.unwrap_or(false),
            deferred_permits: Arc::new(AtomicUsize::new(usize::from(
                options.poll_deferred == Some(true),
            ))),
            gate,
            control,
            completion: DriveCompletion::new(),
            close_signal: CancellationToken::new(),
            close_error: Mutex::new(None),
        }
    }
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }
    pub fn context(&self) -> &Context {
        &self.context
    }
    pub fn wait_for_retry(&self) -> bool {
        self.wait_for_retry
    }
    pub fn gate(&self) -> &EffectGate {
        &self.gate
    }
    pub fn completion(&self) -> DriveCompletion {
        self.completion.clone()
    }
    pub fn close_signal(&self) -> CancellationToken {
        self.close_signal.clone()
    }
    pub fn close_reason(&self) -> Option<DriveError> {
        self.close_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
    pub fn deferred_permits(&self) -> usize {
        self.deferred_permits.load(Ordering::SeqCst)
    }
    /// A shared handle to the deferred-permit counter, so a `'static`
    /// settlement materialize closure can consume the permit at the commit
    /// boundary (the port's substitute for upstream `drive.deferredPermits--`
    /// on a captured drive).
    pub fn deferred_permit_counter(&self) -> Arc<std::sync::atomic::AtomicUsize> {
        Arc::clone(&self.deferred_permits)
    }
    /// Call only at the upstream successful deferred-effect commit boundary;
    /// checking permission does not consume it. There is at most one per pass.
    pub fn consume_deferred_permit(&self) -> bool {
        self.deferred_permits
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
    }
    pub fn settle(&self, outcome: DriveOutcome) {
        self.completion.finish(Ok(Arc::new(outcome)));
    }
    pub fn fail(&self, error: DriveError) {
        self.completion.finish(Err(error));
    }
    pub fn begin_abort(&self, cancellation: CancellationToken) {
        self.control.begin_abort(cancellation);
    }
    pub fn signal_abort(&self) {
        self.control.signal_abort();
    }
    pub fn close_gate(&self, error: DriveError) {
        // Serialize the compound close for Rust callers on different threads.
        let mut reason = self
            .close_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        self.control.close(error.clone());
        if reason.is_none() {
            *reason = Some(error.clone());
            self.close_signal.cancel();
        }
        self.fail(error);
    }
}

/// Only the procedure result vocabulary; no dispatcher is implied.
#[derive(Debug, Clone, PartialEq)]
pub enum ProcedureResult {
    Continue,
    Waiting { outcome: DriveOutcome },
    Settled { outcome: OperationResultRecord },
}
