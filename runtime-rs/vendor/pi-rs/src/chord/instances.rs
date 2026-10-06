//! Keyed instance lifetime and the cancellable tasks observing those
//! instances. Port of `packages/chord/src/services/instances.ts` (upstream
//! sha256
//! `5d77d7ac5d2b6d6859b209c9ab8203bc638b52bd4149aba1c16e6236935a1192`).
//!
//! Upstream observer tasks run `Promise.resolve(handler(...)).catch(...)`
//! over a `withCancel(BACKGROUND_CONTEXT)` child context; the port is
//! synchronous (see the [`crate::chord`] module docs, divergence D2), so a
//! "task" is the handler invocation plus its stored cancellable child context
//! ([`crate::agent_core::harness::context::with_cancel`]). Cancelling an
//! observer closes the child contexts of its deliveries, which is what keyed
//! views observe to raise `... observation is closed` errors. Observer task
//! maps are keyed by `(key, generation)` (upstream keys by entry identity).
//! Handler invocations run outside the directory lock, matching the
//! upstream's unguarded single-threaded field access.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::agent_core::harness::context::{with_cancel, CancelledContext};
use crate::chord::context::Context;
use crate::chord::services::errors::ChordError;

use super::consumer::ErrorReporter;

/// The target handed to keyed observers (upstream `entry.service`, an opaque
/// object; here an `Arc<dyn Any>` the observer downcasts — either a local
/// implementation or a remote [`super::consumer::ServiceFacade`]).
pub type DirectoryTarget = Arc<dyn std::any::Any + Send + Sync>;

pub type DirectoryHandler =
    Arc<dyn Fn(&DirectoryTarget, &Context) -> Result<(), ChordError> + Send + Sync>;

/// The live identity of one entry, used as the observer task key.
type EntryIdentity = (String, u64);

/// Upstream `InstanceDirectoryEntry` (`instances.ts:4-9`).
pub struct InstanceDirectoryEntry {
    pub key: String,
    pub generation: u64,
    pub service: DirectoryTarget,
    deactivate: Box<dyn Fn() + Send + Sync>,
}

impl std::fmt::Debug for InstanceDirectoryEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InstanceDirectoryEntry")
            .field("key", &self.key)
            .field("generation", &self.generation)
            .finish()
    }
}

impl InstanceDirectoryEntry {
    pub fn new(
        key: impl Into<String>,
        generation: u64,
        service: DirectoryTarget,
        deactivate: Box<dyn Fn() + Send + Sync>,
    ) -> Self {
        InstanceDirectoryEntry {
            key: key.into(),
            generation,
            service,
            deactivate,
        }
    }

    fn call_deactivate(&self) {
        (self.deactivate)();
    }
}

/// Per-observer state, shared with the running tasks so a stop can cancel
/// the child contexts of the tasks it owns (upstream `Observer`,
/// `instances.ts:11-15`).
struct ObserverState {
    handler: DirectoryHandler,
    tasks: Mutex<HashMap<EntryIdentity, CancelledContext>>,
    closed: AtomicBool,
}

impl ObserverState {
    fn cancel_all(&self) {
        for (_, task) in self
            .tasks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .drain()
        {
            task.cancel();
        }
    }
}

struct DirectoryInner {
    entries: HashMap<String, InstanceDirectoryEntry>,
    observers: Vec<(u64, Arc<ObserverState>)>,
    ready: bool,
    disposed: bool,
    next_observer: u64,
}

struct DirectoryCore {
    inner: Mutex<DirectoryInner>,
    report_error: ErrorReporter,
}

/// A `#start` invocation collected under the lock and run after releasing it
/// (upstream `#start(observer, entry)`, `instances.ts:127-138`).
struct PendingStart {
    core: Arc<DirectoryCore>,
    observer: Arc<ObserverState>,
    identity: EntryIdentity,
    service: DirectoryTarget,
}

impl PendingStart {
    fn run(self) {
        if self.observer.closed.load(Ordering::SeqCst) {
            return;
        }
        let cancelled = with_cancel(Context::background());
        {
            let mut tasks = self
                .observer
                .tasks
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if tasks.contains_key(&self.identity) {
                return;
            }
            tasks.insert(self.identity.clone(), cancelled);
        }
        let context = self
            .observer
            .tasks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&self.identity)
            .map(|task| task.context.clone())
            .expect("registered above");
        let already_cancelled = context
            .abort_signal()
            .is_some_and(|signal| signal.is_cancelled());
        // The handler receives the task's child context, matching the
        // upstream `withCancel(BACKGROUND_CONTEXT)` delivery.
        let report_error = self.core.report_error.clone();
        let handler = self.observer.handler.clone();
        if let Err(error) = handler(&self.service, &context) {
            if !already_cancelled {
                report_error(&error);
            }
        }
    }
}

/// Port of upstream `InstanceDirectory` (`instances.ts:18-143`). Clones share
/// one directory, so stop closures can detach their observer.
#[derive(Clone)]
pub struct InstanceDirectory {
    core: Arc<DirectoryCore>,
}

impl std::fmt::Debug for InstanceDirectory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inner = self.lock();
        f.debug_struct("InstanceDirectory")
            .field("entries", &inner.entries.len())
            .field("observers", &inner.observers.len())
            .field("ready", &inner.ready)
            .field("disposed", &inner.disposed)
            .finish()
    }
}

impl InstanceDirectory {
    /// `new InstanceDirectory({ ready, onError })` (`instances.ts:25-28`).
    pub fn new(ready: bool, report_error: ErrorReporter) -> Self {
        InstanceDirectory {
            core: Arc::new(DirectoryCore {
                inner: Mutex::new(DirectoryInner {
                    entries: HashMap::new(),
                    observers: Vec::new(),
                    ready,
                    disposed: false,
                    next_observer: 0,
                }),
                report_error,
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, DirectoryInner> {
        self.core
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn assert_active(inner: &DirectoryInner) -> Result<(), ChordError> {
        if inner.disposed {
            return Err(ChordError::Type(
                "Keyed service directory is disposed".to_owned(),
            ));
        }
        Ok(())
    }

    /// `get observerCount` (`instances.ts:30-32`).
    pub fn observer_count(&self) -> usize {
        self.lock().observers.len()
    }

    //` values() (`instances.ts:34-36` at the delta): iterate the live
    //` entries. Upstream returns the `#entries` map's value iterator; the
    //` port snapshots them under the lock.
    pub fn values(&self) -> Vec<InstanceDirectoryEntry> {
        self.lock()
            .entries
            .values()
            .map(|entry| InstanceDirectoryEntry {
                key: entry.key.clone(),
                generation: entry.generation,
                service: Arc::clone(&entry.service),
                deactivate: Box::new(|| {}),
            })
            .collect()
    }

    /// `get(key)` (`instances.ts:34-36`), reduced to the identity the keyed
    /// binding routes by (`generation` comparisons).
    pub fn generation_of(&self, key: &str) -> Option<u64> {
        self.lock().entries.get(key).map(|entry| entry.generation)
    }

    /// The live entry's service target, when its generation still matches.
    pub fn service_of(&self, key: &str, generation: u64) -> Option<DirectoryTarget> {
        let inner = self.lock();
        let entry = inner.entries.get(key)?;
        if entry.generation != generation {
            return None;
        }
        Some(entry.service.clone())
    }

    /// `insert(entry)` (`instances.ts:38-45`).
    pub fn insert(&self, entry: InstanceDirectoryEntry) -> Result<(), ChordError> {
        let starts = {
            let mut inner = self.lock();
            Self::assert_active(&inner)?;
            if inner.entries.contains_key(&entry.key) {
                return Err(ChordError::Type(format!(
                    "Keyed service already has a live instance with key {}",
                    entry.key
                )));
            }
            let mut starts = Vec::new();
            let identity = (entry.key.clone(), entry.generation);
            let service = entry.service.clone();
            inner.entries.insert(entry.key.clone(), entry);
            if inner.ready {
                starts.extend(Self::collect_starts(self, &mut inner, identity, service));
            }
            starts
        };
        run_starts(starts);
        Ok(())
    }

    /// `replace(entry)` (`instances.ts:47-58`).
    pub fn replace(&self, entry: InstanceDirectoryEntry) -> Result<(), ChordError> {
        let starts = {
            let mut inner = self.lock();
            Self::assert_active(&inner)?;
            if let Some(previous) = inner.entries.get(&entry.key) {
                if previous.generation == entry.generation {
                    return Err(ChordError::Type(
                        "Keyed service repeated a live generation".to_owned(),
                    ));
                }
            }
            if let Some(previous) = inner.entries.remove(&entry.key) {
                Self::remove_tasks(&mut inner, &(previous.key.clone(), previous.generation));
                previous.call_deactivate();
            }
            let mut starts = Vec::new();
            let identity = (entry.key.clone(), entry.generation);
            let service = entry.service.clone();
            inner.entries.insert(entry.key.clone(), entry);
            if inner.ready {
                starts.extend(Self::collect_starts(self, &mut inner, identity, service));
            }
            starts
        };
        run_starts(starts);
        Ok(())
    }

    /// `remove(entry)` (`instances.ts:60-63`): identity is (key, generation).
    pub fn remove(&self, key: &str, generation: u64) {
        let mut inner = self.lock();
        let matches = inner
            .entries
            .get(key)
            .is_some_and(|entry| entry.generation == generation);
        if !matches {
            return;
        }
        let entry = inner.entries.remove(key).expect("checked above");
        Self::remove_tasks(&mut inner, &(key.to_owned(), generation));
        entry.call_deactivate();
    }

    /// `ready()` (`instances.ts:65-70`).
    pub fn ready(&self) -> Result<(), ChordError> {
        let starts = {
            let mut inner = self.lock();
            Self::assert_active(&inner)?;
            let mut starts = Vec::new();
            if !inner.ready {
                inner.ready = true;
                let planned: Vec<(EntryIdentity, DirectoryTarget)> = inner
                    .entries
                    .values()
                    .map(|entry| ((entry.key.clone(), entry.generation), entry.service.clone()))
                    .collect();
                for (identity, service) in planned {
                    starts.extend(Self::collect_starts(self, &mut inner, identity, service));
                }
            }
            starts
        };
        run_starts(starts);
        Ok(())
    }

    /// `reset()` (`instances.ts:72-76`).
    pub fn reset(&self) {
        let mut inner = self.lock();
        if inner.disposed {
            return;
        }
        inner.ready = false;
        let removed: Vec<InstanceDirectoryEntry> =
            inner.entries.drain().map(|(_, entry)| entry).collect();
        for entry in removed {
            Self::remove_tasks(&mut inner, &(entry.key.clone(), entry.generation));
            entry.call_deactivate();
        }
    }

    /// `observe(handler)` (`instances.ts:78-96`). Returns the stop closure.
    pub fn observe(
        &self,
        handler: DirectoryHandler,
    ) -> Result<Box<dyn Fn() + Send + Sync>, ChordError> {
        let (observer_id, observer, starts) = {
            let mut inner = self.lock();
            Self::assert_active(&inner)?;
            let id = inner.next_observer;
            inner.next_observer += 1;
            let observer = Arc::new(ObserverState {
                handler,
                tasks: Mutex::new(HashMap::new()),
                closed: AtomicBool::new(false),
            });
            inner.observers.push((id, observer.clone()));
            let mut starts = Vec::new();
            if inner.ready {
                let planned: Vec<(EntryIdentity, DirectoryTarget)> = inner
                    .entries
                    .values()
                    .map(|entry| ((entry.key.clone(), entry.generation), entry.service.clone()))
                    .collect();
                for (identity, service) in planned {
                    starts.extend(Self::collect_starts(self, &mut inner, identity, service));
                }
            }
            (id, observer, starts)
        };
        run_starts(starts);
        let core = self.core.clone();
        Ok(Box::new(move || {
            if observer.closed.swap(true, Ordering::SeqCst) {
                return;
            }
            observer.cancel_all();
            let mut inner = core
                .inner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            inner.observers.retain(|(at, _)| *at != observer_id);
        }))
    }

    /// `dispose()` (`instances.ts:98-111`).
    pub fn dispose(&self) {
        let mut inner = self.lock();
        if inner.disposed {
            return;
        }
        inner.disposed = true;
        for (_, observer) in inner.observers.iter() {
            observer.closed.store(true, Ordering::SeqCst);
            observer.cancel_all();
        }
        inner.observers.clear();
        let removed: Vec<InstanceDirectoryEntry> =
            inner.entries.drain().map(|(_, entry)| entry).collect();
        for entry in removed {
            entry.call_deactivate();
        }
    }

    fn remove_tasks(inner: &mut DirectoryInner, identity: &EntryIdentity) {
        for (_, observer) in inner.observers.iter() {
            observer
                .tasks
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(identity);
        }
    }

    /// `#startAll(entry)` (`instances.ts:123-125`): plan one task per
    /// non-closed observer that is not already observing the entry.
    fn collect_starts(
        directory: &InstanceDirectory,
        inner: &mut DirectoryInner,
        identity: EntryIdentity,
        service: DirectoryTarget,
    ) -> Vec<PendingStart> {
        let mut starts = Vec::new();
        for (_, observer) in inner.observers.iter() {
            if observer.closed.load(Ordering::SeqCst)
                || observer
                    .tasks
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .contains_key(&identity)
            {
                continue;
            }
            starts.push(PendingStart {
                core: directory.core.clone(),
                observer: observer.clone(),
                identity: identity.clone(),
                service: service.clone(),
            });
        }
        starts
    }
}

fn run_starts(starts: Vec<PendingStart>) {
    for start in starts {
        start.run();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn reporter(log: &Arc<Mutex<Vec<String>>>) -> ErrorReporter {
        let log = log.clone();
        Arc::new(move |error: &ChordError| {
            log.lock().unwrap().push(error.message().to_owned());
        })
    }

    fn entry(key: &str, generation: u64, counter: &Arc<AtomicUsize>) -> InstanceDirectoryEntry {
        let counter = counter.clone();
        InstanceDirectoryEntry::new(
            key,
            generation,
            Arc::new(()),
            Box::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
            }),
        )
    }

    #[test]
    fn starts_tasks_for_existing_entries_when_observing_late() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let directory = Arc::new(InstanceDirectory::new(true, reporter(&log)));
        let deactivated = Arc::new(AtomicUsize::new(0));
        directory.insert(entry("a", 1, &deactivated)).unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_for_handler = seen.clone();
        let stop = directory
            .observe(Arc::new(
                move |service: &DirectoryTarget, _context: &Context| {
                    seen_for_handler
                        .lock()
                        .unwrap()
                        .push(Arc::as_ptr(service).addr());
                    Ok(())
                },
            ))
            .unwrap();
        assert_eq!(seen.lock().unwrap().len(), 1);
        assert_eq!(directory.observer_count(), 1);
        stop();
        assert_eq!(directory.observer_count(), 0);
        stop(); // second stop is a no-op
    }

    #[test]
    fn insert_duplicate_and_repeated_generation_errors() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let directory = InstanceDirectory::new(true, reporter(&log));
        let counter = Arc::new(AtomicUsize::new(0));
        directory.insert(entry("a", 1, &counter)).unwrap();
        assert_eq!(
            directory
                .insert(entry("a", 2, &counter))
                .unwrap_err()
                .message(),
            "Keyed service already has a live instance with key a"
        );
        directory.remove("a", 1);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
        // Removing with a stale generation is a no-op.
        directory.remove("a", 1);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
        directory.insert(entry("a", 1, &counter)).unwrap();
        assert_eq!(
            directory
                .replace(entry("a", 1, &counter))
                .unwrap_err()
                .message(),
            "Keyed service repeated a live generation"
        );
        directory.replace(entry("a", 2, &counter)).unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 2);
        assert_eq!(directory.generation_of("a"), Some(2));
    }

    #[test]
    fn reset_and_dispose_deactivate_and_block() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let directory = InstanceDirectory::new(false, reporter(&log));
        let counter = Arc::new(AtomicUsize::new(0));
        directory.insert(entry("a", 1, &counter)).unwrap();
        directory.reset();
        assert_eq!(counter.load(Ordering::SeqCst), 1);
        directory.dispose();
        assert_eq!(
            directory
                .insert(entry("b", 1, &counter))
                .unwrap_err()
                .message(),
            "Keyed service directory is disposed"
        );
    }

    #[test]
    fn handler_failures_are_reported() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let directory = InstanceDirectory::new(true, reporter(&log));
        let _stop = directory
            .observe(Arc::new(
                |_service: &DirectoryTarget, _context: &Context| {
                    Err(ChordError::Type("boom".to_owned()))
                },
            ))
            .unwrap();
        let counter = Arc::new(AtomicUsize::new(0));
        directory.insert(entry("a", 1, &counter)).unwrap();
        assert_eq!(*log.lock().unwrap(), vec!["boom"]);
    }
}
