//! Synchronous registration regressions for upstream model-runtime.ts. Every
//! case uses a real ModelRuntime on a current-thread Tokio runtime, in-memory
//! stores, no models.json path, and faux or registration-only providers.

use super::*;

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use futures::FutureExt;
use tokio::sync::oneshot;
use tokio::time::timeout;

use crate::ai::auth::credential_store::InMemoryCredentialStore;
use crate::ai::auth::types::ProviderAuth;
use crate::ai::models::{
    faux_provider, FauxModelDefinition, FauxProviderOptions, InMemoryModelsStore,
    ModelsPublication, ModelsStoreEntry, ModelsStoreOperationOptions, RefreshModelsContext,
    RefreshModelsError,
};
use crate::coding_agent::core::provider_composer::ExtensionModelDefinition;

const ASYNC_DEADLINE: Duration = Duration::from_secs(5);

fn on_current_thread(test: impl Future<Output = ()> + Send + 'static) {
    let (done, completed) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::Builder::new()
        .name("model-runtime-registration".to_string())
        .spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let executor = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                executor.block_on(test);
                // Dropping this private executor also cancels any queued
                // fire-and-forget refreshes before reporting completion.
            }));
            let _ = done.send(outcome);
        })
        .unwrap();
    // An async timeout cannot interrupt a synchronous block_on/Mutex deadlock
    // on the same executor. This outer watchdog fails instead of joining a
    // stuck thread. The detached failure case owns only isolated test state.
    let outcome = completed
        .recv_timeout(Duration::from_secs(20))
        .expect("synchronous registration blocked the current-thread runtime");
    worker.join().unwrap();
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

async fn in_memory_runtime() -> (ModelRuntime, Arc<InMemoryModelsStore>) {
    let store = Arc::new(InMemoryModelsStore::default());
    let runtime = ModelRuntime::create(CreateModelRuntimeOptions {
        credentials: Some(Arc::new(InMemoryCredentialStore::default())),
        models_path: Some(None),
        models_store: Some(store.clone()),
        allow_model_network: false,
        refresh_on_create: Some(false),
        ..CreateModelRuntimeOptions::default()
    })
    .await
    .unwrap();
    (runtime, store)
}

fn fixture_provider(provider: &str, model_id: &str) -> Arc<dyn Provider> {
    faux_provider(FauxProviderOptions {
        provider: Some(provider.to_string()),
        api: Some("runtime-registration-faux".to_string()),
        models: vec![FauxModelDefinition {
            id: model_id.to_string(),
            ..FauxModelDefinition::default()
        }],
        ..FauxProviderOptions::default()
    })
    .provider
}

fn configured_provider(model_id: &str) -> ProviderConfigInput {
    ProviderConfigInput {
        base_url: Some("https://registration-fixture.invalid/v1".to_string()),
        // A literal fixture value, never a process/environment credential.
        api_key: Some("offline-registration-fixture-only".to_string()),
        api: Some("openai-completions".to_string()),
        stream_simple: Some(Arc::new(|_, _, _| {
            panic!("registration-only provider must not dispatch a model request")
        })),
        models: Some(vec![ExtensionModelDefinition {
            id: model_id.to_string(),
            name: model_id.to_string(),
            api: None,
            base_url: None,
            reasoning: false,
            thinking_level_map: None,
            input: vec![crate::ai::types::ModelInput::Text],
            cost: Default::default(),
            context_window: 4096,
            max_tokens: 512,
            sampling_params: None,
            sampling_params_by_thinking_level: None,
            headers: None,
            compat: None,
        }]),
        ..ProviderConfigInput::default()
    }
}

fn assert_catalog(runtime: &ModelRuntime, old_handle: &Models, provider: &str, expected: &[&str]) {
    let catalogs = [
        runtime.models().get_models(Some(provider)),
        old_handle.get_models(Some(provider)),
        runtime
            .snapshot()
            .all
            .into_iter()
            .filter(|model| model.provider == provider)
            .collect(),
    ];
    for catalog in catalogs {
        let ids: Vec<&str> = catalog.iter().map(|model| model.id.as_str()).collect();
        assert_eq!(ids.as_slice(), expected, "catalog for {provider}");
    }
}

#[test]
fn sync_native_registration_and_removal_are_immediate_on_current_thread() {
    on_current_thread(async {
        let (runtime, _) = in_memory_runtime().await;
        let old_handle = runtime.models();
        let old_runtime = runtime.clone();
        let provider = fixture_provider("native-sync", "first");

        runtime
            .register_native_provider_sync(Arc::clone(&provider))
            .unwrap();
        assert_catalog(&old_runtime, &old_handle, "native-sync", &["first"]);
        assert!(Arc::ptr_eq(
            &old_handle.get_provider("native-sync").unwrap(),
            &provider,
        ));
        assert!(Arc::ptr_eq(
            &old_runtime
                .get_registered_native_provider("native-sync")
                .unwrap(),
            &provider,
        ));
        assert_eq!(runtime.get_registered_provider_ids(), vec!["native-sync"]);

        let replacement = fixture_provider("native-sync", "second");
        old_runtime
            .register_native_provider_sync(Arc::clone(&replacement))
            .unwrap();
        assert_catalog(&runtime, &old_handle, "native-sync", &["second"]);
        assert!(Arc::ptr_eq(
            &old_handle.get_provider("native-sync").unwrap(),
            &replacement,
        ));
        assert_eq!(runtime.get_registered_provider_ids(), vec!["native-sync"]);

        for id in ["", "  ", "\t\n"] {
            let error = runtime
                .register_native_provider_sync(fixture_provider(id, "invalid"))
                .unwrap_err();
            assert_eq!(error.0, "Provider id must not be empty.");
            assert!(old_handle.get_provider(id).is_none());
        }
        runtime.unregister_provider_sync("never-registered");
        assert_catalog(&runtime, &old_handle, "native-sync", &["second"]);

        old_runtime.unregister_provider_sync("native-sync");
        assert_catalog(&runtime, &old_handle, "native-sync", &[]);
        assert!(old_handle.get_provider("native-sync").is_none());
        assert!(runtime
            .get_registered_native_provider("native-sync")
            .is_none());
        assert!(runtime.get_registered_provider_ids().is_empty());
        // No await after create: none of these assertions depends on a spawned
        // refresh running before register/unregister becomes observable.
    });
}

#[test]
fn sync_configured_registration_preserves_validation_and_provisional_availability() {
    on_current_thread(async {
        let (runtime, _) = in_memory_runtime().await;
        let old_handle = runtime.models();
        let mut first = configured_provider("first");
        first.headers = Some(vec![("x-fixture".to_string(), "retained".to_string())]);
        runtime
            .register_provider_sync("configured-sync", first)
            .unwrap();
        assert_catalog(&runtime, &old_handle, "configured-sync", &["first"]);
        assert!(runtime.has_configured_auth("configured-sync"));
        assert!(runtime
            .get_available_snapshot()
            .iter()
            .any(|model| { model.provider == "configured-sync" && model.id == "first" }));

        let mut second = configured_provider("second");
        second.api_key = None;
        runtime
            .register_provider_sync("configured-sync", second)
            .unwrap();
        assert_catalog(&runtime, &old_handle, "configured-sync", &["second"]);
        let saved = runtime
            .get_registered_provider_config("configured-sync")
            .unwrap();
        assert_eq!(
            saved.api_key.as_deref(),
            Some("offline-registration-fixture-only")
        );
        assert_eq!(
            saved.headers,
            Some(vec![("x-fixture".to_string(), "retained".to_string())])
        );
        assert_eq!(
            runtime.get_registered_provider_ids(),
            vec!["configured-sync"]
        );

        let before_invalid = old_handle.get_provider("configured-sync").unwrap();
        let mut invalid = configured_provider("must-not-replace");
        invalid.api = None;
        let error = runtime
            .register_provider_sync("configured-sync", invalid)
            .unwrap_err();
        assert_eq!(
            error.0,
            "Provider configured-sync: \"api\" is required when registering streamSimple.",
        );
        assert!(Arc::ptr_eq(
            &old_handle.get_provider("configured-sync").unwrap(),
            &before_invalid,
        ));
        assert_catalog(&runtime, &old_handle, "configured-sync", &["second"]);
        assert_eq!(
            runtime
                .get_registered_provider_config("configured-sync")
                .unwrap()
                .models
                .unwrap()[0]
                .id,
            "second",
        );

        runtime.unregister_provider_sync("configured-sync");
        assert_catalog(&runtime, &old_handle, "configured-sync", &[]);
        assert!(old_handle.get_provider("configured-sync").is_none());
        assert!(runtime
            .get_registered_provider_config("configured-sync")
            .is_none());
        assert!(runtime
            .get_available_snapshot()
            .iter()
            .all(|model| { model.provider != "configured-sync" }));
    });
}

#[test]
fn native_and_configured_registration_replace_each_other_only_after_validation() {
    on_current_thread(async {
        let (runtime, _) = in_memory_runtime().await;
        let old_handle = runtime.models();
        let native = fixture_provider("switching-provider", "native");
        runtime
            .register_native_provider_sync(Arc::clone(&native))
            .unwrap();

        let mut invalid = configured_provider("invalid");
        invalid.api = None;
        assert!(runtime
            .register_provider_sync("switching-provider", invalid)
            .is_err());
        assert!(Arc::ptr_eq(
            &runtime
                .get_registered_native_provider("switching-provider")
                .unwrap(),
            &native,
        ));
        assert_catalog(&runtime, &old_handle, "switching-provider", &["native"]);

        runtime
            .register_provider_sync("switching-provider", configured_provider("configured"))
            .unwrap();
        assert!(runtime
            .get_registered_native_provider("switching-provider")
            .is_none());
        assert_catalog(&runtime, &old_handle, "switching-provider", &["configured"]);

        runtime
            .register_native_provider_sync(Arc::clone(&native))
            .unwrap();
        assert!(runtime
            .get_registered_provider_config("switching-provider")
            .is_none());
        assert!(Arc::ptr_eq(
            &old_handle.get_provider("switching-provider").unwrap(),
            &native,
        ));
        assert_catalog(&runtime, &old_handle, "switching-provider", &["native"]);
        assert_eq!(
            runtime.get_registered_provider_ids(),
            vec!["switching-provider"]
        );
    });
}

#[test]
fn async_registration_wrappers_mutate_the_same_registry_without_waiting_for_refresh() {
    on_current_thread(async {
        let (runtime, _) = in_memory_runtime().await;
        let old_handle = runtime.models();
        runtime
            .register_native_provider(fixture_provider("async-native", "native"))
            .now_or_never()
            .expect("native wrapper must be ready without a background refresh")
            .unwrap();
        assert_catalog(&runtime, &old_handle, "async-native", &["native"]);
        runtime
            .register_provider("async-configured", configured_provider("configured"))
            .now_or_never()
            .expect("configured wrapper must be ready without a background refresh")
            .unwrap();
        assert_catalog(&runtime, &old_handle, "async-configured", &["configured"]);

        runtime
            .unregister_provider("async-native")
            .now_or_never()
            .expect("native removal must not await refresh");
        runtime
            .unregister_provider("async-configured")
            .now_or_never()
            .expect("configured removal must not await refresh");
        assert_catalog(&runtime, &old_handle, "async-native", &[]);
        assert_catalog(&runtime, &old_handle, "async-configured", &[]);
        assert!(runtime.get_registered_provider_ids().is_empty());
    });
}

struct RefreshGate {
    entered: oneshot::Sender<RefreshModelsContext>,
    release: oneshot::Receiver<()>,
}

struct GatedCatalogProvider {
    inner: Arc<dyn Provider>,
    catalog: Arc<Mutex<Vec<Model>>>,
    next_catalog: Vec<Model>,
    gate: Mutex<Option<RefreshGate>>,
    starts: AtomicUsize,
    updates: Arc<AtomicUsize>,
    drops: Arc<AtomicUsize>,
}

struct RefreshDrop(Arc<AtomicUsize>);

impl Drop for RefreshDrop {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

impl Provider for GatedCatalogProvider {
    fn id(&self) -> &str {
        self.inner.id()
    }

    fn name(&self) -> &str {
        self.inner.name()
    }

    fn auth(&self) -> &ProviderAuth {
        self.inner.auth()
    }

    fn get_models(&self) -> Result<Vec<Model>, ModelsError> {
        Ok(self.catalog.lock().unwrap().clone())
    }

    fn api_for(&self, model: &Model) -> Option<Arc<dyn crate::ai::ApiImpl>> {
        self.inner.api_for(model)
    }

    fn is_dynamic(&self) -> bool {
        true
    }

    fn refresh_models(
        &self,
        context: RefreshModelsContext,
    ) -> Option<BoxFuture<'static, Result<(), RefreshModelsError>>> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        let gate = self.gate.lock().unwrap().take();
        let catalog = Arc::clone(&self.catalog);
        let next_catalog = self.next_catalog.clone();
        let updates = Arc::clone(&self.updates);
        let drops = Arc::clone(&self.drops);
        Some(Box::pin(async move {
            let _drop = RefreshDrop(drops);
            assert!(
                !context.allow_network,
                "fixture must only receive offline refreshes"
            );
            let Some(gate) = gate else {
                return Ok(());
            };
            assert!(gate.entered.send(context.clone()).is_ok());
            gate.release
                .await
                .map_err(|_| RefreshModelsError::Cancelled)?;
            context
                .publish(ModelsPublication {
                    persist: Some(Some(ModelsStoreEntry {
                        models: next_catalog
                            .clone()
                            .into_iter()
                            .map(crate::ai::types::AnyModel::Chat)
                            .collect(),
                        ..ModelsStoreEntry::default()
                    })),
                    update: Some(Box::new(move || {
                        *catalog.lock().unwrap() = next_catalog;
                        updates.fetch_add(1, Ordering::SeqCst);
                    })),
                })
                .await?;
            Ok(())
        }))
    }
}

struct GatedFixture {
    provider: Arc<GatedCatalogProvider>,
    entered: oneshot::Receiver<RefreshModelsContext>,
    release: oneshot::Sender<()>,
}

fn gated_provider(id: &str) -> GatedFixture {
    let inner = fixture_provider(id, "before-refresh");
    let initial = inner.get_models().unwrap();
    let mut next_catalog = initial.clone();
    next_catalog[0].id = "after-refresh".to_string();
    next_catalog[0].name = "After refresh".to_string();
    let (entered, observe_entry) = oneshot::channel();
    let (release, wait_for_release) = oneshot::channel();
    GatedFixture {
        provider: Arc::new(GatedCatalogProvider {
            inner,
            catalog: Arc::new(Mutex::new(initial)),
            next_catalog,
            gate: Mutex::new(Some(RefreshGate {
                entered,
                release: wait_for_release,
            })),
            starts: AtomicUsize::new(0),
            updates: Arc::new(AtomicUsize::new(0)),
            drops: Arc::new(AtomicUsize::new(0)),
        }),
        entered: observe_entry,
        release,
    }
}

fn offline_refresh(provider: &str) -> ModelsRefreshOptions {
    ModelsRefreshOptions {
        providers: Some(vec![provider.to_string()]),
        allow_network: Some(false),
        ..ModelsRefreshOptions::default()
    }
}

async fn enter_gated_refresh<F: Future>(
    mut refresh: Pin<&mut F>,
    entered: oneshot::Receiver<RefreshModelsContext>,
) -> RefreshModelsContext {
    // Poll the public refresh exactly once to its in-memory gate, without
    // yielding the executor to previously queued fire-and-forget refreshes.
    // This is a single poll, not a spin/yield loop or a timing-based sleep.
    assert!(futures::poll!(refresh.as_mut()).is_pending());
    let context = timeout(ASYNC_DEADLINE, entered)
        .await
        .expect("dynamic refresh did not reach its controlled await")
        .expect("dynamic refresh closed the entry channel");
    assert!(!context.allow_network);
    assert!(!context.signal.is_cancelled());
    context
}

async fn assert_stale_publication_rejected(
    context: &RefreshModelsContext,
    provider: &GatedCatalogProvider,
    store: &InMemoryModelsStore,
) {
    assert!(context.signal.is_cancelled());
    let late_updates = Arc::new(AtomicUsize::new(0));
    let updates = Arc::clone(&late_updates);
    let catalog = Arc::clone(&provider.catalog);
    let stale_catalog = provider.next_catalog.clone();
    // Keep a context clone as an upstream unobserved promise could, and try
    // both persistence and an in-memory update after unregister/replacement.
    let result = timeout(
        ASYNC_DEADLINE,
        context.publish(ModelsPublication {
            persist: Some(Some(ModelsStoreEntry {
                models: stale_catalog
                    .clone()
                    .into_iter()
                    .map(crate::ai::types::AnyModel::Chat)
                    .collect(),
                ..ModelsStoreEntry::default()
            })),
            update: Some(Box::new(move || {
                *catalog.lock().unwrap() = stale_catalog;
                updates.fetch_add(1, Ordering::SeqCst);
            })),
        }),
    )
    .await
    .expect("superseded publication must not wait indefinitely");
    assert!(matches!(
        result,
        Ok(false) | Err(RefreshModelsError::Cancelled)
    ));
    assert_eq!(late_updates.load(Ordering::SeqCst), 0);
    assert_eq!(provider.get_models().unwrap()[0].id, "before-refresh");
    assert!(store
        .read(provider.id(), &ModelsStoreOperationOptions::NONE)
        .await
        .unwrap()
        .is_none());
}

#[test]
fn pending_dynamic_refresh_does_not_block_sync_mutations_or_restore_old_registry() {
    on_current_thread(async {
        let (runtime, store) = in_memory_runtime().await;
        let old_handle = runtime.models();
        let GatedFixture {
            provider,
            entered,
            release,
        } = gated_provider("refresh-owner");
        runtime
            .register_native_provider_sync(provider.clone())
            .unwrap();
        assert_eq!(provider.starts.load(Ordering::SeqCst), 0);
        let mut refresh = Box::pin(runtime.refresh(offline_refresh("refresh-owner")));
        let context = enter_gated_refresh(refresh.as_mut(), entered).await;
        assert_eq!(provider.starts.load(Ordering::SeqCst), 1);
        assert_eq!(provider.updates.load(Ordering::SeqCst), 0);

        let keeper = fixture_provider("native-kept", "native");
        runtime
            .register_native_provider_sync(Arc::clone(&keeper))
            .unwrap();
        runtime
            .register_provider_sync("configured-kept", configured_provider("configured"))
            .unwrap();
        runtime
            .register_provider_sync("removed-during-refresh", configured_provider("removed"))
            .unwrap();
        runtime.unregister_provider_sync("removed-during-refresh");
        assert_catalog(&runtime, &old_handle, "native-kept", &["native"]);
        assert_catalog(&runtime, &old_handle, "configured-kept", &["configured"]);
        assert_catalog(&runtime, &old_handle, "removed-during-refresh", &[]);
        assert!(
            !context.signal.is_cancelled(),
            "unrelated registration must not cancel this provider"
        );
        assert_eq!(provider.updates.load(Ordering::SeqCst), 0);

        release.send(()).unwrap();
        let result = timeout(ASYNC_DEADLINE, refresh.as_mut())
            .await
            .unwrap()
            .unwrap();
        assert!(!result.aborted);
        assert!(result.errors.is_empty());
        assert_eq!(provider.starts.load(Ordering::SeqCst), 1);
        assert_eq!(provider.updates.load(Ordering::SeqCst), 1);
        assert_eq!(provider.drops.load(Ordering::SeqCst), 1);
        assert_catalog(&runtime, &old_handle, "refresh-owner", &["after-refresh"]);
        assert_catalog(&runtime, &old_handle, "native-kept", &["native"]);
        assert_catalog(&runtime, &old_handle, "configured-kept", &["configured"]);
        assert_catalog(&runtime, &old_handle, "removed-during-refresh", &[]);
        assert!(Arc::ptr_eq(
            &old_handle.get_provider("native-kept").unwrap(),
            &keeper
        ));
        assert_eq!(
            store
                .read("refresh-owner", &ModelsStoreOperationOptions::NONE)
                .await
                .unwrap()
                .unwrap()
                .models[0]
                .id(),
            "after-refresh",
        );
    });
}

#[test]
fn unregister_cancels_pending_refresh_and_late_publication_cannot_resurrect_provider() {
    on_current_thread(async {
        let (runtime, store) = in_memory_runtime().await;
        let old_handle = runtime.models();
        let GatedFixture {
            provider,
            entered,
            release,
        } = gated_provider("removed-refresh-owner");
        runtime
            .register_native_provider_sync(provider.clone())
            .unwrap();
        let mut refresh = Box::pin(runtime.refresh(offline_refresh("removed-refresh-owner")));
        let context = enter_gated_refresh(refresh.as_mut(), entered).await;

        runtime
            .register_provider_sync("surviving-provider", configured_provider("survivor"))
            .unwrap();
        runtime.unregister_provider_sync("removed-refresh-owner");
        assert_catalog(&runtime, &old_handle, "removed-refresh-owner", &[]);
        assert!(old_handle.get_provider("removed-refresh-owner").is_none());
        assert!(runtime
            .get_registered_native_provider("removed-refresh-owner")
            .is_none());
        assert_stale_publication_rejected(&context, &provider, store.as_ref()).await;

        // Deliberately do not release the gate: removal must cancel the old
        // refresh, not rely on its provider to cooperate or complete first.
        let result = timeout(ASYNC_DEADLINE, refresh.as_mut())
            .await
            .unwrap()
            .unwrap();
        assert!(
            !result.aborted,
            "only the provider was superseded, not the caller"
        );
        assert!(
            result.errors.is_empty(),
            "provider cancellation is not an error"
        );
        assert_eq!(provider.starts.load(Ordering::SeqCst), 1);
        assert_eq!(provider.updates.load(Ordering::SeqCst), 0);
        assert_eq!(provider.drops.load(Ordering::SeqCst), 1);
        assert!(
            release.send(()).is_err(),
            "cancelled provider future must be dropped"
        );
        assert_catalog(&runtime, &old_handle, "removed-refresh-owner", &[]);
        assert_catalog(&runtime, &old_handle, "surviving-provider", &["survivor"]);
        assert!(runtime
            .get_available_snapshot()
            .iter()
            .all(|model| model.provider != "removed-refresh-owner"));
        assert_eq!(
            runtime.get_registered_provider_ids(),
            vec!["surviving-provider"]
        );
    });
}

#[test]
fn replacing_pending_native_provider_keeps_new_identity_after_old_refresh_finishes() {
    on_current_thread(async {
        let (runtime, store) = in_memory_runtime().await;
        let old_handle = runtime.models();
        let GatedFixture {
            provider,
            entered,
            release,
        } = gated_provider("replaced-refresh-owner");
        runtime
            .register_native_provider_sync(provider.clone())
            .unwrap();
        let mut refresh = Box::pin(runtime.refresh(offline_refresh("replaced-refresh-owner")));
        let context = enter_gated_refresh(refresh.as_mut(), entered).await;

        let replacement = fixture_provider("replaced-refresh-owner", "replacement");
        runtime
            .register_native_provider_sync(Arc::clone(&replacement))
            .unwrap();
        assert_catalog(
            &runtime,
            &old_handle,
            "replaced-refresh-owner",
            &["replacement"],
        );
        assert_stale_publication_rejected(&context, &provider, store.as_ref()).await;
        let result = timeout(ASYNC_DEADLINE, refresh.as_mut())
            .await
            .unwrap()
            .unwrap();
        assert!(!result.aborted);
        assert!(result.errors.is_empty());
        assert_eq!(provider.starts.load(Ordering::SeqCst), 1);
        assert_eq!(provider.updates.load(Ordering::SeqCst), 0);
        assert_eq!(provider.drops.load(Ordering::SeqCst), 1);
        assert!(release.send(()).is_err());
        assert_catalog(
            &runtime,
            &old_handle,
            "replaced-refresh-owner",
            &["replacement"],
        );
        assert!(Arc::ptr_eq(
            &old_handle.get_provider("replaced-refresh-owner").unwrap(),
            &replacement
        ));
        assert!(Arc::ptr_eq(
            &runtime
                .get_registered_native_provider("replaced-refresh-owner")
                .unwrap(),
            &replacement
        ));
        assert_eq!(
            runtime.get_registered_provider_ids(),
            vec!["replaced-refresh-owner"]
        );
    });
}
