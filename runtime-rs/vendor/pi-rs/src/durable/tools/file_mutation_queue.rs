//! Port of `src/tools/file-mutation-queue.ts`: serialize `edit` and `write`
//! mutations of one file within this process — same file system id and
//! canonical path, whichever environment object the call got (v1.0.0). Other
//! files, and other file systems, never wait. Concurrent calls on one file run
//! in the order their keys resolve. Not a lock against `bash` or other
//! processes.
//!
//! Upstream chains a promise queue per `${env.id}\0${canonicalPath}` key in one
//! global map; the port keeps an equivalent per-key async mutex in one global
//! map, with entries dropping out weakly (D33) where upstream deletes the tail
//! entry in its `finally`.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use tokio::sync::Mutex as AsyncMutex;

use crate::agent_core::chord_support::context::Context;
use crate::durable::env::{ExecutionEnv, FileErrorCode};
use crate::durable::errors::PlainError;

#[derive(Default)]
struct QueueState {
    /// Live per-key queues; entries drop out weakly (D33).
    paths: Mutex<HashMap<String, Weak<AsyncMutex<()>>>>,
}

/// The upstream module-level `queues` map: one process-wide key space.
fn queues() -> &'static QueueState {
    static QUEUES: OnceLock<QueueState> = OnceLock::new();
    QUEUES.get_or_init(QueueState::default)
}

/// The canonical path; for a file that does not exist yet, its canonical
/// parent joined with its name, so a `write` that creates a file and a later
/// mutation of it share one key even under a symlinked directory.
fn canonical(
    env: &dyn ExecutionEnv,
    absolute_path: &str,
    context: &Context,
) -> Result<String, PlainError> {
    match env.canonical_path(absolute_path, context) {
        Ok(canonical_path) => Ok(canonical_path),
        Err(error) if error.code == FileErrorCode::NotSupported => Ok(absolute_path.to_string()),
        Err(error) if error.code != FileErrorCode::NotFound => Err(PlainError::new(error.message)),
        Err(_) => {
            // The file system splits the path, so a name may contain
            // characters that are separators elsewhere.
            let parent = env
                .join_path(&[absolute_path, ".."], context)
                .map_err(|error| PlainError::new(error.message))?;
            if parent == absolute_path || !absolute_path.starts_with(&parent) {
                return Ok(absolute_path.to_string());
            }
            let name = &absolute_path[parent.len()
                + if parent.ends_with('/') || parent.ends_with('\\') {
                    0
                } else {
                    1
                }..];
            let canonical_parent = canonical(env, &parent, context)?;
            env.join_path(&[&canonical_parent, name], context)
                .map_err(|error| PlainError::new(error.message))
        }
    }
}

/// `mutationKey(env, path, context)`: the file system id and canonical path.
fn mutation_key(
    env: &dyn ExecutionEnv,
    path: &str,
    context: &Context,
) -> Result<String, PlainError> {
    let absolute_path = env
        .absolute_path(path, context)
        .map_err(|error| PlainError::new(error.message))?;
    Ok(format!(
        "{}\0{}",
        env.id(),
        canonical(env, &absolute_path, context)?
    ))
}

/// `withFileMutationQueue(env, path, fn, context)` (`tools/file-mutation-queue.ts`).
pub async fn with_file_mutation_queue<T, F, Fut>(
    env: &Arc<dyn ExecutionEnv>,
    path: &str,
    operation: F,
    context: &Context,
) -> Result<T, PlainError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<T, PlainError>>,
{
    let key = mutation_key(env.as_ref(), path, context)?;
    // Take the slot without awaiting, so no other call can take it in
    // between (the upstream read/set runs without an intervening await).
    let lock = {
        let state = queues();
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
    let _guard = lock.lock().await;
    // The upstream `finally` releases the queue even when `fn` throws; the
    // guard's drop covers both outcomes here.
    operation().await
}
