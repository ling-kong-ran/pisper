//! Tests for `services.rs`: behavior pinned to upstream `services/worker.ts`
//! (`createSessionWorkerServices` bookkeeping — endpoint-per-scope,
//! subscription removal, serialized reload tail, dispose aggregation).
//! Upstream has no dedicated vitest file; expectations follow the upstream
//! implementation text.

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::coding_agent::experimental::session_worker::ServiceCall;

fn scope(id: &str) -> WorkerServiceScope {
    WorkerServiceScope {
        server_connection_id: "srv-1".to_string(),
        attachment_id: id.to_string(),
    }
}

fn call(member: &str) -> ServiceCall {
    ServiceCall {
        service_id: service_ids::AGENT_CONTROLLER.to_string(),
        instance: None,
        member: member.to_string(),
        args: Vec::new(),
    }
}

struct FakeEndpoint {
    disposed: AtomicUsize,
}

impl ServiceEndpoint for FakeEndpoint {
    fn invoke(
        self: Arc<Self>,
        _call: ServiceCall,
        _publish: Box<dyn FnOnce(String, Value) -> BoxFuture<'static, ()> + Send>,
    ) -> BoxFuture<'static, Result<Option<Value>, String>> {
        Box::pin(async { Ok(Some(Value::Null)) })
    }

    fn dispose(&self) {
        self.disposed.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Default)]
pub(crate) struct FakeHost {
    reloads: AtomicUsize,
    disposed: AtomicUsize,
    fail_reload: std::sync::atomic::AtomicBool,
}

impl FacetHostSeam for FakeHost {
    fn reload(&self, _facets: Vec<String>) -> BoxFuture<'_, Result<(), String>> {
        self.reloads.fetch_add(1, Ordering::SeqCst);
        if self.fail_reload.load(Ordering::SeqCst) {
            Box::pin(async { Err("reload exploded".to_string()) })
        } else {
            Box::pin(async { Ok(()) })
        }
    }

    fn dispose(&self) -> BoxFuture<'_, Result<(), String>> {
        self.disposed.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(()) })
    }
}

#[derive(Default)]
pub(crate) struct CountingLoader {
    loads: AtomicUsize,
}

impl PluginLoader for CountingLoader {
    fn load(&self) -> BoxFuture<'static, Result<FacetGeneration, String>> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            Ok(FacetGeneration {
                facets: vec!["facet".to_string()],
                dispose: Box::new(|| {
                    Box::pin(async { Ok(()) }) as BoxFuture<'static, Result<(), String>>
                }),
            })
        })
    }
}

fn build(
    host: Arc<FakeHost>,
    loader: Arc<CountingLoader>,
) -> (SessionWorkerServices, Arc<Mutex<Vec<Arc<FakeEndpoint>>>>) {
    let registry: Arc<Mutex<Vec<Arc<FakeEndpoint>>>> = Arc::new(Mutex::new(Vec::new()));
    let registry_for_factory = Arc::clone(&registry);
    let initial = FacetGeneration {
        facets: vec!["initial".to_string()],
        dispose: Box::new(|| Box::pin(async { Ok(()) }) as BoxFuture<'static, Result<(), String>>),
    };
    let services = SessionWorkerServices::new(
        Box::new(move |_scope| {
            let endpoint = Arc::new(FakeEndpoint {
                disposed: AtomicUsize::new(0),
            });
            registry_for_factory
                .lock()
                .unwrap()
                .push(Arc::clone(&endpoint));
            endpoint as Arc<dyn ServiceEndpoint>
        }),
        host,
        loader,
        initial,
    );
    (services, registry)
}

#[tokio::test]
async fn creates_one_endpoint_per_scope_key() {
    let (services, registry) = build(
        Arc::new(FakeHost::default()),
        Arc::new(CountingLoader::default()),
    );
    let _ = services.invoke(call("prompt"), &scope("att-1")).await;
    let _ = services.invoke(call("prompt"), &scope("att-1")).await;
    let _ = services.invoke(call("prompt"), &scope("att-2")).await;
    assert_eq!(services.endpoints.lock().unwrap().len(), 2);
    assert_eq!(registry.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn remove_subscriptions_disposes_matching_scopes_only() {
    let (services, registry) = build(
        Arc::new(FakeHost::default()),
        Arc::new(CountingLoader::default()),
    );
    let _ = services.invoke(call("prompt"), &scope("att-1")).await;
    let _ = services.invoke(call("prompt"), &scope("att-2")).await;
    let first = Arc::clone(&registry.lock().unwrap()[0]);

    services.remove_subscriptions(|entry| entry.attachment_id == "att-1");
    assert_eq!(services.endpoints.lock().unwrap().len(), 1);
    assert_eq!(first.disposed.load(Ordering::SeqCst), 1);

    services.remove_subscriptions(|_entry| true);
    assert!(services.endpoints.lock().unwrap().is_empty());
}

#[tokio::test]
async fn reload_serializes_through_the_tail_chain() {
    let host = Arc::new(FakeHost::default());
    let loader = Arc::new(CountingLoader::default());
    let (services, _registry) = build(Arc::clone(&host), Arc::clone(&loader));
    let first = services.reload_plugins();
    let second = services.reload_plugins();
    let (first, second) = tokio::join!(first, second);
    assert_eq!(first.unwrap(), ());
    assert_eq!(second.unwrap(), ());
    assert_eq!(loader.loads.load(Ordering::SeqCst), 2);
    assert_eq!(host.reloads.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn reload_failure_cleans_up_candidate_and_keeps_previous() {
    let host = Arc::new(FakeHost::default());
    host.fail_reload.store(true, Ordering::SeqCst);
    let loader = Arc::new(CountingLoader::default());
    let (services, _registry) = build(Arc::clone(&host), Arc::clone(&loader));
    let error = services.reload_plugins().await.unwrap_err();
    assert_eq!(error, "reload exploded");
    // The previous generation is retained for dispose at shutdown.
    assert!(services
        .reload_plugins
        .lock()
        .unwrap()
        .loaded_plugins
        .lock()
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn dispose_drops_subscriptions_and_aggregates_errors() {
    let host = Arc::new(FakeHost::default());
    let (services, _registry) = build(Arc::clone(&host), Arc::new(CountingLoader::default()));
    let _ = services.invoke(call("prompt"), &scope("att-1")).await;
    services.dispose().await.unwrap();
    assert!(services.endpoints.lock().unwrap().is_empty());
    assert_eq!(host.disposed.load(Ordering::SeqCst), 1);
    // Double dispose of the host does not happen: generation already taken.
    services.dispose().await.unwrap();
    assert_eq!(host.disposed.load(Ordering::SeqCst), 2);
}

#[test]
fn service_scope_keys_round_trip() {
    assert_eq!(service_scope_key(&scope("att-1")), "srv-1\0att-1");
    let decoded = decode_scope_key("srv-1\0att-1");
    assert_eq!(decoded.server_connection_id, "srv-1");
    assert_eq!(decoded.attachment_id, "att-1");
}

#[test]
fn service_ids_match_upstream_define_service_calls() {
    assert_eq!(service_ids::AGENT_CONTROLLER, "pi.agent-controller");
    assert_eq!(service_ids::MODELS, "pi.models");
    assert_eq!(service_ids::TRANSCRIPT, "pi.transcript");
    assert_eq!(service_ids::PRESENTATION_PLUGINS, "pi.presentation-plugins");
    assert_eq!(service_ids::SESSION_PLUGINS, "pi.session-plugins");
}

#[test]
fn agent_controller_payloads_round_trip_with_wire_names() {
    let request = AgentPromptRequest {
        message: "hi".to_string(),
        images: Some(vec![AgentPromptImage {
            data: "abc".to_string(),
            mime_type: "image/png".to_string(),
        }]),
    };
    assert_eq!(
        serde_json::to_string(&request).unwrap(),
        "{\"message\":\"hi\",\"images\":[{\"type\":\"image\",\"data\":\"abc\",\"mimeType\":\"image/png\"}]}"
    );
    let response = AgentOperationResponse {
        accepted: true,
        operation_id: Some("op-1".to_string()),
        error: None,
    };
    assert_eq!(
        serde_json::to_string(&response).unwrap(),
        "{\"accepted\":true,\"operationId\":\"op-1\",\"error\":null}"
    );
}

// ── D6: real chord FacetHost wiring (`ChordSessionWorkerServices`) ─────────
//
// Upstream authority: experimental/services/worker.ts
// (sha256 3d7f193d6d6fe3f5bd956e8f06bf4c3ba57ed4f2f941669d74f32225a01dcafd,
// `createSessionWorkerServices` over the chord `FacetHost` +
// `createRemoteServiceEndpoint`). Facet identity/oracle traces follow
// `src/chord/consumer_oracle.rs`.

use crate::chord::context::Context;
use crate::chord::facets::{define_facet, FacetLoader, LoadedFacets, ProvidedImplementation};
use crate::chord::services::errors::ChordError;
use crate::chord::services::provider::{Implementation, InstanceMember};
use crate::chord::types::Service;

fn chord_service(id: &str) -> Service {
    Service {
        id: id.to_owned(),
        local: false,
    }
}

/// A facet providing one `read` method on its own service id.
fn read_facet(
    facet_id: &str,
    service_id: &'static str,
    value: &'static str,
) -> crate::chord::facets::Facet {
    define_facet(facet_id, move |env| {
        let mut members = Implementation::new();
        members.insert(
            "read".to_owned(),
            InstanceMember::Method(Arc::new(
                move |_args: &[serde_json::Value], _context: &Context| {
                    Ok(Some(serde_json::json!(value)))
                },
            )),
        );
        env.provide(
            &chord_service(service_id),
            ProvidedImplementation::Remote(members),
        )?;
        Ok(())
    })
}

/// A facet whose setup always fails (drives host reload/creation failures).
fn broken_facet(facet_id: &str) -> crate::chord::facets::Facet {
    define_facet(facet_id, |_env| {
        Err(ChordError::Type("setup exploded".to_owned()))
    })
}

/// Counting plugin loader (upstream `pluginLoader.load()` seam); the loaded
/// generation carries a `dispose` hook so retirement/cleanup is observable.
/// Loads after the first hand out `reload_facets` (tests use this to make the
/// create generation healthy and a later reload broken).
#[derive(Clone)]
struct ChordCountingLoader {
    loads: Arc<AtomicUsize>,
    disposes: Arc<AtomicUsize>,
    facets: Vec<crate::chord::facets::Facet>,
    reload_facets: Vec<crate::chord::facets::Facet>,
    dispose_error: Option<&'static str>,
}

impl ChordCountingLoader {
    fn new(facets: Vec<crate::chord::facets::Facet>) -> (Self, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let loads = Arc::new(AtomicUsize::new(0));
        let disposes = Arc::new(AtomicUsize::new(0));
        (
            ChordCountingLoader {
                loads: Arc::clone(&loads),
                disposes: Arc::clone(&disposes),
                facets,
                reload_facets: Vec::new(),
                dispose_error: None,
            },
            loads,
            disposes,
        )
    }
}

impl FacetLoader for ChordCountingLoader {
    fn load(&self) -> Result<LoadedFacets, ChordError> {
        let generation = self.loads.fetch_add(1, Ordering::SeqCst);
        let disposes = Arc::clone(&self.disposes);
        let dispose_error = self.dispose_error;
        let facets = if generation == 0 {
            self.facets.clone()
        } else {
            self.reload_facets.clone()
        };
        Ok(LoadedFacets::new(
            facets,
            Box::new(move || {
                disposes.fetch_add(1, Ordering::SeqCst);
                match dispose_error {
                    Some(message) => Err(ChordError::Type(message.to_owned())),
                    None => Ok(()),
                }
            }),
        ))
    }
}

fn noop_chord_publish() -> ChordPublishFn {
    Arc::new(|_scope, _subscription_id, _update| ())
}

fn chord_call(service_id: &str, member: &str, args: Vec<Value>) -> ServiceCall {
    ServiceCall {
        service_id: service_id.to_string(),
        instance: None,
        member: member.to_string(),
        args,
    }
}

#[tokio::test]
async fn chord_services_builds_one_host_and_serves_calls() {
    let (loader, loads, _disposes) = ChordCountingLoader::new(vec![read_facet(
        "@test/plugin",
        "test.plugin-svc",
        "plugin",
    )]);
    let builtins = vec![read_facet("@pi/builtin", "test.builtin-svc", "builtin")];
    let services = ChordSessionWorkerServices::create(builtins, Arc::new(loader)).expect("create");
    // The initial plugin generation loads exactly once during creation.
    assert_eq!(loads.load(Ordering::SeqCst), 1);

    // Both facet generations are live in the one host, through the same
    // endpoint (scope att-1).
    let scope = scope("att-1");
    let builtin = services
        .invoke(
            chord_call("test.builtin-svc", "read", Vec::new()),
            &scope,
            noop_chord_publish(),
        )
        .await
        .unwrap();
    assert_eq!(builtin, Some(Value::String("builtin".to_string())));
    let plugin = services
        .invoke(
            chord_call("test.plugin-svc", "read", Vec::new()),
            &scope,
            noop_chord_publish(),
        )
        .await
        .unwrap();
    assert_eq!(plugin, Some(Value::String("plugin".to_string())));
    assert_eq!(services.test_endpoint_count(), 1);
    services.dispose().await.unwrap();
}

#[tokio::test]
async fn chord_services_catalogue_lists_host_services() {
    let (loader, _loads, _disposes) = ChordCountingLoader::new(vec![read_facet(
        "@test/plugin",
        "test.plugin-svc",
        "plugin",
    )]);
    let builtins = vec![read_facet("@pi/builtin", "test.builtin-svc", "builtin")];
    let services = ChordSessionWorkerServices::create(builtins, Arc::new(loader)).expect("create");
    let catalogue = services
        .invoke(
            chord_call("$chord.service", "catalogue", Vec::new()),
            &scope("att-1"),
            noop_chord_publish(),
        )
        .await
        .unwrap()
        .expect("catalogue result");
    let rendered = catalogue.to_string();
    assert!(
        rendered.contains("test.builtin-svc"),
        "unexpected: {rendered}"
    );
    assert!(
        rendered.contains("test.plugin-svc"),
        "unexpected: {rendered}"
    );
    services.dispose().await.unwrap();
}

#[tokio::test]
async fn chord_services_endpoints_are_scoped_and_disposable() {
    let (loader, _loads, _disposes) = ChordCountingLoader::new(Vec::new());
    let builtins = vec![read_facet("@pi/builtin", "test.builtin-svc", "v1")];
    let services = ChordSessionWorkerServices::create(builtins, Arc::new(loader)).expect("create");
    let first = scope("att-1");
    let second = scope("att-2");
    services
        .invoke(
            chord_call("test.builtin-svc", "read", Vec::new()),
            &first,
            noop_chord_publish(),
        )
        .await
        .unwrap();
    services
        .invoke(
            chord_call("test.builtin-svc", "read", Vec::new()),
            &second,
            noop_chord_publish(),
        )
        .await
        .unwrap();
    // Repeated invokes on the same scope reuse the endpoint.
    services
        .invoke(
            chord_call("test.builtin-svc", "read", Vec::new()),
            &first,
            noop_chord_publish(),
        )
        .await
        .unwrap();
    assert_eq!(services.test_endpoint_count(), 2);

    services.remove_subscriptions(|entry| entry.attachment_id == "att-1");
    assert_eq!(services.test_endpoint_count(), 1);
    // The removed endpoint is gone for good; the stored face reports it.
    let error = services
        .test_invoke_stored(
            &first,
            chord_call("test.builtin-svc", "read", Vec::new()),
            noop_chord_publish(),
        )
        .unwrap_err();
    assert_eq!(error, "no stored endpoint");
    // The surviving endpoint still invokes.
    services
        .test_invoke_stored(
            &second,
            chord_call("test.builtin-svc", "read", Vec::new()),
            noop_chord_publish(),
        )
        .unwrap();
    services.dispose().await.unwrap();
}

#[tokio::test]
async fn chord_services_subscribe_control_calls_bind_subscriptions() {
    let (loader, _loads, _disposes) = ChordCountingLoader::new(Vec::new());
    let services =
        ChordSessionWorkerServices::create(Vec::new(), Arc::new(loader)).expect("create");
    let scope = scope("att-1");
    // The catalogue call routes through the endpoint's control channel; a
    // subscribe for an absent service id surfaces the provider error.
    let subscribe = services
        .invoke(
            chord_call(
                "$chord.service",
                "subscribe",
                vec![
                    serde_json::json!("sub-1"),
                    serde_json::json!("test.session"),
                    serde_json::json!("singleton"),
                ],
            ),
            &scope,
            noop_chord_publish(),
        )
        .await;
    // With no facet providing `test.session` the provider rejects the
    // subscription (service lookup failure surfaces as an error string).
    assert!(subscribe.is_err(), "unexpected: {subscribe:?}");
    services.dispose().await.unwrap();
}

#[tokio::test]
async fn chord_services_subscribe_and_unsubscribe_round_trip_on_a_provided_service() {
    let (loader, _loads, _disposes) = ChordCountingLoader::new(Vec::new());
    let builtins = vec![read_facet("@pi/builtin", "test.builtin-svc", "v1")];
    let services = ChordSessionWorkerServices::create(builtins, Arc::new(loader)).expect("create");
    let scope = scope("att-1");
    let subscribe = services
        .invoke(
            chord_call(
                "$chord.service",
                "subscribe",
                vec![
                    serde_json::json!("sub-1"),
                    serde_json::json!("test.builtin-svc"),
                    serde_json::json!("singleton"),
                ],
            ),
            &scope,
            noop_chord_publish(),
        )
        .await
        .expect("subscribe");
    // The subscribe result carries the instance snapshot.
    assert!(subscribe.is_some(), "expected a snapshot: {subscribe:?}");
    // A second subscription with the same id on the same endpoint is refused.
    let duplicate = services
        .invoke(
            chord_call(
                "$chord.service",
                "subscribe",
                vec![
                    serde_json::json!("sub-1"),
                    serde_json::json!("test.builtin-svc"),
                    serde_json::json!("singleton"),
                ],
            ),
            &scope,
            noop_chord_publish(),
        )
        .await
        .unwrap_err();
    assert!(
        duplicate.contains("Service subscription ID is already active"),
        "unexpected: {duplicate}"
    );
    let unsubscribe = services
        .invoke(
            chord_call(
                "$chord.service",
                "unsubscribe",
                vec![serde_json::json!("sub-1")],
            ),
            &scope,
            noop_chord_publish(),
        )
        .await
        .expect("unsubscribe");
    assert_eq!(unsubscribe, None);
    services.dispose().await.unwrap();
}

#[tokio::test]
async fn chord_services_reload_disposes_the_retired_generation() {
    let (loader, loads, disposes) = ChordCountingLoader::new(vec![read_facet(
        "@test/plugin-1",
        "test.plugin-svc-1",
        "gen-1",
    )]);
    let services =
        ChordSessionWorkerServices::create(Vec::new(), Arc::new(loader)).expect("create");
    services.reload_plugins().await.unwrap();
    services.reload_plugins().await.unwrap();
    // create + two reloads.
    assert_eq!(loads.load(Ordering::SeqCst), 3);
    // Each successful reload retires the previous generation.
    assert_eq!(disposes.load(Ordering::SeqCst), 2);
    services.dispose().await.unwrap();
    // The final generation disposes once with the host.
    assert_eq!(disposes.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn chord_services_serializes_concurrent_reloads() {
    let (loader, loads, disposes) =
        ChordCountingLoader::new(vec![read_facet("@test/plugin", "test.plugin-svc", "gen")]);
    let services =
        ChordSessionWorkerServices::create(Vec::new(), Arc::new(loader)).expect("create");
    let first = services.reload_plugins();
    let second = services.reload_plugins();
    let (first, second) = tokio::join!(first, second);
    first.unwrap();
    second.unwrap();
    assert_eq!(loads.load(Ordering::SeqCst), 3);
    assert_eq!(disposes.load(Ordering::SeqCst), 2);
    services.dispose().await.unwrap();
}

#[tokio::test]
async fn chord_services_reload_failure_cleans_up_the_candidate() {
    // A healthy create generation; the reload hands out a broken facet so the
    // host reload fails while the create path stays green.
    let (mut loader, _loads, disposes) = ChordCountingLoader::new(Vec::new());
    loader.reload_facets = vec![broken_facet("@test/bad")];
    let services =
        ChordSessionWorkerServices::create(Vec::new(), Arc::new(loader)).expect("create");
    let error = services.reload_plugins().await.unwrap_err();
    // The host reload failure surfaces with the broken facet identity
    // (candidate cleanup succeeded).
    assert!(error.contains("@test/bad"), "unexpected: {error}");
    assert_eq!(disposes.load(Ordering::SeqCst), 1);

    // When the candidate cleanup also fails the upstream AggregateError
    // message wraps both (oracle: aggregateMessages.reloadCleanup).
    let (mut loader, _loads, disposes) = ChordCountingLoader::new(Vec::new());
    loader.reload_facets = vec![broken_facet("@test/bad")];
    loader.dispose_error = Some("cleanup exploded");
    let services =
        ChordSessionWorkerServices::create(Vec::new(), Arc::new(loader)).expect("create");
    let error = services.reload_plugins().await.unwrap_err();
    assert!(
        error.starts_with("Session plugin reload and cleanup failed: ")
            && error.contains("@test/bad")
            && error.ends_with("cleanup exploded"),
        "unexpected: {error}"
    );
    assert_eq!(disposes.load(Ordering::SeqCst), 1);
    // The retained (failed-reload path keeps the old) generation still
    // disposes with the host at shutdown, surfacing its configured error.
    let dispose_error = services.dispose().await.unwrap_err();
    assert_eq!(dispose_error, "cleanup exploded");
    assert_eq!(disposes.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn chord_services_dispose_aggregates_errors_with_upstream_message() {
    // Oracle: aggregateMessages.disposeFacets. A failing plugin-generation
    // dispose is the single error and is re-raised bare.
    let (mut loader, _loads, disposes) = ChordCountingLoader::new(Vec::new());
    loader.dispose_error = Some("plugin dispose exploded");
    let services =
        ChordSessionWorkerServices::create(Vec::new(), Arc::new(loader)).expect("create");
    let error = services.dispose().await.unwrap_err();
    assert_eq!(error, "plugin dispose exploded");
    assert_eq!(disposes.load(Ordering::SeqCst), 1);
    // Disposing again is stable (the generation slot is already taken).
    services.dispose().await.unwrap();
}

#[test]
fn chord_services_create_aggregates_host_startup_and_cleanup_errors() {
    // Oracle: aggregateMessages.startupCleanup. Host creation failure with a
    // failing plugin dispose aggregates both.
    let (mut loader, _loads, disposes) = ChordCountingLoader::new(vec![broken_facet("@test/bad")]);
    loader.dispose_error = Some("plugin dispose exploded");
    let error = ChordSessionWorkerServices::create(
        vec![broken_facet("@pi/broken-builtin")],
        Arc::new(loader),
    )
    .err()
    .unwrap();
    assert!(
        error.starts_with("Session facets failed to start and clean up: ")
            && error.contains("setup exploded")
            && error.ends_with("plugin dispose exploded"),
        "unexpected: {error}"
    );
    // The plugin generation was cleaned up during the failed start.
    assert_eq!(disposes.load(Ordering::SeqCst), 1);

    // Without a cleanup error the host failure is re-thrown bare (upstream
    // `throw error`).
    let (loader, _loads, disposes) = ChordCountingLoader::new(vec![broken_facet("@test/bad")]);
    let error = ChordSessionWorkerServices::create(
        vec![broken_facet("@pi/broken-builtin")],
        Arc::new(loader),
    )
    .err()
    .unwrap();
    assert!(error.contains("setup exploded"), "unexpected: {error}");
    assert!(
        !error.contains("Session facets failed to start"),
        "unexpected: {error}"
    );
    assert_eq!(disposes.load(Ordering::SeqCst), 1);
}

#[test]
fn chord_services_create_propagates_loader_errors() {
    struct FailingLoader;
    impl ChordPluginLoaderSeam for FailingLoader {
        fn load(&self) -> Result<ChordFacetGeneration, String> {
            Err("plugin missing".to_string())
        }
    }
    let error = ChordSessionWorkerServices::create(Vec::new(), Arc::new(FailingLoader))
        .err()
        .unwrap();
    assert_eq!(error, "plugin missing");
}
