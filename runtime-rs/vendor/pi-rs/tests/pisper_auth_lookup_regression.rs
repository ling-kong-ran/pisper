//! Allocation and snapshot-publication regressions for the production auth lookup.
//! Only these tests' current threads count allocations; concurrent tests are excluded.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use pi_rust::ai::auth::credential_store::{CredentialStore, InMemoryCredentialStore};
use pi_rust::ai::auth::resolve::ModelsError;
use pi_rust::ai::auth::types::{
    ApiKeyAuth, ApiKeyAuthInput, ApiKeyCredential, AuthCheck, AuthError, AuthEvent,
    AuthInteraction, AuthOperationOptions, AuthPrompt, AuthResult, AuthType, Credential,
    ProviderAuth, ProviderAuthInteraction,
};
use pi_rust::ai::models::{
    faux_provider, FauxModelDefinition, FauxProviderOptions, InMemoryModelsStore,
    ModelsRefreshOptions, Provider,
};
use pi_rust::ai::types::Model;
use pi_rust::coding_agent::core::model_runtime::{CreateModelRuntimeOptions, ModelRuntime};
use tokio_util::sync::CancellationToken;

thread_local! {
    static ALLOCATIONS: Cell<Option<usize>> = const { Cell::new(None) };
}

struct ThreadCountingAllocator;

fn record_allocation() {
    // TLS destruction and allocations on every other thread remain unobserved.
    let _ = ALLOCATIONS.try_with(|counter| {
        if let Some(count) = counter.get() {
            counter.set(Some(count + 1));
        }
    });
}

// SAFETY: all pointers and layouts pass unchanged to System. Counting uses a
// const-initialized Cell and cannot allocate or alter the allocator's contract.
unsafe impl GlobalAlloc for ThreadCountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record_allocation();
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record_allocation();
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        record_allocation();
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: ThreadCountingAllocator = ThreadCountingAllocator;

fn count_allocations<T>(operation: impl FnOnce() -> T) -> (T, usize) {
    struct StopCounting;
    impl Drop for StopCounting {
        fn drop(&mut self) {
            ALLOCATIONS.with(|counter| counter.set(None));
        }
    }
    ALLOCATIONS.with(|counter| assert!(counter.replace(Some(0)).is_none()));
    let stop = StopCounting;
    let result = operation();
    let count = ALLOCATIONS.with(|counter| counter.get().unwrap());
    drop(stop);
    (result, count)
}

fn run(test: impl Future<Output = ()>) {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(test);
}

async fn unrefreshed_runtime() -> (ModelRuntime, Arc<InMemoryCredentialStore>) {
    let credentials = Arc::new(InMemoryCredentialStore::default());
    let runtime = ModelRuntime::create(CreateModelRuntimeOptions {
        credentials: Some(credentials.clone()),
        models_path: Some(None),
        models_store: Some(Arc::new(InMemoryModelsStore::default())),
        refresh_on_create: Some(false),
        allow_model_network: false,
        ..Default::default()
    })
    .await
    .unwrap();
    (runtime, credentials)
}

struct FixtureKeyAuth;

impl ApiKeyAuth for FixtureKeyAuth {
    fn name(&self) -> &str {
        "In-memory fixture key"
    }

    fn login<'a>(
        &'a self,
        _interaction: ProviderAuthInteraction,
    ) -> Option<BoxFuture<'a, Result<ApiKeyCredential, AuthError>>> {
        Some(Box::pin(async {
            Ok(ApiKeyCredential {
                key: Some("synthetic-login-key".to_string()),
                ..Default::default()
            })
        }))
    }

    fn check<'a>(
        &'a self,
        input: ApiKeyAuthInput<'a>,
    ) -> Option<BoxFuture<'a, Result<Option<AuthCheck>, AuthError>>> {
        Some(Box::pin(async move {
            input.options.check()?;
            // Never consult AuthContext: even a scheduled full refresh is isolated.
            Ok(input.credential.and_then(|credential| {
                credential.key.as_ref().map(|_| AuthCheck {
                    r#type: AuthType::ApiKey,
                    source: Some("fixture".to_string()),
                })
            }))
        }))
    }

    fn resolve<'a>(
        &'a self,
        _input: ApiKeyAuthInput<'a>,
    ) -> BoxFuture<'a, Result<Option<AuthResult>, AuthError>> {
        panic!("availability tests must not resolve request-time authentication")
    }
}

struct FixtureProvider {
    id: String,
    models: Vec<Model>,
    auth: ProviderAuth,
}

impl Provider for FixtureProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.id
    }

    fn auth(&self) -> &ProviderAuth {
        &self.auth
    }

    fn get_models(&self) -> Result<Vec<Model>, ModelsError> {
        Ok(self.models.clone())
    }
}

fn fixture_provider(id: &str, model_count: usize) -> Arc<dyn Provider> {
    let models = if model_count == 0 {
        Vec::new()
    } else {
        faux_provider(FauxProviderOptions {
            provider: Some(id.to_string()),
            api: Some("auth-lookup-fixture".to_string()),
            models: (0..model_count)
                .map(|index| FauxModelDefinition {
                    id: format!("fixture-{index}"),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        })
        .provider
        .get_models()
        .unwrap()
    };
    Arc::new(FixtureProvider {
        id: id.to_string(),
        models,
        auth: ProviderAuth {
            api_key: Some(Arc::new(FixtureKeyAuth)),
            oauth: None,
        },
    })
}

async fn isolated_runtime() -> (ModelRuntime, Arc<InMemoryCredentialStore>) {
    let (runtime, credentials) = unrefreshed_runtime().await;
    // Current-thread execution does not poll the registration background tasks
    // during this synchronous loop. Replace every built-in before any refresh
    // can inspect ambient environment/file authentication.
    let providers = runtime.get_providers().await;
    for provider in providers {
        runtime
            .register_native_provider_sync(fixture_provider(provider.id(), 0))
            .unwrap();
    }
    (runtime, credentials)
}

async fn refresh_provider(runtime: &ModelRuntime, id: &str) {
    let result = runtime
        .refresh(ModelsRefreshOptions {
            providers: Some(vec![id.to_string()]),
            allow_network: Some(false),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(!result.aborted);
    assert!(
        result.errors.is_empty(),
        "refresh errors: {:?}",
        result.errors
    );
}

#[test]
fn configured_auth_catalog_filter_does_not_allocate_catalog_copies() {
    run(async {
        let (runtime, _) = unrefreshed_runtime().await;
        // No refresh or request is performed: built-in catalog reads cannot
        // access real auth. The production session factory uses this same scan.
        let catalog = runtime.get_models(None).await;
        assert!(!catalog.is_empty());
        let started = Instant::now();
        let (configured, allocations) = count_allocations(|| {
            catalog
                .iter()
                .filter(|model| runtime.has_configured_auth(&model.provider))
                .count()
        });
        println!(
            "built-in auth scan: models={}, lookups={}, allocations={}, elapsed={:?}",
            catalog.len(),
            catalog.len(),
            allocations,
            started.elapsed()
        );
        assert_eq!(configured, 0);
        assert_eq!(
            allocations, 0,
            "boolean auth probes must not copy the catalog"
        );
    });
}

#[test]
fn configured_and_missing_auth_probes_do_not_allocate_as_catalog_grows() {
    run(async {
        let mut measurements = Vec::new();
        for model_count in [16, 256] {
            let (runtime, _) = isolated_runtime().await;
            runtime
                .register_native_provider_sync(fixture_provider("scan-fixture", model_count))
                .unwrap();
            runtime
                .set_runtime_api_key("scan-fixture", "synthetic-runtime-key", None)
                .await
                .unwrap();
            let catalog = runtime.get_models(None).await;
            assert_eq!(catalog.len(), model_count);
            let started = Instant::now();
            let (configured, allocations) = count_allocations(|| {
                catalog
                    .iter()
                    .filter(|model| {
                        let present = runtime.has_configured_auth(&model.provider);
                        let missing = runtime.has_configured_auth("missing-fixture");
                        present && !missing
                    })
                    .count()
            });
            println!(
                "configured auth scan: models={model_count}, lookups={}, allocations={allocations}, elapsed={:?}",
                model_count * 2,
                started.elapsed()
            );
            assert_eq!(configured, model_count);
            measurements.push(allocations);
        }
        assert_eq!(
            measurements,
            [0, 0],
            "catalog size must not add allocations to auth probes"
        );
    });
}

struct FixtureInteraction;

impl AuthInteraction for FixtureInteraction {
    fn signal(&self) -> Option<CancellationToken> {
        None
    }

    fn prompt(&self, _prompt: AuthPrompt) -> BoxFuture<'_, Result<String, AuthError>> {
        panic!("fixture login must not prompt a user")
    }

    fn notify(&self, _event: AuthEvent) {
        panic!("fixture login must not notify a user")
    }
}

#[test]
fn auth_probes_on_shared_threads_follow_runtime_keys_and_login_logout_publication() {
    run(async {
        let (runtime, _) = isolated_runtime().await;
        let old_handle = runtime.clone();
        runtime
            .register_native_provider_sync(fixture_provider("published-auth", 1))
            .unwrap();
        refresh_provider(&runtime, "published-auth").await;
        let (commands, receive) = std::sync::mpsc::channel();
        let (responses, received) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            for () in receive {
                responses
                    .send(old_handle.has_configured_auth("published-auth"))
                    .unwrap();
            }
        });
        let assert_published = |expected| {
            assert_eq!(runtime.has_configured_auth("published-auth"), expected);
            commands.send(()).unwrap();
            assert_eq!(
                received.recv_timeout(Duration::from_secs(5)).unwrap(),
                expected
            );
        };
        assert_published(false);
        runtime
            .set_runtime_api_key("published-auth", "synthetic-runtime-key", None)
            .await
            .unwrap();
        assert_published(true);
        runtime
            .remove_runtime_api_key("published-auth", None)
            .await
            .unwrap();
        assert_published(false);
        runtime
            .login(
                "published-auth",
                AuthType::ApiKey,
                Arc::new(FixtureInteraction),
            )
            .await
            .unwrap();
        assert_published(true);
        runtime.logout("published-auth", None).await.unwrap();
        assert_published(false);
        drop(commands);
        reader.join().unwrap();
    });
}

#[test]
fn auth_probes_keep_cached_credentials_until_an_explicit_refresh_publishes_changes() {
    run(async {
        let (runtime, credentials) = isolated_runtime().await;
        let old_handle = runtime.clone();
        runtime
            .register_native_provider_sync(fixture_provider("external-auth", 1))
            .unwrap();
        refresh_provider(&runtime, "external-auth").await;
        credentials
            .modify(
                "external-auth",
                Box::new(|_| {
                    Box::pin(async {
                        Ok(Some(Credential::ApiKey(ApiKeyCredential {
                            key: Some("synthetic-stored-key".to_string()),
                            ..Default::default()
                        })))
                    })
                }),
                &AuthOperationOptions::NONE,
            )
            .await
            .unwrap();
        assert!(!runtime.has_configured_auth("external-auth"));
        refresh_provider(&runtime, "external-auth").await;
        assert!(old_handle.has_configured_auth("external-auth"));
        credentials
            .delete("external-auth", &AuthOperationOptions::NONE)
            .await
            .unwrap();
        assert!(runtime.has_configured_auth("external-auth"));
        refresh_provider(&runtime, "external-auth").await;
        assert!(!old_handle.has_configured_auth("external-auth"));
    });
}

#[test]
fn auth_probes_follow_provisional_registration_and_removal_refresh() {
    run(async {
        let (runtime, _) = isolated_runtime().await;
        let old_handle = runtime.clone();
        runtime
            .register_provider_sync(
                "registered-auth",
                super::configured_provider("registered-model"),
            )
            .unwrap();
        assert!(old_handle.has_configured_auth("registered-auth"));
        runtime.unregister_provider_sync("registered-auth");
        refresh_provider(&runtime, "registered-auth").await;
        assert!(!old_handle.has_configured_auth("registered-auth"));
    });
}
