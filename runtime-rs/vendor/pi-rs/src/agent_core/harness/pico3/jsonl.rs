//! Port of `packages/agent/src/harness/pico3/jsonl.ts` (339 lines): the
//! durable JSONL backend over the memory tables
//! ([`JsonlStorage`] extends [`super::memory::MemoryStorage`] upstream; the
//! port composes).
//!
//! Files in a session directory (`jsonl.ts:31-38`):
//! - `main.jsonl` — conversations, entries, inputs, rewindable/session doc
//!   ops, each task's CREATE and TERMINAL records, and one marker per commit.
//!   Append-only, never rewritten.
//! - `sticky-<conversation>.jsonl` — sticky doc ops; rewritten from its last
//!   base by `truncate` (on the Session line).
//! - `task-<id>.jsonl` — a live task's intermediate patches; unlinked after
//!   its terminal record is published in main.
//!
//! Publication (§10, `jsonl.ts:40-56`): one commit `Seq`; sidecar records
//! are appended first, then exactly one main record, last, listing the
//! sidecar refs it expects — the publication point. Replay applies a sidecar
//! record only when main has the marker for that seq naming that file;
//! unconfirmed sidecar tails are ignored. Every record carries the
//! committed-id high-water. Bytes after the last newline of any file are a
//! torn write and are truncated before the file is opened for append. There
//! is no compaction. The fsync/false recovery and fsync/true caveats of the
//! upstream header apply verbatim.
//!
//! Disclosed substitutions: the port writes the same record JSON
//! (`{seq,maxId,writes,refs?}`) with serde; sync `node:fs` calls map to
//! `std::fs` (the storage is called on the session line, and the upstream
//! calls are synchronous too). `ftruncate`/`fsync` map 1:1 through
//! [`std::fs::File`].

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom, Write as IoWrite};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use serde_json::Value;

use crate::agent_core::chord_support::delta::{is_base, Op};
use crate::agent_core::chord_support::Context;

use super::memory::MemoryStorage;
use super::types::{DocRef, Id, Input, JsonObject, Seq, Storage, Task, TaskStatus, Write};

/// One line of any file (`jsonl.ts:23-28`); `refs` (main only) lists the
/// sidecar files this commit also wrote. Parsing is manual so a record with
/// a wrong-typed `seq` fails with the upstream "lacks seq/maxId/writes"
/// message rather than a serde field error (`jsonl.ts:110-111`).
#[derive(Debug, Clone)]
struct JsonlRecord {
    seq: Seq,
    max_id: Id,
    writes: Vec<Value>,
    refs: Option<Vec<String>>,
}

impl JsonlRecord {
    fn parse(line: &str, file: &Path) -> anyhow::Result<JsonlRecord> {
        let value: Value = serde_json::from_str(line)
            .map_err(|error| anyhow::anyhow!("{}: malformed record: {error}", file.display()))?;
        let object = value
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("{}: malformed record", file.display()))?;
        let seq = object
            .get("seq")
            .and_then(Value::as_i64)
            .ok_or_else(|| anyhow::anyhow!("{}: record lacks seq/maxId/writes", file.display()))?;
        let max_id = object
            .get("maxId")
            .and_then(Value::as_i64)
            .ok_or_else(|| anyhow::anyhow!("{}: record lacks seq/maxId/writes", file.display()))?;
        let writes = object
            .get("writes")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow::anyhow!("{}: record lacks seq/maxId/writes", file.display()))?
            .clone();
        let refs = object.get("refs").and_then(Value::as_array).map(|refs| {
            refs.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        });
        Ok(JsonlRecord {
            seq,
            max_id,
            writes,
            refs,
        })
    }

    fn to_line(&self) -> anyhow::Result<Vec<u8>> {
        let mut object = serde_json::Map::new();
        object.insert(
            "seq".to_owned(),
            crate::agent_core::harness::pico3::types::number(self.seq),
        );
        object.insert(
            "maxId".to_owned(),
            crate::agent_core::harness::pico3::types::number(self.max_id),
        );
        object.insert("writes".to_owned(), Value::Array(self.writes.clone()));
        if let Some(refs) = &self.refs {
            object.insert(
                "refs".to_owned(),
                Value::Array(refs.iter().map(|r| Value::String(r.clone())).collect()),
            );
        }
        let mut line = serde_json::to_vec(&Value::Object(object))?;
        line.push(b'\n');
        Ok(line)
    }
}

/// Upstream `JsonlStorage` (`jsonl.ts:58-321`).
pub struct JsonlStorage {
    memory: MemoryStorage,
    inner: Mutex<Files>,
    dir: PathBuf,
    pub fsync: bool,
    closed_once: AtomicBool,
}

#[derive(Default, Debug)]
struct Files {
    main: Option<std::fs::File>,
    /// Sticky docs, by conversation (`jsonl.ts:60`).
    sidecars: HashMap<Id, std::fs::File>,
    task_sidecars: HashMap<Id, std::fs::File>,
}

impl std::fmt::Debug for JsonlStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsonlStorage")
            .field("dir", &self.dir)
            .finish_non_exhaustive()
    }
}

impl JsonlStorage {
    /// Upstream `open` (`jsonl.ts:73-84`): create the directory, truncate
    /// torn tails, replay.
    pub async fn open(dir: impl AsRef<Path>, fsync: bool) -> anyhow::Result<Arc<JsonlStorage>> {
        let dir = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir)?;
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".jsonl") {
                truncate_torn_tail(&entry.path())?;
            }
        }
        let storage = JsonlStorage {
            memory: MemoryStorage::new(),
            inner: Mutex::new(Files {
                main: Some(
                    std::fs::OpenOptions::new()
                        .append(true)
                        .create(true)
                        .open(dir.join("main.jsonl"))?,
                ),
                sidecars: HashMap::new(),
                task_sidecars: HashMap::new(),
            }),
            dir,
            fsync,
            closed_once: AtomicBool::new(false),
        };
        storage.replay().await?;
        Ok(Arc::new(storage))
    }

    /// For tests and tooling: sizes of every file in the directory
    /// (`jsonl.ts:86-91`).
    pub fn sizes(&self) -> HashMap<String, u64> {
        let mut out = HashMap::new();
        if let Ok(entries) = std::fs::read_dir(&self.dir) {
            for entry in entries.flatten() {
                if let Ok(metadata) = entry.metadata() {
                    out.insert(
                        entry.file_name().to_string_lossy().to_string(),
                        metadata.len(),
                    );
                }
            }
        }
        out
    }

    /// Upstream `replay` (`jsonl.ts:93-193`).
    async fn replay(&self) -> anyhow::Result<()> {
        let read = |file: &Path| -> anyhow::Result<Vec<(JsonlRecord, usize)>> {
            if !file.exists() {
                return Ok(Vec::new());
            }
            let bytes = std::fs::read(file)?;
            let mut out: Vec<(JsonlRecord, usize)> = Vec::new();
            let mut start = 0usize;
            let mut last: Seq = 0;
            while let Some(end) = bytes[start..].iter().position(|byte| *byte == 0x0a) {
                let end = start + end;
                let line = String::from_utf8_lossy(&bytes[start..end]).to_string();
                start = end + 1;
                if line.is_empty() {
                    continue;
                }
                let record = JsonlRecord::parse(&line, file)?;
                if record.seq <= last {
                    anyhow::bail!(
                        "{}: sequence not increasing at {}",
                        file.display(),
                        record.seq
                    );
                }
                last = record.seq;
                out.push((record, start));
            }
            Ok(out)
        };

        let decode_writes = |values: &[Value]| -> anyhow::Result<Vec<Write>> {
            values
                .iter()
                .map(|value| {
                    crate::agent_core::harness::pico3::types::write_from_json(value)
                        .map_err(|error| anyhow::anyhow!("malformed write: {error}"))
                })
                .collect()
        };

        let mut max_id: Id = 0;
        // Main is the commit log. Confirmed = (seq -> refs) for every marker
        // (`jsonl.ts:119-122`).
        let main_records: Vec<JsonlRecord> = read(&self.dir.join("main.jsonl"))?
            .into_iter()
            .map(|(record, _)| record)
            .collect();
        let mut confirmed: HashMap<Seq, HashSet<String>> = HashMap::new();
        for record in &main_records {
            confirmed.insert(
                record.seq,
                record
                    .refs
                    .clone()
                    .unwrap_or_default()
                    .into_iter()
                    .collect(),
            );
        }
        // Merge every file's records by seq so batches apply in commit order
        // (`jsonl.ts:123-130`).
        let mut sidecar_files: Vec<String> = std::fs::read_dir(&self.dir)?
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|file| file.starts_with("sticky-") || file.starts_with("task-"))
            .collect();
        sidecar_files.sort();
        let mut by_seq: BTreeMap<Seq, Vec<Write>> = BTreeMap::new();
        let mut found: HashMap<Seq, HashSet<String>> = HashMap::new();
        let mut retained_sticky_base: HashMap<String, Seq> = HashMap::new();
        let mut retired_tasks: HashMap<Id, Seq> = HashMap::new();
        for record in &main_records {
            let writes = decode_writes(&record.writes)?;
            for write in &writes {
                if let Write::TaskPatch { patch } = write {
                    if patch.status == Some(TaskStatus::Terminal) {
                        retired_tasks.insert(patch.id, record.seq);
                    }
                }
            }
            by_seq.insert(record.seq, writes);
            max_id = max_id.max(record.max_id);
        }
        for file in &sidecar_files {
            let path = self.dir.join(file);
            let records = read(&path)?;
            if let Some((first, _)) = records.first() {
                if file.starts_with("sticky-") {
                    let writes = decode_writes(&first.writes)?;
                    let ops: Vec<Op> = writes
                        .iter()
                        .flat_map(|write| match write {
                            Write::Doc { ops, .. } => ops.clone(),
                            _ => Vec::new(),
                        })
                        .collect();
                    if is_base(&ops) {
                        retained_sticky_base.insert(file.clone(), first.seq);
                    }
                }
            }
            let mut confirmed_end = 0usize;
            let mut saw_unconfirmed = false;
            for (record, end) in records {
                if !confirmed
                    .get(&record.seq)
                    .is_some_and(|refs| refs.contains(file))
                {
                    saw_unconfirmed = true;
                    continue;
                }
                if saw_unconfirmed {
                    anyhow::bail!(
                        "{}: confirmed record follows an unconfirmed tail at {}",
                        path.display(),
                        record.seq
                    );
                }
                confirmed_end = end;
                let writes = decode_writes(&record.writes)?;
                found.entry(record.seq).or_default().insert(file.clone());
                by_seq.entry(record.seq).or_default().extend(writes);
                max_id = max_id.max(record.max_id);
            }
            if saw_unconfirmed {
                truncate_to(&path, confirmed_end)?;
            }
        }
        for record in &main_records {
            for file in record.refs.clone().unwrap_or_default() {
                if found
                    .get(&record.seq)
                    .is_some_and(|files| files.contains(&file))
                {
                    continue;
                }
                if let Some(retained_from) = retained_sticky_base.get(&file) {
                    if record.seq < *retained_from {
                        continue;
                    }
                }
                if let Some(rest) = file
                    .strip_prefix("task-")
                    .and_then(|rest| rest.strip_suffix(".jsonl"))
                {
                    if let Ok(id) = rest.parse::<Id>() {
                        if let Some(retired_at) = retired_tasks.get(&id) {
                            if record.seq < *retired_at {
                                continue;
                            }
                        }
                    }
                }
                anyhow::bail!(
                    "{}: missing record for published sequence {}",
                    self.dir.join(&file).display(),
                    record.seq
                );
            }
        }
        for (seq, writes) in by_seq {
            // Reuse MemoryStorage's batch application at each committed seq
            // (`jsonl.ts:181-185`): roll the sequence back so the staged
            // batch lands at `seq`.
            self.memory.set_seq(seq - 1);
            self.memory
                .commit_sync(writes, Context::background())
                .map_err(|error| anyhow::anyhow!("replay: {error}"))?;
        }
        self.memory.set_next_id(max_id + 1);
        // Task sidecars belong to live tasks only; a leftover for a terminal
        // (or unknown) task cannot resurrect anything (`jsonl.ts:186-192`).
        for file in sidecar_files {
            let Some(rest) = file
                .strip_prefix("task-")
                .and_then(|rest| rest.strip_suffix(".jsonl"))
            else {
                continue;
            };
            let Ok(id) = rest.parse::<Id>() else {
                continue;
            };
            let task = self.memory.task_sync(id)?;
            let terminal = task.is_none_or(|task| task.status == TaskStatus::Terminal);
            if terminal {
                let mut files = self.inner.lock().expect("jsonl files");
                files.task_sidecars.remove(&id);
                drop(files);
                let _ = std::fs::remove_file(self.dir.join(&file));
            }
        }
        Ok(())
    }

    /// Append one record to a file (`jsonl.ts:250-253`).
    fn append(file: &mut std::fs::File, record: &JsonlRecord, fsync: bool) -> anyhow::Result<()> {
        file.write_all(&record.to_line()?)?;
        if fsync {
            file.sync_data()?;
        }
        Ok(())
    }

    fn sidecar(&self, files: &mut Files, conversation_id: Id) -> anyhow::Result<()> {
        if let std::collections::hash_map::Entry::Vacant(vacant) =
            files.sidecars.entry(conversation_id)
        {
            let file = std::fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(self.dir.join(format!("sticky-{conversation_id}.jsonl")))?;
            vacant.insert(file);
        }
        Ok(())
    }

    fn task_sidecar(&self, files: &mut Files, id: Id) -> anyhow::Result<()> {
        if let std::collections::hash_map::Entry::Vacant(vacant) = files.task_sidecars.entry(id) {
            let file = std::fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(self.dir.join(format!("task-{id}.jsonl")))?;
            vacant.insert(file);
        }
        Ok(())
    }

    /// Upstream `retireTaskSidecar` (`jsonl.ts:271-278`).
    fn retire_task_sidecar(&self, files: &mut Files, id: Id) {
        files.task_sidecars.remove(&id);
        let _ = std::fs::remove_file(self.dir.join(format!("task-{id}.jsonl")));
    }

    /// Upstream `nextIdAfter` (`jsonl.ts:232-248`).
    fn next_id_after(&self, writes: &[Write]) -> Id {
        let mut max = self.memory.peek_next_id() - 1;
        for write in writes {
            let id = match write {
                Write::Conversation { conversation } => conversation.id,
                Write::Entry { entry } => entry.id,
                Write::Task { task } => task.id,
                Write::Input { input } => input.id,
                _ => 0,
            };
            max = max.max(id);
        }
        max
    }
}

impl Storage for JsonlStorage {
    fn commit<'a>(
        &'a self,
        writes: Vec<Write>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Seq>> {
        Box::pin(async move {
            if self.closed_once.load(Ordering::SeqCst) {
                anyhow::bail!("storage closed");
            }
            let seq = self.memory.current_seq() + 1;
            // Validate the whole batch first; throws before any file is
            // touched (`jsonl.ts:198`).
            let staged = self.memory.stage(writes.clone(), seq)?;
            let max_id = self.next_id_after(&writes);
            let mut main: Vec<Write> = Vec::new();
            let mut sticky: BTreeMap<Id, Vec<Write>> = BTreeMap::new();
            let mut tasks: BTreeMap<Id, Vec<Write>> = BTreeMap::new();
            let mut retired: Vec<Id> = Vec::new();
            for write in writes {
                match write {
                    Write::Doc {
                        r#ref: DocRef::Sticky { conversation_id },
                        ops,
                    } => {
                        sticky.entry(conversation_id).or_default().push(Write::Doc {
                            r#ref: DocRef::Sticky { conversation_id },
                            ops,
                        });
                    }
                    Write::TaskPatch { patch } if patch.status != Some(TaskStatus::Terminal) => {
                        tasks
                            .entry(patch.id)
                            .or_default()
                            .push(Write::TaskPatch { patch });
                    }
                    other => {
                        if let Write::TaskPatch { patch } = &other {
                            if patch.status == Some(TaskStatus::Terminal) {
                                retired.push(patch.id);
                            }
                        }
                        main.push(other);
                    }
                }
            }
            let mut refs: Vec<String> = Vec::new();
            {
                let mut files = self.inner.lock().expect("jsonl files");
                for (id, task_writes) in &tasks {
                    self.task_sidecar(&mut files, *id)?;
                    let file = files.task_sidecars.get_mut(id).expect("opened above");
                    JsonlStorage::append(
                        file,
                        &JsonlRecord {
                            seq,
                            max_id,
                            writes: task_writes.iter().map(write_to_json).collect(),
                            refs: None,
                        },
                        self.fsync,
                    )?;
                    refs.push(format!("task-{id}.jsonl"));
                }
                for (conversation_id, sticky_writes) in &sticky {
                    self.sidecar(&mut files, *conversation_id)?;
                    let file = files
                        .sidecars
                        .get_mut(conversation_id)
                        .expect("opened above");
                    JsonlStorage::append(
                        file,
                        &JsonlRecord {
                            seq,
                            max_id,
                            writes: sticky_writes.iter().map(write_to_json).collect(),
                            refs: None,
                        },
                        self.fsync,
                    )?;
                    refs.push(format!("sticky-{conversation_id}.jsonl"));
                }
                // The publication point (`jsonl.ts:225`).
                let main_file = files.main.as_mut().expect("main open");
                JsonlStorage::append(
                    main_file,
                    &JsonlRecord {
                        seq,
                        max_id,
                        writes: main.iter().map(write_to_json).collect(),
                        refs: if refs.is_empty() { None } else { Some(refs) },
                    },
                    self.fsync,
                )?;
            }
            self.memory.apply_staged(staged);
            self.memory.set_seq(seq);
            for id in retired {
                let mut files = self.inner.lock().expect("jsonl files");
                // After the terminal record is durably published
                // (`jsonl.ts:228`).
                self.retire_task_sidecar(&mut files, id);
            }
            let _ = context;
            Ok(seq)
        })
    }

    fn mint_id(&self) -> Id {
        self.memory.mint_id()
    }

    fn conversation<'a>(
        &'a self,
        id: Id,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<super::types::Conversation>>> {
        Box::pin(async move { self.memory.conversation(id, context).await })
    }

    fn conversations<'a>(
        &'a self,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<super::types::Conversation>>> {
        Box::pin(async move { self.memory.conversations(context).await })
    }

    fn entries<'a>(
        &'a self,
        ids: &[Id],
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<HashMap<Id, super::types::Entry>>> {
        let ids = ids.to_vec();
        Box::pin(async move { self.memory.entries(&ids, context).await })
    }

    fn scan_entries<'a>(
        &'a self,
        scan: &super::types::EntryScan,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<super::types::Entry>>> {
        let scan = scan.clone();
        Box::pin(async move { self.memory.scan_entries(&scan, context).await })
    }

    fn task<'a>(&'a self, id: Id, context: Context) -> BoxFuture<'a, anyhow::Result<Option<Task>>> {
        Box::pin(async move { self.memory.task(id, context).await })
    }

    fn scan_tasks<'a>(
        &'a self,
        scan: &super::types::TaskScan,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<Task>>> {
        let scan = scan.clone();
        Box::pin(async move { self.memory.scan_tasks(&scan, context).await })
    }

    fn input<'a>(
        &'a self,
        id: Id,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<Input>>> {
        Box::pin(async move { self.memory.input(id, context).await })
    }

    fn input_by_request<'a>(
        &'a self,
        conversation_id: Id,
        request_id: &str,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<Input>>> {
        let request_id = request_id.to_owned();
        Box::pin(async move {
            self.memory
                .input_by_request(conversation_id, &request_id, context)
                .await
        })
    }

    fn doc<'a>(
        &'a self,
        r#ref: &DocRef,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<JsonObject>>> {
        let r#ref = *r#ref;
        Box::pin(async move { self.memory.doc(&r#ref, context).await })
    }

    fn doc_as_of<'a>(
        &'a self,
        conversation_id: Id,
        at: Id,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<JsonObject>>> {
        Box::pin(async move { self.memory.doc_as_of(conversation_id, at, context).await })
    }

    /// Rewrite the sticky sidecar from its last base: temp file, optional
    /// fsync, rename (`jsonl.ts:280-311`). Called on the Session line.
    fn truncate<'a>(
        &'a self,
        r#ref: &DocRef,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        let r#ref = *r#ref;
        Box::pin(async move {
            self.memory.truncate(&r#ref, context.clone()).await?;
            let DocRef::Sticky { conversation_id } = r#ref else {
                return Ok(());
            };
            let file = self.dir.join(format!("sticky-{conversation_id}.jsonl"));
            if !file.exists() {
                return Ok(());
            }
            let content = std::fs::read_to_string(&file)?;
            let lines: Vec<&str> = content
                .split('\n')
                .filter(|line| !line.is_empty())
                .collect();
            let mut start = 0usize;
            for index in (0..lines.len()).rev() {
                let record: JsonlRecord = match JsonlRecord::parse(lines[index], &file) {
                    Ok(record) => record,
                    Err(_) => continue,
                };
                let ops: Vec<Op> = match decode_writes_of(&record.writes) {
                    Ok(writes) => writes
                        .iter()
                        .flat_map(|write| match write {
                            Write::Doc { ops, .. } => ops.clone(),
                            _ => Vec::new(),
                        })
                        .collect(),
                    Err(_) => continue,
                };
                if is_base(&ops) {
                    start = index;
                    break;
                }
            }
            if start == 0 {
                return Ok(());
            }
            let tmp = self.dir.join(format!("sticky-{conversation_id}.jsonl.tmp"));
            {
                let mut fd = std::fs::File::create(&tmp)?;
                for line in &lines[start..] {
                    fd.write_all(line.as_bytes())?;
                    fd.write_all(b"\n")?;
                }
                if self.fsync {
                    fd.sync_data()?;
                }
            }
            {
                let mut files = self.inner.lock().expect("jsonl files");
                files.sidecars.remove(&conversation_id);
            }
            std::fs::rename(&tmp, &file)?;
            let reopened = std::fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(&file)?;
            self.inner
                .lock()
                .expect("jsonl files")
                .sidecars
                .insert(conversation_id, reopened);
            let _ = context;
            Ok(())
        })
    }

    fn close<'a>(&'a self, _context: Context) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            if self.closed_once.swap(true, Ordering::SeqCst) {
                return Ok(());
            }
            self.memory.close(Context::background()).await?;
            let mut files = self.inner.lock().expect("jsonl files");
            files.main = None;
            files.sidecars.clear();
            files.task_sidecars.clear();
            Ok(())
        })
    }
}

use crate::agent_core::harness::pico3::types::{write_from_json, write_to_json};

fn decode_writes_of(values: &[Value]) -> anyhow::Result<Vec<Write>> {
    values.iter().map(write_from_json).collect()
}

/// `truncateTo` (`jsonl.ts:323-330`).
fn truncate_to(file: &Path, length: usize) -> anyhow::Result<()> {
    let mut fd = std::fs::OpenOptions::new().write(true).open(file)?;
    fd.set_len(length as u64)?;
    fd.flush()?;
    Ok(())
}

/// Bytes after the last newline are a torn write: cut them so a later append
/// cannot extend a corrupt line (`jsonl.ts:332-339`).
fn truncate_torn_tail(file: &Path) -> anyhow::Result<()> {
    let mut fd = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(file)?;
    let size = fd.metadata()?.len();
    if size == 0 {
        return Ok(());
    }
    let mut buf = Vec::new();
    fd.read_to_end(&mut buf)?;
    // `lastIndexOf` returns -1 when the file has no newline at all, so
    // `end + 1` is 0 and the whole file is a torn record: cut it
    // (`jsonl.ts:337-338`).
    let keep = buf
        .iter()
        .rposition(|byte| *byte == 0x0a)
        .map_or(0, |end| end + 1);
    if keep != buf.len() {
        fd.seek(SeekFrom::Start(0))?;
        fd.set_len(keep as u64)?;
    }
    Ok(())
}
