//! Port of `packages/server/src/testing/host.ts` (SHA256
//! `269b0eb8411f19e9efe318ba7437b668fc6171648e5d8e9e7a1d801604281dfa` @
//! v1.0.0 `a276dabe5`): the controllable `ServerHost` double, the observable
//! `TestHarness` session handle, and the session-management server service
//! used by the conformance tests.
//!
//! Upstream counters are plain mutable fields driven from the event loop;
//! the port guards them with atomics/mutexes. `Deferred<T>` becomes a
//! oneshot-backed shared future. Since v1.0.0 the host keeps a plain
//! `sessions` map instead of a `MemorySessionRepo`, and `TestHarness` holds
//! only the session's metadata (it no longer owns a repo session to close).

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use futures::future::{BoxFuture, Shared};
use futures::FutureExt;
use tokio::sync::oneshot;

use crate::agent_core::chord_support::Context;
use crate::chord::types::ServiceCall;

use super::super::errors::OperationError;
use super::super::types::{
    PublishCallback, RoutedServerPresentation, RoutedServerServiceAttachment,
    RoutedServerServiceHost, RoutedSessionAttachment, RoutedSessionHandle, ServerHost,
    SessionMetadata, TerminatedSignal,
};

/// The chord-side JSON value tree.
type ChordValue = serde_json::Value;

/// Upstream `Deferred<T>` (`host.ts:7-20`).
pub struct Deferred<T: Clone> {
    sender: Mutex<Option<oneshot::Sender<T>>>,
    receiver: Shared<BoxFuture<'static, T>>,
}

impl<T: Clone + Send + Sync + 'static> Default for Deferred<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Clone + Send + Sync + 'static> Deferred<T> {
    pub fn new() -> Deferred<T> {
        let (sender, receiver) = oneshot::channel();
        let receiver: Shared<BoxFuture<'static, T>> = Box::pin(async move {
            receiver
                .await
                .expect("deferred resolved before dropped sender poll")
        })
        .boxed()
        .shared();
        Deferred {
            sender: Mutex::new(Some(sender)),
            receiver,
        }
    }

    pub fn resolve(&self, value: T) {
        if let Some(sender) = self
            .sender
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            let _ = sender.send(value);
        }
    }

    /// Upstream `promise`.
    pub fn promise(&self) -> Shared<BoxFuture<'static, T>> {
        self.receiver.clone()
    }
}

/// Upstream `OpenGate` (`host.ts:22-25`).
#[derive(Clone)]
pub struct OpenGate {
    pub entered: Arc<Deferred<()>>,
    pub release: Arc<Deferred<()>>,
}

fn open_gate() -> OpenGate {
    OpenGate {
        entered: Arc::new(Deferred::new()),
        release: Arc::new(Deferred::new()),
    }
}

/// The lease handed out by `TestHarness::attach_client` (upstream the
/// object literal returned by `TestHarness.attachClient`, `host.ts:47-63`).
struct HarnessLease {
    harness: Arc<HarnessCore>,
    released: Arc<std::sync::atomic::AtomicBool>,
}

impl RoutedSessionAttachment for HarnessLease {
    fn invoke_service(
        &self,
        call: ServiceCall,
        _publish: PublishCallback,
        _context: Context,
    ) -> BoxFuture<'static, Result<Option<ChordValue>, OperationError>> {
        let harness = self.harness.clone();
        Box::pin(async move { harness.invoke_service(call).await })
    }

    fn release(&self, _context: Context) -> BoxFuture<'static, Result<(), OperationError>> {
        let harness = self.harness.clone();
        let released = self.released.clone();
        Box::pin(async move {
            if released.load(Ordering::SeqCst) {
                return Ok(());
            }
            harness
                .attachment_release_count
                .fetch_add(1, Ordering::SeqCst);
            let failure = harness
                .fail_attachment_release
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if let Some(error) = failure {
                return Err(error);
            }
            released.store(true, Ordering::SeqCst);
            harness.attached_clients.fetch_sub(1, Ordering::SeqCst);
            Ok(())
        })
    }
}

/// Shared `TestHarness` state (upstream the instance fields,
/// `host.ts:26-40`: `metadata`, `closed`, `termination`, and the counters).
struct HarnessCore {
    metadata: SessionMetadata,
    closed: Deferred<()>,
    termination: Deferred<Option<OperationError>>,
    attached_clients: AtomicI64,
    attachment_release_count: AtomicI64,
    close_count: AtomicI64,
    service_calls: Mutex<Vec<ServiceCall>>,
    fail_attachment_release: Mutex<Option<OperationError>>,
    fail_close: Mutex<Option<OperationError>>,
    next_service_error: Mutex<Option<OperationError>>,
    /// `None` stands for the upstream default `{ ok: true }`; an explicit
    /// `Some(Null)` is the upstream `null`.
    next_service_result: Mutex<Option<ChordValue>>,
    next_close_gate: Mutex<Option<OpenGate>>,
    next_service_gate: Mutex<Option<OpenGate>>,
}

impl HarnessCore {
    async fn invoke_service(
        &self,
        call: ServiceCall,
    ) -> Result<Option<ChordValue>, OperationError> {
        self.service_calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(call);
        if let Some(error) = self
            .next_service_error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            return Err(error);
        }
        let gate = self
            .next_service_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(gate) = gate {
            gate.entered.resolve(());
            let _ = gate.release.promise().await;
        }
        let mut result = self
            .next_service_result
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let value = result
            .take()
            .unwrap_or_else(|| serde_json::json!({ "ok": true }));
        // `this.nextServiceResult = { ok: true }` after every call.
        *result = None;
        Ok(Some(value))
    }
}

/// Upstream `TestHarness` (`host.ts:26-111`).
pub struct TestHarness {
    core: Arc<HarnessCore>,
    terminated_signal: Shared<BoxFuture<'static, Option<OperationError>>>,
}

impl TestHarness {
    pub fn new(metadata: SessionMetadata) -> Arc<TestHarness> {
        let termination: Deferred<Option<OperationError>> = Deferred::new();
        let terminated_signal = termination.promise();
        Arc::new(TestHarness {
            core: Arc::new(HarnessCore {
                metadata,
                closed: Deferred::new(),
                termination,
                attached_clients: AtomicI64::new(0),
                attachment_release_count: AtomicI64::new(0),
                close_count: AtomicI64::new(0),
                service_calls: Mutex::new(Vec::new()),
                fail_attachment_release: Mutex::new(None),
                fail_close: Mutex::new(None),
                next_service_error: Mutex::new(None),
                next_service_result: Mutex::new(None),
                next_close_gate: Mutex::new(None),
                next_service_gate: Mutex::new(None),
            }),
            terminated_signal,
        })
    }

    /// `host.ts:26` `metadata`.
    pub fn metadata(&self) -> &SessionMetadata {
        &self.core.metadata
    }

    /// `host.ts:31` `attachedClients`.
    pub fn attached_clients(&self) -> i64 {
        self.core.attached_clients.load(Ordering::SeqCst)
    }

    /// `host.ts:33` `attachmentReleaseCount`.
    pub fn attachment_release_count(&self) -> i64 {
        self.core.attachment_release_count.load(Ordering::SeqCst)
    }

    /// `host.ts:34` `closeCount`.
    pub fn close_count(&self) -> i64 {
        self.core.close_count.load(Ordering::SeqCst)
    }

    /// `host.ts:35` `serviceCalls`.
    pub fn service_calls(&self) -> Vec<ServiceCall> {
        self.core
            .service_calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// `host.ts:36` `failAttachmentRelease`.
    pub fn set_fail_attachment_release(&self, error: Option<OperationError>) {
        *self
            .core
            .fail_attachment_release
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = error;
    }

    /// `host.ts:38` `nextServiceError`.
    pub fn set_next_service_error(&self, error: Option<OperationError>) {
        *self
            .core
            .next_service_error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = error;
    }

    /// `host.ts:39` `nextServiceResult`.
    pub fn set_next_service_result(&self, result: Option<ChordValue>) {
        *self
            .core
            .next_service_result
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = result;
    }

    /// `host.ts:38` `failClose`.
    pub fn set_fail_close(&self, error: Option<OperationError>) {
        *self
            .core
            .fail_close
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = error;
    }

    /// `host.ts:97` `closed`.
    pub fn closed(&self) -> Shared<BoxFuture<'static, ()>> {
        self.core.closed.promise()
    }

    /// `host.ts:31` `terminated`.
    pub fn terminated(&self) -> Shared<BoxFuture<'static, Option<OperationError>>> {
        self.terminated_signal.clone()
    }

    /// `host.ts:96-98` `terminate(error)`: only the termination promise;
    /// the harness holds no session to close.
    pub async fn terminate(&self, error: OperationError) {
        self.core.termination.resolve(Some(error));
    }

    /// `host.ts:106-110` `gateNextClose`.
    pub fn gate_next_close(&self) -> OpenGate {
        let gate = open_gate();
        *self
            .core
            .next_close_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(gate.clone());
        gate
    }

    /// `host.ts:112-116` `gateNextServiceCall`.
    pub fn gate_next_service_call(&self) -> OpenGate {
        let gate = open_gate();
        *self
            .core
            .next_service_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(gate.clone());
        gate
    }
}

impl RoutedSessionHandle for TestHarness {
    fn attach_client(
        &self,
        _context: Context,
    ) -> BoxFuture<'static, Result<Arc<dyn RoutedSessionAttachment>, OperationError>> {
        self.core.attached_clients.fetch_add(1, Ordering::SeqCst);
        let lease = Arc::new(HarnessLease {
            harness: self.core.clone(),
            released: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        });
        Box::pin(async move { Ok(lease as Arc<dyn RoutedSessionAttachment>) })
    }

    fn terminated(&self) -> Option<TerminatedSignal> {
        Some(self.terminated_signal.clone())
    }

    fn close(&self, _context: Context) -> BoxFuture<'static, Result<(), OperationError>> {
        let core = self.core.clone();
        Box::pin(async move {
            core.close_count.fetch_add(1, Ordering::SeqCst);
            let gate = core
                .next_close_gate
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            if let Some(gate) = gate {
                gate.entered.resolve(());
                let _ = gate.release.promise().await;
            }
            let failure = core
                .fail_close
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            if let Some(error) = failure {
                return Err(error);
            }
            core.closed.resolve(());
            core.termination.resolve(None);
            Ok(())
        })
    }
}

/// Upstream `createTestServerServices` (`host.ts:119-149`): the
/// session-management server service that routes attach/detach through the
/// presentation seam.
pub fn create_test_server_services() -> Arc<dyn RoutedServerServiceHost> {
    Arc::new(TestServerServices)
}

struct TestServerServices;

impl RoutedServerServiceHost for TestServerServices {
    fn attach_client(
        &self,
        presentation: Arc<dyn RoutedServerPresentation>,
        _context: Context,
    ) -> BoxFuture<'static, Result<Arc<dyn RoutedServerServiceAttachment>, OperationError>> {
        let attachment: Arc<dyn RoutedServerServiceAttachment> =
            Arc::new(TestServerServicesAttachment { presentation });
        Box::pin(async move { Ok(attachment) })
    }
}

struct TestServerServicesAttachment {
    presentation: Arc<dyn RoutedServerPresentation>,
}

impl RoutedServerServiceAttachment for TestServerServicesAttachment {
    fn invoke_service(
        &self,
        call: ServiceCall,
        _publish: PublishCallback,
        context: Context,
    ) -> BoxFuture<'static, Result<Option<ChordValue>, OperationError>> {
        let presentation = self.presentation.clone();
        Box::pin(async move {
            let is_session_management =
                call.instance.is_none() && call.service_id == "pi.session-management";
            if is_session_management
                && call.member == "attach"
                && call.args.len() == 1
                && call.args[0].is_string()
            {
                let session_id = call.args[0].as_str().expect("checked").to_string();
                presentation.attach_session(session_id, context).await?;
                return Ok(Some(serde_json::Value::Null));
            }
            if is_session_management && call.member == "detach" && call.args.is_empty() {
                presentation.detach_session(context).await?;
                return Ok(Some(serde_json::Value::Null));
            }
            Err(OperationError::Other(format!(
                "Unsupported test server service {}.{}",
                call.service_id, call.member
            )))
        })
    }

    fn release(&self, _context: Context) -> BoxFuture<'static, Result<(), OperationError>> {
        Box::pin(async { Ok(()) })
    }
}

/// Shared `TestServerHost` state (upstream the instance fields,
/// `host.ts:148-156`).
struct HostCore {
    server_services: Mutex<Arc<dyn RoutedServerServiceHost>>,
    sessions: Mutex<HashMap<String, SessionMetadata>>,
    harnesses: Mutex<HashMap<String, Vec<Arc<TestHarness>>>>,
    open_session_count: AtomicI64,
    next_open_session_error: Mutex<Option<OperationError>>,
    next_harness_close_error: Mutex<Option<OperationError>>,
    next_open_session_gate: Mutex<Option<OpenGate>>,
}

/// Upstream `TestServerHost` (`host.ts:146-193`).
pub struct TestServerHost {
    core: Arc<HostCore>,
}

impl TestServerHost {
    /// `new TestServerHost()`.
    pub fn new() -> Arc<TestServerHost> {
        Arc::new(TestServerHost {
            core: Arc::new(HostCore {
                server_services: Mutex::new(create_test_server_services()),
                sessions: Mutex::new(HashMap::new()),
                harnesses: Mutex::new(HashMap::new()),
                open_session_count: AtomicI64::new(0),
                next_open_session_error: Mutex::new(None),
                next_harness_close_error: Mutex::new(None),
                next_open_session_gate: Mutex::new(None),
            }),
        })
    }

    /// `host.ts:150` `sessions` — the metadata per session id.
    pub fn sessions(&self) -> HashMap<String, SessionMetadata> {
        self.core
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// `host.ts:151` `harnesses` — the harnesses per session id.
    pub fn harnesses(&self, id: &str) -> Vec<Arc<TestHarness>> {
        self.core
            .harnesses
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(id)
            .cloned()
            .unwrap_or_default()
    }

    /// `host.harnesses.size` — distinct session ids.
    pub fn harness_session_count(&self) -> usize {
        self.core
            .harnesses
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// `host.ts:152` `openSessionCount`.
    pub fn open_session_count(&self) -> i64 {
        self.core.open_session_count.load(Ordering::SeqCst)
    }

    /// `host.ts:153` `nextOpenSessionError`.
    pub fn set_next_open_session_error(&self, error: Option<OperationError>) {
        *self
            .core
            .next_open_session_error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = error;
    }

    /// `host.ts:154` `nextHarnessCloseError`.
    pub fn set_next_harness_close_error(&self, error: Option<OperationError>) {
        *self
            .core
            .next_harness_close_error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = error;
    }

    /// Replaces the server-services host (the upstream field is `readonly`;
    /// the oracle scenarios need the substitution, so the port exposes a
    /// setter disclosed as test-only surface).
    pub fn set_server_services(&self, services: Arc<dyn RoutedServerServiceHost>) {
        *self
            .core
            .server_services
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = services;
    }

    /// `host.ts:175-178` `seed(id = "session-1")`: register the metadata.
    pub fn seed(&self, id: &str) -> SessionMetadata {
        let metadata = SessionMetadata::new(id);
        self.core
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id.to_string(), metadata.clone());
        metadata
    }

    /// `host.ts:204-208` `gateNextOpenSession`.
    pub fn gate_next_open_session(&self) -> OpenGate {
        let gate = open_gate();
        *self
            .core
            .next_open_session_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(gate.clone());
        gate
    }

    /// `host.ts:210-214` `latestHarness(id)`.
    pub fn latest_harness(&self, id: &str) -> Result<Arc<TestHarness>, OperationError> {
        let harnesses = self
            .core
            .harnesses
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        harnesses
            .get(id)
            .and_then(|list| list.last().cloned())
            .ok_or_else(|| OperationError::Other(format!("No harness for {id}")))
    }
}

impl ServerHost for TestServerHost {
    fn server_services(&self) -> Arc<dyn RoutedServerServiceHost> {
        self.core
            .server_services
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn resolve_session(
        &self,
        session_id: String,
        _context: Context,
    ) -> BoxFuture<'static, Result<SessionMetadata, OperationError>> {
        let core = self.core.clone();
        Box::pin(async move {
            let metadata = core
                .sessions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(&session_id)
                .cloned();
            metadata.ok_or_else(|| {
                OperationError::Server(crate::server::errors::ServerError::session_not_found(Some(
                    format!("Unknown session: {session_id}"),
                )))
            })
        })
    }

    fn open_session(
        &self,
        metadata: SessionMetadata,
        _context: Context,
    ) -> BoxFuture<'static, Result<Arc<dyn RoutedSessionHandle>, OperationError>> {
        let core = self.core.clone();
        Box::pin(async move {
            core.open_session_count.fetch_add(1, Ordering::SeqCst);
            let gate = core
                .next_open_session_gate
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            if let Some(gate) = gate {
                gate.entered.resolve(());
                let _ = gate.release.promise().await;
            }
            if !core
                .sessions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains_key(&metadata.id)
            {
                return Err(OperationError::Server(
                    crate::server::errors::ServerError::session_not_found(Some(format!(
                        "Unknown session: {}",
                        metadata.id
                    ))),
                ));
            }
            if let Some(error) = core
                .next_open_session_error
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
            {
                return Err(error);
            }
            let harness = TestHarness::new(metadata.clone());
            if let Some(error) = core
                .next_harness_close_error
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
            {
                harness.set_fail_close(Some(error));
            }
            core.harnesses
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .entry(metadata.id.clone())
                .or_default()
                .push(harness.clone());
            Ok(harness as Arc<dyn RoutedSessionHandle>)
        })
    }
}
