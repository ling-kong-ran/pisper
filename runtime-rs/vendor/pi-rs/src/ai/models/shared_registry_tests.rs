//! Regressions for shared Models handles and callback re-entry. The ordering
//! oracle is the Map-backed registry in upstream packages/ai/src/models.ts.

use super::*;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crate::ai::auth::types::ProviderAuth;

fn fixture_provider(id: &str, model_id: &str) -> Arc<dyn Provider> {
    faux_provider(FauxProviderOptions {
        provider: Some(id.to_string()),
        api: Some("shared-registry-fixture".to_string()),
        models: vec![FauxModelDefinition {
            id: model_id.to_string(),
            ..FauxModelDefinition::default()
        }],
        ..FauxProviderOptions::default()
    })
    .provider
}

fn provider_ids(models: &Models) -> Vec<String> {
    models
        .get_providers()
        .iter()
        .map(|provider| provider.id().to_string())
        .collect()
}

fn catalog_ids(models: &Models, provider: Option<&str>) -> Vec<String> {
    models
        .get_models(provider)
        .iter()
        .map(|model| format!("{}/{}", model.provider, model.id))
        .collect()
}

#[test]
fn old_clones_observe_late_registration_and_upserts_preserve_order() {
    let mut original = create_models(CreateModelsOptions::default());
    let oldest_reader = original.clone();
    let mut writer = original.clone();
    let first = fixture_provider("z-first", "v1");
    let second = fixture_provider("a-second", "v1");

    writer.set_provider(Arc::clone(&first));
    assert!(Arc::ptr_eq(
        &oldest_reader.get_provider("z-first").unwrap(),
        &first,
    ));
    assert_eq!(catalog_ids(&original, None), vec!["z-first/v1"]);

    let later_reader = writer.clone();
    original.set_provider(Arc::clone(&second));
    let detached_listing = oldest_reader.get_providers();
    let replacement = fixture_provider("z-first", "v2");
    writer.set_provider(Arc::clone(&replacement));

    for reader in [&original, &oldest_reader, &later_reader, &writer] {
        assert_eq!(provider_ids(reader), vec!["z-first", "a-second"]);
        assert_eq!(catalog_ids(reader, None), vec!["z-first/v2", "a-second/v1"]);
        assert!(Arc::ptr_eq(
            &reader.get_provider("z-first").unwrap(),
            &replacement,
        ));
        assert!(Arc::ptr_eq(
            &reader.get_provider("a-second").unwrap(),
            &second,
        ));
        assert!(reader.get_model("z-first", "v1").is_none());
        assert!(reader.get_model("z-first", "v2").is_some());
    }
    // A returned provider array is a snapshot, not another registry handle.
    assert!(Arc::ptr_eq(&detached_listing[0], &first));
    assert!(Arc::ptr_eq(&detached_listing[1], &second));
}

#[test]
fn deletion_is_shared_and_reinsertion_appends_after_survivors() {
    let mut original = create_models(CreateModelsOptions::default());
    let old_reader = original.clone();
    let mut writer = original.clone();
    for id in ["third", "first", "second"] {
        original.set_provider(fixture_provider(id, "initial"));
    }

    writer.delete_provider("not-registered");
    assert_eq!(provider_ids(&old_reader), vec!["third", "first", "second"]);
    writer.delete_provider("first");
    for reader in [&original, &old_reader, &writer] {
        assert_eq!(provider_ids(reader), vec!["third", "second"]);
        assert!(reader.get_provider("first").is_none());
        assert!(reader.get_models(Some("first")).is_empty());
        assert!(reader.get_model("first", "initial").is_none());
    }

    original.set_provider(fixture_provider("first", "reinserted"));
    for reader in [&original, &old_reader, &writer] {
        assert_eq!(provider_ids(reader), vec!["third", "second", "first"]);
        assert_eq!(
            catalog_ids(reader, None),
            vec!["third/initial", "second/initial", "first/reinserted"],
        );
    }
}

#[test]
fn clear_is_shared_and_does_not_detach_preexisting_handles() {
    let mut original = create_models(CreateModelsOptions::default());
    original.set_provider(fixture_provider("one", "old"));
    original.set_provider(fixture_provider("two", "old"));
    let old_reader = original.clone();
    let mut clearing_handle = original.clone();

    clearing_handle.clear_providers();
    clearing_handle.clear_providers();
    for reader in [&original, &old_reader, &clearing_handle] {
        assert!(reader.get_providers().is_empty());
        assert!(reader.get_models(None).is_empty());
        assert!(reader.get_provider("one").is_none());
        assert!(reader.get_model("two", "old").is_none());
    }

    let mut another_writer = old_reader.clone();
    another_writer.set_provider(fixture_provider("after-clear", "new"));
    for reader in [&original, &old_reader, &clearing_handle, &another_writer] {
        assert_eq!(provider_ids(reader), vec!["after-clear"]);
        assert_eq!(catalog_ids(reader, None), vec!["after-clear/new"]);
    }
}

type OnceCallback = Box<dyn FnOnce() + Send>;

struct ReentrantProvider {
    inner: Arc<dyn Provider>,
    on_id: Mutex<Option<OnceCallback>>,
    on_get_models: Mutex<Option<OnceCallback>>,
}

impl ReentrantProvider {
    fn call_once(callback: &Mutex<Option<OnceCallback>>) {
        // Consume the guard before calling user code. Recursive reads then see
        // None, rather than recursing forever or locking this fixture's mutex.
        let callback = callback.lock().unwrap().take();
        if let Some(callback) = callback {
            callback();
        }
    }
}

impl Provider for ReentrantProvider {
    fn id(&self) -> &str {
        Self::call_once(&self.on_id);
        self.inner.id()
    }

    fn name(&self) -> &str {
        self.inner.name()
    }

    fn auth(&self) -> &ProviderAuth {
        self.inner.auth()
    }

    fn get_models(&self) -> Result<Vec<Model>, ModelsError> {
        Self::call_once(&self.on_get_models);
        self.inner.get_models()
    }

    fn api_for(&self, model: &Model) -> Option<Arc<dyn crate::ai::ApiImpl>> {
        self.inner.api_for(model)
    }
}

fn bounded_reentry(test: impl FnOnce() + Send + 'static) {
    let (done, completed) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::Builder::new()
        .name("models-registry-reentry".to_string())
        .spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(test));
            let _ = done.send(outcome);
        })
        .unwrap();
    // A synchronous Mutex deadlock cannot be interrupted by a Tokio timer.
    // Join only after the worker reports completion. On timeout its handle is
    // detached and this test fails; it owns only this test's in-memory objects,
    // so a regression cannot hang the test harness on a blocking join.
    let outcome = completed
        .recv_timeout(Duration::from_secs(10))
        .expect("provider callback blocked while re-entering its Models registry");
    worker.join().unwrap();
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

#[test]
fn provider_id_can_read_and_mutate_the_same_registry_before_insertion() {
    bounded_reentry(|| {
        let mut models = create_models(CreateModelsOptions::default());
        models.set_provider(fixture_provider("before", "baseline"));
        let mut callback_models = models.clone();
        let calls = Arc::new(AtomicUsize::new(0));
        let callback_calls = Arc::clone(&calls);
        let provider: Arc<dyn Provider> = Arc::new(ReentrantProvider {
            inner: fixture_provider("reentrant", "outer"),
            on_id: Mutex::new(Some(Box::new(move || {
                assert_eq!(provider_ids(&callback_models), vec!["before"]);
                assert_eq!(catalog_ids(&callback_models, None), vec!["before/baseline"]);
                assert!(callback_models.get_provider("reentrant").is_none());
                callback_models.set_provider(fixture_provider("during-id", "nested"));
                callback_calls.fetch_add(1, Ordering::SeqCst);
            }))),
            on_get_models: Mutex::new(None),
        });

        models.set_provider(Arc::clone(&provider));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            provider_ids(&models),
            vec!["before", "during-id", "reentrant"]
        );
        assert_eq!(
            catalog_ids(&models, None),
            vec!["before/baseline", "during-id/nested", "reentrant/outer"],
        );
        assert!(Arc::ptr_eq(
            &models.get_provider("reentrant").unwrap(),
            &provider
        ));
    });
}

fn check_get_models_reentry(filter: Option<&'static str>) {
    bounded_reentry(move || {
        let mut models = create_models(CreateModelsOptions::default());
        models.set_provider(fixture_provider("before", "baseline"));
        let callback_models = models.clone();
        let calls = Arc::new(AtomicUsize::new(0));
        let callback_calls = Arc::clone(&calls);
        models.set_provider(Arc::new(ReentrantProvider {
            inner: fixture_provider("reentrant", "catalog"),
            on_id: Mutex::new(None),
            on_get_models: Mutex::new(Some(Box::new(move || {
                assert_eq!(provider_ids(&callback_models), vec!["before", "reentrant"]);
                assert!(callback_models.get_provider("reentrant").is_some());
                // Exercise real recursive catalog reads; the consumed callback
                // is the one-shot recursion guard, not a substitute for re-entry.
                assert_eq!(
                    catalog_ids(&callback_models, Some("reentrant")),
                    vec!["reentrant/catalog"],
                );
                assert_eq!(
                    catalog_ids(&callback_models, None),
                    vec!["before/baseline", "reentrant/catalog"],
                );
                callback_calls.fetch_add(1, Ordering::SeqCst);
            }))),
        }));

        let expected = if filter.is_some() {
            vec!["reentrant/catalog"]
        } else {
            vec!["before/baseline", "reentrant/catalog"]
        };
        assert_eq!(catalog_ids(&models, filter), expected);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(catalog_ids(&models, filter), expected);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    });
}

#[test]
fn filtered_get_models_allows_provider_callback_reentry() {
    check_get_models_reentry(Some("reentrant"));
}

#[test]
fn unfiltered_get_models_allows_provider_callback_reentry() {
    check_get_models_reentry(None);
}
