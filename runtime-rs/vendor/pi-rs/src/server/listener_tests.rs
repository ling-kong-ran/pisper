//! Port of `packages/server/test/listener.test.ts` (52 lines, SHA256
//! `4cb835caa4f3d59e33dbcac31490e2d333b6741322a5f324a996a280a5bf1890`):
//! server listener composition.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;

use super::connection::ByteConnectionAcceptor;
use super::errors::OperationError;
use super::listener::{ServerListener, ServerListenerHandle};
use super::testing::{create_test_server, TestServerOptions};

struct TestListener {
    accept: Mutex<Option<ByteConnectionAcceptor>>,
    close_count: AtomicI64,
    start_error: Option<OperationError>,
}

impl TestListener {
    fn new(start_error: Option<OperationError>) -> Arc<TestListener> {
        Arc::new(TestListener {
            accept: Mutex::new(None),
            close_count: AtomicI64::new(0),
            start_error,
        })
    }

    fn close_count(&self) -> i64 {
        self.close_count.load(Ordering::SeqCst)
    }
}

impl ServerListener for TestListener {
    fn start(
        &self,
        accept: ByteConnectionAcceptor,
    ) -> BoxFuture<'static, Result<(), OperationError>> {
        *self.accept.lock().unwrap() = Some(accept);
        let error = self.start_error.clone();
        Box::pin(async move {
            if let Some(error) = error {
                return Err(error);
            }
            Ok(())
        })
    }

    fn close(&self) -> BoxFuture<'static, Result<(), OperationError>> {
        self.close_count.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(()) })
    }
}

fn options(listeners: Vec<ServerListenerHandle>) -> TestServerOptions {
    TestServerOptions {
        listeners,
        host: None,
        server_id: None,
        max_frame_length: None,
        handshake_timeout_ms: None,
    }
}

#[tokio::test]
async fn starts_and_closes_every_configured_listener() {
    let first = TestListener::new(None);
    let second = TestListener::new(None);
    let test = create_test_server(options(vec![first.clone(), second.clone()])).unwrap();

    test.server.start().await.unwrap();
    assert!(first.accept.lock().unwrap().is_some());
    assert!(second.accept.lock().unwrap().is_some());

    test.server.close().await.unwrap();
    assert_eq!(first.close_count(), 1);
    assert_eq!(second.close_count(), 1);
}

#[tokio::test]
async fn closes_previously_started_listeners_when_startup_fails() {
    let first = TestListener::new(None);
    let failure = OperationError::Other("listener failed".to_string());
    let second = TestListener::new(Some(failure.clone()));
    let test = create_test_server(options(vec![first.clone(), second.clone()])).unwrap();

    let error = test.server.start().await.unwrap_err();
    assert_eq!(error.message(), failure.message());
    assert_eq!(first.close_count(), 1);
    assert_eq!(second.close_count(), 0);
}
