//! Upstream tools/file-mutation-queue.ts. Registration is serialized before
//! canonical resolution, then only mutations of the same env/path share a lock.
use super::super::{Context, ExecutionEnv, FileErrorCode};
use std::collections::HashMap;
use std::future::{poll_fn, Future};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::task::Poll;
use tokio::sync::Mutex as AsyncMutex;

#[derive(Default)]
struct QueueState {
    registration: AsyncMutex<()>,
    paths: Mutex<HashMap<String, Weak<AsyncMutex<()>>>>,
}
struct EnvQueue {
    env: Weak<dyn ExecutionEnv>,
    state: Arc<QueueState>,
}
fn state_for(env: &Arc<dyn ExecutionEnv>) -> Arc<QueueState> {
    static STATES: OnceLock<Mutex<Vec<EnvQueue>>> = OnceLock::new();
    let mut states = STATES.get_or_init(Mutex::default).lock().unwrap();
    states.retain(|entry| entry.env.strong_count() > 0);
    let weak = Arc::downgrade(env);
    if let Some(entry) = states.iter().find(|entry| entry.env.ptr_eq(&weak)) {
        return entry.state.clone();
    }
    let state = Arc::new(QueueState::default());
    states.push(EnvQueue {
        env: weak,
        state: state.clone(),
    });
    state
}

pub(super) async fn with_file_mutation_queue<T, F, Fut>(
    env: &Arc<dyn ExecutionEnv>,
    path: &str,
    operation: F,
    context: Context,
) -> anyhow::Result<T>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = anyhow::Result<T>>,
{
    let state = state_for(env);
    let registration = state.registration.lock().await;
    let absolute = env.absolute_path(path, context.clone()).await?;
    let key = match env.canonical_path(&absolute, context).await {
        Ok(path) => path,
        Err(error)
            if matches!(
                error.code,
                FileErrorCode::NotFound | FileErrorCode::NotSupported
            ) =>
        {
            absolute
        }
        Err(error) => return Err(error.into()),
    };
    let lock = {
        let mut paths = state.paths.lock().unwrap();
        paths.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = paths.get(&key).and_then(Weak::upgrade) {
            lock
        } else {
            let lock = Arc::new(AsyncMutex::new(()));
            paths.insert(key, Arc::downgrade(&lock));
            lock
        }
    };
    let mut waiting = Box::pin(lock.lock_owned());
    // Poll once while holding registration, placing this waiter in Tokio's
    // FIFO before a second thread can register an alias of the same file.
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
    // Do not race cancellation against the write: its effect must settle
    // before releasing the queue. The tool checks the signal on both sides.
    operation().await
}
