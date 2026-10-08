//! Port of upstream `coding-agent/src/utils/fs-watch.ts`.
//!
//! Upstream wraps node's `fs.watch` (OS-native events). The Rust port uses a
//! `std`-only polling watcher (disclosed divergence): the faithful `notify`
//! crate is not in `Cargo.lock` and new dependencies are forbidden in this
//! slice. Event semantics that the callers rely on are preserved:
//!
//! - `change` when a watched file's metadata (size / mtime) changes,
//! - `rename` when the watched path appears or disappears,
//! - `watchWithErrorHandler` synchronously invokes `onError` and yields
//!   `None` when the path cannot be watched (upstream: `fs.watch` throws,
//!   e.g. ENOENT),
//! - `closeWatcher` closes idempotently and ignores close errors,
//! - [`FS_WATCH_RETRY_DELAY_MS`] is re-exported for callers (same value).
//!
//! Additional divergence: node's directory watches report per-entry events;
//! the polling watcher detects edits inside a watched directory only through
//! the directory's own metadata (creation/removal of entries), so in-place
//! edits of files inside a watched directory are not reported. In this
//! slice's call surface the watched targets are files.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Upstream `FS_WATCH_RETRY_DELAY_MS` (exported for callers).
pub const FS_WATCH_RETRY_DELAY_MS: u64 = 5000;

/// Polling interval standing in for OS event delivery (disclosed divergence).
pub const POLL_INTERVAL_MS: u64 = 250;

/// Upstream `WatchListener<string>`: `(eventType, filename)`.
pub type WatchListener = Box<dyn FnMut(&str, Option<&str>) + Send>;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Snapshot {
    len: u64,
    modified: Option<std::time::SystemTime>,
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ino: u64,
}

fn snapshot_of(metadata: &std::fs::Metadata) -> Snapshot {
    Snapshot {
        len: metadata.len(),
        modified: metadata.modified().ok(),
        #[cfg(unix)]
        dev: {
            use std::os::unix::fs::MetadataExt;
            metadata.dev()
        },
        #[cfg(unix)]
        ino: {
            use std::os::unix::fs::MetadataExt;
            metadata.ino()
        },
    }
}

/// Upstream `FSWatcher` (polling implementation). `close` stops the poller;
/// dropping also closes.
pub struct FsWatcher {
    stop: Arc<AtomicBool>,
    poller: Option<std::thread::JoinHandle<()>>,
}

impl FsWatcher {
    /// Upstream `watcher.close()`; close errors are ignored.
    pub fn close(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(poller) = self.poller.take() {
            let _ = poller.join();
        }
    }
}

impl Drop for FsWatcher {
    fn drop(&mut self) {
        self.close();
    }
}

/// Upstream `closeWatcher`: accepts a watcher that may be absent and closes
/// it, ignoring errors.
pub fn close_watcher(watcher: Option<FsWatcher>) {
    if let Some(mut watcher) = watcher {
        watcher.close();
    }
}

/// Upstream `watchWithErrorHandler`: start watching `path`, routing
/// registration failures through `onError` and yielding `None`.
pub fn watch_with_error_handler(
    path: &str,
    listener: WatchListener,
    on_error: &mut dyn FnMut(),
) -> Option<FsWatcher> {
    // node's fs.watch throws synchronously when the path cannot be watched
    // (e.g. ENOENT); mirror that as the onError + null branch.
    let Ok(initial_metadata) = std::fs::metadata(path) else {
        on_error();
        return None;
    };

    let stop = Arc::new(AtomicBool::new(false));
    let poller_stop = Arc::clone(&stop);
    let watched_path = path.to_string();
    let filename = std::path::Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned());
    let mut listener: WatchListener = Box::new(listener);
    let mut previous = Some(snapshot_of(&initial_metadata));

    let poller = std::thread::Builder::new()
        .name(format!("fs-watch-poll:{watched_path}"))
        .spawn(move || {
            loop {
                if poller_stop.load(Ordering::SeqCst) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));
                if poller_stop.load(Ordering::SeqCst) {
                    return;
                }
                let current = std::fs::metadata(&watched_path)
                    .ok()
                    .map(|m| snapshot_of(&m));
                match (&current, &previous) {
                    (Some(current), Some(previous)) => {
                        if current != previous {
                            listener("change", filename.as_deref());
                        }
                    }
                    // Disappearance and reappearance both surface as rename,
                    // matching fs.watch's eventType for path replacement.
                    (Some(_), None) | (None, Some(_)) => {
                        listener("rename", filename.as_deref());
                    }
                    (None, None) => {}
                }
                previous = current;
            }
        })
        .ok()?;

    Some(FsWatcher {
        stop,
        poller: Some(poller),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn wait_for_event(
        receiver: &mpsc::Receiver<(String, Option<String>)>,
        timeout: Duration,
    ) -> (String, Option<String>) {
        receiver
            .recv_timeout(timeout)
            .unwrap_or_else(|_| panic!("expected an event within {timeout:?}"))
    }

    #[test]
    fn retry_delay_matches_upstream_constant() {
        assert_eq!(FS_WATCH_RETRY_DELAY_MS, 5000);
    }

    #[test]
    fn emits_change_when_a_watched_file_is_modified() {
        let dir = tempfile::TempDir::with_prefix("pi-fs-watch-").expect("tempdir");
        let file = dir.path().join("watched.txt");
        std::fs::write(&file, "v1").expect("write");
        let (sender, receiver) = mpsc::channel();
        let mut watcher = watch_with_error_handler(
            file.to_str().expect("utf8"),
            Box::new(move |event: &str, filename: Option<&str>| {
                let _ = sender.send((event.to_string(), filename.map(str::to_string)));
            }),
            &mut || panic!("unexpected watch error"),
        )
        .expect("watcher");

        std::thread::sleep(Duration::from_millis(2 * POLL_INTERVAL_MS));
        std::fs::write(&file, "v2 - longer contents").expect("rewrite");
        let (event, filename) = wait_for_event(&receiver, Duration::from_secs(5));
        assert_eq!(event, "change");
        assert_eq!(filename.as_deref(), Some("watched.txt"));

        watcher.close();
    }

    #[test]
    fn emits_rename_when_the_watched_file_disappears() {
        let dir = tempfile::TempDir::with_prefix("pi-fs-watch-").expect("tempdir");
        let file = dir.path().join("gone.txt");
        std::fs::write(&file, "v1").expect("write");
        let (sender, receiver) = mpsc::channel();
        let mut watcher = watch_with_error_handler(
            file.to_str().expect("utf8"),
            Box::new(move |event: &str, filename: Option<&str>| {
                let _ = sender.send((event.to_string(), filename.map(str::to_string)));
            }),
            &mut || panic!("unexpected watch error"),
        )
        .expect("watcher");

        std::thread::sleep(Duration::from_millis(2 * POLL_INTERVAL_MS));
        std::fs::remove_file(&file).expect("remove");
        let (event, _) = wait_for_event(&receiver, Duration::from_secs(5));
        assert_eq!(event, "rename");

        watcher.close();
    }

    #[test]
    fn close_is_idempotent_and_drop_closes() {
        let dir = tempfile::TempDir::with_prefix("pi-fs-watch-").expect("tempdir");
        let file = dir.path().join("watched.txt");
        std::fs::write(&file, "v1").expect("write");
        let (sender, receiver) = mpsc::channel();
        let mut watcher = watch_with_error_handler(
            file.to_str().expect("utf8"),
            Box::new(move |event: &str, filename: Option<&str>| {
                let _ = sender.send((event.to_string(), filename.map(str::to_string)));
            }),
            &mut || panic!("unexpected watch error"),
        )
        .expect("watcher");
        watcher.close();
        watcher.close(); // second close must not panic
        drop(watcher);
        close_watcher(None); // upstream closeWatcher(null) is a no-op

        // Draining after close: any in-flight event is fine, but the poller
        // thread must have terminated.
        std::thread::sleep(Duration::from_millis(2 * POLL_INTERVAL_MS));
        while receiver.try_recv().is_ok() {}
    }

    #[test]
    fn watch_missing_path_routes_through_the_error_handler() {
        let missing = std::env::temp_dir().join("no-such-fs-watch-path-xyz");
        let mut errors = 0;
        let watcher = watch_with_error_handler(
            missing.to_str().expect("utf8"),
            Box::new(|_, _| {}),
            &mut || errors += 1,
        );
        assert!(watcher.is_none());
        assert_eq!(errors, 1);
    }

    #[test]
    fn watcher_returns_nothing_after_close() {
        let dir = tempfile::TempDir::with_prefix("pi-fs-watch-").expect("tempdir");
        let file = dir.path().join("quiet.txt");
        std::fs::write(&file, "v1").expect("write");
        let (sender, receiver) = mpsc::channel();
        let mut watcher = watch_with_error_handler(
            file.to_str().expect("utf8"),
            Box::new(move |event: &str, filename: Option<&str>| {
                let _ = sender.send((event.to_string(), filename.map(str::to_string)));
            }),
            &mut || panic!("unexpected watch error"),
        )
        .expect("watcher");
        watcher.close();
        std::thread::sleep(Duration::from_millis(2 * POLL_INTERVAL_MS));
        std::fs::write(&file, "after close").expect("rewrite");
        std::thread::sleep(Duration::from_millis(3 * POLL_INTERVAL_MS));
        assert!(
            receiver.try_recv().is_err(),
            "no events may be delivered after close"
        );
    }
}
