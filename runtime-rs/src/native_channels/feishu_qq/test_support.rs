use crate::native_channels::GatewayCallbacks;
use serde_json::Value;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
pub(super) struct Server {
    pub(super) url: String,
    cancel: tokio_util::sync::CancellationToken,
    task: Option<tokio::task::JoinHandle<()>>,
}
impl Server {
    pub(super) async fn new(router: axum::Router) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let cancel = tokio_util::sync::CancellationToken::new();
        let shutdown = cancel.clone();
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(shutdown.cancelled_owned())
                .await
                .unwrap()
        });
        Self {
            url,
            cancel,
            task: Some(task),
        }
    }
    pub(super) async fn close(&mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            tokio::time::timeout(Duration::from_secs(5), task)
                .await
                .unwrap()
                .unwrap()
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            task.abort()
        }
    }
}
#[derive(Default)]
pub(super) struct Recorder {
    pub(super) values: Mutex<Vec<Value>>,
    notify: tokio::sync::Notify,
}
impl Recorder {
    pub(super) fn push(&self, value: Value) {
        self.values.lock().unwrap().push(value);
        self.notify.notify_waiters()
    }
    pub(super) async fn wait(&self, count: usize) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let notified = self.notify.notified();
                if self.values.lock().unwrap().len() >= count {
                    return;
                }
                notified.await
            }
        })
        .await
        .unwrap();
    }
}
pub(super) fn callbacks() -> (GatewayCallbacks, Arc<Recorder>, Arc<Recorder>) {
    let messages = Arc::new(Recorder::default());
    let statuses = Arc::new(Recorder::default());
    let received = messages.clone();
    let updated = statuses.clone();
    (
        GatewayCallbacks {
            on_message: Arc::new(move |value| received.push(value)),
            on_status: Arc::new(move |value| updated.push(value)),
            on_sync: Arc::new(|_| {}),
        },
        messages,
        statuses,
    )
}
