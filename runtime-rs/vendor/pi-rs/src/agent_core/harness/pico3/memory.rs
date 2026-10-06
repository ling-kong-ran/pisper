//! Port of `packages/agent/src/harness/pico3/memory.ts` (278 lines): the
//! in-memory pico3 [`Storage`] backend — the reference backend, and the read
//! path of [`jsonl::JsonlStorage`].
//!
//! A batch is validated and staged in full before any table changes: one
//! failed batch changes no table, no document, no id high-water, no sequence
//! (`memory.ts:20-24`). Committed ids are never reused; ids minted but never
//! committed may be reused after reopen.
//!
//! Disclosed substitution: upstream `clone` is `JSON.parse(JSON.stringify)`
//! (`memory.ts:272-274`); the port's records are serde values and structs, so
//! the same isolation is `serde_json` round-trips / `Clone` on owned JSON
//! trees.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use futures::future::BoxFuture;
use serde_json::{Map, Value};

use crate::agent_core::chord_support::delta::{apply_immutable, is_base, Op};
use crate::agent_core::chord_support::Context;

use super::types::{
    Conversation, DocRef, Entry, EntryScan, Id, Input, JsonObject, Seq, Storage, Task, TaskPatch,
    TaskScan, Write,
};

/// Upstream `MemoryStorage` (`memory.ts:25-270`).
pub struct MemoryStorage {
    /// Upstream `protected` table fields (`memory.ts:26-38`); the session
    /// line serializes access, and the delta appliers need `&mut`, so the
    /// tables live behind one lock.
    inner: Mutex<Tables>,
    closed: AtomicBool,
}

#[derive(Default)]
pub(crate) struct Tables {
    conversations_by_id: HashMap<Id, Conversation>,
    entries_by_id: HashMap<Id, Entry>,
    /// Ascending per conversation (`memory.ts:28`).
    entries_by_conversation: HashMap<Id, Vec<Entry>>,
    tasks_by_id: HashMap<Id, Task>,
    /// JS Map iteration is insertion ordered, even for non-monotonic ids.
    task_order: Vec<Id>,
    inputs_by_id: HashMap<Id, Input>,
    inputs_by_request: HashMap<String, Input>,
    /// Rewindable docs keep their full op history with the seq of each
    /// batch, for `doc_as_of` (`memory.ts:32-33`).
    rewindable_log: HashMap<Id, Vec<(Seq, Vec<Op>)>>,
    sticky_log: HashMap<Id, Vec<Vec<Op>>>,
    session_doc: Option<JsonObject>,
    /// Entry id -> seq of its batch (`memory.ts:36`).
    entry_seq: HashMap<Id, Seq>,
    next_id_value: Id,
    seq: Seq,
}

impl std::fmt::Debug for MemoryStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryStorage").finish_non_exhaustive()
    }
}

impl Default for MemoryStorage {
    fn default() -> Self {
        MemoryStorage::new()
    }
}

impl MemoryStorage {
    /// Upstream `new MemoryStorage()` (`memory.ts:66`): `nextIdValue = 1`,
    /// `seq = 0`.
    pub fn new() -> MemoryStorage {
        MemoryStorage {
            inner: Mutex::new(Tables {
                next_id_value: 1,
                seq: 0,
                ..Tables::default()
            }),
            closed: AtomicBool::new(false),
        }
    }

    /// Upstream `mintId` (`memory.ts:41-43`).
    pub fn mint_id(&self) -> Id {
        let mut tables = self.inner.lock().expect("storage tables");
        let id = tables.next_id_value;
        tables.next_id_value += 1;
        id
    }

    /// Upstream `setNextId` (`memory.ts:44-46`).
    pub(crate) fn set_next_id(&self, n: Id) {
        let mut tables = self.inner.lock().expect("storage tables");
        tables.next_id_value = tables.next_id_value.max(n);
    }

    /// The commit sequence (`memory.ts:38`); crate-visible for JsonlStorage.
    pub(crate) fn current_seq(&self) -> Seq {
        self.inner.lock().expect("storage tables").seq
    }

    /// Force the sequence (replay seeding, `jsonl.ts:181-185`: upstream sets
    /// `this.seq = seq - 1` before delegating each replayed batch).
    pub(crate) fn set_seq(&self, seq: Seq) {
        self.inner.lock().expect("storage tables").seq = seq;
    }

    /// The next id without minting (`jsonl.ts:232-248` reads
    /// `nextIdValue - 1`).
    pub(crate) fn peek_next_id(&self) -> Id {
        self.inner.lock().expect("storage tables").next_id_value
    }

    /// Synchronous single-task read (the JsonlStorage replay path,
    /// `jsonl.ts:190`).
    pub(crate) fn task_sync(&self, id: Id) -> anyhow::Result<Option<Task>> {
        Ok(self
            .inner
            .lock()
            .expect("storage tables")
            .tasks_by_id
            .get(&id)
            .cloned())
    }

    fn assert_open(&self) -> anyhow::Result<()> {
        if self.closed.load(Ordering::SeqCst) {
            anyhow::bail!("storage closed");
        }
        Ok(())
    }

    /// Upstream `commit` (`memory.ts:48-55`): validate the whole batch, then
    /// apply, then advance the sequence.
    pub fn commit_sync(&self, writes: Vec<Write>, _context: Context) -> anyhow::Result<Seq> {
        self.assert_open()?;
        let mut tables = self.inner.lock().expect("storage tables");
        let seq = tables.seq + 1;
        let staged = stage(&mut tables, writes, seq)?; // throws before any mutation
        staged.apply(&mut tables);
        tables.seq = seq;
        Ok(seq)
    }

    /// Upstream `stage` (`memory.ts:57-160`): validate the whole batch
    /// against the current tables; return the staged batch for the caller to
    /// apply.
    pub(crate) fn stage(&self, writes: Vec<Write>, seq: Seq) -> anyhow::Result<StagedBatch> {
        let mut tables = self.inner.lock().expect("storage tables");
        stage(&mut tables, writes, seq)
    }

    /// Apply already-staged writes (crate-visible for JsonlStorage's split
    /// apply, `jsonl.ts:226-227`).
    pub(crate) fn apply_staged(&self, staged: StagedBatch) {
        let mut tables = self.inner.lock().expect("storage tables");
        staged.apply(&mut tables);
    }

    /// Upstream `fold` (`memory.ts:244-254`): fold a doc log from its last
    /// base batch.
    pub(crate) fn fold(log: &[Vec<Op>]) -> JsonObject {
        let mut start = 0;
        for (index, ops) in log.iter().enumerate().rev() {
            if is_base(ops) {
                start = index;
                break;
            }
        }
        let mut state = Value::Object(Map::new());
        for ops in &log[start..] {
            state = apply_immutable(Some(&state), ops).expect("folded ops apply");
        }
        match state {
            Value::Object(object) => object,
            _ => unreachable!("fold always applies object documents"),
        }
    }
}

/// One staged write of a validated batch (`memory.ts:68-155`).
enum Pending {
    Conversation { conversation: Conversation },
    Entry { entry: Entry },
    Task { task: Task },
    TaskPatch { patch: TaskPatch },
    Input { input: Input },
    Doc { r#ref: DocRef, ops: Vec<Op> },
}

/// A fully validated batch (`memory.ts:156-159`): applying it cannot fail.
pub(crate) struct StagedBatch {
    pendings: Vec<Pending>,
    max_id: Id,
}

impl StagedBatch {
    /// The `staged()` closure (`memory.ts:156-159`).
    pub(super) fn apply(self, tables: &mut Tables) {
        for pending in self.pendings {
            match pending {
                Pending::Conversation { conversation } => {
                    tables
                        .entries_by_conversation
                        .insert(conversation.id, Vec::new());
                    tables
                        .conversations_by_id
                        .insert(conversation.id, conversation);
                }
                Pending::Entry { entry } => {
                    tables
                        .entries_by_conversation
                        .entry(entry.conversation_id)
                        .or_default()
                        .push(entry.clone());
                    tables.entry_seq.insert(entry.id, tables.seq + 1);
                    tables.entries_by_id.insert(entry.id, entry);
                }
                Pending::Task { task } => {
                    tables.task_order.push(task.id);
                    tables.tasks_by_id.insert(task.id, task);
                }
                Pending::TaskPatch { patch } => {
                    let id = patch.id;
                    if let Some(task) = tables.tasks_by_id.get_mut(&id) {
                        patch.apply_to(task);
                    }
                }
                Pending::Input { input } => {
                    if let Some(request_id) = &input.request_id {
                        tables.inputs_by_request.insert(
                            format!("{}:{}", input.conversation_id, request_id),
                            input.clone(),
                        );
                    }
                    tables.inputs_by_id.insert(input.id, input);
                }
                Pending::Doc {
                    r#ref,
                    ops: doc_ops,
                } => match r#ref {
                    DocRef::Session => {
                        let base = tables.session_doc.clone().unwrap_or_else(|| {
                            JsonObject::from_iter([(
                                "plugins".to_owned(),
                                Value::Object(Map::new()),
                            )])
                        });
                        let applied = apply_immutable(Some(&Value::Object(base)), &doc_ops)
                            .expect("session doc ops apply");
                        tables.session_doc = match applied {
                            Value::Object(object) => Some(object),
                            _ => unreachable!("session doc stays an object"),
                        };
                    }
                    DocRef::Rewindable { conversation_id } => {
                        tables
                            .rewindable_log
                            .entry(conversation_id)
                            .or_default()
                            .push((tables.seq + 1, doc_ops));
                    }
                    DocRef::Sticky { conversation_id } => {
                        tables
                            .sticky_log
                            .entry(conversation_id)
                            .or_default()
                            .push(doc_ops);
                    }
                },
            }
        }
        tables.next_id_value = tables.next_id_value.max(self.max_id + 1);
    }
}

/// The staged-apply half of upstream `MemoryStorage#stage`
/// (`memory.ts:57-160`). All validation runs before any record is staged, so
/// a failed batch changes nothing.
fn stage(tables: &mut Tables, writes: Vec<Write>, _seq: Seq) -> anyhow::Result<StagedBatch> {
    let mut pendings: Vec<Pending> = Vec::new();
    let mut created: std::collections::HashSet<Id> = std::collections::HashSet::new();
    let mut max_id: Id = 0;
    // Upstream finds a patch's previous record anywhere in the same batch,
    // regardless of order (`memory.ts:104-107`).
    let batch_task_ids: std::collections::HashSet<Id> = writes
        .iter()
        .filter_map(|write| match write {
            Write::Task { task } => Some(task.id),
            _ => None,
        })
        .collect();
    let mut claim = |id: Id, what: &str| -> anyhow::Result<()> {
        if id <= 0 {
            anyhow::bail!("{what}: invalid id {id}");
        }
        if created.contains(&id) {
            anyhow::bail!("{what}: id {id} created twice in one batch");
        }
        created.insert(id);
        max_id = max_id.max(id);
        Ok(())
    };
    for write in writes {
        match write {
            Write::Conversation { conversation } => {
                if tables.conversations_by_id.contains_key(&conversation.id) {
                    anyhow::bail!("conversation {} exists", conversation.id);
                }
                claim(conversation.id, "conversation")?;
                pendings.push(Pending::Conversation { conversation });
            }
            Write::Entry { entry } => {
                if tables.entries_by_id.contains_key(&entry.id) {
                    anyhow::bail!("entry {} exists", entry.id);
                }
                claim(entry.id, "entry")?;
                pendings.push(Pending::Entry { entry });
            }
            Write::Task { task } => {
                if tables.tasks_by_id.contains_key(&task.id) {
                    anyhow::bail!("task {} exists", task.id);
                }
                claim(task.id, "task")?;
                pendings.push(Pending::Task { task });
            }
            Write::TaskPatch { patch } => {
                // The previous record may be in the tables or created
                // earlier in this same batch (`memory.ts:103-107`).
                let known = tables.tasks_by_id.contains_key(&patch.id)
                    || batch_task_ids.contains(&patch.id);
                if !known {
                    anyhow::bail!("patch for unknown task {}", patch.id);
                }
                pendings.push(Pending::TaskPatch { patch });
            }
            Write::Input { input } => {
                if !tables.inputs_by_id.contains_key(&input.id) {
                    claim(input.id, "input")?;
                }
                pendings.push(Pending::Input { input });
            }
            Write::Doc {
                r#ref,
                ops: doc_ops,
            } => {
                pendings.push(Pending::Doc {
                    r#ref,
                    ops: doc_ops,
                });
            }
        }
    }
    Ok(StagedBatch { pendings, max_id })
}

impl Storage for MemoryStorage {
    fn commit<'a>(
        &'a self,
        writes: Vec<Write>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Seq>> {
        Box::pin(async move { self.commit_sync(writes, context) })
    }

    fn mint_id(&self) -> Id {
        MemoryStorage::mint_id(self)
    }

    fn conversation<'a>(
        &'a self,
        id: Id,
        _context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<Conversation>>> {
        Box::pin(async move {
            Ok(self
                .inner
                .lock()
                .expect("storage tables")
                .conversations_by_id
                .get(&id)
                .cloned())
        })
    }

    fn conversations<'a>(
        &'a self,
        _context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<Conversation>>> {
        Box::pin(async move {
            Ok(self
                .inner
                .lock()
                .expect("storage tables")
                .conversations_by_id
                .values()
                .cloned()
                .collect())
        })
    }

    fn entries<'a>(
        &'a self,
        ids: &[Id],
        _context: Context,
    ) -> BoxFuture<'a, anyhow::Result<HashMap<Id, Entry>>> {
        let ids = ids.to_vec();
        Box::pin(async move {
            let tables = self.inner.lock().expect("storage tables");
            let mut out = HashMap::new();
            for id in &ids {
                if let Some(entry) = tables.entries_by_id.get(id) {
                    out.insert(*id, entry.clone());
                }
            }
            Ok(out)
        })
    }

    fn scan_entries<'a>(
        &'a self,
        scan: &EntryScan,
        _context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<Entry>>> {
        // Upstream `scanEntries` (`memory.ts:177-195`): newest-first,
        // fork-aware — after this conversation's own entries, the parent's
        // up to the fork point, and so on.
        let scan = scan.clone();
        Box::pin(async move {
            let tables = self.inner.lock().expect("storage tables");
            let mut out: Vec<Entry> = Vec::new();
            let mut conversation_id: Option<Id> = Some(scan.conversation_id);
            let mut cap: Option<Id> = scan.before;
            while let Some(current) = conversation_id {
                if out.len() >= scan.limit {
                    break;
                }
                if let Some(own) = tables.entries_by_conversation.get(&current) {
                    for entry in own.iter().rev() {
                        if out.len() >= scan.limit {
                            break;
                        }
                        if let Some(before) = cap {
                            if entry.id >= before {
                                continue;
                            }
                        }
                        if let Some(kind) = &scan.kind {
                            if &entry.kind != kind {
                                continue;
                            }
                        }
                        if scan.with_head && entry.head.is_none() {
                            continue;
                        }
                        out.push(entry.clone());
                    }
                }
                let parent = tables
                    .conversations_by_id
                    .get(&current)
                    .and_then(|conversation| conversation.parent);
                conversation_id = parent.map(|parent| parent.conversation_id);
                cap = parent.map(|parent| cap.unwrap_or(Id::MAX).min(parent.at + 1));
            }
            Ok(out)
        })
    }

    fn task<'a>(
        &'a self,
        id: Id,
        _context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<Task>>> {
        Box::pin(async move {
            Ok(self
                .inner
                .lock()
                .expect("storage tables")
                .tasks_by_id
                .get(&id)
                .cloned())
        })
    }

    fn scan_tasks<'a>(
        &'a self,
        scan: &TaskScan,
        _context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<Task>>> {
        let scan = scan.clone();
        Box::pin(async move {
            let tables = self.inner.lock().expect("storage tables");
            let mut out = Vec::new();
            for id in &tables.task_order {
                let task = tables.tasks_by_id.get(id).expect("task order is indexed");
                if let Some(conversation_id) = scan.conversation_id {
                    if task.conversation_id != conversation_id {
                        continue;
                    }
                }
                if let Some(statuses) = &scan.status {
                    if !statuses.contains(&task.status) {
                        continue;
                    }
                }
                if let Some(kind) = &scan.kind {
                    if &task.kind != kind {
                        continue;
                    }
                }
                out.push(task.clone());
            }
            Ok(out)
        })
    }

    fn input<'a>(
        &'a self,
        id: Id,
        _context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<Input>>> {
        Box::pin(async move {
            Ok(self
                .inner
                .lock()
                .expect("storage tables")
                .inputs_by_id
                .get(&id)
                .cloned())
        })
    }

    fn input_by_request<'a>(
        &'a self,
        conversation_id: Id,
        request_id: &str,
        _context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<Input>>> {
        let request_id = request_id.to_owned();
        Box::pin(async move {
            let tables = self.inner.lock().expect("storage tables");
            Ok(tables
                .inputs_by_request
                .get(&format!("{conversation_id}:{request_id}"))
                .cloned())
        })
    }

    fn doc<'a>(
        &'a self,
        r#ref: &DocRef,
        _context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<JsonObject>>> {
        let r#ref = *r#ref;
        Box::pin(async move {
            let tables = self.inner.lock().expect("storage tables");
            match r#ref {
                DocRef::Session => Ok(Some(tables.session_doc.clone().unwrap_or_else(|| {
                    JsonObject::from_iter([("plugins".to_owned(), Value::Object(Map::new()))])
                }))),
                DocRef::Rewindable { conversation_id } => {
                    let log: Option<Vec<Vec<Op>>> = tables
                        .rewindable_log
                        .get(&conversation_id)
                        .map(|records| records.iter().map(|(_, ops)| ops.clone()).collect());
                    Ok(log.map(|log| MemoryStorage::fold(&log)))
                }
                DocRef::Sticky { conversation_id } => Ok(tables
                    .sticky_log
                    .get(&conversation_id)
                    .map(|log| MemoryStorage::fold(log))),
            }
        })
    }

    fn doc_as_of<'a>(
        &'a self,
        conversation_id: Id,
        at: Id,
        _context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<JsonObject>>> {
        // Upstream `docAsOf` (`memory.ts:230-243`): commit-granular history
        // — the rewindable state after the atomic commit that contains entry
        // `at`, walking the fork chain for entries owned by ancestors.
        Box::pin(async move {
            let tables = self.inner.lock().expect("storage tables");
            let Some(owner) = tables
                .entries_by_id
                .get(&at)
                .map(|entry| entry.conversation_id)
            else {
                return Ok(None);
            };
            // Walk the fork chain from `conversationId` until the owner
            // conversation; the walk itself only checks reachability
            // (`memory.ts:234-239`).
            let mut current = tables.conversations_by_id.get(&conversation_id);
            while let Some(conversation) = current {
                if conversation.id == owner {
                    break;
                }
                current = conversation
                    .parent
                    .and_then(|parent| tables.conversations_by_id.get(&parent.conversation_id));
            }
            if current.is_none() {
                return Ok(None);
            }
            let seq_at = tables.entry_seq.get(&at).copied().unwrap_or(0);
            let log: Vec<Vec<Op>> = tables
                .rewindable_log
                .get(&owner)
                .map(|records| {
                    records
                        .iter()
                        .filter(|(seq, _)| *seq <= seq_at)
                        .map(|(_, ops)| ops.clone())
                        .collect()
                })
                .unwrap_or_default();
            if log.is_empty() {
                return Ok(None);
            }
            Ok(Some(MemoryStorage::fold(&log)))
        })
    }

    fn truncate<'a>(
        &'a self,
        r#ref: &DocRef,
        _context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        // Upstream `truncate` (`memory.ts:255-266`): drop the sticky log
        // prefix before its last base.
        let r#ref = *r#ref;
        Box::pin(async move {
            if !matches!(r#ref, DocRef::Sticky { .. }) {
                return Ok(());
            }
            let DocRef::Sticky { conversation_id } = r#ref else {
                unreachable!("checked above")
            };
            let mut tables = self.inner.lock().expect("storage tables");
            if let Some(log) = tables.sticky_log.get_mut(&conversation_id) {
                let mut start = 0;
                for (index, ops) in log.iter().enumerate().rev() {
                    if is_base(ops) {
                        start = index;
                        break;
                    }
                }
                log.drain(0..start);
            }
            Ok(())
        })
    }

    fn close<'a>(&'a self, _context: Context) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            self.closed.store(true, Ordering::SeqCst);
            Ok(())
        })
    }
}
