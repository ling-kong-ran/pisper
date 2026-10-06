//! Shared fixtures for the pico3 oracle ports (the port of the
//! `helpers.ts` subset that the storage-engine oracles need; see the
//! parent module's disclosed substitution note).

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use futures::FutureExt;
use serde_json::{json, Value};

use crate::agent_core::chord_support::Context;
use crate::agent_core::harness::pico3::session::{CommitOptions, CommitResult, Session, Tx};
use crate::agent_core::harness::pico3::types::{
    BasicKind, InvocationToken, Invoker, JsonObject, Namespace, NamespaceDefaultsValue,
    NamespaceRegistration,
};

/// Upstream `helpers.ctx` (`helpers.ts:51`).
pub fn ctx() -> Context {
    Context::background()
}

/// The stub kind registry: the four core kinds (turn) plus `pi.plugin`
/// (background), mirroring what `harness.ts` registers for the oracle
/// tests' createTask calls. Disclosed fixture detail: the core config
/// declarations the upstream built-ins spread across `pi.collapse`/`pi.*`
/// kinds (`threshold`/`keepRecent`/`steeringMode`/...) are declared here on
/// `pi.collapse` as one disjoint block, so `Defaults` routes them and the
/// seeded documents carry the upstream defaults.
pub fn stub_kinds() -> HashMap<String, Arc<dyn crate::agent_core::harness::pico3::types::AnyKind>> {
    use crate::agent_core::harness::pico3::types::KindConfig;
    let collapse = BasicKind::new("pi.collapse").turn(true).config(KindConfig {
        rewindable: json!({
            "thinkingLevel": "off",
            "selectedTools": [],
            "profile": "default",
            "threshold": 0,
            "keepRecent": 20000,
        })
        .as_object()
        .cloned()
        .unwrap(),
        sticky: json!({
            "retry": { "enabled": false, "maxRetries": 0, "baseDelayMs": 1 },
            "steeringMode": "all",
            "followUpMode": "one-at-a-time",
        })
        .as_object()
        .cloned()
        .unwrap(),
        ..Default::default()
    });
    let kinds: Vec<Arc<dyn crate::agent_core::harness::pico3::types::AnyKind>> = vec![
        Arc::new(BasicKind::new("pi.generation").turn(true)),
        Arc::new(BasicKind::new("pi.tool").turn(true)),
        Arc::new(BasicKind::new("pi.post_tools").turn(true)),
        Arc::new(collapse),
        Arc::new(BasicKind::new("pi.plugin")),
    ];
    kinds
        .into_iter()
        .map(|kind| (kind.name().to_owned(), kind))
        .collect()
}

/// A registered namespace with declared defaults
/// (upstream `harness.namespace`, `helpers.ts` usage).
#[derive(Clone)]
pub struct TestNamespace {
    pub token: Namespace,
    pub registration: NamespaceRegistration,
}

impl TestNamespace {
    pub fn new(id: &str, rewindable: Value, sticky: Value, session: Value) -> TestNamespace {
        TestNamespace {
            token: Namespace {
                id: id.to_owned(),
                generation: 0,
            },
            registration: NamespaceRegistration {
                generation: 0,
                defaults: NamespaceDefaultsValue {
                    rewindable: rewindable.as_object().cloned().unwrap_or_default(),
                    sticky: sticky.as_object().cloned().unwrap_or_default(),
                    session: session.as_object().cloned().unwrap_or_default(),
                },
                routes: {
                    let mut routes = Vec::new();
                    for (key, _) in rewindable.as_object().unwrap_or(&serde_json::Map::new()) {
                        routes.push((key.clone(), "rewindable".to_owned()));
                    }
                    for (key, _) in sticky.as_object().unwrap_or(&serde_json::Map::new()) {
                        routes.push((key.clone(), "sticky".to_owned()));
                    }
                    for (key, _) in session.as_object().unwrap_or(&serde_json::Map::new()) {
                        routes.push((key.clone(), "session".to_owned()));
                    }
                    routes
                },
                project: None,
            },
        }
    }
}

/// The test environment: a Session over a storage backend with the stub
/// kinds and the root conversation seeded (the `helpers.open` equivalent).
pub struct Env {
    pub session: Arc<Session>,
    /// `dir` when jsonl-backed (upstream `env.dir`).
    pub dir: Option<std::path::PathBuf>,
    storage: Arc<dyn crate::agent_core::harness::pico3::types::Storage>,
    next_generation: RwLock<u64>,
    /// Registered namespaces (upstream `h.namespace`).
    namespaces: RwLock<HashMap<String, NamespaceRegistration>>,
}

impl Env {
    /// Open a memory-backed env (`helpers.open` with the default backend).
    pub async fn open_memory() -> anyhow::Result<Env> {
        Self::open_with_storage(Arc::new(
            crate::agent_core::harness::pico3::memory::MemoryStorage::new(),
        ))
        .await
    }

    /// Open a memory-backed env with a custom kind registry (the upstream
    /// harness installs task kinds before open; `reads.test.ts`'s config
    /// test needs a kind that declares config).
    pub async fn open_memory_with_kinds(
        kinds: HashMap<String, Arc<dyn crate::agent_core::harness::pico3::types::AnyKind>>,
    ) -> anyhow::Result<Env> {
        let storage: Arc<dyn crate::agent_core::harness::pico3::types::Storage> =
            Arc::new(crate::agent_core::harness::pico3::memory::MemoryStorage::new());
        let session = crate::agent_core::harness::pico3::session::Session::new(
            storage.clone(),
            kinds,
            Default::default(),
        )?;
        let env = Env {
            session,
            dir: None,
            storage,
            next_generation: RwLock::new(0),
            namespaces: RwLock::new(HashMap::new()),
        };
        env.commit_kernel(|tx, _ctx| {
            async move { tx.create_conversation(&Default::default()) }.boxed()
        })
        .await?;
        Ok(env)
    }

    /// Open a jsonl-backed env in a fresh temp dir
    /// (`helpers.open({ backend: "jsonl" })`).
    pub async fn open_jsonl() -> anyhow::Result<Env> {
        let dir = tempfile::tempdir()?.keep();
        let storage =
            crate::agent_core::harness::pico3::jsonl::JsonlStorage::open(&dir, false).await?;
        Self::from_parts(storage, Some(dir)).await
    }

    /// Reopen on an existing jsonl dir after a crash
    /// (`open({ dir, backend: "jsonl" })`).
    pub async fn reopen_jsonl(dir: std::path::PathBuf) -> anyhow::Result<Env> {
        let storage =
            crate::agent_core::harness::pico3::jsonl::JsonlStorage::open(&dir, false).await?;
        Self::from_parts(storage, Some(dir)).await
    }

    pub async fn open_with_storage(
        storage: Arc<dyn crate::agent_core::harness::pico3::types::Storage>,
    ) -> anyhow::Result<Env> {
        Self::from_parts(storage, None).await
    }

    async fn from_parts(
        storage: Arc<dyn crate::agent_core::harness::pico3::types::Storage>,
        dir: Option<std::path::PathBuf>,
    ) -> anyhow::Result<Env> {
        let session = Session::new(storage.clone(), stub_kinds(), HashMap::new())?;
        let env = Env {
            session,
            dir,
            storage,
            next_generation: RwLock::new(0),
            namespaces: RwLock::new(HashMap::new()),
        };
        // Seed the root conversation (conversation 1) with default
        // rewindable/sticky documents: the harness.open seeding this layer's
        // tests rely on. The core rewindable defaults mirror the oracle
        // tests' `root.rewindable` defaults (model absent).
        let kernel = Invoker::Kernel {
            conversation_id: None,
        };
        env.session
            .commit(kernel, ctx(), CommitOptions::default(), |tx, _ctx| {
                async move {
                    tx.create_conversation(
                        &crate::agent_core::harness::pico3::types::ConversationSpec::default(),
                    )?;
                    Ok(())
                }
                .boxed()
            })
            .await?;
        Ok(env)
    }

    /// Upstream `env.h.namespace(id, defaults)` (`helpers.ts:348`).
    pub fn namespace(&self, test_namespace: TestNamespace) -> Namespace {
        let generation = *self.next_generation.read().expect("generation");
        *self.next_generation.write().expect("generation") += 1;
        let token = Namespace {
            id: test_namespace.token.id.clone(),
            generation,
        };
        let registration = NamespaceRegistration {
            generation,
            ..test_namespace.registration
        };
        self.namespaces
            .write()
            .expect("namespaces")
            .insert(token.id.clone(), registration.clone());
        self.session
            .namespaces()
            .write()
            .expect("session namespaces")
            .insert(token.id.clone(), registration);
        token
    }

    /// Unregister a namespace token (upstream `namespace.unregister()`).
    pub fn unregister_namespace(&self, token: &Namespace) {
        self.namespaces
            .write()
            .expect("namespaces")
            .remove(&token.id);
        self.session
            .namespaces()
            .write()
            .expect("session namespaces")
            .remove(&token.id);
    }

    /// The storage handle (`env.storage`).
    pub fn storage(&self) -> &Arc<dyn crate::agent_core::harness::pico3::types::Storage> {
        &self.storage
    }

    /// Upstream `env.root.commit(fn, ctx)` (`helpers.ts:362`-ish): a host
    /// commit on conversation 1, preloading the conversation documents.
    pub async fn commit_host<T>(
        &self,
        f: impl for<'tx> FnOnce(
            &'tx mut Tx,
            Context,
        ) -> futures::future::BoxFuture<'tx, anyhow::Result<T>>,
    ) -> anyhow::Result<CommitResult<T>> {
        self.commit_conversation(1, f).await
    }

    /// A host commit on an arbitrary conversation.
    pub async fn commit_conversation<T>(
        &self,
        conversation_id: i64,
        f: impl for<'tx> FnOnce(
            &'tx mut Tx,
            Context,
        ) -> futures::future::BoxFuture<'tx, anyhow::Result<T>>,
    ) -> anyhow::Result<CommitResult<T>> {
        self.session
            .commit(
                Invoker::Host {
                    conversation_id: Some(conversation_id),
                },
                ctx(),
                CommitOptions {
                    docs: vec![
                        crate::agent_core::harness::pico3::types::DocRef::Rewindable {
                            conversation_id,
                        },
                        crate::agent_core::harness::pico3::types::DocRef::Sticky {
                            conversation_id,
                        },
                    ],
                    closing: false,
                },
                f,
            )
            .await
    }

    /// A kernel commit (harness-internal operations: seeding, aborts).
    pub async fn commit_kernel<T>(
        &self,
        f: impl for<'tx> FnOnce(
            &'tx mut Tx,
            Context,
        ) -> futures::future::BoxFuture<'tx, anyhow::Result<T>>,
    ) -> anyhow::Result<CommitResult<T>> {
        self.session
            .commit(
                Invoker::Kernel {
                    conversation_id: None,
                },
                ctx(),
                CommitOptions::default(),
                f,
            )
            .await
    }

    /// A task-invoker commit on conversation 1 (`runtime.commit`).
    pub async fn commit_task<T>(
        &self,
        token: &Arc<InvocationToken>,
        task_id: i64,
        f: impl for<'tx> FnOnce(
            &'tx mut Tx,
            Context,
        ) -> futures::future::BoxFuture<'tx, anyhow::Result<T>>,
    ) -> anyhow::Result<CommitResult<T>> {
        self.commit_task_on(token, task_id, 1, false, f).await
    }

    pub async fn commit_task_on<T>(
        &self,
        token: &Arc<InvocationToken>,
        task_id: i64,
        conversation_id: i64,
        core: bool,
        f: impl for<'tx> FnOnce(
            &'tx mut Tx,
            Context,
        ) -> futures::future::BoxFuture<'tx, anyhow::Result<T>>,
    ) -> anyhow::Result<CommitResult<T>> {
        let kind = self
            .session
            .kinds()
            .read()
            .expect("kind registry")
            .get("pi.plugin")
            .cloned()
            .expect("pi.plugin stub kind");
        self.session
            .commit(
                Invoker::Task {
                    token: token.clone(),
                    id: task_id,
                    conversation_id,
                    kind,
                    core,
                    mode: crate::agent_core::harness::pico3::types::InvocationMode::Run,
                },
                ctx(),
                CommitOptions::default(),
                f,
            )
            .await
    }

    /// Create a background task on conversation 1 via the kernel and return
    /// `(task_id, token)`; the task is live after the commit (upstream
    /// scheduler registers the live task before invoking).
    pub async fn create_background_task(&self) -> anyhow::Result<(i64, Arc<InvocationToken>)> {
        let kind = self
            .session
            .kinds()
            .read()
            .expect("kind registry")
            .get("pi.plugin")
            .cloned()
            .expect("registered");
        let created = self
            .commit_kernel(move |tx, _ctx| {
                let kind = kind.clone();
                async move {
                    let reference = tx.create_task_kind(
                        &kind,
                        Value::Null,
                        crate::agent_core::harness::pico3::session::CreateTaskOptions {
                            conversation_id: Some(1),
                            background: true,
                            after: Vec::new(),
                        },
                    )?;
                    Ok(reference.id)
                }
                .boxed()
            })
            .await?;
        Ok((created.value, InvocationToken::new()))
    }

    /// Ascending entries of a conversation (`helpers.entries`, `env.ts:361`).
    pub async fn entries(
        &self,
        conversation_id: i64,
    ) -> anyhow::Result<Vec<crate::agent_core::harness::pico3::types::Entry>> {
        let mut entries = self
            .storage()
            .scan_entries(
                &crate::agent_core::harness::pico3::types::EntryScan {
                    conversation_id,
                    limit: 1000,
                    ..Default::default()
                },
                ctx(),
            )
            .await?;
        entries.reverse();
        Ok(entries)
    }

    /// The rewindable document of a conversation (upstream
    /// `env.root.rewindable(ctx)` — a plain copy of the stored doc).
    pub async fn rewindable(&self, conversation_id: i64) -> anyhow::Result<JsonObject> {
        self.storage()
            .doc(
                &crate::agent_core::harness::pico3::types::DocRef::Rewindable { conversation_id },
                ctx(),
            )
            .await
            .map(|doc| doc.unwrap_or_default())
    }

    /// The sticky document of a conversation (upstream `env.root.sticky`).
    pub async fn sticky(&self, conversation_id: i64) -> anyhow::Result<JsonObject> {
        self.storage()
            .doc(
                &crate::agent_core::harness::pico3::types::DocRef::Sticky { conversation_id },
                ctx(),
            )
            .await
            .map(|doc| doc.unwrap_or_default())
    }

    /// Crash: close the storage without writing (`helpers.crash`).
    pub async fn crash(&self) -> anyhow::Result<()> {
        self.session.close(ctx()).await
    }
}

/// The registered `pi.plugin` kind token from an env.
pub fn plugin_kind(env: &Env) -> Arc<dyn crate::agent_core::harness::pico3::types::AnyKind> {
    env.session
        .kinds()
        .read()
        .expect("kind registry")
        .get("pi.plugin")
        .cloned()
        .expect("registered")
}

/// The registered `pi.generation` kind token from an env.
pub fn generation_kind(env: &Env) -> Arc<dyn crate::agent_core::harness::pico3::types::AnyKind> {
    env.session
        .kinds()
        .read()
        .expect("kind registry")
        .get("pi.generation")
        .cloned()
        .expect("registered")
}

/// Assert helper: the error chain carries the upstream class name.
pub fn assert_named(error: &anyhow::Error, name: &str) {
    assert!(
        crate::agent_core::harness::pico3::types::is_named(error, name),
        "expected {name}, got: {error:#}"
    );
}

/// A namespace definition with a sticky default (common fixture shape).
pub fn ns(id: &str, rewindable: Value, sticky: Value) -> TestNamespace {
    TestNamespace::new(id, rewindable, sticky, json!({}))
}
