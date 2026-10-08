//! Port of `packages/agent/src/harness/pico3/harness.ts` (812 lines): the
//! assembled pico3 harness — registries (kinds/tools/sections/entry
//! kinds/namespaces/hooks), the scheduler wiring, conversation and input
//! handles, watch capture, and lifecycle (`resume`/`suspend`/`hold`).
//!
//! Disclosed substitutions:
//! - **Handles are structs.** Upstream `handle(c)` builds a
//!   `ConversationHandle` object literal (`harness.ts:633-744`); the port's
//!   [`ConversationHandle`] holds the [`Harness`] and the conversation id,
//!   with the same host/kernel commit split and preloaded conversation docs.
//! - **`ConfigFacade` splits into methods** ([`ConversationHandle::config_get`]
//!   / `config_set` / `config_reset`) with identical host authority.
//!   Upstream rejects `undefined` values in `set` ("use config.reset()");
//!   Rust values have no `undefined`, so that one rejection has no port
//!   shape (reset is the removal path).
//! - **Listener wiring.** The Session listener closures hold the harness
//!   through a `Weak` so the Arc graph stays acyclic; upstream relies on GC
//!   for the same shape.
//! - **Runtime registrations.** `registerTaskKind` mutates the scheduler's
//!   execution registry, the runtime metadata mirror, and the Session's
//!   registry in one step (upstream: one `Map` + `session.defaults`).
#![allow(clippy::type_complexity)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use futures::future::BoxFuture;
use futures::FutureExt;
use serde_json::{json, Value};

use crate::agent_core::chord_support::Context;
use crate::agent_core::harness::pico3::hooks::{HookRegistration, HookRunner};
use crate::agent_core::harness::pico3::scheduler::Scheduler;
use crate::agent_core::harness::pico3::session::ConversationIndex;
use crate::agent_core::harness::pico3::session::{
    CommitOptions, CreateTaskOptions, Resolution, Session, Tx,
};
use crate::agent_core::harness::pico3::types::{
    forbidden, AnyKind, BasicKind, ContextView, ConversationSpec, DocRef, Entry, EntryKind,
    EntryScan, Head, Id, Input, Invoker, JsonObject, Namespace, NamespaceDefaultsValue,
    NamespaceRegistration, NewEntry, ParentAt, SendInput, Storage, Task, TaskStatus, UserInput,
    ViewEvent,
};
use crate::agent_core::harness::pico3::view::{ViewManager, Watch};

use super::kinds;
use super::runtime::{
    Kind, Models, PluginHandler, ProcessHost, Runtime, RuntimeOps, ToolDeclaration,
};
use super::scheduler::SchedulerDeps;
use super::system::{SectionRegistry, SystemSection, ToolRegistry};

// ---------------------------------------------------------------------------
// Options (harness.ts:58-71)
// ---------------------------------------------------------------------------

/// Upstream `HarnessOptions` (`harness.ts:58-71`).
#[allow(clippy::type_complexity)]
pub struct HarnessOptions {
    /// Upstream `models`.
    pub models: Arc<dyn Models>,
    /// Upstream `tools`.
    pub tools: Vec<Arc<ToolDeclaration>>,
    /// Upstream `taskKinds`: ordinary kinds; `pi.`-prefixed names and
    /// duplicates reject.
    pub task_kinds: Vec<Arc<dyn Kind>>,
    /// Upstream `sections`.
    pub sections: Vec<SystemSection>,
    /// Upstream `plugins`.
    pub plugins: HashMap<String, PluginHandler>,
    /// Upstream `processHost`.
    pub process_host: Option<Arc<dyn ProcessHost>>,
    /// Upstream `now`: clock for durable kernel timestamps and retry
    /// scheduling.
    pub now: Option<Arc<dyn Fn() -> i64 + Send + Sync>>,
    /// Upstream `root`.
    pub root: Option<RootOptions>,
    /// Upstream `onReport`: errors from listeners, hooks, watches, and the
    /// scheduler. Never delivered as commit failures.
    pub on_report: Option<Arc<dyn Fn(&anyhow::Error) + Send + Sync>>,
}

/// Upstream `root?: { rewindable?, sticky? }` (`harness.ts:68`).
#[derive(Default)]
pub struct RootOptions {
    pub rewindable: Option<JsonObject>,
    pub sticky: Option<JsonObject>,
}

impl HarnessOptions {
    /// Defaults with `now` excepted (`Date.now` upstream).
    pub fn new(models: Arc<dyn Models>) -> HarnessOptions {
        HarnessOptions {
            models,
            tools: Vec::new(),
            task_kinds: Vec::new(),
            sections: Vec::new(),
            plugins: HashMap::new(),
            process_host: None,
            now: None,
            root: None,
            on_report: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Harness (harness.ts:114-775)
// ---------------------------------------------------------------------------

/// Upstream `Harness` (`harness.ts:114`).
#[allow(clippy::type_complexity)]
pub struct Harness {
    session: Arc<Session>,
    scheduler: Arc<Scheduler>,
    views: Arc<ViewManager>,
    /// The registered execution kinds (`harness.ts:157-164`).
    kinds: Arc<RwLock<HashMap<String, Arc<dyn Kind>>>>,
    /// The metadata mirror the runtime hands out (`rt.kinds`,
    /// `harness.ts:223`).
    kinds_metadata: Arc<RwLock<HashMap<String, Arc<dyn AnyKind>>>>,
    tools: Arc<RwLock<HashMap<String, Arc<ToolDeclaration>>>>,
    sections: SectionRegistry,
    tools_registry: ToolRegistry,
    entry_kinds: Arc<Mutex<HashMap<String, EntryKind>>>,
    #[allow(dead_code)]
    plugins: Arc<RwLock<HashMap<String, PluginHandler>>>,
    hook_registrations: Arc<RwLock<Vec<Arc<HookRegistration>>>>,
    conversation_listeners: Mutex<Vec<Arc<dyn Fn(&ConversationHandle) + Send + Sync>>>,
    on_report: Arc<dyn Fn(&anyhow::Error) + Send + Sync>,
    now: Arc<dyn Fn() -> i64 + Send + Sync>,
    base_ctx: Context,
    next_generation: AtomicU64,
    resumed: AtomicBool,
    suspended: AtomicBool,
}

impl Harness {
    /// Upstream `Harness.open` (`harness.ts:134-145`).
    pub async fn open(
        storage: Arc<dyn Storage>,
        options: HarnessOptions,
        ctx: Context,
    ) -> anyhow::Result<Arc<Harness>> {
        let on_report: Arc<dyn Fn(&anyhow::Error) + Send + Sync> = {
            let user = options.on_report.clone();
            Arc::new(move |error: &anyhow::Error| {
                if let Some(user) = &user {
                    // `try { options.onReport?.(error) } catch {}`
                    // (`harness.ts:151-155`).
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| user(error)));
                }
            })
        };
        // Fixed core, installed internally. User kinds may not replace or
        // shadow a built-in (`harness.ts:156-164`).
        let builtins = kinds::builtins();
        let mut kinds_map: HashMap<String, Arc<dyn Kind>> = HashMap::new();
        kinds_map.insert("pi.generation".to_owned(), builtins.generation.clone());
        kinds_map.insert("pi.tool".to_owned(), builtins.tool.clone());
        kinds_map.insert("pi.post_tools".to_owned(), builtins.post_tools.clone());
        kinds_map.insert("pi.collapse".to_owned(), builtins.collapse.clone());
        kinds_map.insert("pi.job".to_owned(), builtins.job.clone());
        kinds_map.insert("pi.plugin".to_owned(), builtins.plugin.clone());
        for kind in &options.task_kinds {
            let name = kind.metadata().name().to_owned();
            if name.starts_with("pi.") {
                anyhow::bail!(r#"task kind "{name}": names beginning with "pi." are reserved"#);
            }
            if kinds_map.contains_key(&name) {
                anyhow::bail!("task kind \"{name}\" registered twice");
            }
            kinds_map.insert(name, kind.clone());
        }
        let kinds_map: HashMap<String, Arc<dyn Kind>> = kinds_map
            .into_iter()
            .map(|(name, kind)| (name, super::runtime::RegisteredKind::capture(kind)))
            .collect();
        let kinds_metadata_map: HashMap<String, Arc<dyn AnyKind>> = kinds_map
            .iter()
            .map(|(name, kind)| (name.clone(), kind.metadata()))
            .collect();
        let kinds: Arc<RwLock<HashMap<String, Arc<dyn Kind>>>> = Arc::new(RwLock::new(kinds_map));
        let kinds_metadata: Arc<RwLock<HashMap<String, Arc<dyn AnyKind>>>> =
            Arc::new(RwLock::new(kinds_metadata_map));
        let tools: Arc<RwLock<HashMap<String, Arc<ToolDeclaration>>>> =
            Arc::new(RwLock::new(HashMap::new()));
        let tools_revision = Arc::new(AtomicU64::new(0));
        let sections = SectionRegistry::new();
        {
            let mut registry = sections.map.write().expect("section registry");
            for section in super::system::system_sections() {
                registry.insert(section.key.clone(), section);
            }
        }
        for section in &options.sections {
            let mut registry = sections.map.write().expect("section registry");
            if registry.contains_key(&section.key) {
                anyhow::bail!("section \"{}\" already registered", section.key);
            }
            registry.insert(section.key.clone(), section.clone());
            drop(registry);
            sections.revision.fetch_add(1, Ordering::SeqCst);
        }
        let tools_registry = ToolRegistry {
            map: tools.clone(),
            revision: tools_revision.clone(),
        };
        for tool in &options.tools {
            let mut registry = tools.write().expect("tools registry");
            if registry.contains_key(&tool.name) {
                anyhow::bail!("tool \"{}\" already registered", tool.name);
            }
            registry.insert(tool.name.clone(), tool.clone());
            drop(registry);
            tools_revision.fetch_add(1, Ordering::SeqCst);
        }
        let mut entry_kinds = HashMap::new();
        for witness in kinds::entries::builtin_entries() {
            entry_kinds.insert(witness.kind.clone(), witness);
        }
        let plugins: Arc<RwLock<HashMap<String, PluginHandler>>> =
            Arc::new(RwLock::new(options.plugins.clone()));
        let now: Arc<dyn Fn() -> i64 + Send + Sync> = options
            .now
            .clone()
            .unwrap_or_else(|| Arc::new(crate::ai::now_ms));
        // Throws if the Storage already has an owner; validates config
        // disjointness (`harness.ts:170`).
        let session = Session::with_clock(
            storage,
            kinds_metadata.read().expect("kinds").clone(),
            HashMap::new(),
            now.clone(),
        )?;
        session.set_on_report(on_report.clone());
        session.defaults();
        let views = ViewManager::new(session.clone());
        let hook_registrations: Arc<RwLock<Vec<Arc<HookRegistration>>>> =
            Arc::new(RwLock::new(Vec::new()));
        // The scheduler-backed runtime ops; the scheduler back-reference is
        // installed right after construction.
        let scheduler_ops = Arc::new(SchedulerOps {
            scheduler: std::sync::OnceLock::new(),
            session: session.clone(),
        });
        let make_runtime: Arc<dyn Fn(Invoker) -> Arc<Runtime> + Send + Sync> = {
            let session = session.clone();
            let models = options.models.clone();
            let tools = tools.clone();
            let sections = sections.clone();
            let tools_registry = tools_registry.clone();
            let kinds_metadata = kinds_metadata.clone();
            let process_host = options.process_host.clone();
            let plugins = plugins.clone();
            let now = now.clone();
            let registrations = hook_registrations.clone();
            let on_report = on_report.clone();
            let ops: Arc<dyn RuntimeOps> = scheduler_ops.clone();
            Arc::new(move |invoker: Invoker| {
                let kind = invoker.task_kind().cloned();
                let task_id = invoker.task_id();
                let conversation_id = invoker.conversation_id().unwrap_or(0);
                let registrations_for_runner = registrations.clone();
                let ancestors_session = session.clone();
                let hook_runner = HookRunner::new(
                    kind.unwrap_or_else(|| Arc::new(BasicKind::new(""))),
                    task_id,
                    conversation_id,
                    Arc::new(move || {
                        registrations_for_runner
                            .read()
                            .expect("hook registrations")
                            .clone()
                    }),
                    Arc::new(move |conversation_id| {
                        ancestors_session.index().ancestors(conversation_id)
                    }),
                    on_report.clone(),
                );
                Arc::new(Runtime::new(
                    session.clone(),
                    invoker,
                    Arc::new(hook_runner),
                    models.clone(),
                    tools.clone(),
                    sections.clone(),
                    tools_registry.clone(),
                    kinds_metadata.clone(),
                    process_host.clone(),
                    plugins.clone(),
                    now.clone(),
                    ops.clone(),
                ))
            })
        };
        let scheduler = Scheduler::new(SchedulerDeps {
            session: session.clone(),
            kinds: kinds.clone(),
            make_runtime,
            on_report: on_report.clone(),
            ctx: ctx.clone(),
        });
        let _ = scheduler_ops.scheduler.set(scheduler.clone());
        let harness = Arc::new(Harness {
            session: session.clone(),
            scheduler,
            views: views.clone(),
            kinds,
            kinds_metadata,
            tools,
            sections,
            tools_registry,
            entry_kinds: Arc::new(Mutex::new(entry_kinds)),
            plugins,
            hook_registrations,
            conversation_listeners: Mutex::new(Vec::new()),
            on_report,
            now,
            base_ctx: ctx.clone(),
            next_generation: AtomicU64::new(0),
            resumed: AtomicBool::new(false),
            suspended: AtomicBool::new(false),
        });
        // `this.session.lineListeners.add((result) => this.views.update(result))`
        // and `this.session.listeners.add(...)` (`harness.ts:173-177`).
        let weak = Arc::downgrade(&harness);
        harness.session.add_line_listener(Arc::new(move |record| {
            if let Some(harness) = weak.upgrade() {
                harness.views.update(record);
            }
        }));
        let weak = Arc::downgrade(&harness);
        harness.session.add_listener(Arc::new(move |record| {
            if let Some(harness) = weak.upgrade() {
                harness.views.deliver();
                for conversation in &record.changes.conversations {
                    harness.notify_conversation(conversation);
                }
            }
        }));
        harness
            .init(
                ctx.clone(),
                options
                    .root
                    .as_ref()
                    .and_then(|root| root.rewindable.clone()),
                options.root.as_ref().and_then(|root| root.sticky.clone()),
            )
            .await?;
        Ok(harness)
    }

    /// Upstream `private init` (`harness.ts:343-364`).
    async fn init(
        self: &Arc<Self>,
        ctx: Context,
        root_rewindable: Option<JsonObject>,
        root_sticky: Option<JsonObject>,
    ) -> anyhow::Result<()> {
        let session = self.session.clone();
        let conversations = session
            .read(
                |storage, line_ctx| async move { storage.conversations(line_ctx).await }.boxed(),
                ctx.clone(),
            )
            .await?;
        let live = session
            .read(
                |storage, line_ctx| {
                    async move {
                        storage
                            .scan_tasks(
                                &crate::agent_core::harness::pico3::types::TaskScan {
                                    status: Some(vec![TaskStatus::Pending, TaskStatus::Running]),
                                    ..Default::default()
                                },
                                line_ctx,
                            )
                            .await
                    }
                    .boxed()
                },
                ctx.clone(),
            )
            .await?;
        // Owner tasks that are terminal but still own existing conversations:
        // needed for ancestry (`harness.ts:349-355`).
        let mut owner_tasks = Vec::new();
        let owners: Vec<Id> = conversations
            .iter()
            .filter_map(|c: &crate::agent_core::harness::pico3::types::Conversation| c.owner)
            .collect();
        let live_ids: Vec<Id> = live.iter().map(|task| task.id).collect();
        for id in owners {
            if !live_ids.contains(&id) {
                let task = session
                    .read(
                        |storage, line_ctx| async move { storage.task(id, line_ctx).await }.boxed(),
                        ctx.clone(),
                    )
                    .await?;
                if let Some(task) = task {
                    owner_tasks.push(task);
                }
            }
        }
        session.seed_recovered_state(conversations.clone(), live, owner_tasks);
        if conversations.is_empty() {
            let rewindable = root_rewindable;
            let sticky = root_sticky;
            self.session
                .commit(
                    kernel(None),
                    ctx,
                    CommitOptions::default(),
                    move |tx: &mut Tx, _line_ctx: Context| {
                        let rewindable = rewindable.clone();
                        let sticky = sticky.clone();
                        async move {
                            tx.create_conversation(&ConversationSpec {
                                parent: None,
                                rewindable,
                                sticky,
                                sections: None,
                            })?;
                            Ok(())
                        }
                        .boxed()
                    },
                )
                .await?;
        }
        Ok(())
    }

    /// Upstream `notifyConversation` (`harness.ts:310-320`).
    fn notify_conversation(
        self: &Arc<Self>,
        conversation: &crate::agent_core::harness::pico3::types::Conversation,
    ) {
        let listeners = self
            .conversation_listeners
            .lock()
            .expect("listeners")
            .clone();
        if listeners.is_empty() {
            return;
        }
        let handle = self.handle(conversation.id);
        for listener in listeners {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| listener(&handle)))
                .map_err(|payload| {
                    let message = panic_message(payload);
                    (self.on_report)(&anyhow::anyhow!(message));
                });
        }
    }

    /// Upstream `resume` (`harness.ts:366-375`).
    pub fn resume(self: &Arc<Self>) {
        if self.suspended.load(Ordering::SeqCst) {
            panic!("cannot resume a suspended harness; reopen storage with a new harness");
        }
        if self.resumed.swap(true, Ordering::SeqCst) {
            return;
        }
        let harness = self.clone();
        tokio::spawn(async move {
            let outcome = harness.reconcile_orphans().await;
            match outcome {
                Ok(()) => {
                    if !harness.suspended.load(Ordering::SeqCst) {
                        harness.scheduler.resume();
                    }
                }
                Err(error) => (harness.on_report)(&error),
            }
        });
    }

    /// Upstream `reconcileOrphans` (`harness.ts:376-390`).
    async fn reconcile_orphans(self: &Arc<Self>) -> anyhow::Result<()> {
        let orphaned: Vec<Task> = self
            .session
            .live_tasks()
            .into_values()
            .filter(|task| !self.kinds.read().expect("kinds").contains_key(&task.kind))
            .collect();
        if orphaned.is_empty() {
            return Ok(());
        }
        let mut docs: Vec<DocRef> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for task in &orphaned {
            if seen.insert(task.conversation_id) {
                docs.push(DocRef::Sticky {
                    conversation_id: task.conversation_id,
                });
            }
        }
        let orphaned = Arc::new(orphaned);
        self.session
            .commit(
                kernel(None),
                self.base_ctx.clone(),
                CommitOptions {
                    docs,
                    closing: false,
                },
                move |tx: &mut Tx, _line_ctx: Context| {
                    let orphaned = orphaned.clone();
                    async move {
                        for task in orphaned.iter() {
                            let mut terminal = task.clone();
                            terminal.status = TaskStatus::Terminal;
                            terminal.outcome =
                                Some(crate::agent_core::harness::pico3::types::Outcome::orphaned());
                            terminal.checkpoint = None;
                            tx.set_task(terminal)?;
                        }
                        Ok(())
                    }
                    .boxed()
                },
            )
            .await?;
        Ok(())
    }

    /// Upstream `quiescent` (`harness.ts:391-393`).
    pub fn quiescent(&self) -> bool {
        self.scheduler.quiescent()
    }

    /// Upstream `hold` (`harness.ts:394-397`).
    pub fn hold(self: &Arc<Self>) -> anyhow::Result<Box<dyn FnOnce() + Send>> {
        if !self.scheduler.quiescent() {
            anyhow::bail!("cannot hold a non-quiescent harness; suspend it instead");
        }
        Ok(self.scheduler.hold())
    }

    /// Cancel and join in-process invocations, clear transient waits, then
    /// close without terminalizing tasks (`harness.ts:399-423`).
    pub async fn suspend(&self, ctx: Context) -> anyhow::Result<()> {
        if self.suspended.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        self.scheduler.join_all().await;
        let tools: Vec<Task> = self
            .session
            .live_tasks()
            .into_values()
            .filter(|task| task.kind == "pi.tool")
            .collect();
        if !tools.is_empty() {
            let mut docs: Vec<DocRef> = Vec::new();
            let mut seen = std::collections::HashSet::new();
            for task in &tools {
                if seen.insert(task.conversation_id) {
                    docs.push(DocRef::Sticky {
                        conversation_id: task.conversation_id,
                    });
                }
            }
            let tools = Arc::new(tools);
            self.session
                .commit(
                    kernel(None),
                    ctx.clone(),
                    CommitOptions {
                        docs,
                        closing: false,
                    },
                    move |tx: &mut Tx, _line_ctx: Context| {
                        let tools = tools.clone();
                        async move {
                            for task in tools.iter() {
                                // `const slot = tx.sticky(...).turn.tools[index];
                                // if (slot?.waitingOn !== undefined) delete
                                // slot.waitingOn` (`harness.ts:411-415`).
                                let index =
                                    task.input.get("index").and_then(Value::as_i64).unwrap_or(0)
                                        as usize;
                                let sticky = tx.snapshot(DocRef::Sticky {
                                    conversation_id: task.conversation_id,
                                })?;
                                let mut turn = sticky
                                    .get("turn")
                                    .and_then(Value::as_object)
                                    .cloned()
                                    .unwrap_or_default();
                                let mut slot_tools = turn
                                    .get("tools")
                                    .and_then(Value::as_array)
                                    .cloned()
                                    .unwrap_or_default();
                                if let Some(slot) = slot_tools.get_mut(index) {
                                    if let Some(object) = slot.as_object_mut() {
                                        if object.shift_remove("waitingOn").is_some() {
                                            turn.insert(
                                                "tools".to_owned(),
                                                Value::Array(slot_tools.clone()),
                                            );
                                            tx.sticky_set(
                                                task.conversation_id,
                                                "turn",
                                                Value::Object(turn.clone()),
                                            )?;
                                        }
                                    }
                                }
                            }
                            Ok(())
                        }
                        .boxed()
                    },
                )
                .await?;
        }
        self.views.close();
        self.session.close(ctx).await
    }

    /// Signals every invocation, waits for them, closes storage. Writes
    /// nothing (`harness.ts:629-631`).
    pub async fn close(&self, ctx: Context) -> anyhow::Result<()> {
        self.suspend(ctx).await
    }

    // --- registries (`harness.ts:427-548`) -------------------------------

    /// Upstream `registerTaskKind` (`harness.ts:427-439`).
    pub fn register_task_kind(
        self: &Arc<Self>,
        kind: Arc<dyn Kind>,
    ) -> anyhow::Result<Box<dyn Fn() + Send + Sync>> {
        let kind = super::runtime::RegisteredKind::capture(kind);
        let metadata = kind.metadata();
        let name = metadata.name().to_owned();
        if name.starts_with("pi.") {
            anyhow::bail!("task kind \"{name}\": names beginning with \"pi.\" are reserved");
        }
        // Serializes the three registry updates with other registrations and
        // unsubscriptions, not with asynchronous handler execution.
        let mut kinds = self.kinds.write().expect("kinds");
        if kinds.contains_key(&name) {
            anyhow::bail!(r#"task kind "{name}" already registered"#);
        }
        self.session.register_kind(metadata.clone())?;
        self.kinds_metadata
            .write()
            .expect("kinds")
            .insert(name.clone(), metadata);
        kinds.insert(name.clone(), kind.clone());
        drop(kinds);
        self.scheduler.kick();
        let harness = self.clone();
        Ok(Box::new(move || {
            let mut kinds = harness.kinds.write().expect("kinds");
            if !kinds
                .get(&name)
                .is_some_and(|current| Arc::ptr_eq(current, &kind))
            {
                return;
            }
            kinds.remove(&name);
            harness.kinds_metadata.write().expect("kinds").remove(&name);
            harness.session.unregister_kind(&name);
        }))
    }

    /// The registered metadata token of a kind (`harness.ts` kind tokens
    /// are object identities; the port hands out the stored metadata Arc).
    pub fn builtin_kind(&self, name: &str) -> Option<Arc<dyn AnyKind>> {
        self.kinds_metadata
            .read()
            .expect("kinds")
            .get(name)
            .cloned()
    }

    /// Upstream `namespace` (`harness.ts:441-485`).
    pub fn namespace(
        &self,
        id: &str,
        defaults: NamespaceDefaultsValue,
        project: Option<crate::agent_core::harness::pico3::types::ProjectFn>,
    ) -> anyhow::Result<Namespace> {
        let valid = id
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic())
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
        if !valid || id.starts_with("pi.") {
            anyhow::bail!("invalid namespace \"{id}\"");
        }
        if self
            .session
            .namespaces()
            .read()
            .expect("namespaces")
            .contains_key(id)
        {
            anyhow::bail!("namespace \"{id}\" already registered");
        }
        // Route declaration: a key in more than one document rejects
        // (`harness.ts:453-460`).
        let mut routes: Vec<(String, String)> = Vec::new();
        for (doc, source) in [
            ("rewindable", &defaults.rewindable),
            ("sticky", &defaults.sticky),
            ("session", &defaults.session),
        ] {
            for key in source.keys() {
                if routes.iter().any(|(existing, _)| existing == key) {
                    anyhow::bail!(
                        "namespace \"{id}\" key \"{key}\" is declared in more than one document"
                    );
                }
                routes.push((key.clone(), doc.to_owned()));
            }
        }
        let generation = self.next_generation.fetch_add(1, Ordering::SeqCst);
        let token = Namespace {
            id: id.to_owned(),
            generation,
        };
        let registration = NamespaceRegistration {
            generation,
            defaults,
            routes,
            project,
        };
        self.session
            .namespaces()
            .write()
            .expect("namespaces")
            .insert(token.id.clone(), registration);
        Ok(token)
    }

    /// Unregister a namespace token (`harness.ts:463-471`): registration and
    /// its hook registrations go together.
    pub fn unregister_namespace(&self, token: &Namespace) {
        {
            let mut namespaces = self.session.namespaces().write().expect("namespaces");
            let matches = namespaces
                .get(&token.id)
                .map(|registration| registration.generation == token.generation)
                .unwrap_or(false);
            if matches {
                namespaces.remove(&token.id);
            } else {
                return;
            }
        }
        self.hook_registrations
            .write()
            .expect("hook registrations")
            .retain(|registration| {
                !(registration.namespace.id == token.id
                    && registration.namespace.generation == token.generation)
            });
    }

    /// Upstream `registerTool` (`harness.ts:488-499`).
    pub fn register_tool(
        &self,
        tool: Arc<ToolDeclaration>,
    ) -> anyhow::Result<Box<dyn FnOnce() + Send>> {
        {
            let mut registry = self.tools.write().expect("tools registry");
            if registry.contains_key(&tool.name) {
                anyhow::bail!("tool \"{}\" already registered", tool.name);
            }
            registry.insert(tool.name.clone(), tool.clone());
        }
        self.tools_registry.revision.fetch_add(1, Ordering::SeqCst);
        let tools = self.tools.clone();
        let revision = self.tools_registry.revision.clone();
        Ok(Box::new(move || {
            let mut registry = tools.write().expect("tools registry");
            // Unregister removes only this exact declaration; idempotent.
            if Arc::ptr_eq(&registry.remove(&tool.name).unwrap_or(tool.clone()), &tool) {
                drop(registry);
                revision.fetch_add(1, Ordering::SeqCst);
            }
        }))
    }

    /// Upstream `registerSection` (`harness.ts:500-511`).
    pub fn register_section(
        &self,
        section: SystemSection,
    ) -> anyhow::Result<Box<dyn FnOnce() + Send>> {
        {
            let mut registry = self.sections.map.write().expect("section registry");
            if registry.contains_key(&section.key) {
                anyhow::bail!("section \"{}\" already registered", section.key);
            }
            registry.insert(section.key.clone(), section.clone());
        }
        self.sections.revision.fetch_add(1, Ordering::SeqCst);
        let map = self.sections.map.clone();
        let revision = self.sections.revision.clone();
        Ok(Box::new(move || {
            let mut registry = map.write().expect("section registry");
            let removed = registry.remove(&section.key);
            drop(registry);
            if removed.is_some() {
                revision.fetch_add(1, Ordering::SeqCst);
            }
        }))
    }

    /// Upstream `registerEntryKind` (`harness.ts:512-520`).
    pub fn register_entry_kind(&self, kind: EntryKind) -> anyhow::Result<Box<dyn FnOnce() + Send>> {
        if kind.kind.starts_with("pi.") {
            anyhow::bail!(
                "entry kind \"{}\": names beginning with \"pi.\" are reserved",
                kind.kind
            );
        }
        let entry_kinds = self.entry_kinds.clone();
        {
            let mut registry = entry_kinds.lock().expect("entry kinds");
            if registry.contains_key(&kind.kind) {
                anyhow::bail!("entry kind \"{}\" already registered", kind.kind);
            }
            registry.insert(kind.kind.clone(), kind.clone());
        }
        Ok(Box::new(move || {
            // `if (this.entryKinds.get(kind.kind) === kind)
            // this.entryKinds.delete(kind.kind)` (`harness.ts:517-518`).
            let mut registry = entry_kinds.lock().expect("entry kinds");
            if registry
                .get(&kind.kind)
                .map(|existing| existing.kind == kind.kind)
                .unwrap_or(false)
            {
                registry.remove(&kind.kind);
            }
        }))
    }

    /// Upstream `hooks` (`harness.ts:522-529`): register namespace-bound
    /// handlers for one kind's hook points, harness-wide. Both tokens must
    /// be current.
    pub fn hooks(
        &self,
        namespace: &Namespace,
        kind: &Arc<dyn AnyKind>,
        handlers: Arc<dyn std::any::Any + Send + Sync>,
    ) -> anyhow::Result<Box<dyn Fn() + Send + Sync>> {
        self.check_namespace(namespace)?;
        self.check_kind(kind)?;
        Ok(self.add_hooks(HookRegistration {
            namespace: namespace.clone(),
            kind: kind.clone(),
            handlers,
            conversation_id: None,
            subtree: false,
        }))
    }

    fn check_kind(&self, kind: &Arc<dyn AnyKind>) -> anyhow::Result<()> {
        let registered = self
            .kinds_metadata
            .read()
            .expect("kinds")
            .get(kind.name())
            .cloned();
        match registered {
            Some(token) if Arc::ptr_eq(&token, kind) => Ok(()),
            _ => anyhow::bail!("kind \"{}\" is not the registered token", kind.name()),
        }
    }

    fn check_namespace(&self, namespace: &Namespace) -> anyhow::Result<()> {
        let registration = self
            .session
            .namespaces()
            .read()
            .expect("namespaces")
            .get(&namespace.id)
            .cloned();
        match registration {
            Some(registration) if registration.generation == namespace.generation => Ok(()),
            _ => Err(forbidden(format!(
                "namespace \"{}\" is stale",
                namespace.id
            ))),
        }
    }

    /// Upstream `addHooks` (`harness.ts:539-548`).
    fn add_hooks(&self, registration: HookRegistration) -> Box<dyn Fn() + Send + Sync> {
        // Distinct registrations may share the same handler Arc. Capture the
        // registration itself, not a tuple of its user-supplied fields.
        let registration = Arc::new(registration);
        self.hook_registrations
            .write()
            .expect("hook registrations")
            .push(registration.clone());
        let registrations = self.hook_registrations.clone();
        Box::new(move || {
            let mut registry = registrations.write().expect("hook registrations");
            if let Some(index) = registry
                .iter()
                .position(|existing| Arc::ptr_eq(existing, &registration))
            {
                registry.remove(index);
            }
        })
    }

    /// The hook registrations snapshot (the runner factory reads through
    /// this).
    pub fn hook_registrations(&self) -> Arc<RwLock<Vec<Arc<HookRegistration>>>> {
        self.hook_registrations.clone()
    }

    // --- conversations (`harness.ts:552-627`) ----------------------------

    /// Upstream `root` (`harness.ts:552-554`).
    pub async fn root(self: &Arc<Self>, ctx: Context) -> anyhow::Result<ConversationHandle> {
        match self.conversation(1, ctx).await? {
            Some(handle) => Ok(handle),
            None => anyhow::bail!("root conversation missing"),
        }
    }

    /// Upstream `onConversation` (`harness.ts:555-565`).
    pub fn on_conversation(
        self: &Arc<Self>,
        listener: Arc<dyn Fn(&ConversationHandle) + Send + Sync>,
    ) -> Box<dyn FnOnce() + Send> {
        {
            self.conversation_listeners
                .lock()
                .expect("listeners")
                .push(listener.clone());
        }
        for conversation in self.session.conversation_records().values() {
            let handle = self.handle(conversation.id);
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| listener(&handle)))
                .map_err(|payload| {
                    (self.on_report)(&anyhow::anyhow!(panic_message(payload)));
                });
        }
        let listeners = {
            let harness = Arc::downgrade(self);
            // The unregistrer closure captures the listener identity by Arc
            // pointer comparison.
            let listener_for_removal = listener;
            Box::new(move || {
                if let Some(harness) = harness.upgrade() {
                    harness
                        .conversation_listeners
                        .lock()
                        .expect("listeners")
                        .retain(|existing| !Arc::ptr_eq(existing, &listener_for_removal));
                }
            }) as Box<dyn FnOnce() + Send>
        };
        listeners
    }

    /// Upstream `conversation` (`harness.ts:566-569`).
    pub async fn conversation(
        self: &Arc<Self>,
        id: Id,
        ctx: Context,
    ) -> anyhow::Result<Option<ConversationHandle>> {
        let found = self
            .session
            .read(
                |storage, line_ctx| async move { storage.conversation(id, line_ctx).await }.boxed(),
                ctx,
            )
            .await?;
        Ok(found.map(|_| self.handle(id)))
    }

    /// Upstream `createConversation` (`harness.ts:570-587`).
    pub async fn create_conversation(
        self: &Arc<Self>,
        spec: ConversationSpec,
        input: Option<UserInput>,
        ctx: Context,
    ) -> anyhow::Result<ConversationHandle> {
        let id = self
            .session
            .commit(
                kernel(None),
                ctx.clone(),
                CommitOptions::default(),
                move |tx: &mut Tx, _line_ctx: Context| {
                    let spec = spec.clone();
                    let input = input.clone();
                    async move {
                        let id = tx.create_conversation(&spec)?;
                        if let Some(input) = input {
                            tx.send(
                                id,
                                SendInput {
                                    content: input,
                                    ..Default::default()
                                },
                            )
                            .await?;
                        }
                        Ok(id)
                    }
                    .boxed()
                },
            )
            .await?
            .value;
        match self.conversation(id, ctx).await? {
            Some(handle) => Ok(handle),
            None => anyhow::bail!("conversation {id} missing after creation"),
        }
    }

    /// Upstream `entries` (`harness.ts:588-590`).
    pub async fn entries(&self, scan: EntryScan, ctx: Context) -> anyhow::Result<Vec<Entry>> {
        self.session
            .read(
                |storage, line_ctx| {
                    async move { storage.scan_entries(&scan, line_ctx).await }.boxed()
                },
                ctx,
            )
            .await
    }

    /// Upstream `getTask` (`harness.ts:591-593`).
    pub async fn get_task(&self, id: Id, ctx: Context) -> anyhow::Result<Option<Task>> {
        self.session
            .read(
                |storage, line_ctx| async move { storage.task(id, line_ctx).await }.boxed(),
                ctx,
            )
            .await
    }

    /// Upstream `abortInput` (`harness.ts:594-601`).
    pub async fn abort_input(
        self: &Arc<Self>,
        id: Id,
        ctx: Context,
        conversation_id: Option<Id>,
    ) -> anyhow::Result<&'static str> {
        if let Some(conversation_id) = conversation_id {
            let input = self
                .session
                .read(
                    |storage, line_ctx| async move { storage.input(id, line_ctx).await }.boxed(),
                    ctx.clone(),
                )
                .await?;
            if let Some(input) = input {
                if input.conversation_id != conversation_id {
                    return Err(forbidden(format!(
                        "input {id} is outside conversation {conversation_id}"
                    )));
                }
            }
        }
        self.input_handle(id).abort(ctx).await
    }

    /// Upstream `abortTask` (`harness.ts:602-604`).
    pub async fn abort_task(&self, id: Id, ctx: Context) -> anyhow::Result<&'static str> {
        self.scheduler.abort_task(id, ctx).await
    }

    /// Durably mark a task for abort without signalling its invocation
    /// (`harness.ts:606-620`).
    pub async fn mark_task(&self, id: Id, ctx: Context) -> anyhow::Result<&'static str> {
        Ok(self
            .session
            .commit(
                kernel(None),
                ctx,
                CommitOptions::default(),
                move |tx: &mut Tx, _line_ctx: Context| {
                    async move {
                        let task = tx
                            .task(id)
                            .await?
                            .ok_or_else(|| anyhow::anyhow!("task {id} not found"))?;
                        if task.status == TaskStatus::Terminal {
                            return Ok("terminal");
                        }
                        tx.mark_task(id)?;
                        Ok("marked")
                    }
                    .boxed()
                },
            )
            .await?
            .value)
    }

    /// Upstream `waitForIdle` (`harness.ts:621-623`).
    pub async fn wait_for_idle(&self, ctx: Context) -> anyhow::Result<()> {
        self.scheduler.wait_for_idle(None, ctx).await
    }

    /// Upstream `waitForTask` (`harness.ts:624-626`).
    pub async fn wait_for_task(&self, id: Id, ctx: Context) -> anyhow::Result<Task> {
        self.scheduler.wait_for_task(id, ctx).await
    }

    /// Upstream `private watch` (`harness.ts:747-764`): capture and
    /// subscribe in one line operation.
    pub async fn watch_conversation(
        self: &Arc<Self>,
        conversation_id: Id,
        ctx: Context,
    ) -> anyhow::Result<Arc<Watch>> {
        let harness = self.clone();
        let result = self
            .session
            .commit(
                Invoker::Host {
                    conversation_id: Some(conversation_id),
                },
                ctx,
                CommitOptions {
                    docs: vec![
                        DocRef::Rewindable { conversation_id },
                        DocRef::Sticky { conversation_id },
                    ],
                    closing: false,
                },
                move |tx: &mut Tx, _line_ctx: Context| {
                    let harness = harness.clone();
                    async move {
                        let entries = capture_active_transcript_tx(tx, conversation_id).await?;
                        let conversation =
                            tx.conversation(conversation_id).await?.ok_or_else(|| {
                                anyhow::anyhow!("conversation {conversation_id} missing")
                            })?;
                        let watch = harness.views.watch_in_tx(&conversation, entries, tx)?;
                        Ok(watch)
                    }
                    .boxed()
                },
            )
            .await?;
        Ok(result.value)
    }

    /// Upstream `private handle` (`harness.ts:633-744`).
    pub fn handle(self: &Arc<Self>, id: Id) -> ConversationHandle {
        ConversationHandle {
            harness: self.clone(),
            id,
        }
    }

    /// Upstream `private inputHandle` (`harness.ts:766-774`).
    pub fn input_handle(self: &Arc<Self>, id: Id) -> InputHandle {
        InputHandle {
            harness: self.clone(),
            id,
        }
    }

    /// The session (view and storage-level reads).
    pub fn session(&self) -> &Arc<Session> {
        &self.session
    }

    /// The scheduler (runtime ops for tests).
    pub fn scheduler(&self) -> &Arc<Scheduler> {
        &self.scheduler
    }

    /// The view manager.
    pub fn views(&self) -> &Arc<ViewManager> {
        &self.views
    }
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "listener panicked".to_owned()
    }
}

/// The kernel invoker (`harness.ts:112`).
pub fn kernel(conversation_id: Option<Id>) -> Invoker {
    Invoker::Kernel { conversation_id }
}

// ---------------------------------------------------------------------------
// The scheduler-backed runtime operations (`harness.ts:227-258`)
// ---------------------------------------------------------------------------

struct SchedulerOps {
    scheduler: std::sync::OnceLock<Arc<Scheduler>>,
    session: Arc<Session>,
}

impl SchedulerOps {
    fn scheduler(&self) -> Arc<Scheduler> {
        self.scheduler.get().cloned().expect("scheduler installed")
    }
}

impl RuntimeOps for SchedulerOps {
    fn sleep(&self, until_ms: i64, ctx: Context) -> BoxFuture<'static, anyhow::Result<()>> {
        async move {
            // `const ms = Math.max(0, untilMs - Date.now())`
            // (`harness.ts:229`).
            let ms = (until_ms - crate::ai::now_ms()).max(0) as u64;
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_millis(ms)) => Ok(()),
                _ = async {
                    if let Some(signal) = ctx.abort_signal() {
                        signal.cancelled().await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => Err(anyhow::anyhow!("aborted")),
            }
        }
        .boxed()
    }

    fn wait_for_input(&self, id: Id, ctx: Context) -> BoxFuture<'static, anyhow::Result<Input>> {
        let scheduler = self.scheduler();
        async move { scheduler.wait_for_input(id, ctx).await }.boxed()
    }

    fn wait_for_task(&self, id: Id, ctx: Context) -> BoxFuture<'static, anyhow::Result<Task>> {
        let scheduler = self.scheduler();
        async move { scheduler.wait_for_task(id, ctx).await }.boxed()
    }

    fn abort_task(&self, id: Id, ctx: Context) -> BoxFuture<'static, anyhow::Result<&'static str>> {
        let scheduler = self.scheduler();
        async move { scheduler.abort_task(id, ctx).await }.boxed()
    }

    fn abort_conversation(&self, id: Id, ctx: Context) -> BoxFuture<'static, anyhow::Result<()>> {
        // `abortConversation` (`harness.ts:251-258`): assertions, then the
        // conversation abort sequence.
        let session = self.session.clone();
        let scheduler = self.scheduler();
        async move { abort_conversation_impl(&session, &scheduler, id, ctx).await }.boxed()
    }
}

/// The conversation abort sequence (`harness.ts:712-732`).
pub async fn abort_conversation_impl(
    session: &Arc<Session>,
    scheduler: &Arc<Scheduler>,
    conversation_id: Id,
    ctx: Context,
) -> anyhow::Result<()> {
    let marked: Vec<Id> = session
        .commit(
            kernel(Some(conversation_id)),
            ctx.clone(),
            CommitOptions {
                docs: vec![
                    DocRef::Rewindable { conversation_id },
                    DocRef::Sticky { conversation_id },
                ],
                closing: false,
            },
            move |tx: &mut Tx, _line_ctx: Context| {
                async move {
                    let sticky = tx.snapshot(DocRef::Sticky { conversation_id })?;
                    // Withdraw queued steer/followUp inputs
                    // (`harness.ts:714-719`).
                    let inbox = sticky
                        .get("inbox")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    let withdrawn: Vec<Id> = inbox
                        .iter()
                        .filter(|queued| {
                            queued.get("mode").and_then(Value::as_str) != Some("write")
                        })
                        .filter_map(|queued| queued.get("id").and_then(Value::as_i64))
                        .collect();
                    if !withdrawn.is_empty() {
                        tx.resolve_inputs(
                            &withdrawn,
                            &Resolution::Unanswered {
                                reason: "aborted".to_owned(),
                                detail: None,
                            },
                        )
                        .await?;
                        for input in &withdrawn {
                            tx.emit_event(ViewEvent::InputAborted { input: *input })?;
                        }
                        // Remove them from the inbox (`harness.ts:717`).
                        let mut updated = sticky.clone();
                        if let Some(array) = updated.get_mut("inbox").and_then(Value::as_array_mut)
                        {
                            array.retain(|queued| {
                                let id = queued.get("id").and_then(Value::as_i64);
                                !id.is_some_and(|id| withdrawn.contains(&id))
                            });
                        }
                        tx.sticky_set(
                            conversation_id,
                            "inbox",
                            updated
                                .get("inbox")
                                .cloned()
                                .unwrap_or(Value::Array(Vec::new())),
                        )?;
                    }
                    // Mark every live non-background task of the
                    // conversation (`harness.ts:721-727`).
                    let mut ids: Vec<Id> = Vec::new();
                    let live = tx
                        .tasks(&crate::agent_core::harness::pico3::types::TaskScan {
                            conversation_id: Some(conversation_id),
                            status: Some(vec![TaskStatus::Pending, TaskStatus::Running]),
                            kind: None,
                        })
                        .await?;
                    for task in live {
                        if task.background == Some(true) {
                            continue;
                        }
                        if task.abort != Some(true) {
                            tx.mark_task(task.id)?;
                        }
                        ids.push(task.id);
                    }
                    Ok(ids)
                }
                .boxed()
            },
        )
        .await?
        .value;
    for id in marked {
        scheduler.abort_task(id, ctx.clone()).await?;
    }
    scheduler
        .wait_for_idle(Some(conversation_id), ctx.clone())
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// ConversationHandle / InputHandle (`harness.ts:83-110`, 633-744)
// ---------------------------------------------------------------------------

/// Upstream `ConversationHandle` (`harness.ts:83-104`).
#[derive(Clone)]
pub struct ConversationHandle {
    pub harness: Arc<Harness>,
    /// Upstream `id`.
    pub id: Id,
}

impl ConversationHandle {
    /// The host commit with the conversation docs preloaded
    /// (`harness.ts:641-642`).
    pub async fn commit<T, F>(&self, f: F, ctx: Context) -> anyhow::Result<T>
    where
        T: Send + 'static,
        F: for<'tx> FnOnce(&'tx mut Tx, Context) -> BoxFuture<'tx, anyhow::Result<T>>
            + Send
            + 'static,
    {
        let result = self
            .harness
            .session
            .commit(
                Invoker::Host {
                    conversation_id: Some(self.id),
                },
                ctx,
                CommitOptions {
                    docs: vec![
                        DocRef::Rewindable {
                            conversation_id: self.id,
                        },
                        DocRef::Sticky {
                            conversation_id: self.id,
                        },
                    ],
                    closing: false,
                },
                f,
            )
            .await?;
        Ok(result.value)
    }

    /// The kernel commit with the conversation docs preloaded
    /// (`harness.ts:643-644`).
    pub async fn commit_kernel<T, F>(&self, f: F, ctx: Context) -> anyhow::Result<T>
    where
        T: Send + 'static,
        F: for<'tx> FnOnce(&'tx mut Tx, Context) -> BoxFuture<'tx, anyhow::Result<T>>
            + Send
            + 'static,
    {
        let result = self
            .harness
            .session
            .commit(
                Invoker::Kernel {
                    conversation_id: Some(self.id),
                },
                ctx,
                CommitOptions {
                    docs: vec![
                        DocRef::Rewindable {
                            conversation_id: self.id,
                        },
                        DocRef::Sticky {
                            conversation_id: self.id,
                        },
                    ],
                    closing: false,
                },
                f,
            )
            .await?;
        Ok(result.value)
    }

    /// Upstream `send` (`harness.ts:670-673`).
    pub async fn send(&self, input: SendInput, ctx: Context) -> anyhow::Result<InputHandle> {
        let conversation_id = self.id;
        let input_id = self
            .commit_kernel(
                move |tx: &mut Tx, _line_ctx: Context| {
                    let input = input.clone();
                    async move { tx.send(conversation_id, input).await }.boxed()
                },
                ctx,
            )
            .await?;
        Ok(self.harness.input_handle(input_id))
    }

    /// Upstream `write` (`harness.ts:674`).
    pub async fn write(&self, entry: NewEntry, ctx: Context) -> anyhow::Result<Id> {
        let conversation_id = self.id;
        self.commit(
            move |tx: &mut Tx, _line_ctx: Context| {
                let entry = entry.clone();
                async move { tx.write(conversation_id, entry).await }.boxed()
            },
            ctx,
        )
        .await
    }

    /// Upstream `rewindable` (`harness.ts:676`).
    pub async fn rewindable(&self, ctx: Context) -> anyhow::Result<JsonObject> {
        let conversation_id = self.id;
        self.commit(
            move |tx: &mut Tx, _line_ctx: Context| {
                async move { tx.snapshot(DocRef::Rewindable { conversation_id }) }.boxed()
            },
            ctx,
        )
        .await
    }

    /// Upstream `sticky` (`harness.ts:677`).
    pub async fn sticky(&self, ctx: Context) -> anyhow::Result<JsonObject> {
        let conversation_id = self.id;
        self.commit(
            move |tx: &mut Tx, _line_ctx: Context| {
                async move { tx.snapshot(DocRef::Sticky { conversation_id }) }.boxed()
            },
            ctx,
        )
        .await
    }

    /// Upstream `context` (`harness.ts:678`).
    pub async fn context(&self, ctx: Context) -> anyhow::Result<ContextView> {
        let conversation_id = self.id;
        self.commit(
            move |tx: &mut Tx, _line_ctx: Context| {
                async move { tx.context(conversation_id, None).await }.boxed()
            },
            ctx,
        )
        .await
    }

    /// Upstream `fork` (`harness.ts:679-682`).
    pub async fn fork(
        &self,
        at: ParentAt,
        spec: ConversationSpec,
        ctx: Context,
    ) -> anyhow::Result<ConversationHandle> {
        let id = self
            .harness
            .session
            .fork(self.id, at, spec, ctx.clone())
            .await?;
        match self.harness.conversation(id, ctx).await? {
            Some(handle) => Ok(handle),
            None => anyhow::bail!("forked conversation {id} missing"),
        }
    }

    /// Upstream `collapse` (`harness.ts:683-695`).
    pub async fn collapse(&self, instructions: Option<String>, ctx: Context) -> anyhow::Result<Id> {
        let conversation_id = self.id;
        self.commit_kernel(
            move |tx: &mut Tx, _line_ctx: Context| {
                let instructions = instructions.clone();
                async move {
                    let context = tx.context(conversation_id, None).await?;
                    let state = tx.snapshot(DocRef::Rewindable { conversation_id })?;
                    let keep_recent = state
                        .get("keepRecent")
                        .and_then(Value::as_f64)
                        .unwrap_or(0.0);
                    let through =
                        super::kinds::collapse::choose_through(&context.entries, keep_recent);
                    let Some(through) = through else {
                        anyhow::bail!("nothing to collapse");
                    };
                    let mut input = json!({ "reason": "manual", "through": through });
                    if let Some(instructions) = instructions {
                        input["instructions"] = Value::String(instructions);
                    }
                    let reference = tx.create_task_kind(
                        &tx.kind_registry()
                            .read()
                            .expect("kinds")
                            .get("pi.collapse")
                            .cloned()
                            .expect("registered"),
                        input,
                        CreateTaskOptions {
                            conversation_id: Some(conversation_id),
                            background: true,
                            after: Vec::new(),
                        },
                    )?;
                    Ok(reference.id)
                }
                .boxed()
            },
            ctx,
        )
        .await
    }

    /// Upstream `reset` (`harness.ts:696-711`).
    pub async fn reset(&self, handoff: Option<String>, ctx: Context) -> anyhow::Result<()> {
        let conversation_id = self.id;
        let now = self.harness.now.clone();
        self.commit_kernel(
            move |tx: &mut Tx, _line_ctx: Context| {
                let handoff = handoff.clone();
                let now = now.clone();
                async move {
                    let entry = match handoff {
                        None => NewEntry {
                            kind: "pi.reset".to_owned(),
                            head: Some(Head::Self_),
                            ..Default::default()
                        },
                        Some(handoff) => NewEntry {
                            kind: "pi.handoff".to_owned(),
                            head: Some(Head::Self_),
                            model: Some(vec![json!({
                                "role": "user",
                                "content": handoff,
                                "timestamp": now(),
                            })]),
                            ..Default::default()
                        },
                    };
                    tx.write(conversation_id, entry).await?;
                    Ok(())
                }
                .boxed()
            },
            ctx,
        )
        .await
    }

    /// Upstream `abort` (`harness.ts:712-732`).
    pub async fn abort(&self, ctx: Context) -> anyhow::Result<()> {
        let harness = self.harness.clone();
        let scheduler = harness.scheduler.clone();
        let session = harness.session.clone();
        abort_conversation_impl(&session, &scheduler, self.id, ctx).await
    }

    /// Upstream `waitForIdle` (`harness.ts:733`).
    pub async fn wait_for_idle(&self, ctx: Context) -> anyhow::Result<()> {
        self.harness
            .scheduler
            .wait_for_idle(Some(self.id), ctx)
            .await
    }

    /// Upstream `hooks` (`harness.ts:734-741`).
    pub fn hooks(
        &self,
        namespace: &Namespace,
        kind: &Arc<dyn AnyKind>,
        handlers: Arc<dyn std::any::Any + Send + Sync>,
        subtree: bool,
    ) -> anyhow::Result<Box<dyn Fn() + Send + Sync>> {
        self.harness.check_namespace(namespace)?;
        self.harness.check_kind(kind)?;
        Ok(self.harness.add_hooks(HookRegistration {
            namespace: namespace.clone(),
            kind: kind.clone(),
            handlers,
            conversation_id: Some(self.id),
            subtree,
        }))
    }

    /// Upstream `watch` (`harness.ts:742`).
    pub async fn watch(&self, ctx: Context) -> anyhow::Result<Arc<Watch>> {
        self.harness.watch_conversation(self.id, ctx).await
    }

    /// Upstream `config.get` (`harness.ts:645-652`): every declared key's
    /// effective value, keyed by name.
    pub async fn config_get(&self, ctx: Context) -> anyhow::Result<JsonObject> {
        let defaults = self.harness.session.defaults();
        let conversation_id = self.id;
        self.commit(
            move |tx: &mut Tx, _line_ctx: Context| {
                let route_keys: Vec<String> = defaults.route.keys().cloned().collect();
                async move {
                    let mut out = JsonObject::new();
                    for key in route_keys {
                        let value = tx.config_get(conversation_id, &key)?;
                        out.insert(key, value);
                    }
                    Ok(out)
                }
                .boxed()
            },
            ctx,
        )
        .await
    }

    /// Upstream `config.set` (`harness.ts:653-660`).
    pub async fn config_set(&self, patch: JsonObject, ctx: Context) -> anyhow::Result<()> {
        let conversation_id = self.id;
        self.commit(
            move |tx: &mut Tx, _line_ctx: Context| {
                let patch = patch.clone();
                async move {
                    for (key, value) in &patch {
                        tx.config_set(conversation_id, key, value.clone())?;
                    }
                    Ok(())
                }
                .boxed()
            },
            ctx,
        )
        .await
    }

    /// Upstream `config.reset` (`harness.ts:661-665`).
    pub async fn config_reset(&self, keys: Vec<String>, ctx: Context) -> anyhow::Result<()> {
        let conversation_id = self.id;
        self.commit(
            move |tx: &mut Tx, _line_ctx: Context| {
                let keys = keys.clone();
                async move {
                    for key in &keys {
                        tx.config_reset(conversation_id, key)?;
                    }
                    Ok(())
                }
                .boxed()
            },
            ctx,
        )
        .await
    }
}

/// Upstream `InputHandle` (`harness.ts:105-110`).
#[derive(Clone)]
pub struct InputHandle {
    pub harness: Arc<Harness>,
    /// Upstream `id`.
    pub id: Id,
}

impl InputHandle {
    /// Upstream `result` (`harness.ts:769`).
    pub async fn result(&self, ctx: Context) -> anyhow::Result<Option<Input>> {
        let id = self.id;
        self.harness
            .session
            .read(
                |storage, line_ctx| async move { storage.input(id, line_ctx).await }.boxed(),
                ctx,
            )
            .await
    }

    /// Upstream `wait` (`harness.ts:770`).
    pub async fn wait(&self, ctx: Context) -> anyhow::Result<Input> {
        self.harness.scheduler.wait_for_input(self.id, ctx).await
    }

    /// Upstream `abort` (`harness.ts:771-772`).
    pub async fn abort(&self, ctx: Context) -> anyhow::Result<&'static str> {
        let id = self.id;
        Ok(self
            .harness
            .session
            .commit(
                kernel(None),
                ctx,
                CommitOptions {
                    docs: Vec::new(),
                    closing: false,
                },
                move |tx: &mut Tx, _line_ctx: Context| {
                    async move { tx.withdraw_input(id).await }.boxed()
                },
            )
            .await?
            .value)
    }
}

// ---------------------------------------------------------------------------
// §9 active-transcript capture (`harness.ts:777-803`)
// ---------------------------------------------------------------------------

/// Upstream `captureActiveTranscript` over a transaction scan.
pub async fn capture_active_transcript_tx(
    tx: &mut Tx,
    conversation_id: Id,
) -> anyhow::Result<Vec<Entry>> {
    let head = tx
        .scan_entries(&EntryScan {
            conversation_id,
            with_head: true,
            limit: 1,
            ..EntryScan::default()
        })
        .await?;
    let from = head.first().and_then(|entry| entry.head);
    let mut out: Vec<Entry> = Vec::new();
    let mut before: Option<Id> = None;
    loop {
        let page = tx
            .scan_entries(&EntryScan {
                conversation_id,
                before,
                limit: 256,
                ..EntryScan::default()
            })
            .await?;
        let mut done = page.len() < 256;
        for entry in page {
            if let Some(from) = from {
                if entry.id < from {
                    done = true;
                    break;
                }
            }
            out.push(entry);
        }
        if done {
            break;
        }
        before = out.last().map(|entry| entry.id);
    }
    out.reverse();
    Ok(out)
}

/// Upstream `captureActiveTranscript` (`harness.ts:781-803`) over a storage
/// scan closure.
pub async fn capture_active_transcript(
    scan: impl Fn(EntryScan) -> BoxFuture<'static, anyhow::Result<Vec<Entry>>>,
    conversation_id: Id,
) -> anyhow::Result<Vec<Entry>> {
    let head = scan(EntryScan {
        conversation_id,
        with_head: true,
        limit: 1,
        ..EntryScan::default()
    })
    .await?;
    let from = head.first().and_then(|entry| entry.head);
    let mut out: Vec<Entry> = Vec::new();
    let mut before: Option<Id> = None;
    loop {
        let page = scan(EntryScan {
            conversation_id,
            before,
            limit: 256,
            ..EntryScan::default()
        })
        .await?;
        let mut done = page.len() < 256;
        let last_id = page.last().map(|entry| entry.id);
        for entry in page {
            if let Some(from) = from {
                if entry.id < from {
                    done = true;
                    break;
                }
            }
            out.push(entry);
        }
        if done {
            break;
        }
        before = last_id;
    }
    out.reverse();
    Ok(out)
}

pub use crate::agent_core::harness::context::with_abort_signal;
/// Upstream `isCoreKind`/`withAbortSignal` re-exports (`harness.ts:805`).
pub use crate::agent_core::harness::pico3::types::is_core_kind;

/// Upstream `WATCH_CAPACITY`/`applyEnvelope` re-exports (`harness.ts:808`).
pub use crate::agent_core::harness::pico3::view::{apply_envelope, WATCH_CAPACITY};

/// Upstream `kinds` witness table accessor (`harness.ts:810`).
pub fn builtin_kinds() -> kinds::BuiltinKinds {
    kinds::builtins()
}
