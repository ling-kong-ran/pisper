//! Awaitable command results with JavaScript Promise-style task lifetime.
//!
//! Command-context methods are deliberately **not** `async fn`: upstream checks
//! staleness and invokes the current handler synchronously, returning its exact
//! Promise. The outer `Result` models a synchronous throw; awaiting this handle
//! observes fulfilment/rejection. A handler performs its synchronous prefix
//! before calling [`CommandFuture::spawn`] for the part after its first await.
//!
//! Tasks start without polling this handle, and dropping all handles does not
//! cancel them. Explicit cancellation belongs to the underlying session action.
//! A Tokio runtime is required only for pending work, not settled defaults.
//! This is not a JS microtask executor: arbitrary JS thrown values/stacks and
//! event-handler asynchronicity remain separate migration seams.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use futures::future::{BoxFuture, FutureExt, Shared};

use super::types::HandlerError;

/// One shared completion, returned unchanged by command-context dispatch.
#[derive(Clone)]
#[must_use = "await the command result to observe completion or rejection"]
pub struct CommandFuture<T: Clone> {
    completion: Shared<BoxFuture<'static, Result<T, HandlerError>>>,
    identity: Arc<()>,
}

impl<T: Clone + Send + Sync + 'static> CommandFuture<T> {
    fn completion(future: impl Future<Output = Result<T, HandlerError>> + Send + 'static) -> Self {
        Self {
            completion: future.boxed().shared(),
            identity: Arc::new(()),
        }
    }

    /// An already fulfilled Promise (no runtime needed).
    pub fn resolved(value: T) -> Self {
        Self::completion(std::future::ready(Ok(value)))
    }

    /// An already rejected Promise, distinct from an immediate handler error.
    pub fn rejected(error: HandlerError) -> Self {
        Self::completion(std::future::ready(Err(error)))
    }

    /// Start pending work now, without requiring the caller to poll its result.
    ///
    /// Invoke this inside the handler, after its synchronous prefix. Rust task
    /// panics/cancellation are reported to awaiters rather than silently lost.
    /// Lack of a Tokio runtime is an explicit host-configuration error.
    pub fn spawn(
        work: impl Future<Output = Result<T, HandlerError>> + Send + 'static,
    ) -> Result<Self, HandlerError> {
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|_| "Pending extension commands require a Tokio runtime".to_string())?;
        let task = runtime.spawn(work);
        Ok(Self::completion(async move {
            task.await
                .map_err(|error| format!("Extension command task failed: {error}"))?
        }))
    }

    /// Promise identity is stable even after another clone has completed.
    pub fn same_promise(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.identity, &other.identity)
    }
}

impl<T: Clone> Future for CommandFuture<T> {
    type Output = Result<T, HandlerError>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.completion).poll(context)
    }
}
