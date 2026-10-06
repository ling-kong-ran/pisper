//! Sanity checks for the adaptive-publisher port (upstream has no dedicated
//! oracle file; its behavior is exercised through the output-capture tests).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use super::*;

fn publisher(publish_count: Arc<AtomicUsize>) -> AdaptivePublisher<String, String> {
    publisher_with(publish_count, |previous, current| {
        if previous != Some(current) {
            Some(current.clone())
        } else {
            None
        }
    })
}

fn publisher_with(
    publish_count: Arc<AtomicUsize>,
    update: impl Fn(Option<&String>, &String) -> Option<String> + Send + Sync + 'static,
) -> AdaptivePublisher<String, String> {
    AdaptivePublisher::new(AdaptivePublisherOptions {
        snapshot: Box::new(|| "current".to_string()),
        update: Box::new(update),
        measure: Box::new(|update| update.len()),
        publish: Box::new(move |_| {
            publish_count.fetch_add(1, Ordering::SeqCst);
        }),
        on_error: Box::new(|_| {}),
        min_interval_ms: Some(30),
        target_bytes_per_second: Some(100 * 1024),
    })
}

#[tokio::test]
async fn first_mark_dirty_publishes_immediately_and_dirty_gate_deduplicates() {
    let count = Arc::new(AtomicUsize::new(0));
    let publisher = publisher(Arc::clone(&count));
    publisher.mark_dirty();
    assert_eq!(count.load(Ordering::SeqCst), 1);
    // A flush without new dirt does not publish again.
    publisher.flush(true);
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn rate_limited_publications_collapse_into_one_trailing_flush() {
    let count = Arc::new(AtomicUsize::new(0));
    // The fixture snapshot never changes, so give this publisher a diff that
    // always reports dirt (the collapse semantics under test are the rate
    // limiter's, not the diff's).
    let publisher = publisher_with(Arc::clone(&count), |_, current| Some(current.clone()));
    publisher.mark_dirty();
    assert_eq!(count.load(Ordering::SeqCst), 1);
    // Inside the rate-limit window the writes collapse into the armed timer.
    publisher.mark_dirty();
    publisher.mark_dirty();
    assert_eq!(count.load(Ordering::SeqCst), 1);
    tokio::time::sleep(std::time::Duration::from_millis(120)).await;
    assert_eq!(count.load(Ordering::SeqCst), 2);
    publisher.dispose();
    publisher.mark_dirty();
    assert_eq!(count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn publish_panics_are_reported_through_on_error() {
    let errors = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let errors_writer = Arc::clone(&errors);
    let publisher: AdaptivePublisher<String, String> =
        AdaptivePublisher::new(AdaptivePublisherOptions {
            snapshot: Box::new(|| "current".to_string()),
            update: Box::new(|_, current| Some(current.clone())),
            measure: Box::new(|update| update.len()),
            publish: Box::new(|_| panic!("publish failed")),
            on_error: Box::new(move |message| errors_writer.lock().unwrap().push(message)),
            min_interval_ms: None,
            target_bytes_per_second: None,
        });
    publisher.mark_dirty();
    assert_eq!(*errors.lock().unwrap(), vec!["publish failed".to_string()]);
}
