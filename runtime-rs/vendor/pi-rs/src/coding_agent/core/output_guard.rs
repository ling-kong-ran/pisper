//! Upstream `core/output-guard.ts`: callback-complete raw writes, a persistent
//! promise tail, coded retries, and idempotent stdout takeover/restore.
//!
//! The router is real: ordinary writes through `write_stdout` go to the saved
//! stderr writer during takeover, while protocol writes use saved stdout.
//! Rust `println!` and a future JS host must explicitly use this router; Rust
//! cannot safely monkey-patch arbitrary process-wide stdout calls. Print/RPC
//! and the JS host must explicitly share this queue; this is not a claim of
//! global interception. The process adapter is UTF-8, and is not a Node stream codec.

use futures::future::{BoxFuture, FutureExt, Shared};
use serde_json::Value;
use std::fmt;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::sync::oneshot;

mod process_stream;

/// An IO boundary error. `code` is deliberately not coerced to a string:
/// upstream retries exactly three string codes, not numeric/null lookalikes.
#[derive(Debug, Clone, PartialEq)]
pub struct OutputError {
    pub message: String,
    pub code: Option<Value>,
}
impl OutputError {
    pub fn new(message: impl Into<String>, code: Option<Value>) -> Arc<Self> {
        Arc::new(Self {
            message: message.into(),
            code,
        })
    }
    fn retryable(&self) -> bool {
        matches!(
            self.code.as_ref().and_then(Value::as_str),
            Some("ENOBUFS" | "EAGAIN" | "EWOULDBLOCK")
        )
    }
}
impl fmt::Display for OutputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for OutputError {}
pub type WriteResult = Result<(), Arc<OutputError>>;
pub type WriteCallback = Box<dyn FnOnce(WriteResult) + Send>;

/// Buffer and plain Uint8Array stringify differently in the takeover wrapper.
#[derive(Debug, Clone, PartialEq)]
pub enum OutputChunk {
    Text(String),
    Buffer(Vec<u8>),
    Uint8Array(Vec<u8>),
}
impl OutputChunk {
    fn js_string(self) -> String {
        match self {
            Self::Text(text) => text,
            Self::Buffer(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Self::Uint8Array(bytes) => bytes
                .iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join(","),
        }
    }
}

/// A Node-style stream boundary: an immediate return/throw and an independent
/// completion callback. A callback followed by a throw settles only once.
pub trait OutputStream: Send + Sync {
    fn write(
        &self,
        chunk: OutputChunk,
        encoding: Option<String>,
        callback: Option<WriteCallback>,
    ) -> Result<bool, Arc<OutputError>>;
}

/// Injectable only to make the exact 10ms retries deterministic in tests.
pub trait RetryDelay: Send + Sync {
    fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()>;
}
struct TokioDelay;
impl RetryDelay for TokioDelay {
    fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()> {
        async move { tokio::time::sleep(duration).await }.boxed()
    }
}
struct Redirect(Arc<dyn OutputStream>);
impl OutputStream for Redirect {
    fn write(
        &self,
        chunk: OutputChunk,
        _: Option<String>,
        callback: Option<WriteCallback>,
    ) -> Result<bool, Arc<OutputError>> {
        // Both (chunk, callback) and (chunk, encoding, callback) upstream drop
        // encoding, stringify the chunk, and return the raw stderr boolean.
        self.0
            .write(OutputChunk::Text(chunk.js_string()), None, callback)
    }
}
struct Takeover {
    stdout: Arc<dyn OutputStream>,
}
struct Routing {
    stdout: Arc<dyn OutputStream>,
    stderr: Arc<dyn OutputStream>,
    takeover: Option<Takeover>,
}
type Tail = Shared<BoxFuture<'static, WriteResult>>;
struct Queue {
    tail: Tail,
    identity: Arc<()>,
}
struct Inner {
    routing: Mutex<Routing>,
    queue: Mutex<Queue>,
    delay: Arc<dyn RetryDelay>,
    exit: Arc<dyn Fn(i32) + Send + Sync>,
    runtime: Handle,
}

/// Owned guard allows isolated runtimes/tests without global sink replacement.
#[derive(Clone)]
pub struct OutputGuard {
    inner: Arc<Inner>,
}
impl OutputGuard {
    /// Must be constructed inside a Tokio runtime; enqueueing never blocks on IO.
    pub fn new(
        stdout: Arc<dyn OutputStream>,
        stderr: Arc<dyn OutputStream>,
        exit: Arc<dyn Fn(i32) + Send + Sync>,
    ) -> Self {
        Self::with_delay(stdout, stderr, exit, Arc::new(TokioDelay))
    }
    pub fn with_delay(
        stdout: Arc<dyn OutputStream>,
        stderr: Arc<dyn OutputStream>,
        exit: Arc<dyn Fn(i32) + Send + Sync>,
        delay: Arc<dyn RetryDelay>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                routing: Mutex::new(Routing {
                    stdout,
                    stderr,
                    takeover: None,
                }),
                queue: Mutex::new(Queue {
                    tail: async { Ok(()) }.boxed().shared(),
                    identity: Arc::new(()),
                }),
                delay,
                exit,
                runtime: Handle::current(),
            }),
        }
    }
    pub fn process() -> Self {
        Self::new(
            Arc::new(process_stream::ProcessStream::stdout()),
            Arc::new(process_stream::ProcessStream::stderr()),
            Arc::new(|code| std::process::exit(code)),
        )
    }
    pub fn take_over_stdout(&self) {
        let mut routing = self.inner.routing.lock().expect("stdout routing");
        if routing.takeover.is_some() {
            return;
        }
        let original = Arc::clone(&routing.stdout);
        routing.stdout = Arc::new(Redirect(Arc::clone(&routing.stderr)));
        routing.takeover = Some(Takeover { stdout: original });
    }
    pub fn restore_stdout(&self) {
        let mut routing = self.inner.routing.lock().expect("stdout routing");
        if let Some(saved) = routing.takeover.take() {
            routing.stdout = saved.stdout;
        }
    }
    pub fn is_stdout_taken_over(&self) -> bool {
        self.inner
            .routing
            .lock()
            .expect("stdout routing")
            .takeover
            .is_some()
    }
    /// JS-host hook for process.stdout.write assignment. Restoring a takeover
    /// reinstates its saved original, not a writer assigned while taken over.
    pub fn set_stdout_writer(&self, writer: Arc<dyn OutputStream>) {
        self.inner.routing.lock().expect("stdout routing").stdout = writer;
    }
    pub fn set_stderr_writer(&self, writer: Arc<dyn OutputStream>) {
        self.inner.routing.lock().expect("stdout routing").stderr = writer;
    }
    pub fn write_stdout(
        &self,
        chunk: OutputChunk,
        encoding: Option<String>,
        callback: Option<WriteCallback>,
    ) -> Result<bool, Arc<OutputError>> {
        let writer = Arc::clone(&self.inner.routing.lock().expect("stdout routing").stdout);
        writer.write(chunk, encoding, callback)
    }
    /// Empty strings do not extend the tail. Each non-empty enqueue attaches
    /// its own fatal handler; a rejected tail is never silently reset.
    pub fn write_raw_stdout(&self, text: &str) {
        if text.is_empty() {
            return;
        }
        let (previous, completion) = {
            let mut queue = self.inner.queue.lock().expect("stdout queue");
            let previous = queue.tail.clone();
            let (completion, receiver) = oneshot::channel();
            // The tail is only a completion notification, not a future that
            // recursively polls the previous tail. A JS Promise chain is
            // stack-safe even for thousands of enqueues without yielding.
            queue.tail = async move { receiver.await.expect("output writer runtime stays alive") }
                .boxed()
                .shared();
            queue.identity = Arc::new(());
            (previous, completion)
        };
        let guard = self.clone();
        let text = text.to_owned();
        let exit = Arc::clone(&self.inner.exit);
        self.inner.runtime.spawn(async move {
            let result = match previous.await {
                Ok(()) => guard.write_chunk(text).await,
                Err(error) => Err(error),
            };
            let failed = result.is_err();
            let _ = completion.send(result);
            // Every enqueue has its own catch handler, including those that
            // inherit a failed tail and therefore never attempt a write.
            if failed {
                exit(1);
            }
        });
    }

    pub async fn wait_for_raw_stdout_backpressure(&self) -> WriteResult {
        loop {
            let (tail, identity) = {
                let queue = self.inner.queue.lock().expect("stdout queue");
                (queue.tail.clone(), Arc::clone(&queue.identity))
            };
            tail.await?;
            if Arc::ptr_eq(
                &identity,
                &self.inner.queue.lock().expect("stdout queue").identity,
            ) {
                return Ok(());
            }
        }
    }
    /// The final empty write is not an enqueue: its error is returned, does
    /// not poison the tail, and does not attach a process-exit handler.
    pub async fn flush_raw_stdout(&self) -> WriteResult {
        self.wait_for_raw_stdout_backpressure().await?;
        self.write_chunk(String::new()).await
    }
    async fn write_chunk(&self, text: String) -> WriteResult {
        loop {
            let writer = {
                let routing = self.inner.routing.lock().expect("stdout routing");
                Arc::clone(
                    routing
                        .takeover
                        .as_ref()
                        .map(|s| &s.stdout)
                        .unwrap_or(&routing.stdout),
                )
            };
            let (sender, receiver) = oneshot::channel();
            let sender = Arc::new(Mutex::new(Some(sender)));
            let complete = Arc::clone(&sender);
            let callback = Box::new(move |result| {
                if let Some(sender) = complete.lock().expect("write completion").take() {
                    let _ = sender.send(result);
                }
            });
            if let Err(error) = writer.write(OutputChunk::Text(text.clone()), None, Some(callback))
            {
                if let Some(sender) = sender.lock().expect("write completion").take() {
                    let _ = sender.send(Err(error));
                }
            }
            // A stream dropping its callback cannot constitute success. Keep
            // the unsettled sender alive until this attempt actually completes.
            let result = receiver.await.expect("completion owner stays alive");
            drop(sender);
            match result {
                Err(error) if error.retryable() => {
                    self.inner.delay.sleep(Duration::from_millis(10)).await
                }
                other => return other,
            }
        }
    }
}

pub(crate) fn process_guard() -> &'static OutputGuard {
    static GUARD: OnceLock<OutputGuard> = OnceLock::new();
    GUARD.get_or_init(OutputGuard::process)
}
pub fn write_raw_stdout(text: &str) {
    process_guard().write_raw_stdout(text);
}
pub async fn wait_for_raw_stdout_backpressure() -> WriteResult {
    process_guard().wait_for_raw_stdout_backpressure().await
}
pub async fn flush_raw_stdout() -> WriteResult {
    process_guard().flush_raw_stdout().await
}
pub fn take_over_stdout() {
    process_guard().take_over_stdout();
}
pub fn restore_stdout() {
    process_guard().restore_stdout();
}
pub fn is_stdout_taken_over() -> bool {
    process_guard().is_stdout_taken_over()
}
pub fn write_stdout(
    chunk: OutputChunk,
    encoding: Option<String>,
    callback: Option<WriteCallback>,
) -> Result<bool, Arc<OutputError>> {
    process_guard().write_stdout(chunk, encoding, callback)
}

#[cfg(test)]
#[path = "output_guard_tests.rs"]
mod tests;
