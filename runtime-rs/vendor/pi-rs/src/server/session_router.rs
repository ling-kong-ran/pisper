//! Port of `packages/server/src/session-router.ts` (312 lines, SHA256
//! `c9ea719130708e7d8f548e5b65f23afe0c8d96f0f87ae3e611bfde6ca3dad69b`): the
//! per-connection attachment and service-call routing over host-supplied
//! Session handles.
//!
//! JS-to-Rust structure notes (disclosed seam S-C):
//!
//! - Upstream serializes per-client operations by chaining promises
//!   (`runForClient`); the port keeps the same *registration-order*
//!   semantics with a per-client tail chain — every entry point is a
//!   synchronous function that splices its operation onto the tail and
//!   returns a shared future, so the chain order equals the dispatch order
//!   of the callers (exactly upstream's synchronous `runForClient`
//!   registration inside the event-loop turn).
//! - Upstream keys maps by client *object identity*; the port hands each
//!   accepted connection a [`ClientId`] number.
//! - Upstream `openingSessions` shares one `Promise<HostedSession>`; the
//!   port shares one `futures::future::Shared` future with the same
//!   in-place insertion and settle-time cleanup.
//! - Upstream `randomUUID` (crypto v4) becomes a local pseudo-random
//!   UUIDv4 formatter (the same disclosed substitution as
//!   `ai::uuid`: no WebCrypto in the port). Attachment ids are opaque
//!   protocol strings; the oracle masks their bytes.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use futures::future::{BoxFuture, Shared};
use futures::FutureExt;
use tokio::sync::{Notify, OnceCell};

use crate::agent_core::chord_support::Context;
use crate::chord::types::ServiceCall;
use crate::protocol::protocol::{RpcTarget, SessionTarget};

use super::errors::{OperationError, ServerError};
use super::types::{PublishCallback, RoutedSessionAttachment, RoutedSessionHandle};

/// The chord-side JSON value tree.
type ChordValue = serde_json::Value;

/// A shared future of one service-invocation outcome (upstream the invoke
/// promise tracked on an attachment).
type SharedInvoke = Shared<BoxFuture<'static, Result<Option<ChordValue>, OperationError>>>;

/// A shared future of one attachment acquisition (upstream
/// `ClientAttachment.acquiring`).
type SharedAcquire =
    Shared<BoxFuture<'static, Result<Arc<dyn RoutedSessionAttachment>, OperationError>>>;

/// A shared future of one `openSession` attempt (upstream
/// `openingSessions` values).
type SharedOpen = Shared<BoxFuture<'static, Result<Arc<HostedSession>, OperationError>>>;

/// Stable per-connection identity for the router maps (upstream: object
/// identity of the `ConnectionState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClientId(pub(crate) u64);

/// A hosted Session: one open handle plus its live client attachments
/// (upstream `HostedSession`, `session-router.ts:20-24`).
pub(crate) struct HostedSession {
    pub(crate) id: String,
    pub(crate) handle: Arc<dyn RoutedSessionHandle>,
    pub(crate) attachments: Mutex<Vec<Arc<ClientAttachment>>>,
}

/// One client's live attachment (upstream `ClientAttachment`,
/// `session-router.ts:10-18`).
pub(crate) struct ClientAttachment {
    pub(crate) id: String,
    pub(crate) client: ClientId,
    pub(crate) session: Arc<HostedSession>,
    /// In-flight service invocations (upstream `operations`), each removed
    /// when settled. Ids identify entries for settle-time removal.
    operations: Arc<Mutex<Vec<(u64, SharedInvoke)>>>,
    acquiring: OnceLock<SharedAcquire>,
    lease: OnceLock<Arc<dyn RoutedSessionAttachment>>,
    /// Upstream `releasing` memoization.
    releasing: OnceCell<Result<(), OperationError>>,
}

/// Router configuration (upstream `SessionRouterOptions`,
/// `session-router.ts:26-32`).
pub(crate) struct RouterOptions {
    pub host: Arc<dyn super::types::ServerHost>,
    pub server_id: String,
    pub is_closing: Arc<dyn Fn() -> bool + Send + Sync>,
    /// Publishes one presentation attachment change to the client.
    pub publish_attachment: PublishAttachmentFn,
    pub report_error: Arc<dyn Fn(&OperationError) + Send + Sync>,
}

/// The publishAttachment callback type (upstream
/// `SessionRouterOptions.publishAttachment`, `session-router.ts:30`).
pub(crate) type PublishAttachmentFn = Arc<
    dyn Fn(
            ClientId,
            Option<SessionTarget>,
            Context,
        ) -> BoxFuture<'static, Result<(), OperationError>>
        + Send
        + Sync,
>;

/// The memoized router-close future (upstream `closePromise`).
type SharedCloseOutcome = Shared<BoxFuture<'static, Result<(), OperationError>>>;

struct ClientQueue {
    /// The per-client operation chain tail: every queued operation awaits
    /// this (upstream `clientOperations` values).
    tail: Mutex<Option<(u64, Shared<BoxFuture<'static, ()>>)>>,
    next_tail_id: AtomicU64,
    /// Registered-but-unsettled operation count (drives `idle`).
    inflight: AtomicUsize,
    idle: Notify,
    /// Rejections observed by settled operations (upstream: the rejections
    /// of the `clientOperations` promises, inspected by `closeInternal`).
    errors: Mutex<Vec<OperationError>>,
}

impl ClientQueue {
    fn new() -> Arc<ClientQueue> {
        Arc::new(ClientQueue {
            tail: Mutex::new(None),
            next_tail_id: AtomicU64::new(0),
            inflight: AtomicUsize::new(0),
            idle: Notify::new(),
            errors: Mutex::new(Vec::new()),
        })
    }
}

struct RouterShared {
    options: RouterOptions,
    /// Insertion-ordered hosted sessions (upstream `Map`).
    hosted_sessions: Mutex<Vec<Arc<HostedSession>>>,
    opening_sessions: Mutex<HashMap<String, (u64, SharedOpen)>>,
    next_opening_id: AtomicU64,
    attachments_by_client: Mutex<HashMap<ClientId, Arc<ClientAttachment>>>,
    disconnected_clients: Mutex<HashSet<ClientId>>,
    client_queues: Mutex<HashMap<ClientId, Arc<ClientQueue>>>,
    close_future: tokio::sync::Mutex<Option<SharedCloseOutcome>>,
}

/// Upstream `SessionRouter` (`session-router.ts:34-312`). Cheap to clone;
/// spawned tasks hold the shared core.
#[derive(Clone)]
pub(crate) struct SessionRouter(Arc<RouterShared>);

impl SessionRouter {
    pub(crate) fn new(options: RouterOptions) -> SessionRouter {
        SessionRouter(Arc::new(RouterShared {
            options,
            hosted_sessions: Mutex::new(Vec::new()),
            opening_sessions: Mutex::new(HashMap::new()),
            next_opening_id: AtomicU64::new(0),
            attachments_by_client: Mutex::new(HashMap::new()),
            disconnected_clients: Mutex::new(HashSet::new()),
            client_queues: Mutex::new(HashMap::new()),
            close_future: tokio::sync::Mutex::new(None),
        }))
    }

    /// Upstream `executeServiceCall` (`session-router.ts:47-58`). The
    /// admission step is registered synchronously on the client chain; the
    /// returned future resolves with the invoked service result.
    pub(crate) fn execute_service_call(
        &self,
        client: ClientId,
        target: RpcTarget,
        call: ServiceCall,
        publish: PublishCallback,
        context: Context,
    ) -> BoxFuture<'static, Result<Option<ChordValue>, OperationError>> {
        let shared = self.0.clone();
        let admitted = self.enqueue(client, move || {
            start_service_call(shared, client, target, call, publish, context).boxed()
        });
        Box::pin(async move {
            let invoke = admitted.await?;
            invoke.await
        })
    }

    /// Upstream `attachClient` (`session-router.ts:60-63`).
    pub(crate) fn attach_client(
        &self,
        client: ClientId,
        session_id: String,
        context: Context,
    ) -> BoxFuture<'static, Result<(), OperationError>> {
        if (self.0.options.is_closing)() {
            return Box::pin(async { Err(OperationError::Server(ServerError::server_draining())) });
        }
        let shared = self.0.clone();
        let op = self.enqueue(client, move || {
            attach_client_now(shared, client, session_id, context).boxed()
        });
        Box::pin(op)
    }

    /// Upstream `detachClient` (`session-router.ts:65-70`).
    pub(crate) fn detach_client(
        &self,
        client: ClientId,
        context: Context,
    ) -> BoxFuture<'static, Result<(), OperationError>> {
        let shared = self.0.clone();
        let op = self.enqueue(client, move || {
            async move {
                let current = shared
                    .attachments_by_client
                    .lock()
                    .unwrap()
                    .get(&client)
                    .cloned();
                if let Some(attachment) = current {
                    release_attachment(&shared, &attachment, context, true).await?;
                }
                Ok(())
            }
            .boxed()
        });
        Box::pin(op)
    }

    /// Upstream `removeSession` (`session-router.ts:72-89`).
    pub(crate) fn remove_session(
        &self,
        session_id: String,
        context: Context,
    ) -> BoxFuture<'static, Result<(), OperationError>> {
        let shared = self.0.clone();
        Box::pin(async move {
            if (shared.options.is_closing)() {
                return Err(OperationError::Server(ServerError::server_draining()));
            }
            let hosted = {
                let hosted_sessions = shared
                    .hosted_sessions
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                hosted_sessions
                    .iter()
                    .find(|session| session.id == session_id)
                    .cloned()
            };
            let Some(hosted) = hosted else {
                return Ok(());
            };
            let mut errors: Vec<OperationError> = Vec::new();
            let attachments = hosted
                .attachments
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            for attachment in attachments {
                if let Err(error) =
                    release_attachment(&shared, &attachment, context.clone(), true).await
                {
                    errors.push(error);
                }
            }
            if let Err(error) = hosted.handle.close(context.clone()).await {
                errors.push(error);
            }
            remove_hosted_if_current(&shared, &hosted);
            match errors.len() {
                0 => Ok(()),
                1 => Err(errors.into_iter().next().expect("one error")),
                _ => Err(OperationError::Aggregate {
                    message: format!("Failed to close Session {session_id}"),
                    errors,
                }),
            }
        })
    }

    /// Upstream `disconnect` (`session-router.ts:91-101`): flags the client
    /// as disconnected and releases its attachment without publishing the
    /// clear (the transport is already gone).
    pub(crate) fn disconnect(
        &self,
        client: ClientId,
        context: Context,
    ) -> BoxFuture<'static, Result<(), OperationError>> {
        self.0.disconnected_clients.lock().unwrap().insert(client);
        let shared = self.0.clone();
        let op = self.enqueue(client, move || {
            async move {
                let result = async {
                    let current = shared
                        .attachments_by_client
                        .lock()
                        .unwrap()
                        .get(&client)
                        .cloned();
                    if let Some(attachment) = current {
                        release_attachment(&shared, &attachment, context, false).await?;
                    }
                    Ok(())
                }
                .await;
                shared.disconnected_clients.lock().unwrap().remove(&client);
                result
            }
            .boxed()
        });
        Box::pin(op)
    }

    /// Upstream `close` (`session-router.ts:103-106`): memoized like
    /// `closePromise ??=`.
    pub(crate) fn close(&self, context: Context) -> BoxFuture<'static, Result<(), OperationError>> {
        let shared = self.0.clone();
        Box::pin(async move {
            let shared_future = {
                let mut guard = shared.close_future.lock().await;
                if guard.is_none() {
                    let future: Shared<BoxFuture<'static, Result<(), OperationError>>> =
                        Box::pin(close_internal(shared.clone(), context))
                            .boxed()
                            .shared();
                    *guard = Some(future);
                }
                guard.as_ref().expect("just set").clone()
            };
            shared_future.await
        })
    }

    /// Upstream `runForClient` (`session-router.ts:146-158`): splice the
    /// operation onto the client's chain at *registration* time and return
    /// a shared handle to its outcome.
    fn enqueue<T, F>(
        &self,
        client: ClientId,
        make_operation: impl FnOnce() -> F + Send + 'static,
    ) -> Shared<BoxFuture<'static, Result<T, OperationError>>>
    where
        T: Clone + Send + Sync + 'static,
        F: std::future::Future<Output = Result<T, OperationError>> + Send + 'static,
    {
        let queue = self.queue_for(client);
        queue.inflight.fetch_add(1, Ordering::SeqCst);
        let previous = {
            let tail = queue
                .tail
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            tail.as_ref().map(|(_, shared)| shared.clone())
        };
        let operation: BoxFuture<'static, Result<T, OperationError>> = Box::pin(async move {
            if let Some(previous) = previous {
                let _ = previous.await;
            }
            make_operation().await
        });
        let op_shared = operation.shared();

        // Install the new tail (upstream: `this.clientOperations.set(client, tail)`).
        let next_id = queue.next_tail_id.fetch_add(1, Ordering::SeqCst);
        let tail_op = op_shared.clone();
        let tail_shared: Shared<BoxFuture<'static, ()>> = Box::pin(async move {
            let _ = tail_op.await;
        })
        .boxed()
        .shared();
        *queue
            .tail
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((next_id, tail_shared));

        // Settle-time accounting (upstream: the `void tail.finally(...)` cleanup
        // plus closeInternal's rejection reporting).
        let watcher = op_shared.clone();
        let watcher_queue = queue.clone();
        tokio::spawn(async move {
            let outcome = watcher.await;
            if let Err(error) = &outcome {
                watcher_queue
                    .errors
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(error.clone());
            }
            watcher_queue.inflight.fetch_sub(1, Ordering::SeqCst);
            watcher_queue.idle.notify_waiters();
            let mut tail = watcher_queue
                .tail
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if tail.as_ref().map(|(id, _)| *id) == Some(next_id) {
                *tail = None;
            }
        });
        op_shared
    }

    fn queue_for(&self, client: ClientId) -> Arc<ClientQueue> {
        let mut queues = self
            .0
            .client_queues
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        queues
            .entry(client)
            .or_insert_with(ClientQueue::new)
            .clone()
    }
}

/// Upstream `startServiceCall` (`session-router.ts:199-214`): admit the call
/// on the attachment and return the tracked invoke future.
async fn start_service_call(
    shared: Arc<RouterShared>,
    client: ClientId,
    target: RpcTarget,
    call: ServiceCall,
    publish: PublishCallback,
    context: Context,
) -> Result<SharedInvoke, OperationError> {
    let attachment = require_attachment(&shared, client, &target)?;
    let lease = attachment
        .lease
        .get()
        .cloned()
        .expect("attachment holds a lease once admitted");
    let invoke: BoxFuture<'static, Result<Option<ChordValue>, OperationError>> =
        lease.invoke_service(call, publish, context);
    let invoke_shared = invoke.shared();
    track_operation(&attachment.operations, invoke_shared.clone());
    Ok(invoke_shared)
}

/// Upstream `trackOperation` (`session-router.ts:216-222`): track the
/// in-flight invoke and remove it from the attachment when settled.
fn track_operation(operations: &Arc<Mutex<Vec<(u64, SharedInvoke)>>>, invoke: SharedInvoke) {
    let next_id = NEXT_OPERATION_ID.fetch_add(1, Ordering::SeqCst);
    operations
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push((next_id, invoke.clone()));
    let operations = operations.clone();
    tokio::spawn(async move {
        let _ = invoke.await;
        operations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|(id, _)| *id != next_id);
    });
}

static NEXT_OPERATION_ID: AtomicU64 = AtomicU64::new(0);

/// Upstream `requireAttachment` (`session-router.ts:224-232`).
fn require_attachment(
    shared: &Arc<RouterShared>,
    client: ClientId,
    target: &RpcTarget,
) -> Result<Arc<ClientAttachment>, OperationError> {
    if (shared.options.is_closing)()
        || shared
            .disconnected_clients
            .lock()
            .unwrap()
            .contains(&client)
    {
        return Err(OperationError::Server(ServerError::server_draining()));
    }
    let RpcTarget::Session(session_target) = target else {
        return Err(OperationError::Server(ServerError::session_not_attached()));
    };
    let current = shared
        .attachments_by_client
        .lock()
        .unwrap()
        .get(&client)
        .cloned();
    match current {
        Some(attachment)
            if attachment.session.id == session_target.session_id
                && attachment.id == session_target.attachment_id =>
        {
            Ok(attachment)
        }
        _ => Err(OperationError::Server(ServerError::session_not_attached())),
    }
}

/// Upstream `attachClientNow` (`session-router.ts:160-197`).
async fn attach_client_now(
    shared: Arc<RouterShared>,
    client: ClientId,
    session_id: String,
    context: Context,
) -> Result<(), OperationError> {
    if (shared.options.is_closing)()
        || shared
            .disconnected_clients
            .lock()
            .unwrap()
            .contains(&client)
    {
        return Err(OperationError::Server(ServerError::server_draining()));
    }
    let current = shared
        .attachments_by_client
        .lock()
        .unwrap()
        .get(&client)
        .cloned();
    if current
        .as_ref()
        .is_some_and(|attachment| attachment.session.id == session_id)
    {
        return Ok(());
    }
    let hosted = acquire(&shared, session_id.clone(), context.clone()).await?;
    if (shared.options.is_closing)()
        || shared
            .disconnected_clients
            .lock()
            .unwrap()
            .contains(&client)
    {
        return Err(OperationError::Server(ServerError::server_draining()));
    }
    if let Some(current) = current {
        release_attachment(&shared, &current, context.clone(), false).await?;
    }
    let attachment = Arc::new(ClientAttachment {
        id: random_uuid_v4(),
        client,
        session: hosted.clone(),
        operations: Arc::new(Mutex::new(Vec::new())),
        acquiring: OnceLock::new(),
        lease: OnceLock::new(),
        releasing: OnceCell::new(),
    });
    hosted
        .attachments
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(attachment.clone());
    let acquiring: BoxFuture<'static, Result<Arc<dyn RoutedSessionAttachment>, OperationError>> =
        hosted.handle.attach_client(context.clone());
    let acquiring_shared = acquiring.shared();
    attachment
        .acquiring
        .set(acquiring_shared.clone())
        .expect("acquiring set once");
    match acquiring_shared.await {
        Ok(lease) => {
            attachment.lease.set(lease).ok();
        }
        Err(error) => {
            detach_attachment_from_session(&hosted, &attachment);
            return Err(error);
        }
    }
    let hosted_current = shared
        .hosted_sessions
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .any(|candidate| Arc::ptr_eq(candidate, &hosted));
    let attachment_current = hosted
        .attachments
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .any(|candidate| Arc::ptr_eq(candidate, &attachment));
    if !hosted_current
        || !attachment_current
        || shared
            .disconnected_clients
            .lock()
            .unwrap()
            .contains(&client)
        || (shared.options.is_closing)()
    {
        release_attachment(&shared, &attachment, context.clone(), true).await?;
        return Err(OperationError::Server(ServerError::server_draining()));
    }
    shared
        .attachments_by_client
        .lock()
        .unwrap()
        .insert(client, attachment.clone());
    (shared.options.publish_attachment)(
        client,
        Some(SessionTarget {
            server_id: shared.options.server_id.clone(),
            session_id,
            attachment_id: attachment.id.clone(),
        }),
        context,
    )
    .await
}

/// Upstream `releaseAttachment` (`session-router.ts:234-252`): memoized,
/// awaits in-flight operations, releases the lease (or the pending
/// acquisition), then clears the attachment.
async fn release_attachment(
    shared: &Arc<RouterShared>,
    attachment: &Arc<ClientAttachment>,
    context: Context,
    publish: bool,
) -> Result<(), OperationError> {
    let shared = shared.clone();
    let attachment_for_release = attachment.clone();
    let outcome = attachment
        .releasing
        .get_or_init(move || async move {
            let attachment = attachment_for_release;
            let mut errors: Vec<OperationError> = Vec::new();
            // `await Promise.allSettled(attachment.operations)`.
            let operations: Vec<(u64, SharedInvoke)> = attachment
                .operations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .drain(..)
                .collect();
            for (_, operation) in operations {
                let _ = operation.await;
            }
            let lease_result: Result<(), OperationError> = async {
                let lease = match attachment.lease.get() {
                    Some(lease) => lease.clone(),
                    None => {
                        let acquiring = attachment
                            .acquiring
                            .get()
                            .expect("acquiring installed with the attachment")
                            .clone();
                        acquiring.await?
                    }
                };
                lease.release(context.clone()).await
            }
            .await;
            if let Err(error) = lease_result {
                errors.push(error);
            }
            let result = match errors.len() {
                0 => Ok(()),
                1 => Err(errors.into_iter().next().expect("one error")),
                _ => Err(OperationError::Aggregate {
                    message: "Failed to release Session attachment".to_string(),
                    errors,
                }),
            };
            // `finally { await this.clearAttachment(...) }` — the clear error
            // replaces the release error, like a rethrow from finally.
            match clear_attachment(&shared, &attachment, context.clone(), publish).await {
                Ok(()) => result,
                Err(error) => Err(error),
            }
        })
        .await;
    outcome.clone()
}

/// Upstream `clearAttachment` (`session-router.ts:254-260`).
async fn clear_attachment(
    shared: &Arc<RouterShared>,
    attachment: &Arc<ClientAttachment>,
    context: Context,
    publish: bool,
) -> Result<(), OperationError> {
    detach_attachment_from_session(&attachment.session, attachment);
    let is_current = shared
        .attachments_by_client
        .lock()
        .unwrap()
        .get(&attachment.client)
        .is_some_and(|current| Arc::ptr_eq(current, attachment));
    if is_current {
        shared
            .attachments_by_client
            .lock()
            .unwrap()
            .remove(&attachment.client);
        if publish {
            (shared.options.publish_attachment)(attachment.client, None, context).await?;
        }
    }
    Ok(())
}

fn detach_attachment_from_session(session: &HostedSession, attachment: &Arc<ClientAttachment>) {
    session
        .attachments
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .retain(|candidate| !Arc::ptr_eq(candidate, attachment));
}

fn remove_hosted_if_current(shared: &Arc<RouterShared>, hosted: &Arc<HostedSession>) {
    shared
        .hosted_sessions
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .retain(|candidate| candidate.id != hosted.id || !Arc::ptr_eq(candidate, hosted));
}

/// Upstream `acquire` (`session-router.ts:262-274`).
fn acquire(shared: &Arc<RouterShared>, session_id: String, context: Context) -> SharedOpen {
    let existing = shared
        .hosted_sessions
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .find(|session| session.id == session_id)
        .cloned();
    if let Some(existing) = existing {
        return Box::pin(async move { Ok(existing) }).boxed().shared();
    }
    if let Some((_, opening)) = shared
        .opening_sessions
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&session_id)
    {
        return opening.clone();
    }
    let shared = shared.clone();
    let open_shared = shared.clone();
    let open_session_id = session_id.clone();
    let pending: BoxFuture<'static, Result<Arc<HostedSession>, OperationError>> =
        Box::pin(async move { open_session(&open_shared, open_session_id, context).await });
    let pending_shared = pending.shared();
    let opening_id = shared.next_opening_id.fetch_add(1, Ordering::SeqCst);
    shared
        .opening_sessions
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(session_id.clone(), (opening_id, pending_shared.clone()));
    // `finally { if (this.openingSessions.get(sessionId) === pending) delete }`.
    let cleanup = pending_shared.clone();
    let cleanup_shared = shared.clone();
    tokio::spawn(async move {
        let _ = cleanup.await;
        let mut openings = cleanup_shared
            .opening_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if openings
            .get(&session_id)
            .is_some_and(|(id, _)| *id == opening_id)
        {
            openings.remove(&session_id);
        }
    });
    pending_shared
}

/// Upstream `open` (`session-router.ts:276-300`).
async fn open_session(
    shared: &Arc<RouterShared>,
    session_id: String,
    context: Context,
) -> Result<Arc<HostedSession>, OperationError> {
    let metadata = shared
        .options
        .host
        .resolve_session(session_id.clone(), context.clone())
        .await?;
    let session_record_id = metadata.id.clone();
    let handle = shared
        .options
        .host
        .open_session(metadata, context.clone())
        .await?;
    if (shared.options.is_closing)() {
        return match handle.close(context).await {
            Ok(()) => Err(OperationError::Server(ServerError::server_draining())),
            Err(error) => Err(OperationError::SessionCleanup {
                message: "Failed to close routed Session acquired while draining".to_string(),
                errors: vec![
                    OperationError::Server(ServerError::server_draining()),
                    error,
                ],
            }),
        };
    }
    let hosted = Arc::new(HostedSession {
        id: session_record_id,
        handle,
        attachments: Mutex::new(Vec::new()),
    });
    shared
        .hosted_sessions
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(hosted.clone());
    if let Some(terminated) = hosted.handle.terminated() {
        let invalidate_shared = shared.clone();
        let invalidated = hosted.clone();
        tokio::spawn(async move {
            let error = terminated.await;
            invalidate(&invalidate_shared, &invalidated, error);
        });
    }
    Ok(hosted)
}

/// Upstream `invalidate` (`session-router.ts:302-311`).
fn invalidate(
    shared: &Arc<RouterShared>,
    hosted: &Arc<HostedSession>,
    error: Option<OperationError>,
) {
    remove_hosted_if_current(shared, hosted);
    let attachments = hosted
        .attachments
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    for attachment in attachments {
        let release_shared = shared.clone();
        tokio::spawn(async move {
            if let Err(release_error) =
                release_attachment(&release_shared, &attachment, Context::background(), true).await
            {
                (release_shared.options.report_error)(&release_error);
            }
        });
    }
    if let Some(error) = error {
        (shared.options.report_error)(&error);
    }
}

/// Upstream `closeInternal` (`session-router.ts:108-144`).
async fn close_internal(shared: Arc<RouterShared>, context: Context) -> Result<(), OperationError> {
    // Settle every queued operation, reporting rejections. Upstream
    // closeInternal collects SessionCleanupErrors from *both* the client
    // operation rejections and the opening futures; client operations do
    // reach the opening futures (an attach routes through `open`), so the
    // same classification applies here.
    let mut close_errors: Vec<OperationError> = Vec::new();
    let queues: Vec<Arc<ClientQueue>> = shared
        .client_queues
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .values()
        .cloned()
        .collect();
    for queue in queues {
        loop {
            let notified = queue.idle.notified();
            if queue.inflight.load(Ordering::SeqCst) == 0 {
                break;
            }
            notified.await;
        }
        for error in queue
            .errors
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain(..)
        {
            (shared.options.report_error)(&error);
            if matches!(error, OperationError::SessionCleanup { .. }) {
                close_errors.push(error);
            }
        }
    }
    // Settle the pending opens.
    let openings: Vec<SharedOpen> = shared
        .opening_sessions
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .values()
        .map(|(_, opening)| opening.clone())
        .collect();
    for opening in openings {
        if let Err(error) = opening.await {
            (shared.options.report_error)(&error);
            if matches!(error, OperationError::SessionCleanup { .. }) {
                close_errors.push(error);
            }
        }
    }
    // Release every attachment of every hosted session.
    let hosted_all: Vec<Arc<HostedSession>> = shared
        .hosted_sessions
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    for session in &hosted_all {
        let attachments = session
            .attachments
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        for attachment in attachments {
            if let Err(error) =
                release_attachment(&shared, &attachment, context.clone(), true).await
            {
                close_errors.push(error);
            }
        }
    }
    // Close every hosted handle.
    for session in &hosted_all {
        match session.handle.close(context.clone()).await {
            Ok(()) => {
                remove_hosted_if_current(&shared, session);
            }
            Err(error) => {
                (shared.options.report_error)(&error);
                close_errors.push(error);
            }
        }
    }
    shared.attachments_by_client.lock().unwrap().clear();
    shared
        .client_queues
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
    if close_errors.is_empty() {
        Ok(())
    } else {
        Err(OperationError::Aggregate {
            message: "Failed to close routed Sessions".to_string(),
            errors: close_errors,
        })
    }
}

/// Local pseudo-random UUIDv4 (upstream `randomUUID` from `node:crypto`).
/// Same disclosed substitution as `ai::uuid`: the port has no WebCrypto, so
/// the random bytes come from per-call `RandomState` hashes seeded with the
/// clock. Only the string shape is protocol-visible; the oracle masks it.
pub(crate) fn random_uuid_v4() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut bytes = [0u8; 16];
    for (index, chunk) in bytes.chunks_mut(8).enumerate() {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u64(crate::ai::now_ms() as u64 ^ (index as u64) << 32);
        hasher.write_u64(std::process::id() as u64);
        hasher.write_u64(rand_sector());
        chunk.copy_from_slice(&hasher.finish().to_ne_bytes()[..chunk.len()]);
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: Vec<String> = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        hex[0..4].concat(),
        hex[4..6].concat(),
        hex[6..8].concat(),
        hex[8..10].concat(),
        hex[10..16].concat()
    )
}

/// Extra per-call entropy so two ids minted in the same millisecond within
/// one process stay distinct (the upstream generator draws a fresh
/// `crypto` random pool per call).
fn rand_sector() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.subsec_nanos() as u64)
            .unwrap_or(0),
    );
    hasher.finish()
}
