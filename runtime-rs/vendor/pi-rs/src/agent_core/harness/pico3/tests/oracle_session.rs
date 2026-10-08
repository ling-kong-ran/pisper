//! Ports of the Session-level oracles: `reads.test.ts`,
//! `spec-transactions.test.ts`, `spec-context-capabilities.test.ts`,
//! `authority.test.ts` (storage engine halves), `busy.test.ts` (prospective
//! busy), and `spec-plugins-lifecycle.test.ts` (namespace routing halves).

use crate::agent_core::chord_support::Context;
use crate::agent_core::harness::pico3::memory::MemoryStorage;
use crate::agent_core::harness::pico3::session::Resolution;
use crate::agent_core::harness::pico3::tests::support::*;
use crate::agent_core::harness::pico3::types::{
    InvocationMode, InvocationToken, Invoker, NewEntry, ReadAfterWrite, SendInput, UserInput,
};

use futures::FutureExt;
use serde_json::{json, Value};

/// `reads.test.ts` "read-your-writes" + `spec-transactions.test.ts` "direct
/// table reads ... observe all same-batch creations" (`session.ts:420-522`).
#[tokio::test]
async fn read_your_writes_sees_same_batch_creations() {
    let env = Env::open_memory().await.unwrap();
    let kind = plugin_kind(&env);
    let seen = env
        .commit_host(move |tx, _ctx| {
            let kind = kind.clone();
            async move {
                let written = tx
                    .write(
                        1,
                        NewEntry {
                            kind: "note".to_owned(),
                            data: Some(json!({ "n": 1 }).as_object().cloned().unwrap()),
                            ..NewEntry::default()
                        },
                    )
                    .await?;
                let input = tx.input(written).await?.expect("input overlay");
                let entry = tx
                    .entry(input.entry.expect("idle write placed"))
                    .await?
                    .expect("entry overlay");
                let reference = tx.create_task_kind(
                    &kind,
                    Value::Null,
                    crate::agent_core::harness::pico3::session::CreateTaskOptions {
                        conversation_id: Some(1),
                        background: true,
                        after: Vec::new(),
                    },
                )?;
                let task = tx.task(reference.id).await?.expect("task overlay");
                let conversation_id = tx.create_conversation(&Default::default())?;
                let conversation = tx
                    .conversation(conversation_id)
                    .await?
                    .expect("conversation overlay");
                let rewindable = tx.snapshot(
                    crate::agent_core::harness::pico3::types::DocRef::Rewindable {
                        conversation_id,
                    },
                )?;
                Ok((
                    input.status,
                    json!(entry.data.unwrap()),
                    task.status,
                    conversation.id == conversation_id,
                    rewindable.get("profile").cloned(),
                ))
            }
            .boxed()
        })
        .await
        .unwrap()
        .value;
    assert_eq!(seen.0, "done");
    assert_eq!(seen.1, json!({ "n": 1 }));
    assert_eq!(format!("{:?}", seen.2), "Pending");
    assert!(seen.3);
    assert_eq!(
        seen.4,
        Some(json!("default")),
        "new-conversation snapshots show declared defaults"
    );
}

/// `reads.test.ts` "ReadAfterWrite: a scan after an append rejects, poisons
/// the transaction even if caught, and nothing is persisted"
/// (`session.ts:410-415, 458-467`).
#[tokio::test]
async fn read_after_write_poisons_the_transaction() {
    let env = Env::open_memory().await.unwrap();
    let before = env.entries(1).await.unwrap().len();
    let error = env
        .commit_host(|tx, _ctx| {
            async move {
                tx.write(1, NewEntry::new("note")).await?;
                // Caught, but the poison stands.
                if let Err(caught) = tx.newest_entry(1, None, false).await {
                    if caught.downcast_ref::<ReadAfterWrite>().is_none() {
                        return Err(caught);
                    }
                } else {
                    anyhow::bail!("newestEntry should have rejected");
                }
                Ok(())
            }
            .boxed()
        })
        .await
        .unwrap_err();
    let read_after_write = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<ReadAfterWrite>())
        .expect("the commit rethrows the poison");
    assert_eq!(read_after_write.read, "newestEntry");
    assert_eq!(read_after_write.write, "entry append");
    assert_eq!(
        env.entries(1).await.unwrap().len(),
        before,
        "nothing persisted"
    );
    assert!(
        env.session.live_tasks().is_empty(),
        "the poisoned transaction's writes are gone"
    );

    // A task scan after a task write rejects too (`reads.test.ts:84-90`).
    let kind = plugin_kind(&env);
    let error = env
        .commit_host(move |tx, _ctx| {
            let kind = kind.clone();
            async move {
                tx.create_task_kind(
                    &kind,
                    Value::Null,
                    crate::agent_core::harness::pico3::session::CreateTaskOptions {
                        conversation_id: Some(1),
                        background: true,
                        after: Vec::new(),
                    },
                )?;
                tx.tasks(&Default::default()).await?;
                Ok(())
            }
            .boxed()
        })
        .await
        .unwrap_err();
    assert!(
        error
            .chain()
            .any(|cause| cause.downcast_ref::<ReadAfterWrite>().is_some()),
        "{error}"
    );

    // A different conversation's scan is fine (`reads.test.ts:91-95`).
    let other = env
        .commit_kernel(|tx, _ctx| {
            async move { tx.create_conversation(&Default::default()) }.boxed()
        })
        .await
        .unwrap()
        .value;
    env.commit_host(|tx, _ctx| {
        async move {
            tx.write(1, NewEntry::new("note")).await?;
            tx.newest_entry(other, None, false).await?;
            Ok(())
        }
        .boxed()
    })
    .await
    .unwrap();

    // context rejects after a same-batch write to the domain.
    let error = env
        .commit_host(|tx, _ctx| {
            async move {
                tx.write(1, NewEntry::new("note")).await?;
                tx.context(1, None).await?;
                Ok(())
            }
            .boxed()
        })
        .await
        .unwrap_err();
    assert!(error
        .chain()
        .any(|cause| cause.downcast_ref::<ReadAfterWrite>().is_some()));
    let error = env
        .commit_host(|tx, _ctx| {
            async move {
                tx.write(1, NewEntry::new("note")).await?;
                tx.scan_entries(&crate::agent_core::harness::pico3::types::EntryScan {
                    conversation_id: 1,
                    limit: 5,
                    ..Default::default()
                })
                .await?;
                Ok(())
            }
            .boxed()
        })
        .await
        .unwrap_err();
    assert!(error
        .chain()
        .any(|cause| cause.downcast_ref::<ReadAfterWrite>().is_some()));
}

/// `reads.test.ts` "config: defaults come from kind declarations; stored
/// null is a value; unset restores the default" (`session.ts:619-663`).
#[tokio::test]
async fn config_facade_defaults_null_and_reset() {
    // A kind declares a rewindable default; the fixture registers it as a
    // namespace-free config route via a fresh session kind.
    let kind: std::sync::Arc<dyn crate::agent_core::harness::pico3::types::AnyKind> =
        std::sync::Arc::new(
            crate::agent_core::harness::pico3::types::BasicKind::new("plan").config(
                crate::agent_core::harness::pico3::types::KindConfig {
                    rewindable: json!({ "planMode": false, "note": "n" })
                        .as_object()
                        .cloned()
                        .unwrap(),
                    sticky: Default::default(),
                    declared_absent: Default::default(),
                },
            ),
        );
    // Rebuild the env with the declaring kind registered (the upstream
    // harness installs task kinds before open).
    let mut kinds = stub_kinds();
    kinds.insert(kind.name().to_owned(), kind);
    let env = Env::open_memory_with_kinds(kinds).await.unwrap();

    let plan_mode = env
        .commit_host(|tx, _ctx| async move { tx.config_get(1, "planMode") }.boxed())
        .await
        .unwrap()
        .value;
    assert_eq!(plan_mode, json!(false), "default from the kind");
    // Stored null is a value, not absence.
    env.commit_host(|tx, _ctx| {
        async move {
            tx.config_set(1, "note", json!(null))?;
            let stored = tx.config_get(1, "note")?;
            if !stored.is_null() {
                anyhow::bail!("stored null must be a value");
            }
            Ok(())
        }
        .boxed()
    })
    .await
    .unwrap();
    // Reset restores the default.
    let note = env
        .commit_host(|tx, _ctx| {
            async move {
                tx.config_reset(1, "note")?;
                tx.config_get(1, "note")
            }
            .boxed()
        })
        .await
        .unwrap()
        .value;
    assert_eq!(note, json!("n"));
    // Unknown key errors (`session.ts:629`).
    let error = env
        .commit_host(|tx, _ctx| async move { tx.config_set(1, "bogus", json!(1)) }.boxed())
        .await
        .unwrap_err();
    assert!(format!("{error}").contains("unknown config key"), "{error}");
    // Ordinary tasks cannot write config (`session.ts:632-635`).
    let (task_id, token) = env.create_background_task().await.unwrap();
    let error = env
        .commit_task(&token, task_id, |tx, _ctx| {
            async move { tx.config_set(1, "keepRecent", json!(5)) }.boxed()
        })
        .await
        .unwrap_err();
    assert_named(&error, "Forbidden");
}

/// `reads.test.ts` "plugin state: registered namespace slices cannot see or
/// clobber each other" + `spec-plugins-lifecycle.test.ts` "namespace
/// defaults route to their declared documents, preserve null, and seed
/// lazily" (`session.ts:565-591`).
#[tokio::test]
async fn namespace_slices_route_and_isolate() {
    let env = Env::open_memory().await.unwrap();
    let a = env.namespace(ns("a", json!({ "count": 0 }), json!({})));
    let b = env.namespace(ns("b", json!({ "count": 10 }), json!({})));
    let routing = env.namespace(TestNamespace::new(
        "spec.routing",
        json!({ "plan": { "enabled": false } }),
        json!({ "cache": null }),
        json!({ "global": { "count": 0 } }),
    ));
    let seen = env
        .commit_host(|tx, _ctx| {
            let routing = routing.clone();
            async move {
                let mut view = tx.plugins(&routing)?;
                let cache = view.get("cache")?;
                if cache != Some(json!(null)) {
                    anyhow::bail!("the declared null default is visible, not absence");
                }
                let plan = view.get("plan")?.unwrap();
                let mut plan = plan.as_object().cloned().unwrap();
                plan.insert("enabled".to_owned(), json!(true));
                view.set("plan", Value::Object(plan))?;
                let global = view.get("global")?.unwrap();
                let mut global = global.as_object().cloned().unwrap();
                global.insert(
                    "count".to_owned(),
                    json!(global.get("count").and_then(Value::as_i64).unwrap_or(0) + 1),
                );
                view.set("global", Value::Object(global))?;
                view.read().map(|slice| json!(slice))
            }
            .boxed()
        })
        .await
        .unwrap()
        .value;
    assert_eq!(
        seen,
        json!({ "plan": { "enabled": true }, "cache": null, "global": { "count": 1 } })
    );
    let rewindable = env.rewindable(1).await.unwrap();
    assert_eq!(
        rewindable
            .get("plugins")
            .and_then(|plugins| plugins.get("spec.routing"))
            .cloned(),
        Some(json!({ "plan": { "enabled": true } })),
        "routes to their declared documents"
    );
    let sticky = env.sticky(1).await.unwrap();
    assert_eq!(
        sticky
            .get("plugins")
            .and_then(|plugins| plugins.get("spec.routing"))
            .cloned(),
        Some(json!({ "cache": null })),
        "null is preserved, not defaulted over"
    );

    // Independent slices increment without clobbering.
    env.commit_host(|tx, _ctx| {
        let a = a.clone();
        let b = b.clone();
        async move {
            for namespace in [&a, &b] {
                let mut view = tx.plugins(namespace)?;
                let count = view.get("count")?.unwrap();
                view.set("count", json!(count.as_i64().unwrap() + 1))?;
            }
            Ok(())
        }
        .boxed()
    })
    .await
    .unwrap();
    let rewindable = env.rewindable(1).await.unwrap();
    let plugins = rewindable
        .get("plugins")
        .and_then(Value::as_object)
        .unwrap();
    assert_eq!(plugins.get("a").cloned(), Some(json!({ "count": 1 })));
    assert_eq!(plugins.get("b").cloned(), Some(json!({ "count": 11 })));

    // A stale token (after unregister) rejects (`spec-plugins-lifecycle`).
    let stale = a;
    env.unregister_namespace(&stale);
    let error = env
        .commit_host(|tx, _ctx| {
            let stale = stale.clone();
            async move {
                tx.plugins(&stale)?;
                Ok(())
            }
            .boxed()
        })
        .await
        .unwrap_err();
    assert_named(&error, "Forbidden");
}

/// `reads.test.ts` "session line: a storage commit failure faults the
/// session; later operations reject Faulted; a callback failure before
/// persistence does not fault" (`session.ts:1340-1350, 1491-1494`).
#[tokio::test]
async fn storage_failure_faults_the_session_but_callback_failures_do_not() {
    struct Flaky {
        inner: MemoryStorage,
        fail: std::sync::atomic::AtomicBool,
    }
    impl crate::agent_core::harness::pico3::types::Storage for Flaky {
        fn commit<'a>(
            &'a self,
            writes: Vec<crate::agent_core::harness::pico3::types::Write>,
            context: Context,
        ) -> futures::future::BoxFuture<'a, anyhow::Result<i64>> {
            Box::pin(async move {
                if self.fail.load(std::sync::atomic::Ordering::SeqCst) {
                    anyhow::bail!("disk full");
                }
                self.inner.commit(writes, context).await
            })
        }
        fn mint_id(&self) -> i64 {
            self.inner.mint_id()
        }
        fn conversation<'a>(
            &'a self,
            id: i64,
            context: Context,
        ) -> futures::future::BoxFuture<
            'a,
            anyhow::Result<Option<crate::agent_core::harness::pico3::types::Conversation>>,
        > {
            Box::pin(async move { self.inner.conversation(id, context).await })
        }
        fn conversations<'a>(
            &'a self,
            context: Context,
        ) -> futures::future::BoxFuture<
            'a,
            anyhow::Result<Vec<crate::agent_core::harness::pico3::types::Conversation>>,
        > {
            Box::pin(async move { self.inner.conversations(context).await })
        }
        fn entries<'a>(
            &'a self,
            ids: &[i64],
            context: Context,
        ) -> futures::future::BoxFuture<
            'a,
            anyhow::Result<
                std::collections::HashMap<i64, crate::agent_core::harness::pico3::types::Entry>,
            >,
        > {
            let ids = ids.to_vec();
            Box::pin(async move { self.inner.entries(&ids, context).await })
        }
        fn scan_entries<'a>(
            &'a self,
            scan: &crate::agent_core::harness::pico3::types::EntryScan,
            context: Context,
        ) -> futures::future::BoxFuture<
            'a,
            anyhow::Result<Vec<crate::agent_core::harness::pico3::types::Entry>>,
        > {
            let scan = scan.clone();
            Box::pin(async move { self.inner.scan_entries(&scan, context).await })
        }
        fn task<'a>(
            &'a self,
            id: i64,
            context: Context,
        ) -> futures::future::BoxFuture<
            'a,
            anyhow::Result<Option<crate::agent_core::harness::pico3::types::Task>>,
        > {
            Box::pin(async move { self.inner.task(id, context).await })
        }
        fn scan_tasks<'a>(
            &'a self,
            scan: &crate::agent_core::harness::pico3::types::TaskScan,
            context: Context,
        ) -> futures::future::BoxFuture<
            'a,
            anyhow::Result<Vec<crate::agent_core::harness::pico3::types::Task>>,
        > {
            let scan = scan.clone();
            Box::pin(async move { self.inner.scan_tasks(&scan, context).await })
        }
        fn input<'a>(
            &'a self,
            id: i64,
            context: Context,
        ) -> futures::future::BoxFuture<
            'a,
            anyhow::Result<Option<crate::agent_core::harness::pico3::types::Input>>,
        > {
            Box::pin(async move { self.inner.input(id, context).await })
        }
        fn input_by_request<'a>(
            &'a self,
            conversation_id: i64,
            request_id: &str,
            context: Context,
        ) -> futures::future::BoxFuture<
            'a,
            anyhow::Result<Option<crate::agent_core::harness::pico3::types::Input>>,
        > {
            let request_id = request_id.to_owned();
            Box::pin(async move {
                self.inner
                    .input_by_request(conversation_id, &request_id, context)
                    .await
            })
        }
        fn doc<'a>(
            &'a self,
            r#ref: &crate::agent_core::harness::pico3::types::DocRef,
            context: Context,
        ) -> futures::future::BoxFuture<
            'a,
            anyhow::Result<Option<serde_json::Map<String, serde_json::Value>>>,
        > {
            let r#ref = *r#ref;
            Box::pin(async move { self.inner.doc(&r#ref, context).await })
        }
        fn doc_as_of<'a>(
            &'a self,
            conversation_id: i64,
            at: i64,
            context: Context,
        ) -> futures::future::BoxFuture<
            'a,
            anyhow::Result<Option<serde_json::Map<String, serde_json::Value>>>,
        > {
            Box::pin(async move { self.inner.doc_as_of(conversation_id, at, context).await })
        }
        fn truncate<'a>(
            &'a self,
            r#ref: &crate::agent_core::harness::pico3::types::DocRef,
            context: Context,
        ) -> futures::future::BoxFuture<'a, anyhow::Result<()>> {
            let r#ref = *r#ref;
            Box::pin(async move { self.inner.truncate(&r#ref, context).await })
        }
        fn close<'a>(
            &'a self,
            context: Context,
        ) -> futures::future::BoxFuture<'a, anyhow::Result<()>> {
            Box::pin(async move { self.inner.close(context).await })
        }
    }
    let flaky = std::sync::Arc::new(Flaky {
        inner: MemoryStorage::new(),
        fail: std::sync::atomic::AtomicBool::new(false),
    });
    let env = Env::open_with_storage(
        flaky.clone() as std::sync::Arc<dyn crate::agent_core::harness::pico3::types::Storage>
    )
    .await
    .unwrap();
    // A callback failure before persistence does not fault.
    let error = env
        .commit_host(
            |_tx, _ctx| -> futures::future::BoxFuture<'_, anyhow::Result<()>> {
                Box::pin(async move { anyhow::bail!("mine") })
            },
        )
        .await
        .unwrap_err();
    assert!(format!("{error}").contains("mine"));
    env.commit_host(|tx, _ctx| {
        async move {
            tx.write(1, NewEntry::new("note")).await?;
            Ok(())
        }
        .boxed()
    })
    .await
    .unwrap();
    // A storage failure faults the session.
    flaky.fail.store(true, std::sync::atomic::Ordering::SeqCst);
    let error = env
        .commit_host(|tx, _ctx| {
            async move {
                tx.write(1, NewEntry::new("note")).await?;
                Ok(())
            }
            .boxed()
        })
        .await
        .unwrap_err();
    assert_named(&error, "Faulted");
    let error = env
        .commit_host(|tx, _ctx| {
            async move {
                tx.write(1, NewEntry::new("note")).await?;
                Ok(())
            }
            .boxed()
        })
        .await
        .unwrap_err();
    assert_named(&error, "Faulted");
}

/// `reads.test.ts` "session line: listener throws never reach the writer;
/// nested line entry rejects; cancellation is checked before the callback".
#[tokio::test]
async fn line_semantics_listener_throws_nested_ops_and_cancellation() {
    let env = Env::open_memory().await.unwrap();
    let reports: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    {
        let reports = reports.clone();
        env.session
            .set_on_report(std::sync::Arc::new(move |error: &anyhow::Error| {
                reports.lock().unwrap().push(format!("{error}"));
            }));
    }
    // A throwing line listener never fails the writer.
    env.session
        .add_line_listener(std::sync::Arc::new(|_record| {
            panic!("listener boom");
        }));
    env.commit_host(|tx, _ctx| {
        async move {
            tx.write(1, NewEntry::new("note")).await?;
            Ok(())
        }
        .boxed()
    })
    .await
    .unwrap();
    assert!(
        reports
            .lock()
            .unwrap()
            .iter()
            .any(|report| report.contains("listener boom")),
        "the listener failure was reported, not surfaced: {:?}",
        reports.lock().unwrap()
    );
    // Nested line operations reject: the line context is passed to the
    // inner commit, as upstream's `reads.test.ts:317-319` does.
    let session = env.session.clone();
    let nested = env
        .commit_host(move |_tx, line_ctx| {
            let session = session.clone();
            Box::pin(async move {
                session
                    .commit(
                        Invoker::Host {
                            conversation_id: Some(1),
                        },
                        line_ctx,
                        Default::default(),
                        |_tx, _ctx| Box::pin(async { Ok(()) }),
                    )
                    .await
                    .map(|_| ())
            }) as futures::future::BoxFuture<'_, anyhow::Result<()>>
        })
        .await
        .unwrap_err();
    assert_named(&nested, "NestedLineOperation");
    // Cancellation is checked before the callback.
    let token = tokio_util::sync::CancellationToken::new();
    let cancelled = crate::agent_core::harness::context::with_abort_signal(token.clone(), ctx());
    token.cancel();
    let error = env
        .session
        .commit(
            Invoker::Host {
                conversation_id: Some(1),
            },
            cancelled,
            Default::default(),
            |tx, _ctx| {
                async move {
                    tx.write(1, NewEntry::new("note")).await?;
                    Ok(())
                }
                .boxed()
            },
        )
        .await
        .unwrap_err();
    assert!(format!("{error}").contains("aborted"), "{error}");
}

/// `spec-context-capabilities.test.ts` "line context is propagated and
/// cannot re-enter commit" (propagation half; reentry covered above).
#[tokio::test]
async fn line_context_propagates_caller_values() {
    let env = Env::open_memory().await.unwrap();
    static CALLER_KEY: std::sync::OnceLock<crate::agent_core::chord_support::ContextKey<String>> =
        std::sync::OnceLock::new();
    let caller_key = CALLER_KEY
        .get_or_init(|| crate::agent_core::chord_support::create_context_key("pico3.spec.caller"));
    let caller_context = ctx().with_value(caller_key, "caller-value".to_owned());
    env.session
        .commit(
            Invoker::Host {
                conversation_id: Some(1),
            },
            caller_context,
            Default::default(),
            |_tx, line_context| {
                async move {
                    assert_eq!(
                        line_context.get(caller_key).map(|value| (*value).clone()),
                        Some("caller-value".to_owned()),
                        "the caller context propagates onto the line"
                    );
                    Ok(())
                }
                .boxed()
            },
        )
        .await
        .unwrap();
}

/// `authority.test.ts` "one owning Session per Storage object in-process"
/// (`session.ts:1243`).
#[tokio::test]
async fn one_owning_session_per_storage() {
    let storage: std::sync::Arc<dyn crate::agent_core::harness::pico3::types::Storage> =
        std::sync::Arc::new(MemoryStorage::new());
    let first = crate::agent_core::harness::pico3::session::Session::new(
        storage.clone(),
        stub_kinds(),
        Default::default(),
    )
    .unwrap();
    let error = crate::agent_core::harness::pico3::session::Session::new(
        storage.clone(),
        stub_kinds(),
        Default::default(),
    )
    .unwrap_err();
    assert!(
        format!("{error}").contains("already has an owning Session"),
        "{error}"
    );
    first.close(ctx()).await.unwrap();
    // Released on close.
    let second = crate::agent_core::harness::pico3::session::Session::new(
        storage.clone(),
        stub_kinds(),
        Default::default(),
    );
    assert!(second.is_ok());
    second.unwrap().close(ctx()).await.unwrap();
}

/// `authority.test.ts` "host tx: core operations reject via cast; host
/// write/plugin state/config work" (`session.ts:368-388`).
#[tokio::test]
async fn host_tx_capability_matrix() {
    let env = Env::open_memory().await.unwrap();
    let state = env.namespace(TestNamespace::new(
        "test.host-session",
        json!({}),
        json!({}),
        json!({ "s": 0 }),
    ));
    for probe_name in ["send", "sticky_view", "boundary"] {
        let error = match probe_name {
            "send" => env
                .commit_host(|tx, _ctx| {
                    async move {
                        tx.send(
                            1,
                            SendInput {
                                content: UserInput::Text("x".to_owned()),
                                ..Default::default()
                            },
                        )
                        .await
                        .map(|_| ())
                    }
                    .boxed()
                })
                .await
                .unwrap_err(),
            "sticky_view" => env
                .commit_host(|tx, _ctx| {
                    async move {
                        tx.sticky_set(1, "steeringMode", json!("all"))?;
                        Ok(())
                    }
                    .boxed()
                })
                .await
                .unwrap_err(),
            _ => env
                .commit_host(|tx, _ctx| {
                    async move { tx.boundary(1, "final", None).await.map(|_| ()) }.boxed()
                })
                .await
                .unwrap_err(),
        };
        assert_named(&error, "Forbidden");
        assert!(
            format!("{error}").contains("core turn machinery only"),
            "{probe_name}: {error}"
        );
    }
    // Host write/plugin state/config work.
    env.commit_host(|tx, _ctx| {
        let state = state.clone();
        async move {
            tx.write(1, NewEntry::new("note")).await?;
            let mut view = tx.plugins(&state)?;
            view.set("s", json!(1))?;
            tx.config_set(1, "profile", json!("p2"))?;
            Ok(())
        }
        .boxed()
    })
    .await
    .unwrap();
    assert_eq!(
        env.commit_host(|tx, _ctx| async move { tx.config_get(1, "profile") }.boxed())
            .await
            .unwrap()
            .value,
        json!("p2"),
    );
}

/// `spec-context-capabilities.test.ts` "ordinary task scope covers its owned
/// subtree but rejects every foreign direct read" (`session.ts:377-405`).
#[tokio::test]
async fn task_scope_covers_owned_subtree_and_rejects_foreign() {
    let env = Env::open_memory().await.unwrap();
    // A foreign conversation with an entry.
    let foreign = env
        .commit_kernel(|tx, _ctx| {
            async move { tx.create_conversation(&Default::default()) }.boxed()
        })
        .await
        .unwrap()
        .value;
    let foreign_input = env
        .commit_conversation(foreign, |tx, _ctx| {
            async move { tx.write(foreign, NewEntry::new("foreign.entry")).await }.boxed()
        })
        .await
        .unwrap()
        .value;
    let foreign_entry = env
        .storage()
        .input(foreign_input, ctx())
        .await
        .unwrap()
        .unwrap()
        .entry
        .expect("idle write placed an entry");
    let (task_id, token) = env.create_background_task().await.unwrap();
    // Inside the task's commit: create child + grandchild, write to them,
    // then attempt every foreign read.
    let attempts: Vec<&str> = env
        .commit_task(&token, task_id, move |tx, _ctx| {
            let foreign = foreign;
            let foreign_entry = foreign_entry;
            async move {
                let child = tx.create_conversation(&Default::default())?;
                let grandchild = tx.create_conversation(&crate::agent_core::harness::pico3::types::ConversationSpec {
                    parent: Some(crate::agent_core::harness::pico3::types::ConversationParentSpec::Parent {
                        conversation_id: child,
                        at: crate::agent_core::harness::pico3::types::ParentAt::Start,
                    }),
                    ..Default::default()
                })?;
                let conversation = tx.conversation(child).await?.expect("child visible");
                if conversation.owner != Some(task_id) {
                    anyhow::bail!("the child should be owned by the creating task");
                }
                // Off-line-shaped reads before any same-batch append (an
                // in-tx context after an append poisons, as upstream).
                tx.context(grandchild, None).await?;
                tx.snapshot(crate::agent_core::harness::pico3::types::DocRef::Sticky {
                    conversation_id: grandchild,
                })?;
                tx.write(child, NewEntry::new("owned.child")).await?;
                tx.write(grandchild, NewEntry::new("owned.grandchild")).await?;
                let mut failures = Vec::new();
                let foreign_conversation = tx.conversation(foreign).await;
                if foreign_conversation.is_ok() {
                    failures.push("tx.conversation");
                }
                let foreign_entry_read = tx.entry(foreign_entry).await;
                if foreign_entry_read.is_ok() {
                    failures.push("tx.entry");
                }
                let foreign_entries = tx.entries(&[foreign_entry]).await;
                if foreign_entries.is_ok() {
                    failures.push("tx.entries");
                }
                let foreign_input_probe = tx.write(foreign, NewEntry::new("intrusion")).await;
                if foreign_input_probe.is_ok() {
                    failures.push("tx.write");
                }
                let foreign_newest = tx.newest_entry(foreign, None, false).await;
                if foreign_newest.is_ok() {
                    failures.push("tx.newestEntry");
                }
                let foreign_context = tx.context(foreign, None).await;
                if foreign_context.is_ok() {
                    failures.push("tx.context");
                }
                let foreign_scan = tx
                    .scan_entries(&crate::agent_core::harness::pico3::types::EntryScan {
                        conversation_id: foreign,
                        limit: 5,
                        ..Default::default()
                    })
                    .await;
                if foreign_scan.is_ok() {
                    failures.push("tx.scanEntries");
                }
                if !failures.is_empty() {
                    anyhow::bail!("foreign reads unexpectedly allowed: {failures:?}");
                }
                Ok(failures)
            }
            .boxed()
        })
        .await
        .unwrap()
        .value;
    assert!(attempts.is_empty());
    // Every foreign failure is Forbidden.
    let error = env
        .commit_task(&token, task_id, |tx, _ctx| {
            async move { tx.conversation(foreign).await.map(|_| ()) }.boxed()
        })
        .await
        .unwrap_err();
    assert_named(&error, "Forbidden");
    assert!(
        format!("{error}").contains("outside this task's subtree"),
        "{error}"
    );
}

/// `authority.test.ts` "a captured runtime cannot commit after its
/// invocation returned, after terminalization, or (run mode) after a durable
/// mark" (`session.ts:1293-1300`).
#[tokio::test]
async fn task_invocation_token_lifetime() {
    let env = Env::open_memory().await.unwrap();
    let (task_id, token) = env.create_background_task().await.unwrap();
    // A live task with an alive token can commit.
    env.commit_task(&token, task_id, |_tx, _ctx| async { Ok(()) }.boxed())
        .await
        .unwrap();
    // A revoked token rejects.
    token.revoke();
    let error = env
        .commit_task(&token, task_id, |_tx, _ctx| async { Ok(()) }.boxed())
        .await
        .unwrap_err();
    assert!(
        format!("{error}").contains("finished invocation"),
        "{error}"
    );
    // An unknown (non-live) task rejects.
    let fresh_token = InvocationToken::new();
    let error = env
        .commit_task(&fresh_token, 999, |_tx, _ctx| async { Ok(()) }.boxed())
        .await
        .unwrap_err();
    assert!(format!("{error}").contains("not live"), "{error}");
    // A marked run invocation rejects.
    let (marked_id, marked_token) = env.create_background_task().await.unwrap();
    env.commit_kernel(|tx, _ctx| async move { tx.mark_task(marked_id) }.boxed())
        .await
        .unwrap();
    let error = env
        .commit_task(&marked_token, marked_id, |_tx, _ctx| {
            async { Ok(()) }.boxed()
        })
        .await
        .unwrap_err();
    assert!(
        format!("{error}").contains("marked run invocation"),
        "{error}"
    );
}

/// `authority.test.ts` "stale/redeclared kind tokens reject for createTask"
/// (`session.ts:832-835`) + core-task guard (`session.ts:842-843`).
#[tokio::test]
async fn stale_kind_tokens_and_core_task_guards() {
    let env = Env::open_memory().await.unwrap();
    // A different Arc with the same name is a stale token.
    let stale: std::sync::Arc<dyn crate::agent_core::harness::pico3::types::AnyKind> =
        std::sync::Arc::new(crate::agent_core::harness::pico3::types::BasicKind::new(
            "pi.plugin",
        ));
    let error = env
        .commit_host(move |tx, _ctx| {
            let stale = stale.clone();
            async move {
                tx.create_task_kind(
                    &stale,
                    Value::Null,
                    crate::agent_core::harness::pico3::session::CreateTaskOptions {
                        conversation_id: Some(1),
                        ..Default::default()
                    },
                )?;
                Ok(())
            }
            .boxed()
        })
        .await
        .unwrap_err();
    assert!(
        format!("{error}").contains("not the registered token"),
        "{error}"
    );
    // A host cannot create a core kind task.
    let generation = generation_kind(&env);
    let error = env
        .commit_host(move |tx, _ctx| {
            let generation = generation.clone();
            async move {
                tx.create_task_kind(
                    &generation,
                    json!({ "inputs": [] }),
                    crate::agent_core::harness::pico3::session::CreateTaskOptions {
                        conversation_id: Some(1),
                        ..Default::default()
                    },
                )?;
                Ok(())
            }
            .boxed()
        })
        .await
        .unwrap_err();
    assert_named(&error, "Forbidden");
    assert!(format!("{error}").contains("create core task"), "{error}");
}

/// `session.ts:847-850` + `busy.test.ts`: a second live `pi.generation`
/// rejects with GenerationInProgress; `busy()` queues passive writes while a
/// turn task is live.
#[tokio::test]
async fn busy_gates_and_generation_in_progress() {
    let env = Env::open_memory().await.unwrap();
    let generation = generation_kind(&env);
    env.commit_kernel(move |tx, _ctx| {
        let generation = generation.clone();
        async move {
            tx.create_task_kind(
                &generation,
                json!({ "inputs": [] }),
                crate::agent_core::harness::pico3::session::CreateTaskOptions {
                    conversation_id: Some(1),
                    ..Default::default()
                },
            )?;
            Ok(())
        }
        .boxed()
    })
    .await
    .unwrap();
    // The turn task makes the conversation busy: a write queues.
    let queued = env
        .commit_host(|tx, _ctx| async move { tx.write(1, NewEntry::new("note")).await }.boxed())
        .await
        .unwrap()
        .value;
    let input = env.storage().input(queued, ctx()).await.unwrap().unwrap();
    assert_eq!(
        input.status, "queued",
        "busy conversations queue passive writes"
    );
    let sticky = env.sticky(1).await.unwrap();
    assert_eq!(
        sticky.get("inbox").and_then(Value::as_array).map(Vec::len),
        Some(1)
    );
    // A second pi.generation rejects with GenerationInProgress.
    let generation = generation_kind(&env);
    let error = env
        .commit_kernel(move |tx, _ctx| {
            let generation = generation.clone();
            async move {
                tx.create_task_kind(
                    &generation,
                    json!({ "inputs": [] }),
                    crate::agent_core::harness::pico3::session::CreateTaskOptions {
                        conversation_id: Some(1),
                        ..Default::default()
                    },
                )?;
                Ok(())
            }
            .boxed()
        })
        .await
        .unwrap_err();
    assert_named(&error, "GenerationInProgress");
    // Terminalizing the generation un-busies the conversation.
    env.commit_kernel(move |tx, _ctx| {
        async move {
            let tasks = tx.tasks(&Default::default()).await?;
            for mut task in tasks {
                if task.kind == "pi.generation" {
                    task.status = crate::agent_core::harness::pico3::types::TaskStatus::Terminal;
                    task.outcome =
                        Some(crate::agent_core::harness::pico3::types::Outcome::orphaned());
                    tx.set_task(task)?;
                }
            }
            Ok(())
        }
        .boxed()
    })
    .await
    .unwrap();
    let placed = env
        .commit_host(|tx, _ctx| async move { tx.write(1, NewEntry::new("note")).await }.boxed())
        .await
        .unwrap()
        .value;
    let input = env.storage().input(placed, ctx()).await.unwrap().unwrap();
    assert_eq!(
        input.status, "done",
        "an idle conversation appends immediately"
    );
}

/// `session.ts:1013-1059`: `send` on an idle conversation places the user
/// entry, settles the input, creates the generation, and emits turn.started;
/// `requestId` dedups across queued sends.
#[tokio::test]
async fn send_places_entries_and_creates_generation() {
    let env = Env::open_memory().await.unwrap();
    let id = env
        .commit_kernel(|tx, _ctx| {
            async move {
                tx.send(
                    1,
                    SendInput {
                        content: UserInput::Text("hello".to_owned()),
                        request_id: Some("r".to_owned()),
                        ..Default::default()
                    },
                )
                .await
            }
            .boxed()
        })
        .await
        .unwrap()
        .value;
    let entries = env.entries(1).await.unwrap();
    assert!(
        entries.iter().any(|entry| entry.kind == "pi.user"),
        "the user entry landed"
    );
    let input = env.storage().input(id, ctx()).await.unwrap().unwrap();
    assert_eq!(input.status, "placed");
    assert!(
        env.session
            .live_tasks()
            .values()
            .any(|task| task.kind == "pi.generation"),
        "the admission created the generation task"
    );
    // requestId dedup: a second send with the same key resolves to the same
    // input (`session.ts:1015-1018`).
    let duplicate = env
        .commit_kernel(|tx, _ctx| {
            async move {
                tx.send(
                    1,
                    SendInput {
                        content: UserInput::Text("dup".to_owned()),
                        request_id: Some("r".to_owned()),
                        ..Default::default()
                    },
                )
                .await
            }
            .boxed()
        })
        .await
        .unwrap()
        .value;
    assert_eq!(
        duplicate, id,
        "the requestId resolves to the original input"
    );
    // whenBusy: reject (`session.ts:1021`).
    let error = env
        .commit_kernel(|tx, _ctx| {
            async move {
                tx.send(
                    1,
                    SendInput {
                        content: UserInput::Text("queued".to_owned()),
                        when_busy: Some("reject".to_owned()),
                        ..Default::default()
                    },
                )
                .await
                .map(|_| ())
            }
            .boxed()
        })
        .await
        .unwrap_err();
    assert_named(&error, "ConversationBusy");
}

/// `session.ts:1067-1086`: `withdrawInput` and `resolveInputs`.
#[tokio::test]
async fn withdraw_and_resolve_inputs() {
    let env = Env::open_memory().await.unwrap();
    // Occupy the conversation so a write queues.
    let generation = generation_kind(&env);
    env.commit_kernel(move |tx, _ctx| {
        let generation = generation.clone();
        async move {
            tx.create_task_kind(
                &generation,
                json!({ "inputs": [] }),
                crate::agent_core::harness::pico3::session::CreateTaskOptions {
                    conversation_id: Some(1),
                    ..Default::default()
                },
            )?;
            Ok(())
        }
        .boxed()
    })
    .await
    .unwrap();
    let queued = env
        .commit_host(|tx, _ctx| async move { tx.write(1, NewEntry::new("note")).await }.boxed())
        .await
        .unwrap()
        .value;
    let outcome = env
        .commit_kernel(|tx, _ctx| async move { tx.withdraw_input(queued).await }.boxed())
        .await
        .unwrap()
        .value;
    assert_eq!(outcome, "aborted");
    let input = env.storage().input(queued, ctx()).await.unwrap().unwrap();
    assert_eq!(input.status, "unanswered");
    assert_eq!(input.reason.as_deref(), Some("aborted"));
    let sticky = env.sticky(1).await.unwrap();
    assert_eq!(
        sticky.get("inbox").and_then(Value::as_array).map(Vec::len),
        Some(0)
    );
    // resolveInputs settles inputs.
    env.commit_kernel(|tx, _ctx| {
        async move {
            tx.resolve_inputs(&[queued], &Resolution::Done { answer: 42 })
                .await?;
            Ok(())
        }
        .boxed()
    })
    .await
    .unwrap();
    let input = env.storage().input(queued, ctx()).await.unwrap().unwrap();
    assert_eq!(input.status, "done");
    assert_eq!(input.answer, Some(42));
}

/// `session.ts:1088-1152` boundary placement: stale heads, `write` items
/// always place, steering policy picks one at a time.
#[tokio::test]
async fn boundary_places_survivors_and_marks_stale_writes() {
    let env = Env::open_memory().await.unwrap();
    // Queue a steer item while busy.
    let generation = generation_kind(&env);
    env.commit_kernel(move |tx, _ctx| {
        let generation = generation.clone();
        async move {
            tx.create_task_kind(
                &generation,
                json!({ "inputs": [] }),
                crate::agent_core::harness::pico3::session::CreateTaskOptions {
                    conversation_id: Some(1),
                    ..Default::default()
                },
            )?;
            Ok(())
        }
        .boxed()
    })
    .await
    .unwrap();
    let steer = env
        .commit_kernel(|tx, _ctx| {
            async move {
                tx.send(
                    1,
                    SendInput {
                        content: UserInput::Text("steer me".to_owned()),
                        when_busy: Some("steer".to_owned()),
                        ..Default::default()
                    },
                )
                .await
            }
            .boxed()
        })
        .await
        .unwrap()
        .value;
    // Terminalize the turn (idle conversation) then run the final boundary:
    // the steer item places as a pi.user entry.
    env.commit_kernel(|tx, _ctx| {
        async move {
            for mut task in tx.tasks(&Default::default()).await? {
                if task.kind == "pi.generation" {
                    task.status = crate::agent_core::harness::pico3::types::TaskStatus::Terminal;
                    task.outcome =
                        Some(crate::agent_core::harness::pico3::types::Outcome::orphaned());
                    tx.set_task(task)?;
                }
            }
            Ok(())
        }
        .boxed()
    })
    .await
    .unwrap();
    let outcome = env
        .commit_kernel(|tx, _ctx| async move { tx.boundary(1, "final", None).await }.boxed())
        .await
        .unwrap()
        .value;
    assert!(
        outcome.triggers.contains(&steer),
        "the steer item placed and triggers a turn"
    );
    assert!(!outcome.terminated);
    let entries = env.entries(1).await.unwrap();
    assert!(
        entries.iter().any(|entry| entry.kind == "pi.user"),
        "the steer item appended as pi.user"
    );
    let input = env.storage().input(steer, ctx()).await.unwrap().unwrap();
    assert_eq!(input.status, "placed");
    let sticky = env.sticky(1).await.unwrap();
    assert_eq!(
        sticky.get("inbox").and_then(Value::as_array).map(Vec::len),
        Some(0)
    );
}

/// `session.ts:1418-1449` + `kinds.test.ts` fork test storage half: fork
/// inherits rewindable as of the fork point; "start" inherits nothing; an
/// invisible entry rejects.
#[tokio::test]
async fn fork_inherits_rewindable_as_of_the_fork_point() {
    let env = Env::open_memory().await.unwrap();
    let state = env.namespace(ns("test.fork-state", json!({ "k": "" }), json!({})));
    env.commit_host(|tx, _ctx| {
        let state = state.clone();
        async move {
            let mut view = tx.plugins(&state)?;
            view.set("k", json!("v1"))?;
            Ok(())
        }
        .boxed()
    })
    .await
    .unwrap();
    let anchor = env
        .commit_host(|tx, _ctx| async move { tx.write(1, NewEntry::new("note")).await }.boxed())
        .await
        .unwrap()
        .value;
    let input = env.storage().input(anchor, ctx()).await.unwrap().unwrap();
    let at = input.entry.unwrap();
    env.commit_host(|tx, _ctx| {
        let state = state.clone();
        async move {
            let mut view = tx.plugins(&state)?;
            view.set("k", json!("v2"))?;
            Ok(())
        }
        .boxed()
    })
    .await
    .unwrap();
    let child = env
        .session
        .fork(
            1,
            crate::agent_core::harness::pico3::types::ParentAt::Id(at),
            crate::agent_core::harness::pico3::types::ConversationSpec::default(),
            ctx(),
        )
        .await
        .unwrap();
    let rewindable = env.rewindable(child).await.unwrap();
    assert_eq!(
        rewindable
            .get("plugins")
            .and_then(|plugins| plugins.get("test.fork-state"))
            .and_then(|slice| slice.get("k"))
            .cloned(),
        Some(json!("v1")),
        "fork sees the state as of the fork point"
    );
    // Fork at "start" inherits nothing.
    let fresh = env
        .session
        .fork(
            1,
            crate::agent_core::harness::pico3::types::ParentAt::Start,
            crate::agent_core::harness::pico3::types::ConversationSpec::default(),
            ctx(),
        )
        .await
        .unwrap();
    let fresh_rewindable = env.rewindable(fresh).await.unwrap();
    assert_eq!(
        fresh_rewindable
            .get("plugins")
            .and_then(|plugins| plugins.get("test.fork-state"))
            .cloned(),
        None,
        "fork at start inherits nothing"
    );
    // An invisible fork point rejects (`session.ts:1427`).
    let error = env
        .session
        .fork(
            1,
            crate::agent_core::harness::pico3::types::ParentAt::Id(999_999),
            Default::default(),
            ctx(),
        )
        .await
        .unwrap_err();
    assert!(format!("{error}").contains("is not visible"), "{error}");
}

/// `spec-transactions.test.ts` "task patch and slot overlays are
/// read-your-writes within one runtime commit" (`session.ts:665-682,
/// 752-759`).
#[tokio::test]
async fn checkpoint_and_slot_overlays_are_read_your_writes() {
    let env = Env::open_memory().await.unwrap();
    let (task_id, token) = env.create_background_task().await.unwrap();
    let observed = env
        .commit_task(&token, task_id, move |tx, _ctx| {
            async move {
                tx.checkpoint(json!({ "phase": "finish" }))?;
                let task = tx.task(task_id).await?.expect("task overlay");
                assert_eq!(
                    crate::agent_core::harness::pico3::types::checkpoint_phase(
                        &task.checkpoint.unwrap()
                    ),
                    Some("finish"),
                    "the checkpoint overlay is visible to the same transaction"
                );
                let kind = tx
                    .kind_registry()
                    .read()
                    .expect("kind registry")
                    .get("pi.plugin")
                    .cloned()
                    .unwrap();
                let reference =
                    crate::agent_core::harness::pico3::session::TaskRef { id: task_id, kind };
                let slot = tx.slot_get(&reference)?;
                assert_eq!(
                    slot,
                    json!({}),
                    "the slot seeds lazily via the kind's initializer"
                );
                let mut updated = slot.as_object().cloned().unwrap();
                updated.insert("count".to_owned(), json!(1));
                tx.slot_update(&reference, |slot| {
                    if let Some(object) = slot.as_object_mut() {
                        object.insert("count".to_owned(), json!(1));
                    }
                })?;
                let _ = updated;
                let sticky =
                    tx.snapshot(crate::agent_core::harness::pico3::types::DocRef::Sticky {
                        conversation_id: 1,
                    })?;
                let count = sticky
                    .get("tasks")
                    .and_then(|tasks| tasks.get(task_id.to_string()))
                    .and_then(|slot| slot.get("count"))
                    .cloned();
                Ok(count)
            }
            .boxed()
        })
        .await
        .unwrap()
        .value;
    assert_eq!(
        observed,
        Some(json!(1)),
        "the slot write is visible through the sticky snapshot"
    );
    // checkpoint from a host invoker rejects (capability).
    let error = env
        .commit_host(|tx, _ctx| async move { tx.checkpoint(json!({ "phase": "x" })) }.boxed())
        .await
        .unwrap_err();
    assert_named(&error, "Forbidden");
}

/// `session.ts:1401-1416` retire: retires the slot and truncates the sticky
/// log to its base when idle (`sticky log` oracle, `kinds.test.ts` last test
/// at Session level).
#[tokio::test]
async fn retire_deletes_the_slot_and_requests_base_when_idle() {
    let env = Env::open_jsonl().await.unwrap();
    let (task_id, token) = env.create_background_task().await.unwrap();
    // Give the task a slot (task-scoped capability).
    env.commit_task(&token, task_id, move |tx, _ctx| {
        async move {
            let kind = tx
                .kind_registry()
                .read()
                .expect("kind registry")
                .get("pi.plugin")
                .cloned()
                .unwrap();
            let reference =
                crate::agent_core::harness::pico3::session::TaskRef { id: task_id, kind };
            tx.slot_update(&reference, |slot| {
                if let Some(object) = slot.as_object_mut() {
                    object.insert("progress".to_owned(), json!("half"));
                }
            })?;
            Ok(())
        }
        .boxed()
    })
    .await
    .unwrap();
    assert_eq!(
        env.sticky(1)
            .await
            .unwrap()
            .get("tasks")
            .and_then(|tasks| tasks.get(task_id.to_string()))
            .and_then(|slot| slot.get("progress"))
            .cloned(),
        Some(json!("half")),
    );
    // Terminalize the task, then retire.
    env.commit_kernel(move |tx, _ctx| {
        async move {
            let mut task = tx.task(task_id).await?.expect("task");
            task.status = crate::agent_core::harness::pico3::types::TaskStatus::Terminal;
            task.outcome = Some(crate::agent_core::harness::pico3::types::Outcome::orphaned());
            tx.set_task(task)?;
            Ok(())
        }
        .boxed()
    })
    .await
    .unwrap();
    let task = env.storage().task(task_id, ctx()).await.unwrap().unwrap();
    env.session.retire(&task, ctx()).await.unwrap();
    let sticky = env.sticky(1).await.unwrap();
    assert!(
        sticky
            .get("tasks")
            .and_then(|tasks| tasks.get(task_id.to_string()))
            .is_none(),
        "the retired task's slot is gone"
    );
    assert_eq!(
        sticky.get("steeringMode").cloned(),
        Some(json!("all")),
        "declared defaults stay visible after truncation"
    );
}

/// `spec-context-capabilities.test.ts` "runtime capability checks remain
/// authoritative after structural casts and forged task metadata"
/// (`session.ts:368-375`): a core=false task invoker cannot borrow core
/// authority.
#[tokio::test]
async fn forged_core_metadata_confers_nothing() {
    let env = Env::open_memory().await.unwrap();
    let (task_id, token) = env.create_background_task().await.unwrap();
    // Even with the task invoker in hand, core operations reject because the
    // invoker is not core (the stub kind is ordinary).
    let error = env
        .commit_task(&token, task_id, |tx, _ctx| {
            async move {
                tx.append_entry(1, NewEntry::new("forged.entry"))?;
                Ok(())
            }
            .boxed()
        })
        .await
        .unwrap_err();
    assert_named(&error, "Forbidden");
    // A core task invoker (kernel authority in a task shape) may.
    let core_token = InvocationToken::new();
    env.commit_task_on(&core_token, task_id, 1, true, |tx, _ctx| {
        async move {
            tx.append_entry(1, NewEntry::new("core.entry"))?;
            Ok(())
        }
        .boxed()
    })
    .await
    .unwrap();
    assert!(env
        .entries(1)
        .await
        .unwrap()
        .iter()
        .any(|entry| entry.kind == "core.entry"));
}

/// Round-1 review fix 1: `setTask`'s patch diff compares absent (None)
/// against stored null separately (`session.ts:771-775` compares
/// `JSON.stringify(prev?.[key]) === JSON.stringify(task[key])`), so a
/// non-terminal patch on a task without checkpoint/outcome carries neither
/// key on the wire, and an identical setTask writes nothing (the patch is
/// empty) — so a same-transaction `tasks()` scan does not poison.
#[tokio::test]
async fn set_task_patches_only_changed_fields() {
    let env = Env::open_jsonl().await.unwrap();
    let dir = env.dir.clone().unwrap();
    let (task_id, _token) = env.create_background_task().await.unwrap();
    env.commit_kernel(move |tx, _ctx| {
        async move {
            let mut task = tx.task(task_id).await?.expect("task");
            task.status = crate::agent_core::harness::pico3::types::TaskStatus::Running;
            tx.set_task(task)?;
            Ok(())
        }
        .boxed()
    })
    .await
    .unwrap();
    // Non-terminal patches live in the task sidecar (`jsonl.ts:209-215`).
    let sidecar = std::fs::read_to_string(dir.join(format!("task-{task_id}.jsonl"))).unwrap();
    let patch_line = sidecar
        .lines()
        .rev()
        .find(|line| line.contains("\"task.patch\""))
        .expect("the running patch persisted");
    let record: Value = serde_json::from_str(patch_line).unwrap();
    let patch = &record["writes"][0]["patch"];
    assert_eq!(patch["status"], json!("running"));
    assert!(
        patch.get("checkpoint").is_none(),
        "absent checkpoint stays absent on the wire: {patch}"
    );
    assert!(
        patch.get("outcome").is_none(),
        "absent outcome stays absent on the wire: {patch}"
    );
    assert!(
        patch.get("abort").is_none(),
        "absent abort stays absent on the wire: {patch}"
    );

    // An identical setTask produces an empty patch: no write, no seq, and a
    // same-transaction `tasks()` scan does not reject as ReadAfterWrite.
    let main_before = std::fs::read_to_string(dir.join("main.jsonl")).unwrap();
    let result = env
        .commit_kernel(move |tx, _ctx| {
            async move {
                let before = tx.tasks(&Default::default()).await?;
                let task = tx.task(task_id).await?.expect("task");
                tx.set_task(task)?;
                let after = tx.tasks(&Default::default()).await?;
                Ok((before.len(), after.len()))
            }
            .boxed()
        })
        .await
        .unwrap();
    assert_eq!(result.seq, None, "an identical setTask persists nothing");
    assert_eq!(result.value.0, result.value.1);
    let main_after = std::fs::read_to_string(dir.join("main.jsonl")).unwrap();
    assert_eq!(main_before, main_after, "no new main records");
}

/// Round-1 review fix 2 (snapshot half): upstream applies a kind-declared
/// fallback when `fallback !== undefined` (`session.ts:515-519`) — a
/// declared null IS applied to a document missing the key.
#[tokio::test]
async fn snapshot_applies_declared_null_fallbacks() {
    let mut kinds = stub_kinds();
    // The declaring kind is deliberately NOT the session-registered token:
    // the fallback path applies a task's own declaration even when the key
    // never reached the session defaults (nothing seeds it).
    let late_kind: std::sync::Arc<dyn crate::agent_core::harness::pico3::types::AnyKind> =
        std::sync::Arc::new(
            crate::agent_core::harness::pico3::types::BasicKind::new("late.declared").config(
                crate::agent_core::harness::pico3::types::KindConfig {
                    rewindable: Default::default(),
                    sticky: json!({ "w": null }).as_object().cloned().unwrap(),
                    declared_absent: Default::default(),
                },
            ),
        );
    kinds.insert(
        "pi.plugin".to_owned(),
        std::sync::Arc::new(crate::agent_core::harness::pico3::types::BasicKind::new(
            "pi.plugin",
        )),
    );
    let env = Env::open_memory_with_kinds(kinds).await.unwrap();
    let (task_id, token) = env.create_background_task().await.unwrap();
    let observed = env
        .session
        .commit(
            Invoker::Task {
                token,
                id: task_id,
                conversation_id: 1,
                kind: late_kind,
                core: false,
                mode: InvocationMode::Run,
            },
            ctx(),
            crate::agent_core::harness::pico3::session::CommitOptions {
                docs: vec![crate::agent_core::harness::pico3::types::DocRef::Sticky {
                    conversation_id: 1,
                }],
                closing: false,
            },
            |tx, _ctx| {
                async move {
                    let snapshot =
                        tx.snapshot(crate::agent_core::harness::pico3::types::DocRef::Sticky {
                            conversation_id: 1,
                        })?;
                    Ok((
                        snapshot.get("w").cloned(),
                        snapshot.get("undeclared").cloned(),
                    ))
                }
                .boxed()
            },
        )
        .await
        .unwrap()
        .value;
    assert_eq!(
        observed.0,
        Some(json!(null)),
        "a declared null fallback IS applied; it is not treated as absence"
    );
    assert_eq!(observed.1, None, "a key no kind declares stays absent");
}

/// Round-1 review fix 4: constructing the namespace view runs the upstream
/// authority check (`session.ts:570`,
/// `invocationConversationId("plugins(ns)")`) — an invoker with no bound
/// conversation is Forbidden, not a missing-document error.
#[tokio::test]
async fn plugins_view_requires_a_bound_conversation() {
    let env = Env::open_memory().await.unwrap();
    let namespace = env.namespace(ns("test.unbound", json!({}), json!({ "s": 0 })));
    let error = env
        .commit_kernel(move |tx, _ctx| {
            let namespace = namespace.clone();
            async move {
                tx.plugins(&namespace)?;
                Ok(())
            }
            .boxed()
        })
        .await
        .unwrap_err();
    assert_named(&error, "Forbidden");
    assert!(
        format!("{error}").contains("no conversation is bound"),
        "{error}"
    );
}

/// `reads.test.ts` "ordinary tasks can directly read entries inherited by
/// their fork conversation" (`session.ts:389-405` fork-visibility clause).
#[tokio::test]
async fn fork_conversation_tasks_read_inherited_entries() {
    let env = Env::open_memory().await.unwrap();
    let anchor = env
        .commit_host(|tx, _ctx| async move { tx.write(1, NewEntry::new("note")).await }.boxed())
        .await
        .unwrap()
        .value;
    let inherited = env
        .storage()
        .input(anchor, ctx())
        .await
        .unwrap()
        .unwrap()
        .entry
        .unwrap();
    let child = env
        .session
        .fork(
            1,
            crate::agent_core::harness::pico3::types::ParentAt::Id(inherited),
            Default::default(),
            ctx(),
        )
        .await
        .unwrap();
    let (task_id, token) = env.create_background_task().await.unwrap();
    // A task living in the fork conversation reads the inherited entry.
    let observed = env
        .commit_task_on(&token, task_id, child, false, |tx, _ctx| {
            async move {
                let entry = tx.entry(inherited).await?;
                let many = tx.entries(&[inherited]).await?;
                Ok((
                    entry.map(|entry| entry.id),
                    many.get(&inherited).map(|entry| entry.id),
                ))
            }
            .boxed()
        })
        .await
        .unwrap()
        .value;
    assert_eq!(observed, (Some(inherited), Some(inherited)));
}
