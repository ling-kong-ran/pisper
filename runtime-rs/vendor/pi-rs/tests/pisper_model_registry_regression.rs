//! Pisper model registry regressions run against the production pi-rs library.
//! Credentials, catalogs and refreshes are in memory; no user data or network is used.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::BoxFuture;
use tokio::sync::oneshot;
use tokio::time::timeout;

use pi_rust::ai;
use pi_rust::ai::auth::credential_store::{CredentialStore, InMemoryCredentialStore};
use pi_rust::ai::auth::resolve::ModelsError;
use pi_rust::ai::auth::types::{AuthOperationOptions, Credential, ProviderAuth};
use pi_rust::ai::models::store::ModelsStore;
use pi_rust::ai::models::{
    create_models, faux_provider, CreateModelsOptions, FauxModelDefinition, FauxProviderOptions,
    InMemoryModelsStore, Models, ModelsPublication, ModelsRefreshOptions, ModelsStoreEntry,
    ModelsStoreOperationOptions, Provider, RefreshModelsContext, RefreshModelsError,
};
use pi_rust::ai::types::Model;
use pi_rust::coding_agent::core::model_runtime::{CreateModelRuntimeOptions, ModelRuntime};
use pi_rust::coding_agent::core::provider_composer::{
    ExtensionModelDefinition, ProviderConfigInput,
};

const ASYNC_DEADLINE: Duration = Duration::from_secs(5);

// 直接复用无 oracle 依赖的公开 Models 行为回归，保持原测试只读。
#[path = "../src/ai/models/shared_registry_tests.rs"]
mod shared_registry_tests;

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

async fn assert_catalog(
    runtime: &ModelRuntime,
    old_handle: &ModelRuntime,
    provider: &str,
    expected: &[&str],
) {
    for catalog in [
        runtime.get_models(Some(provider)).await,
        old_handle.get_models(Some(provider)).await,
    ] {
        let ids: Vec<&str> = catalog.iter().map(|model| model.id.as_str()).collect();
        assert_eq!(ids.as_slice(), expected, "catalog for {provider}");
    }
}
struct RegistryRebuildGate {
    owner: std::thread::ThreadId,
    entered: std::sync::mpsc::SyncSender<()>,
    release: std::sync::mpsc::Receiver<()>,
}

struct RegistryGateProvider {
    inner: Arc<dyn Provider>,
    gate: Mutex<Option<RegistryRebuildGate>>,
}

impl Provider for RegistryGateProvider {
    fn id(&self) -> &str {
        // 仅暂停指定重建线程；读者仍调用真实注册表和认证解析。
        let gate = {
            let mut gate = self.gate.lock().unwrap();
            if gate
                .as_ref()
                .is_some_and(|gate| gate.owner == std::thread::current().id())
            {
                gate.take()
            } else {
                None
            }
        };
        if let Some(gate) = gate {
            gate.entered.send(()).unwrap();
            gate.release.recv_timeout(ASYNC_DEADLINE).unwrap();
        }
        self.inner.id()
    }

    fn name(&self) -> &str {
        self.inner.name()
    }

    fn auth(&self) -> &ProviderAuth {
        self.inner.auth()
    }

    fn get_models(&self) -> Result<Vec<Model>, ModelsError> {
        self.inner.get_models()
    }

    fn api_for(&self, model: &Model) -> Option<Arc<dyn crate::ai::ApiImpl>> {
        self.inner.api_for(model)
    }
}

fn registry_gate_provider() -> Arc<RegistryGateProvider> {
    Arc::new(RegistryGateProvider {
        inner: fixture_provider("registry-gate", "gate-model"),
        gate: Mutex::new(None),
    })
}

fn start_gated_registry_rebuild(
    runtime: ModelRuntime,
    provider: Arc<RegistryGateProvider>,
) -> (
    std::thread::JoinHandle<()>,
    std::sync::mpsc::Receiver<()>,
    std::sync::mpsc::SyncSender<()>,
) {
    let (entered, entry) = std::sync::mpsc::sync_channel(1);
    let (release, released) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        *provider.gate.lock().unwrap() = Some(RegistryRebuildGate {
            owner: std::thread::current().id(),
            entered,
            release: released,
        });
        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        executor
            .block_on(runtime.refresh(ModelsRefreshOptions {
                allow_network: Some(false),
                ..Default::default()
            }))
            .unwrap();
    });
    (worker, entry, release)
}

#[test]
fn full_rebuild_keeps_stored_auth_and_shared_catalog_visible_during_provider_callbacks() {
    on_current_thread(async {
        let credentials = Arc::new(InMemoryCredentialStore::default());
        credentials
            .modify(
                "stored-chat",
                Box::new(|_| {
                    Box::pin(async {
                        Ok(Some(Credential::ApiKey(
                            crate::ai::auth::types::ApiKeyCredential {
                                key: Some("synthetic-registry-key".to_string()),
                                ..Default::default()
                            },
                        )))
                    })
                }),
                &AuthOperationOptions::default(),
            )
            .await
            .unwrap();
        let runtime = ModelRuntime::create(CreateModelRuntimeOptions {
            credentials: Some(credentials),
            models_path: Some(None),
            refresh_on_create: Some(false),
            ..Default::default()
        })
        .await
        .unwrap();
        let gate = registry_gate_provider();
        runtime.register_native_provider_sync(gate.clone()).unwrap();
        let mut chat = configured_provider("stored-model");
        chat.api_key = None;
        runtime.register_provider_sync("stored-chat", chat).unwrap();
        assert!(runtime
            .check_auth("stored-chat", None)
            .await
            .unwrap()
            .is_some());
        let old_handle = runtime.clone();
        let (rebuild, entered, release) = start_gated_registry_rebuild(runtime.clone(), gate);
        entered.recv_timeout(ASYNC_DEADLINE).unwrap();

        let (read, received) = std::sync::mpsc::sync_channel(1);
        let reader = std::thread::spawn(move || {
            let executor = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let auth = executor.block_on(runtime.check_auth("stored-chat", None));
            let catalog = executor.block_on(old_handle.get_models(Some("stored-chat")));
            read.send((auth, catalog)).unwrap();
        });
        let observed = received.recv_timeout(ASYNC_DEADLINE);
        // 即使读者错误地被注册表锁阻塞，也先释放回调，避免测试残留线程。
        release.send(()).unwrap();
        rebuild.join().unwrap();
        reader.join().unwrap();
        let (auth, catalog) = observed.expect("provider callback must not hold the registry lock");
        assert!(
            auth.unwrap().is_some(),
            "stored auth vanished during full rebuild"
        );
        assert_eq!(
            catalog
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            vec!["stored-model"]
        );
    });
}

#[test]
fn full_rebuild_preserves_registrations_replacements_and_deletions_after_composition_starts() {
    on_current_thread(async {
        let (runtime, _) = in_memory_runtime().await;
        runtime
            .register_native_provider_sync(fixture_provider("replaced-provider", "before"))
            .unwrap();
        runtime
            .register_native_provider_sync(fixture_provider("deleted-provider", "before"))
            .unwrap();
        // 最后一个 provider 的 id 回调暂停完整集合发布前的最后一步。
        let gate = registry_gate_provider();
        runtime.register_native_provider_sync(gate.clone()).unwrap();
        let old_handle = runtime.clone();
        let (rebuild, entered, release) = start_gated_registry_rebuild(runtime.clone(), gate);
        entered.recv_timeout(ASYNC_DEADLINE).unwrap();

        let replacement = fixture_provider("replaced-provider", "after");
        runtime
            .register_native_provider_sync(replacement.clone())
            .unwrap();
        runtime.unregister_provider_sync("deleted-provider");
        runtime
            .register_native_provider_sync(fixture_provider("added-provider", "added"))
            .unwrap();
        release.send(()).unwrap();
        rebuild.join().unwrap();

        assert_catalog(&runtime, &old_handle, "replaced-provider", &["after"]).await;
        assert!(Arc::ptr_eq(
            &old_handle.get_provider("replaced-provider").await.unwrap(),
            &replacement
        ));
        assert_catalog(&runtime, &old_handle, "deleted-provider", &[]).await;
        assert_catalog(&runtime, &old_handle, "added-provider", &["added"]).await;
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
fn pending_refresh_generations_reject_late_publication_after_rebuild_replace_or_delete() {
    on_current_thread(async {
        for mutation in ["rebuild", "replace", "delete"] {
            let (runtime, store) = in_memory_runtime().await;
            let old_handle = runtime.clone();
            let GatedFixture {
                provider,
                entered,
                release,
            } = gated_provider("refresh-owner");
            runtime
                .register_native_provider_sync(provider.clone())
                .unwrap();
            let mut refresh = Box::pin(runtime.refresh(offline_refresh("refresh-owner")));
            let context = enter_gated_refresh(refresh.as_mut(), entered).await;

            match mutation {
                "rebuild" => {
                    runtime
                        .refresh(ModelsRefreshOptions {
                            allow_network: Some(false),
                            ..Default::default()
                        })
                        .await
                        .unwrap();
                }
                "replace" => runtime
                    .register_native_provider_sync(fixture_provider("refresh-owner", "replacement"))
                    .unwrap(),
                "delete" => runtime.unregister_provider_sync("refresh-owner"),
                _ => unreachable!(),
            }
            assert_stale_publication_rejected(&context, &provider, store.as_ref()).await;
            // 不释放旧刷新：实际 supersede 必须取消它并丢弃其 future。
            let result = timeout(ASYNC_DEADLINE, refresh.as_mut())
                .await
                .unwrap()
                .unwrap();
            assert!(
                !result.aborted,
                "provider supersede must not abort the caller"
            );
            assert!(
                result.errors.is_empty(),
                "supersede must not become a refresh error"
            );
            assert_eq!(provider.updates.load(Ordering::SeqCst), 0);
            assert_eq!(
                provider.drops.load(Ordering::SeqCst),
                if mutation == "rebuild" { 2 } else { 1 }
            );
            assert!(
                release.send(()).is_err(),
                "superseded refresh future must be dropped"
            );
            let expected: &[&str] = match mutation {
                "rebuild" => &["before-refresh"],
                "replace" => &["replacement"],
                "delete" => &[],
                _ => unreachable!(),
            };
            assert_catalog(&runtime, &old_handle, "refresh-owner", expected).await;
        }
    });
}

struct RegistryReadingWake {
    read_registry: Box<dyn Fn() + Send + Sync>,
    calls: Arc<AtomicUsize>,
}

impl std::task::Wake for RegistryReadingWake {
    fn wake(self: Arc<Self>) {
        (self.read_registry)();
        self.calls.fetch_add(1, Ordering::SeqCst);
    }
}

fn arm_registry_reading_cancellation<'a>(
    context: &'a RefreshModelsContext,
    read_registry: impl Fn() + Send + Sync + 'static,
) -> (
    Pin<Box<dyn Future<Output = ()> + Send + 'a>>,
    Arc<AtomicUsize>,
) {
    let calls = Arc::new(AtomicUsize::new(0));
    let waker = std::task::Waker::from(Arc::new(RegistryReadingWake {
        read_registry: Box::new(read_registry),
        calls: calls.clone(),
    }));
    let mut cancelled: Pin<Box<dyn Future<Output = ()> + Send + 'a>> =
        Box::pin(context.signal.cancelled());
    assert!(cancelled
        .as_mut()
        .poll(&mut std::task::Context::from_waker(&waker))
        .is_pending());
    (cancelled, calls)
}

fn check_models_cancellation_reentry(mutation: &'static str) {
    // 外线程 watchdog 能捕获同步 waker 的 Mutex 自锁；不依赖同一 executor 的 timer。
    on_current_thread(async move {
        let models = create_models(CreateModelsOptions::default());
        let mut writer = models.clone();
        let GatedFixture {
            provider,
            entered,
            release,
        } = gated_provider("cancel-owner");
        writer.set_provider(provider);
        let mut refresh = Box::pin(models.refresh(offline_refresh("cancel-owner")));
        let context = enter_gated_refresh(refresh.as_mut(), entered).await;
        let reader = models.clone();
        let (cancelled, calls) = arm_registry_reading_cancellation(&context, move || {
            if mutation == "set" {
                assert!(reader.get_model("cancel-owner", "replacement").is_some());
            } else {
                assert!(reader.get_provider("cancel-owner").is_none());
            }
        });

        match mutation {
            "set" => writer.set_provider(fixture_provider("cancel-owner", "replacement")),
            "delete" => writer.delete_provider("cancel-owner"),
            "clear" => writer.clear_providers(),
            _ => unreachable!(),
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        cancelled.await;
        let result = timeout(ASYNC_DEADLINE, refresh.as_mut()).await.unwrap();
        assert!(result.errors.is_empty());
        assert!(release.send(()).is_err());
    });
}

#[test]
fn cancellation_waker_can_reenter_after_set_provider() {
    check_models_cancellation_reentry("set");
}

#[test]
fn cancellation_waker_can_reenter_after_delete_provider() {
    check_models_cancellation_reentry("delete");
}

#[test]
fn cancellation_waker_can_reenter_after_clear_providers() {
    check_models_cancellation_reentry("clear");
}

#[test]
fn cancellation_waker_can_reenter_after_full_registry_publication() {
    on_current_thread(async {
        let (runtime, _) = in_memory_runtime().await;
        let GatedFixture {
            provider,
            entered,
            release,
        } = gated_provider("cancel-owner");
        runtime.register_native_provider_sync(provider).unwrap();
        let mut refresh = Box::pin(runtime.refresh(offline_refresh("cancel-owner")));
        let context = enter_gated_refresh(refresh.as_mut(), entered).await;
        let reader = runtime.clone();
        let (cancelled, calls) = arm_registry_reading_cancellation(&context, move || {
            assert!(reader
                .get_model_sync("cancel-owner", "before-refresh")
                .is_some());
        });
        runtime
            .refresh(ModelsRefreshOptions {
                allow_network: Some(false),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        cancelled.await;
        let result = timeout(ASYNC_DEADLINE, refresh.as_mut())
            .await
            .unwrap()
            .unwrap();
        assert!(result.errors.is_empty());
        assert!(release.send(()).is_err());
    });
}
