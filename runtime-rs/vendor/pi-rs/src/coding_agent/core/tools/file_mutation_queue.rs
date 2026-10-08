//! Upstream file-mutation-queue.ts: canonical aliases share a FIFO mutation
//! queue. Registration order is established before asynchronous realpath.
use std::{
    collections::HashMap,
    future::{poll_fn, Future},
    sync::{Arc, LazyLock, Mutex, Weak},
    task::Poll,
};
use tokio::sync::Mutex as AsyncMutex;
static REGISTRATION: AsyncMutex<()> = AsyncMutex::const_new(());
static PATHS: LazyLock<Mutex<HashMap<String, Weak<AsyncMutex<()>>>>> =
    LazyLock::new(Mutex::default);
pub async fn with_file_mutation_queue<T, F, Fut>(path: &str, operation: F) -> Result<T, String>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<T, String>>,
{
    let registration = REGISTRATION.lock().await;
    let resolved = crate::coding_agent::utils::paths::resolve_path_auto_base(path)
        .map_err(|e| e.to_string())?;
    let key = match tokio::fs::canonicalize(&resolved).await {
        Ok(p) => crate::coding_agent::utils::paths::strip_verbatim_prefix(&p.to_string_lossy()),
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            resolved
        }
        Err(e) => return Err(super::io_error(e, "realpath", &resolved)),
    };
    let lock = {
        let mut paths = PATHS.lock().expect("mutation queues");
        paths.retain(|_, lock| lock.strong_count() > 0);
        match paths.get(&key).and_then(Weak::upgrade) {
            Some(lock) => lock,
            None => {
                let lock = Arc::new(AsyncMutex::new(()));
                paths.insert(key, Arc::downgrade(&lock));
                lock
            }
        }
    };
    let mut waiting = Box::pin(lock.lock_owned());
    let acquired = poll_fn(|cx| {
        Poll::Ready(match waiting.as_mut().poll(cx) {
            Poll::Ready(guard) => Some(guard),
            Poll::Pending => None,
        })
    })
    .await;
    drop(registration);
    let _guard = match acquired {
        Some(guard) => guard,
        None => waiting.await,
    };
    // Do not race an abort against a filesystem mutation: even a rejected
    // operation must settle before a following mutation starts.
    operation().await
}
