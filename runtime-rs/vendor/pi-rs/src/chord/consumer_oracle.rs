//! Oracle-driven tests for the chord consumer-side slice. Expected values
//! were captured from the read-only upstream TypeScript sources
//! (`pi/packages/chord/src`, copied to
//! `tests/fixtures/chord_consumer_oracle/upstream_src`) run under
//! `node --experimental-strip-types` by
//! `tests/fixtures/chord_consumer_oracle/capture.mjs`; the captured output is
//! `tests/fixtures/chord_consumer_oracle/oracle_output.jsonl` (sha256
//! `33d371115f0e12fae3363e5c48e8c9a1548d3d561e8f0f341cbf25b4dae50dda`).
//! Comparisons are byte-identical over the scenario traces.

use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::chord::consumer::{
    create_loopback_service_transport, create_remote_service_binding, RemoteServiceBindingOptions,
};
use crate::chord::context::Context;
use crate::chord::facets::{
    combine_facet_loaders, create_facet_host, define_facet, FacetLoader, FacetOptions,
    LoadedFacets, ProvidedImplementation,
};
use crate::chord::services::errors::ChordError;
use crate::chord::services::provider::{
    Implementation, InstanceMember, ProviderEntry, RemoteServiceProvider,
};
use crate::chord::types::Service;

fn service(id: &str) -> Service {
    Service {
        id: id.to_owned(),
        local: false,
    }
}

fn local_service(id: &str) -> Service {
    Service {
        id: id.to_owned(),
        local: true,
    }
}

fn read_method(value: &'static str) -> Implementation {
    let value = json!(value);
    let mut members = Implementation::new();
    members.insert(
        "read".to_owned(),
        InstanceMember::Method(Arc::new(
            move |_args: &[serde_json::Value], _context: &Context| Ok(Some(value.clone())),
        )),
    );
    members
}

#[test]
fn consumer_singleton_matches_oracle() {
    let provider =
        RemoteServiceProvider::new(&[ProviderEntry::singleton("test.consumer.counter")]).unwrap();
    provider
        .provide(&service("test.consumer.counter"), read_method("v1"))
        .unwrap();
    let binding = create_remote_service_binding(RemoteServiceBindingOptions {
        services: vec![service("test.consumer.counter")],
        transport: create_loopback_service_transport(provider.clone()),
        on_error: None,
        assert_access: None,
        bound: None,
    })
    .unwrap();
    let counter = binding
        .use_service(&service("test.consumer.counter"))
        .unwrap();
    let mut trace: Vec<String> = Vec::new();
    let read = counter
        .invoke("read", &[], &Context::background())
        .unwrap()
        .unwrap();
    trace.push(format!("read:{}", read.as_str().unwrap()));
    binding.rebind(false).unwrap();
    match counter.invoke("read", &[], &Context::background()) {
        Ok(_) => trace.push("read-after-unbind:ok".to_owned()),
        Err(error) => trace.push(format!(
            "read-after-unbind:{}:{}",
            error.code().map(|code| code.as_str()).unwrap_or("error"),
            error.message()
        )),
    }
    binding.rebind(true).unwrap();
    let read = counter
        .invoke("read", &[], &Context::background())
        .unwrap()
        .unwrap();
    trace.push(format!("read-after-rebind:{}", read.as_str().unwrap()));
    let unknown = binding.use_service(&service("test.consumer.unknown"));
    trace.push(format!("unknown:{}", unknown.unwrap_err().message()));
    let local = binding.use_service(&local_service("test.facets.host-values"));
    trace.push(format!("local:{}", local.unwrap_err().message()));
    let mode = binding.observe(
        &service("test.consumer.counter"),
        Arc::new(|_handle, _context| Ok(())),
    );
    match mode {
        Ok(_) => trace.push("mode:ok".to_owned()),
        Err(error) => trace.push(format!("mode:{}", error.message())),
    }
    binding.dispose().unwrap();
    trace.push("disposed:true".to_owned());
    let disposed = binding.use_service(&service("test.consumer.counter"));
    trace.push(format!("disposed-use:{}", disposed.unwrap_err().message()));

    // Captured from `capture.mjs` scenario `consumer-singleton`.
    let expected = vec![
        "read:v1".to_owned(),
        "read-after-unbind:service_stale_instance:Remote service test.consumer.counter binding is closed"
            .to_owned(),
        "read-after-rebind:v1".to_owned(),
        "unknown:Remote service test.consumer.unknown is not allowlisted".to_owned(),
        "local:Service test.facets.host-values is process-local".to_owned(),
        "mode:Remote service test.consumer.counter is already used as singleton".to_owned(),
        "disposed:true".to_owned(),
        "disposed-use:Remote service binding is disposed".to_owned(),
    ];
    assert_eq!(trace, expected);
}

#[test]
fn consumer_duplicate_ids_matches_oracle() {
    let provider =
        RemoteServiceProvider::new(&[ProviderEntry::singleton("test.consumer.counter")]).unwrap();
    let binding = create_remote_service_binding(RemoteServiceBindingOptions {
        services: vec![
            service("test.consumer.counter"),
            service("test.consumer.counter"),
        ],
        transport: create_loopback_service_transport(provider),
        on_error: None,
        assert_access: None,
        bound: None,
    });
    let error = match binding {
        Ok(_) => panic!("duplicate IDs must be rejected"),
        Err(error) => error,
    };
    // Captured from `capture.mjs` scenario `consumer-duplicate-ids`.
    assert_eq!(
        error.message(),
        "Remote service binding has duplicate service IDs"
    );
}

#[test]
fn consumer_keyed_matches_oracle() {
    let provider =
        RemoteServiceProvider::new(&[ProviderEntry::keyed("test.consumer.keyed-counter")]).unwrap();
    let trace: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let error_trace = trace.clone();
    let binding = create_remote_service_binding(RemoteServiceBindingOptions {
        services: vec![service("test.consumer.keyed-counter")],
        transport: create_loopback_service_transport(provider.clone()),
        on_error: Some(Arc::new(move |error: &ChordError| {
            error_trace
                .lock()
                .unwrap()
                .push(format!("error:{}", error.message()));
        })),
        assert_access: None,
        bound: None,
    })
    .unwrap();
    let trace_for_handler = trace.clone();
    let stop = binding
        .observe(
            &service("test.consumer.keyed-counter"),
            Arc::new(move |handle, context| {
                let read = handle
                    .invoke("read", &[], context)?
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .unwrap_or_else(|| "undefined".to_owned());
                trace_for_handler
                    .lock()
                    .unwrap()
                    .push(format!("observed:{read}"));
                Ok(())
            }),
        )
        .unwrap();
    let mut members = Implementation::new();
    members.insert(
        "read".to_owned(),
        InstanceMember::Method(Arc::new(
            |_args: &[serde_json::Value], _context: &Context| Ok(Some(json!("one"))),
        )),
    );
    let close_instance = provider
        .spawn(&service("test.consumer.keyed-counter"), "one", members)
        .unwrap();
    // Keyed deliveries are pumped asynchronously (divergence D11); wait for
    // the observation to land before closing, mirroring the upstream waitFor
    // helper.
    for _ in 0..200 {
        if !trace.lock().unwrap().is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    // Upstream `observers:${JSON.stringify(trace.length)}` at this point.
    let observed = trace.lock().unwrap().len();
    trace.lock().unwrap().push(format!("observers:{observed}"));
    close_instance.close().unwrap();
    stop();
    binding.dispose().unwrap();
    for _ in 0..100 {
        if trace
            .lock()
            .unwrap()
            .iter()
            .any(|line| line.starts_with("error:"))
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let trace = trace.lock().unwrap().clone();
    // Captured from `capture.mjs` scenario `consumer-keyed`.
    assert_eq!(
        trace,
        vec!["observed:one".to_owned(), "observers:1".to_owned()]
    );
}

#[test]
fn facet_host_matches_oracle() {
    let trace: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let retained_source: Arc<Mutex<Option<crate::chord::consumer::ServiceHandle>>> =
        Arc::new(Mutex::new(None));

    let projection_facet = {
        let trace = trace.clone();
        let retained_source = retained_source.clone();
        define_facet("projection", move |env| {
            trace.lock().unwrap().push("setup projection".to_owned());
            let source = env.use_service(&service("test.facets.source"))?;
            match source.invoke("read", &[], &Context::background()) {
                Ok(_) => trace.lock().unwrap().push("early-access:ok".to_owned()),
                Err(error) => trace
                    .lock()
                    .unwrap()
                    .push(format!("early-access:{}", error.message())),
            }
            // Retain the slot view for the delegation below (upstream closes
            // over the `sourceHandle` proxy).
            *retained_source.lock().unwrap() = Some(source);
            let mut members = Implementation::new();
            let retained_source = retained_source.clone();
            members.insert(
                "read".to_owned(),
                InstanceMember::Method(Arc::new(
                    move |args: &[serde_json::Value], context: &Context| {
                        // Delegates to the source handle captured at setup
                        // time (upstream `sourceHandle.read(context)`).
                        retained_source
                            .lock()
                            .unwrap()
                            .as_ref()
                            .expect("source handle")
                            .invoke("read", args, context)
                    },
                )),
            );
            env.provide(
                &service("test.facets.projection"),
                ProvidedImplementation::Remote(members),
            )?;
            let activate_trace = trace.clone();
            env.on_activate(Arc::new(move || {
                activate_trace
                    .lock()
                    .unwrap()
                    .push("activate projection".to_owned());
                Ok(())
            }))?;
            let deactivate_trace = trace.clone();
            env.on_deactivate(Arc::new(move || {
                deactivate_trace
                    .lock()
                    .unwrap()
                    .push("dispose projection".to_owned());
                Ok(())
            }))?;
            Ok(())
        })
    };
    let source_facet = {
        let trace = trace.clone();
        define_facet("source", move |env| {
            trace.lock().unwrap().push("setup source".to_owned());
            env.provide(
                &service("test.facets.source"),
                ProvidedImplementation::Remote(read_method("value")),
            )?;
            let activate_trace = trace.clone();
            env.on_activate(Arc::new(move || {
                activate_trace
                    .lock()
                    .unwrap()
                    .push("activate source".to_owned());
                Ok(())
            }))?;
            let deactivate_trace = trace.clone();
            env.on_deactivate(Arc::new(move || {
                deactivate_trace
                    .lock()
                    .unwrap()
                    .push("dispose source".to_owned());
                Ok(())
            }))?;
            Ok(())
        })
    };
    let host_values_facet = {
        let trace = trace.clone();
        define_facet("host-values", move |env| {
            trace.lock().unwrap().push("setup host-values".to_owned());
            env.provide(
                &local_service("test.facets.host-values"),
                ProvidedImplementation::Local(Arc::new(HostValuesImpl {
                    name: "session".to_owned(),
                    value: "host value".to_owned(),
                })),
            )?;
            Ok(())
        })
    };

    let host = create_facet_host(FacetOptions {
        facets: vec![projection_facet, source_facet, host_values_facet],
        service_sources: Vec::new(),
        on_error: None,
    });
    let host = match host {
        Ok(host) => host,
        Err(error) => panic!("host activation failed: {error}"),
    };

    let push = |trace: &Arc<Mutex<Vec<String>>>, line: String| {
        trace.lock().unwrap().push(line);
    };

    // The port's projection read routes through the loopback provider.
    let binding = create_remote_service_binding(RemoteServiceBindingOptions {
        services: vec![service("test.facets.projection")],
        transport: create_loopback_service_transport(host.services.clone()),
        on_error: None,
        assert_access: None,
        bound: None,
    })
    .unwrap();
    let projection = binding
        .use_service(&service("test.facets.projection"))
        .unwrap();
    let read = projection
        .invoke("read", &[], &Context::background())
        .unwrap()
        .unwrap();
    push(&trace, format!("activated:{}", read.as_str().unwrap()));

    // `host.services.use(HostValues)` rejects process-local services.
    let local_use = binding.use_service(&local_service("test.facets.host-values"));
    push(
        &trace,
        format!("local-use:{}", local_use.unwrap_err().message()),
    );

    // Missing dependency.
    let missing = create_facet_host(FacetOptions {
        facets: vec![define_facet("missing", |env| {
            env.use_service(&service("test.facets.source"))?;
            Ok(())
        })],
        service_sources: Vec::new(),
        on_error: None,
    });
    push(
        &trace,
        format!("missing:{}", missing.unwrap_err().message()),
    );

    // Dependency cycle.
    let cycle = create_facet_host(FacetOptions {
        facets: vec![
            define_facet("first", |env| {
                env.use_service(&service("test.facets.projection"))?;
                env.provide(
                    &service("test.facets.source"),
                    ProvidedImplementation::Remote(read_method("first")),
                )?;
                Ok(())
            }),
            define_facet("second", |env| {
                env.use_service(&service("test.facets.source"))?;
                env.provide(
                    &service("test.facets.projection"),
                    ProvidedImplementation::Remote(read_method("second")),
                )?;
                Ok(())
            }),
        ],
        service_sources: Vec::new(),
        on_error: None,
    });
    push(&trace, format!("cycle:{}", cycle.unwrap_err().message()));

    // Reload shape preservation.
    let reload_shape = host.reload(vec![define_facet("source", |env| {
        let mut members = Implementation::new();
        members.insert(
            "write".to_owned(),
            InstanceMember::Method(Arc::new(
                |_args: &[serde_json::Value], _context: &Context| Ok(None),
            )),
        );
        env.provide(
            &service("test.facets.projection"),
            ProvidedImplementation::Remote(members),
        )?;
        Ok(())
    })]);
    push(
        &trace,
        format!("reload-shape:{}", reload_shape.unwrap_err().message()),
    );

    // A reload of an empty facet list succeeds; dispose ends the generation.
    host.reload(Vec::new()).unwrap();
    host.dispose().unwrap();

    for _ in 0..100 {
        if trace
            .lock()
            .unwrap()
            .iter()
            .any(|line| line.starts_with("error:"))
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let trace = trace.lock().unwrap().clone();
    // Captured from `capture.mjs` scenario `facet-host` (the port performs
    // the setup-time access guard at the first member invocation, same guard
    // text; `reload-not-active` is not exercised because the oracle's reload
    // of an empty list succeeds against an active host).
    let expected = vec![
        "setup projection".to_owned(),
        "early-access:Facet projection service handles cannot be used while setting_up".to_owned(),
        "setup source".to_owned(),
        "setup host-values".to_owned(),
        "activate source".to_owned(),
        "activate projection".to_owned(),
        "activated:value".to_owned(),
        "local-use:Service test.facets.host-values is process-local".to_owned(),
        "missing:Facet missing requires local/test.facets.source/singleton, but no facet provides it"
            .to_owned(),
        "cycle:Facet dependency cycle: first, second".to_owned(),
        "reload-shape:Reloaded facet source must preserve its service requirements and provisions"
            .to_owned(),
        "dispose projection".to_owned(),
        "dispose source".to_owned(),
    ];
    assert_eq!(trace, expected);
}

#[test]
fn facet_loaders_match_oracle() {
    let trace: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    struct TracedLoader {
        name: &'static str,
        trace: Arc<Mutex<Vec<String>>>,
    }
    impl FacetLoader for TracedLoader {
        fn load(&self) -> Result<LoadedFacets, ChordError> {
            self.trace
                .lock()
                .unwrap()
                .push(format!("load {}", self.name));
            let trace = self.trace.clone();
            let name = self.name;
            Ok(LoadedFacets::new(
                vec![define_facet(name, |_env| Ok(()))],
                Box::new(move || {
                    trace.lock().unwrap().push(format!("dispose {name}"));
                    Ok(())
                }),
            ))
        }
    }
    let loader = combine_facet_loaders(vec![
        Arc::new(TracedLoader {
            name: "first",
            trace: trace.clone(),
        }),
        Arc::new(TracedLoader {
            name: "second",
            trace: trace.clone(),
        }),
    ]);
    let loaded = loader.load().unwrap();
    let ids: Vec<String> = loaded.facets.iter().map(|facet| facet.id.clone()).collect();
    trace
        .lock()
        .unwrap()
        .push(format!("facets:{}", ids.join(",")));
    let mut loaded = loaded;
    loaded.dispose().unwrap();
    loaded.dispose().unwrap();
    for _ in 0..100 {
        if trace
            .lock()
            .unwrap()
            .iter()
            .any(|line| line.starts_with("error:"))
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let trace = trace.lock().unwrap().clone();
    // Captured from `capture.mjs` scenario `facet-loaders`.
    assert_eq!(
        trace,
        vec![
            "load first".to_owned(),
            "load second".to_owned(),
            "facets:first,second".to_owned(),
            "dispose second".to_owned(),
            "dispose first".to_owned(),
        ]
    );
}

#[test]
fn replicated_state_matches_oracle() {
    let state = crate::chord::api::replicated_state(json!({ "value": 7 }));
    // Captured from `capture.mjs` scenario `replicated-state`.
    assert_eq!(state.value(), json!({ "value": 7 }));
}

#[test]
fn short_hash_matches_oracle() {
    // Captured from `capture.mjs` scenario `short-hash-main`:
    // createHash('sha256').update('main').digest('hex').slice(0, 12).
    assert_eq!(crate::chord::bundler::short_hash("main"), "0d6e4079e367");
}

#[test]
fn local_target_downcast_round_trip() {
    // The local-implementation arm of the handle duality (divergence D3).
    let handle = crate::chord::consumer::ServiceHandle::new(
        crate::chord::consumer::ServiceTarget::Local(Arc::new(HostValuesImpl {
            name: "session".to_owned(),
            value: "host value".to_owned(),
        })),
        Arc::new(|| Ok(())),
    );
    let target = handle.local::<HostValuesImpl>().unwrap();
    assert_eq!(target.name, "session");
}

struct HostValuesImpl {
    name: String,
    #[allow(dead_code)]
    value: String,
}
