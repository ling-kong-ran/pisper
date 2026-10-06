//! Port of `src/storage/jsonl/**`: the portable JSONL implementation of the
//! storage contract.
//!
//! Layout (`storage/jsonl/storage.ts`): `main.jsonl` holds one commit marker
//! per line (`{format, type: "commit", seq, writes}`); per-document and
//! live-task payload lines are appended to sidecar files
//! (`doc-<id>.jsonl` / `task-<id>.jsonl`) before the main marker, and are
//! reclaimed (compacted or deleted) after the marker publishes them. Reads are
//! served by the embedded [`MemoryStorage`], recovered from the log at open.
//!
//! Parse-side records stay raw `serde_json::Value`s validated field by field
//! with the upstream messages (corruption diagnostics are part of the wire
//! behavior); the encode side serializes typed writes. Divergences:
//! structural (std::fs instead of the injected `FileSystem` capability, with
//! identical ordering: sidecars append, then the main marker; `fsync` flushes
//! sidecars before the marker).

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent_core::chord_support::context::Context;

use super::memory::MemoryStorage;
use super::{EntryWithCommitSeq, Storage, StorageError};
use crate::durable::types::{
    ConversationQuery, ConversationRecord, Cursor, DocumentAddress, DocumentContent,
    DocumentCreate, DocumentHistory, DocumentPoint, DocumentQuery, DocumentRecord, DocumentScope,
    EntryQuery, EntryRecord, Page, StorageWrite, StoredDocument, SubmissionQuery, SubmissionRecord,
    TaskId, TaskQuery, TaskRecord, TaskStatus,
};

const FORMAT_VERSION: i64 = 1;
const MAIN_FILE: &str = "main.jsonl";
const RECLAIM_SUFFIX: &str = ".reclaim";
/// Safe-integer bound (`Number.MAX_SAFE_INTEGER`).
const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

/// One main-log operation written by the encode path (`storage.ts`
/// `MainOperation`). Terminal task records and document retirement write to
/// the main log directly; live task records and document content write to
/// sidecars, referenced by ordinal.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum MainOperation {
    #[serde(rename = "conversation")]
    Conversation { value: ConversationRecord },
    #[serde(rename = "entry")]
    Entry { value: EntryRecord },
    #[serde(rename = "submission")]
    Submission { value: SubmissionRecord },
    #[serde(rename = "task")]
    Task { value: TaskRecord },
    #[serde(rename = "document.retire")]
    DocumentRetire { id: i64 },
    #[serde(rename = "task.sidecar")]
    TaskSidecar { id: i64, ordinal: i64 },
    #[serde(rename = "document.create")]
    DocumentCreate {
        record: DocumentCreate,
        ordinal: i64,
    },
    #[serde(rename = "document.change")]
    DocumentChange { id: i64, ordinal: i64 },
}

/// The main-log commit marker (`storage.ts` `MainMarker`); field order is the
/// upstream literal order and the JSONL wire format.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MainMarker {
    pub format: i64,
    pub r#type: String,
    pub seq: i64,
    pub writes: Vec<MainOperation>,
}

/// One sidecar payload written by the encode path (`storage.ts`
/// `SidecarPayload`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[allow(clippy::large_enum_variant)]
pub enum SidecarPayload {
    #[serde(rename = "task")]
    Task { value: TaskRecord },
    #[serde(rename = "document")]
    Document { id: i64, content: DocumentContent },
}

/// One sidecar record written by the encode path (`storage.ts`
/// `SidecarRecord`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SidecarRecord {
    pub format: i64,
    pub r#type: String,
    pub seq: i64,
    pub ordinal: i64,
    pub payload: SidecarPayload,
}

/// One parsed main-log marker: the sequence plus validated raw write objects
/// (parse side keeps `Value`s so every corruption carries the upstream
/// message).
struct ParsedMainMarker {
    seq: i64,
    writes: Vec<Value>,
}

/// One parsed sidecar record: sequence, ordinal, and the validated raw
/// payload.
#[derive(Clone)]
struct ParsedSidecarRecord {
    seq: i64,
    ordinal: i64,
    payload: Value,
}

/// One parsed line with its byte offset (`storage.ts` `ParsedLine`).
struct ParsedLine<T> {
    value: T,
    start: usize,
}

/// One parsed file (`storage.ts` `ParsedFile`).
struct ParsedFile<T> {
    path: PathBuf,
    lines: Vec<ParsedLine<T>>,
}

/// The encoded writes of one commit (`storage.ts` `EncodedCommit`).
struct EncodedCommit {
    marker: String,
    /// `(file, content)` in first-touch insertion order.
    sidecars: Vec<(String, String)>,
}

/// Options for opening a JSONL storage directory (`storage.ts`
/// `JsonlStorageOptions`).
#[derive(Debug, Clone, Copy, Default)]
pub struct JsonlStorageOptions {
    /// Flush every affected sidecar before appending the main marker.
    /// Defaults to false.
    pub fsync: bool,
}

/// Interior mutable backend state.
struct JsonlState {
    memory: MemoryStorage,
    current_only_documents: Vec<i64>,
    live_task_sidecars: Vec<i64>,
    closed: bool,
    poison: Option<StorageError>,
}

/// Portable JSONL implementation of the storage contract (`storage.ts`
/// `JsonlStorage`).
pub struct JsonlStorage {
    directory: PathBuf,
    main_path: PathBuf,
    fsync: bool,
    state: Mutex<JsonlState>,
}

/// `JsonlCorruptionError` (`storage.ts`).
pub fn corruption(message: impl Into<String>) -> StorageError {
    StorageError::corruption(message)
}

/// `errorFromFile(action, error)` (`storage.ts:80-82`).
fn error_from(action: &str, message: &str) -> StorageError {
    StorageError::generic(format!("JSONL {action} failed: {message}"))
}

/// `sidecarFileName(kind, id)` (`storage.ts:68`).
fn sidecar_file_name(kind: &str, id: i64) -> String {
    format!("{kind}-{id}.jsonl")
}

/// `isSidecarFileName` (`storage.ts:69`): `^(?:doc|task)-(?:0|[1-9]\d*)\.jsonl$`.
fn is_sidecar_file_name(name: &str) -> bool {
    let Some(rest) = name
        .strip_prefix("doc-")
        .or_else(|| name.strip_prefix("task-"))
    else {
        return false;
    };
    let Some(digits) = rest.strip_suffix(".jsonl") else {
        return false;
    };
    !digits.is_empty()
        && (digits == "0" || !digits.starts_with('0'))
        && digits.bytes().all(|byte| byte.is_ascii_digit())
}

/// `isReclaimFileName` (`storage.ts:70`).
fn is_reclaim_file_name(name: &str) -> bool {
    match name.strip_suffix(RECLAIM_SUFFIX) {
        Some(base) => is_sidecar_file_name(base),
        None => false,
    }
}

/// `isCurrentOnly(record)` (`storage.ts:71-73`).
fn is_current_only(record: &DocumentCreate) -> bool {
    !matches!(record.scope, DocumentScope::Conversation { .. })
        || record.history == Some(DocumentHistory::Latest)
}

/// `sidecarKey(file, seq, ordinal)` (`storage.ts:75-77`).
fn sidecar_key(file: &str, seq: i64, ordinal: i64) -> String {
    Value::Array(vec![
        Value::from(file),
        Value::from(seq),
        Value::from(ordinal),
    ])
    .to_string()
}

/// `jsonLine(value)` (`storage.ts:78`): the exact serialized line, newline
/// terminated.
fn json_line<T: Serialize>(value: &T) -> String {
    let mut line = serde_json::to_string(value).expect("jsonl record serializes");
    line.push('\n');
    line
}

/// Re-serialize one parsed sidecar record as its exact line (upstream reuses
/// `jsonLine(line.value)` over the retained parsed records).
fn json_line_sidecar(record: &ParsedSidecarRecord) -> String {
    let object = serde_json::json!({
        "format": FORMAT_VERSION,
        "type": "record",
        "seq": record.seq,
        "ordinal": record.ordinal,
        "payload": record.payload,
    });
    let mut line = serde_json::to_string(&object).expect("sidecar record serializes");
    line.push('\n');
    line
}

/// `safeInt(value)` against `Number.isSafeInteger`.
fn safe_int(value: &Value) -> Option<i64> {
    let number = value.as_i64()?;
    if number.abs() > MAX_SAFE_INTEGER {
        return None;
    }
    Some(number)
}

fn is_object(value: &Value) -> bool {
    value.is_object()
}

impl JsonlStorage {
    /// Open or create a JSONL storage directory (`storage.ts`
    /// `JsonlStorage.open`).
    pub fn open(
        directory: &Path,
        options: JsonlStorageOptions,
    ) -> Result<JsonlStorage, StorageError> {
        let absolute = std::path::absolute(directory)
            .map_err(|error| error_from("path resolution", &error.to_string()))?;
        std::fs::create_dir_all(&absolute)
            .map_err(|error| error_from("directory creation", &error.to_string()))?;
        let main_path = absolute.join(MAIN_FILE);
        let storage = JsonlStorage {
            directory: absolute,
            main_path,
            fsync: options.fsync,
            state: Mutex::new(JsonlState {
                memory: MemoryStorage::new(),
                current_only_documents: Vec::new(),
                live_task_sidecars: Vec::new(),
                closed: false,
                poison: None,
            }),
        };
        storage.recover()?;
        Ok(storage)
    }

    /// `assertUsable` (`storage.ts:838-845`).
    fn assert_usable(&self, state: &JsonlState) -> Result<(), StorageError> {
        if state.closed {
            return Err(StorageError::closed("JsonlStorage is closed"));
        }
        if let Some(poison) = &state.poison {
            return Err(StorageError::poisoned(poison.clone()));
        }
        Ok(())
    }

    /// `poison(cause)` (`storage.ts:830-836`): retain the first poison cause.
    fn poison(&self, state: &mut JsonlState, cause: StorageError) -> StorageError {
        if state.poison.is_none() {
            state.poison = Some(cause);
        }
        StorageError::poisoned(state.poison.as_ref().unwrap().clone())
    }

    /// `encodeCommit` (`storage.ts:396-449`).
    fn encode_commit(&self, seq: i64, writes: &[StorageWrite]) -> EncodedCommit {
        let mut main_writes: Vec<MainOperation> = Vec::new();
        let mut files: Vec<String> = Vec::new();
        let mut records: HashMap<String, Vec<SidecarRecord>> = HashMap::new();
        let mut next_ordinal: i64 = 0;

        for write in writes {
            match write {
                StorageWrite::Conversation { .. }
                | StorageWrite::Entry { .. }
                | StorageWrite::Submission { .. }
                | StorageWrite::DocumentRetire { .. } => {
                    main_writes.push(main_operation_of_write(write));
                }
                StorageWrite::Task { value } => {
                    if value.status() == TaskStatus::Terminal {
                        main_writes.push(main_operation_of_write(write));
                    } else {
                        let file = sidecar_file_name("task", value.id);
                        let ordinal = push_sidecar(
                            &mut files,
                            &mut records,
                            &file,
                            SidecarPayload::Task {
                                value: value.clone(),
                            },
                            seq,
                            next_ordinal,
                        );
                        next_ordinal += 1;
                        main_writes.push(MainOperation::TaskSidecar {
                            id: value.id,
                            ordinal,
                        });
                    }
                }
                StorageWrite::DocumentCreate { record, content } => {
                    let file = sidecar_file_name("doc", record.id);
                    let ordinal = push_sidecar(
                        &mut files,
                        &mut records,
                        &file,
                        SidecarPayload::Document {
                            id: record.id,
                            content: content.clone(),
                        },
                        seq,
                        next_ordinal,
                    );
                    next_ordinal += 1;
                    main_writes.push(MainOperation::DocumentCreate {
                        record: record.clone(),
                        ordinal,
                    });
                }
                StorageWrite::DocumentChange { id, content } => {
                    let file = sidecar_file_name("doc", *id);
                    let ordinal = push_sidecar(
                        &mut files,
                        &mut records,
                        &file,
                        SidecarPayload::Document {
                            id: *id,
                            content: content.clone(),
                        },
                        seq,
                        next_ordinal,
                    );
                    next_ordinal += 1;
                    main_writes.push(MainOperation::DocumentChange { id: *id, ordinal });
                }
                StorageWrite::DocumentCopy { .. } => {
                    unreachable!(
                        "document copies are resolved by the memory backend before encoding"
                    )
                }
            }
        }

        let mut sidecars: Vec<(String, String)> = Vec::new();
        for file in &files {
            let lines: String = records[file].iter().map(json_line).collect();
            sidecars.push((file.clone(), lines));
        }
        let marker = MainMarker {
            format: FORMAT_VERSION,
            r#type: String::from("commit"),
            seq,
            writes: main_writes,
        };
        EncodedCommit {
            marker: json_line(&marker),
            sidecars,
        }
    }

    /// `planReclamations` (`storage.ts:451-498`). The caller holds the state
    /// lock for the whole commit; this variant reads the index sets from it.
    fn plan_reclamations_with_state(
        &self,
        state: &JsonlState,
        writes: &[StorageWrite],
        encoded: &EncodedCommit,
    ) -> Vec<(String, String)> {
        let mut created_current_only_documents: Vec<i64> = Vec::new();
        let mut retired_documents: Vec<i64> = Vec::new();
        let mut base_documents: Vec<i64> = Vec::new();
        let mut final_tasks: Vec<(i64, &TaskRecord)> = Vec::new();
        for write in writes {
            match write {
                StorageWrite::DocumentCreate { record, .. } => {
                    if is_current_only(record) {
                        created_current_only_documents.push(record.id);
                    }
                }
                StorageWrite::DocumentChange { id, content } => {
                    if matches!(content, DocumentContent::Base { .. }) {
                        base_documents.push(*id);
                    }
                }
                StorageWrite::DocumentRetire { id } => retired_documents.push(*id),
                StorageWrite::Task { value } => final_tasks.push((value.id, value)),
                _ => {}
            }
        }

        let mut replacements: Vec<(String, String)> = Vec::new();
        let is_current_only_document = |id: i64| -> bool {
            state.current_only_documents.contains(&id)
                || created_current_only_documents.contains(&id)
        };
        for id in &retired_documents {
            if is_current_only_document(*id) {
                replacements.push((sidecar_file_name("doc", *id), String::new()));
            }
        }
        for id in &base_documents {
            if !is_current_only_document(*id) || retired_documents.contains(id) {
                continue;
            }
            let file = sidecar_file_name("doc", *id);
            if let Some((_, content)) = encoded.sidecars.iter().find(|(name, _)| name == &file) {
                replacements.push((file, content.clone()));
            }
        }
        for (id, task) in final_tasks {
            if task.status() == TaskStatus::Terminal
                && (state.live_task_sidecars.contains(&id)
                    || encoded
                        .sidecars
                        .iter()
                        .any(|(name, _)| name == &sidecar_file_name("task", id)))
            {
                replacements.push((sidecar_file_name("task", id), String::new()));
            }
        }
        replacements
    }

    #[allow(dead_code)] // kept for non-commit readers planned in the harness slice
    fn with_state<T>(&self, read: impl FnOnce(&JsonlState) -> T) -> T {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        read(&state)
    }

    /// `reclaimSidecars` (`storage.ts:517-524`): the marker already published
    /// this state, so reclamation is retryable best-effort maintenance.
    fn reclaim_sidecars(&self, replacements: &[(String, String)]) {
        if replacements.is_empty() {
            return;
        }
        if self.fsync {
            let flushed = std::fs::OpenOptions::new()
                .append(true)
                .open(&self.main_path)
                .and_then(|handle| handle.sync_all());
            if flushed.is_err() {
                return;
            }
        }
        for (file, content) in replacements {
            self.replace_sidecar(file, content);
        }
    }

    /// `replaceSidecar` (`storage.ts:526-543`).
    fn replace_sidecar(&self, file: &str, content: &str) {
        let path = self.directory.join(file);
        if content.is_empty() {
            let _ = std::fs::remove_file(&path);
            return;
        }
        let temporary_path = self.directory.join(format!("{file}{RECLAIM_SUFFIX}"));
        if std::fs::write(&temporary_path, content).is_err() {
            return;
        }
        if self.fsync {
            if let Ok(handle) = std::fs::File::open(&temporary_path) {
                let _ = handle.sync_all();
            }
        }
        let _ = std::fs::rename(&temporary_path, &path);
    }

    /// `recover` (`storage.ts:545-758`): replay the main log against the
    /// sidecars, truncating torn lines and unconfirmed tails, and reclaiming
    /// compactable files.
    fn recover(&self) -> Result<(), StorageError> {
        let main = Self::read_lines(&self.main_path, MAIN_FILE, parse_main_marker)?;
        let mut previous_seq: i64 = 0;
        for marker in &main.lines {
            if marker.value.seq <= previous_seq {
                return Err(corruption(
                    "Commit sequence does not strictly increase in main.jsonl",
                ));
            }
            previous_seq = marker.value.seq;
        }

        let listed = std::fs::read_dir(&self.directory)
            .map_err(|error| error_from("directory listing", &error.to_string()))?;
        let mut infos: Vec<(String, PathBuf, bool)> = Vec::new();
        for entry in listed.flatten() {
            let path = entry.path();
            let is_file = entry
                .file_type()
                .map(|kind| kind.is_file())
                .unwrap_or(false);
            if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
                infos.push((name.to_string(), path, is_file));
            }
        }
        for (name, path, is_file) in &infos {
            if *is_file && is_reclaim_file_name(name) {
                let _ = std::fs::remove_file(path);
            }
        }
        let mut sidecar_files: Vec<String> = infos
            .iter()
            .filter(|(name, _, is_file)| *is_file && is_sidecar_file_name(name))
            .map(|(name, _, _)| name.clone())
            .collect();
        sidecar_files.sort();
        let mut parsed_files: HashMap<String, ParsedFile<ParsedSidecarRecord>> = HashMap::new();
        let mut record_by_key: HashMap<String, ParsedSidecarRecord> = HashMap::new();
        for file in &sidecar_files {
            let path = self.directory.join(file);
            let parsed = Self::read_lines(&path, file, |text, line| {
                parse_sidecar_record(text, file, line)
            })?;
            let mut previous: Option<ParsedSidecarRecord> = None;
            for line in &parsed.lines {
                if let Some(previous) = &previous {
                    if line.value.seq < previous.seq
                        || (line.value.seq == previous.seq
                            && line.value.ordinal <= previous.ordinal)
                    {
                        return Err(corruption(format!(
                            "Sidecar records are out of order in {file}"
                        )));
                    }
                }
                previous = Some(line.value.clone());
                record_by_key.insert(
                    sidecar_key(file, line.value.seq, line.value.ordinal),
                    line.value.clone(),
                );
            }
            parsed_files.insert(file.clone(), parsed);
        }

        let mut current_only_documents: Vec<i64> = Vec::new();
        let mut retired_documents: Vec<i64> = Vec::new();
        let mut final_task_is_live: Vec<(i64, bool)> = Vec::new();
        for marker in &main.lines {
            for operation in &marker.value.writes {
                match operation.get("type").and_then(Value::as_str) {
                    Some("document.create") => {
                        // `isCurrentOnly(operation.record)` over the raw
                        // record; unparseable records simply do not count as
                        // current-only (upstream would throw while reading
                        // `scope.kind`, also aborting recovery).
                        let scope_kind = operation["record"]["scope"]["kind"].as_str();
                        let history = operation["record"]["history"].as_str();
                        let current_only =
                            scope_kind != Some("conversation") || history == Some("latest");
                        let id = operation["record"]["id"].as_i64().unwrap_or_default();
                        if current_only && !current_only_documents.contains(&id) {
                            current_only_documents.push(id);
                        }
                    }
                    Some("document.retire") => {
                        let id = operation["id"].as_i64().unwrap_or_default();
                        if !retired_documents.contains(&id) {
                            retired_documents.push(id);
                        }
                    }
                    Some("task") => {
                        set_pair(
                            &mut final_task_is_live,
                            operation["value"]["id"].as_i64().unwrap_or_default(),
                            false,
                        );
                    }
                    Some("task.sidecar") => {
                        set_pair(
                            &mut final_task_is_live,
                            operation["id"].as_i64().unwrap_or_default(),
                            true,
                        );
                    }
                    _ => {}
                }
            }
        }
        let retired_current_only_documents: Vec<i64> = retired_documents
            .iter()
            .filter(|id| current_only_documents.contains(id))
            .copied()
            .collect();

        // `latestBases` (`storage.ts:640-676`): the newest confirmed base
        // sidecar record per current-only document.
        let mut latest_bases: HashMap<i64, (i64, i64, ParsedSidecarRecord)> = HashMap::new();
        for marker in &main.lines {
            for operation in &marker.value.writes {
                let (id, ordinal) = match operation.get("type").and_then(Value::as_str) {
                    Some("document.create") => (
                        operation["record"]["id"].as_i64().unwrap_or_default(),
                        operation["ordinal"].as_i64().unwrap_or_default(),
                    ),
                    Some("document.change") => (
                        operation["id"].as_i64().unwrap_or_default(),
                        operation["ordinal"].as_i64().unwrap_or_default(),
                    ),
                    _ => continue,
                };
                if !current_only_documents.contains(&id) {
                    continue;
                }
                let record = record_by_key.get(&sidecar_key(
                    &sidecar_file_name("doc", id),
                    marker.value.seq,
                    ordinal,
                ));
                let Some(record) = record else { continue };
                let payload = &record.payload;
                if payload.get("type").and_then(Value::as_str) != Some("document")
                    || payload["id"].as_i64() != Some(id)
                    || payload["content"]["kind"].as_str() != Some("base")
                {
                    continue;
                }
                let newer = match latest_bases.get(&id) {
                    None => true,
                    Some((previous_seq, previous_ordinal, _)) => {
                        record.seq > *previous_seq
                            || (record.seq == *previous_seq && record.ordinal > *previous_ordinal)
                    }
                };
                if newer {
                    latest_bases.insert(id, (record.seq, record.ordinal, record.clone()));
                }
            }
        }

        let is_before_latest_base = |id: i64, seq: i64, ordinal: i64| -> bool {
            match latest_bases.get(&id) {
                Some((base_seq, base_ordinal, _)) => {
                    seq < *base_seq || (seq == *base_seq && ordinal < *base_ordinal)
                }
                None => false,
            }
        };
        let terminal_tasks: Vec<i64> = final_task_is_live
            .iter()
            .filter(|(_, live)| !live)
            .map(|(id, _)| *id)
            .collect();
        let mut confirmed: Vec<String> = Vec::new();
        for line in &main.lines {
            let seq = line.value.seq;
            let mut writes: Vec<Value> = Vec::new();
            for operation in &line.value.writes {
                match operation.get("type").and_then(Value::as_str) {
                    Some("conversation")
                    | Some("entry")
                    | Some("submission")
                    | Some("task")
                    | Some("document.retire") => {
                        writes.push(operation.clone());
                    }
                    Some("task.sidecar") => {
                        let id = operation["id"].as_i64().unwrap_or_default();
                        let ordinal = operation["ordinal"].as_i64().unwrap_or_default();
                        let optional = terminal_tasks.contains(&id);
                        let record = Self::confirm_record(
                            seq,
                            ordinal,
                            &sidecar_file_name("task", id),
                            &record_by_key,
                            &mut confirmed,
                            optional,
                        )?;
                        if let Some(record) = record {
                            let payload = &record.payload;
                            if payload.get("type").and_then(Value::as_str) != Some("task")
                                || payload["value"]["id"].as_i64() != Some(id)
                            {
                                return Err(corruption(format!(
                                    "Confirmed task sidecar data does not match commit {seq}"
                                )));
                            }
                            if !optional {
                                writes.push(serde_json::json!({
                                    "type": "task",
                                    "value": payload["value"].clone(),
                                }));
                            }
                        }
                    }
                    Some("document.create") | Some("document.change") => {
                        let id = if operation.get("type").and_then(Value::as_str)
                            == Some("document.create")
                        {
                            operation["record"]["id"].as_i64().unwrap_or_default()
                        } else {
                            operation["id"].as_i64().unwrap_or_default()
                        };
                        let ordinal = operation["ordinal"].as_i64().unwrap_or_default();
                        let reclaimed = retired_current_only_documents.contains(&id)
                            || is_before_latest_base(id, seq, ordinal);
                        let record = Self::confirm_record(
                            seq,
                            ordinal,
                            &sidecar_file_name("doc", id),
                            &record_by_key,
                            &mut confirmed,
                            reclaimed,
                        )?;
                        let mut content: Option<Value> = None;
                        if let Some(record) = record {
                            let payload = &record.payload;
                            if payload.get("type").and_then(Value::as_str) != Some("document")
                                || payload["id"].as_i64() != Some(id)
                            {
                                return Err(corruption(format!(
                                    "Confirmed document sidecar data does not match commit {seq}"
                                )));
                            }
                            content = Some(payload["content"].clone());
                        }
                        if operation.get("type").and_then(Value::as_str) == Some("document.create")
                        {
                            if let Some(content) = &content {
                                if content["kind"].as_str() != Some("base") {
                                    return Err(corruption(format!(
                                        "Document creation lacks a confirmed base in commit {seq}"
                                    )));
                                }
                            }
                            let effective = match (&content, reclaimed) {
                                (None, _) | (_, true) => serde_json::json!({
                                    "kind": "base",
                                    "version": 1,
                                    "value": {},
                                }),
                                (Some(content), false) => content.clone(),
                            };
                            writes.push(serde_json::json!({
                                "type": "document.create",
                                "record": operation["record"],
                                "content": effective,
                            }));
                        } else if !reclaimed {
                            if let Some(content) = content {
                                writes.push(serde_json::json!({
                                    "type": "document.change",
                                    "id": id,
                                    "content": content,
                                }));
                            }
                        }
                    }
                    _ => {}
                }
            }
            let state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let mut prepared = state
                .memory
                .prepare_commit_value(&writes, Some(seq))
                .map_err(|_| corruption(format!("Invalid committed state at sequence {seq}")))?;
            state
                .memory
                .apply_prepared(&mut prepared)
                .map_err(|_| corruption(format!("Invalid committed state at sequence {seq}")))?;
        }

        // Tail truncation and reclamation planning (`storage.ts:704-756`).
        let mut reclamations: Vec<(String, String)> = Vec::new();
        for file in &sidecar_files {
            let parsed = &parsed_files[file];
            let mut unconfirmed_at: Option<usize> = None;
            for line in &parsed.lines {
                let key = sidecar_key(file, line.value.seq, line.value.ordinal);
                if confirmed.contains(&key) {
                    if unconfirmed_at.is_some() {
                        return Err(corruption(format!(
                            "Confirmed record follows an unconfirmed tail in {file}"
                        )));
                    }
                } else if unconfirmed_at.is_none() {
                    unconfirmed_at = Some(line.start);
                }
            }
            if let Some(unconfirmed_at) = unconfirmed_at {
                let truncated = std::fs::OpenOptions::new()
                    .write(true)
                    .open(&parsed.path)
                    .and_then(|handle| handle.set_len(unconfirmed_at as u64));
                if let Err(error) = truncated {
                    return Err(error_from(
                        &format!("tail truncation of {file}"),
                        &error.to_string(),
                    ));
                }
            }

            let dash = file.find('-').unwrap();
            let numeric_id: i64 = file[dash + 1..file.len() - ".jsonl".len()]
                .parse()
                .unwrap_or(0);
            let confirmed_lines: Vec<&ParsedLine<ParsedSidecarRecord>> = parsed
                .lines
                .iter()
                .filter(|line| {
                    confirmed.contains(&sidecar_key(file, line.value.seq, line.value.ordinal))
                })
                .collect();
            let mut retained_lines: Option<Vec<&ParsedLine<ParsedSidecarRecord>>> = None;
            if file.starts_with("task-") && terminal_tasks.contains(&numeric_id) {
                retained_lines = Some(Vec::new());
            } else if file.starts_with("doc-") {
                let document_id = numeric_id;
                if retired_current_only_documents.contains(&document_id) {
                    retained_lines = Some(Vec::new());
                } else if latest_bases.contains_key(&document_id) {
                    retained_lines = Some(
                        confirmed_lines
                            .iter()
                            .copied()
                            .filter(|line| {
                                !is_before_latest_base(
                                    document_id,
                                    line.value.seq,
                                    line.value.ordinal,
                                )
                            })
                            .collect(),
                    );
                }
            }
            if let Some(retained) = retained_lines {
                if retained.len() < confirmed_lines.len() || retained.is_empty() {
                    let content: String = retained
                        .iter()
                        .map(|line| json_line_sidecar(&line.value))
                        .collect();
                    reclamations.push((file.clone(), content));
                }
            }
        }
        self.reclaim_sidecars(&reclamations);

        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for id in current_only_documents {
            if !state.current_only_documents.contains(&id) {
                state.current_only_documents.push(id);
            }
        }
        for (id, live) in final_task_is_live {
            if live && !state.live_task_sidecars.contains(&id) {
                state.live_task_sidecars.push(id);
            }
        }
        Ok(())
    }

    /// `confirmRecord` (`storage.ts:760-776`).
    fn confirm_record(
        seq: i64,
        ordinal: i64,
        file: &str,
        record_by_key: &HashMap<String, ParsedSidecarRecord>,
        confirmed: &mut Vec<String>,
        optional: bool,
    ) -> Result<Option<ParsedSidecarRecord>, StorageError> {
        let key = sidecar_key(file, seq, ordinal);
        if confirmed.contains(&key) {
            return Err(corruption("Sidecar record is confirmed more than once"));
        }
        let Some(record) = record_by_key.get(&key) else {
            if optional {
                return Ok(None);
            }
            return Err(corruption(format!(
                "Missing confirmed sidecar record {file} at sequence {seq}"
            )));
        };
        confirmed.push(key);
        Ok(Some(record.clone()))
    }

    /// `readLines` (`storage.ts:778-814`): strict-UTF-8, newline-terminated
    /// lines; torn tails are truncated before parsing.
    fn read_lines<T>(
        path: &Path,
        name: &str,
        parse: impl Fn(&str, usize) -> Result<T, StorageError>,
    ) -> Result<ParsedFile<T>, StorageError> {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ParsedFile {
                    path: path.to_path_buf(),
                    lines: Vec::new(),
                });
            }
            Err(error) => {
                return Err(error_from(&format!("read of {name}"), &error.to_string()));
            }
        };
        let mut complete_size = bytes.len();
        if complete_size > 0 && bytes[complete_size - 1] != 0x0a {
            complete_size = bytes
                .iter()
                .rposition(|byte| *byte == 0x0a)
                .map_or(0, |position| position + 1);
            let truncated = std::fs::OpenOptions::new()
                .write(true)
                .open(path)
                .and_then(|handle| handle.set_len(complete_size as u64));
            if let Err(error) = truncated {
                return Err(error_from(
                    &format!("torn-line truncation of {name}"),
                    &error.to_string(),
                ));
            }
        }
        let mut lines: Vec<ParsedLine<T>> = Vec::new();
        let mut start = 0usize;
        let mut line_number = 1usize;
        for end in 0..complete_size {
            if bytes[end] != 0x0a {
                continue;
            }
            let text = match std::str::from_utf8(&bytes[start..end]) {
                Ok(text) => text,
                Err(_) => {
                    return Err(corruption(format!(
                        "Invalid UTF-8 in complete {name} line {line_number}"
                    )));
                }
            };
            lines.push(ParsedLine {
                value: parse(text, line_number)?,
                start,
            });
            start = end + 1;
            line_number += 1;
        }
        Ok(ParsedFile {
            path: path.to_path_buf(),
            lines,
        })
    }
}

/// `addSidecar` (`storage.ts:405-417`): first-touch file registration keeps
/// the upstream insertion order.
fn push_sidecar(
    files: &mut Vec<String>,
    records: &mut HashMap<String, Vec<SidecarRecord>>,
    file: &str,
    payload: SidecarPayload,
    seq: i64,
    ordinal: i64,
) -> i64 {
    if !records.contains_key(file) {
        files.push(file.to_string());
        records.insert(file.to_string(), Vec::new());
    }
    records.get_mut(file).unwrap().push(SidecarRecord {
        format: FORMAT_VERSION,
        r#type: String::from("record"),
        seq,
        ordinal,
        payload,
    });
    ordinal
}

/// Map a table write to its main-log operation (identity for direct writes).
fn main_operation_of_write(write: &StorageWrite) -> MainOperation {
    match write {
        StorageWrite::Conversation { value } => MainOperation::Conversation {
            value: value.clone(),
        },
        StorageWrite::Entry { value } => MainOperation::Entry {
            value: value.clone(),
        },
        StorageWrite::Submission { value } => MainOperation::Submission {
            value: value.clone(),
        },
        StorageWrite::Task { value } => MainOperation::Task {
            value: value.clone(),
        },
        StorageWrite::DocumentRetire { id } => MainOperation::DocumentRetire { id: *id },
        _ => unreachable!("encode_commit only maps direct main-log writes"),
    }
}

fn set_pair(pairs: &mut Vec<(i64, bool)>, id: i64, live: bool) {
    if let Some(pair) = pairs.iter_mut().find(|(existing, _)| *existing == id) {
        pair.1 = live;
    } else {
        pairs.push((id, live));
    }
}

/// `parseJson(text, description)` (`storage.ts:88-96`).
fn parse_raw(text: &str, description: &str) -> Result<Value, StorageError> {
    serde_json::from_str(text).map_err(|_| corruption(format!("Malformed complete {description}")))
}

/// `parseMainMarker` (`storage.ts:155-181`).
fn parse_main_marker(text: &str, line: usize) -> Result<ParsedMainMarker, StorageError> {
    let description = format!("{MAIN_FILE} line {line}");
    let value = parse_raw(text, &description)?;
    if !is_object(&value)
        || value.get("format") != Some(&Value::from(FORMAT_VERSION))
        || value.get("type").and_then(Value::as_str) != Some("commit")
        || value
            .get("seq")
            .and_then(safe_int)
            .is_none_or(|seq| seq < 1)
        || !value.get("writes").is_some_and(Value::is_array)
    {
        return Err(corruption(format!(
            "Invalid commit marker in {description}"
        )));
    }
    let mut writes: Vec<Value> = Vec::new();
    for write in value["writes"].as_array().unwrap() {
        writes.push(validate_main_operation(write, &description)?);
    }
    Ok(ParsedMainMarker {
        seq: value["seq"].as_i64().unwrap(),
        writes,
    })
}

/// `validateMainOperation` (`storage.ts:106-153`): keep the raw object after
/// shape validation.
fn validate_main_operation(value: &Value, description: &str) -> Result<Value, StorageError> {
    let invalid = |message: String| corruption(format!("{message} in {description}"));
    let Some(type_text) = value.get("type").and_then(Value::as_str) else {
        return Err(invalid(String::from("Invalid write")));
    };
    let value_field = || value.get("value");
    match type_text {
        "conversation" | "entry" | "submission" => {
            let ok = value_field().is_some_and(is_object)
                && value_field()
                    .unwrap()
                    .get("id")
                    .and_then(safe_int)
                    .is_some();
            if !ok {
                return Err(invalid(format!("Invalid {type_text} write")));
            }
        }
        "task" => {
            let task_value = value_field().unwrap_or(&Value::Null);
            let ok = is_object(task_value)
                && task_value.get("id").and_then(safe_int).is_some()
                && task_value.get("state").is_some_and(is_object)
                && task_value["state"].get("status").and_then(Value::as_str) == Some("terminal");
            if !ok {
                return Err(invalid(String::from("Invalid terminal task write")));
            }
        }
        "document.retire" => {
            if value.get("id").and_then(safe_int).is_none() {
                return Err(invalid(String::from("Invalid document retirement")));
            }
        }
        "task.sidecar" => {
            let ordinal = value.get("ordinal").and_then(safe_int);
            if value.get("id").and_then(safe_int).is_none()
                || ordinal.is_none_or(|ordinal| ordinal < 0)
            {
                return Err(invalid(String::from("Invalid task sidecar write")));
            }
        }
        "document.create" => {
            let record = value.get("record");
            let ordinal = value.get("ordinal").and_then(safe_int);
            let ok = record.is_some_and(is_object)
                && record.unwrap().get("id").and_then(safe_int).is_some()
                && ordinal.is_some_and(|ordinal| ordinal >= 0);
            if !ok {
                return Err(invalid(String::from("Invalid document creation")));
            }
        }
        "document.change" => {
            let ordinal = value.get("ordinal").and_then(safe_int);
            if value.get("id").and_then(safe_int).is_none()
                || ordinal.is_none_or(|ordinal| ordinal < 0)
            {
                return Err(invalid(String::from("Invalid document change")));
            }
        }
        _ => return Err(invalid(String::from("Unknown write type"))),
    }
    Ok(value.clone())
}

/// `parseSidecarRecord` (`storage.ts:213-277`).
fn parse_sidecar_record(
    text: &str,
    file: &str,
    line: usize,
) -> Result<ParsedSidecarRecord, StorageError> {
    let description = format!("{file} line {line}");
    let value = parse_raw(text, &description)?;
    let invalid = |message: &str| corruption(format!("{message} in {description}"));
    if !is_object(&value)
        || value.get("format") != Some(&Value::from(FORMAT_VERSION))
        || value.get("type").and_then(Value::as_str) != Some("record")
        || value
            .get("seq")
            .and_then(safe_int)
            .is_none_or(|seq| seq < 1)
        || value
            .get("ordinal")
            .and_then(safe_int)
            .is_none_or(|ordinal| ordinal < 0)
        || !value.get("payload").is_some_and(is_object)
        || value["payload"]
            .get("type")
            .and_then(Value::as_str)
            .is_none()
    {
        return Err(invalid("Invalid sidecar record"));
    }
    let payload = &value["payload"];
    match payload.get("type").and_then(Value::as_str) {
        Some("task") => {
            let task_value = payload.get("value");
            let ok = task_value.is_some_and(is_object)
                && task_value.unwrap().get("id").and_then(safe_int).is_some()
                && task_value.unwrap().get("state").is_some_and(is_object)
                && task_value.unwrap()["state"]
                    .get("status")
                    .and_then(Value::as_str)
                    != Some("terminal");
            if !ok {
                return Err(invalid("Invalid live task record"));
            }
        }
        Some("document") => {
            if payload.get("id").and_then(safe_int).is_none() {
                return Err(invalid("Invalid document record"));
            }
            validate_document_content(&payload["content"], &description)?;
        }
        _ => return Err(invalid("Unknown sidecar record type")),
    }
    Ok(ParsedSidecarRecord {
        seq: value["seq"].as_i64().unwrap(),
        ordinal: value["ordinal"].as_i64().unwrap(),
        payload: payload.clone(),
    })
}

/// `validateDocumentContent` (`storage.ts:183-202`).
fn validate_document_content(value: &Value, description: &str) -> Result<(), StorageError> {
    let invalid = |message: &str| corruption(format!("{message} in {description}"));
    if !is_object(value)
        || value
            .get("version")
            .and_then(safe_int)
            .is_none_or(|version| version < 1)
    {
        return Err(invalid("Invalid document content"));
    }
    if value.get("kind").and_then(Value::as_str) == Some("base")
        && value.get("value").is_some_and(is_object)
    {
        return Ok(());
    }
    if value.get("kind").and_then(Value::as_str) == Some("delta")
        && value.get("ops").is_some_and(Value::is_array)
    {
        return Ok(());
    }
    Err(invalid("Invalid document content"))
}

impl Storage for JsonlStorage {
    fn commit(&self, writes: &[StorageWrite], _context: &Context) -> Result<i64, StorageError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.assert_usable(&state)?;
        let mut prepared = state.memory.prepare_commit(writes, None)?;
        let encoded = self.encode_commit(prepared.seq, &prepared.writes);
        let reclamations = self.plan_reclamations_with_state(&state, &prepared.writes, &encoded);

        // Append every sidecar, flush under `fsync`, then the main marker.
        for (file, content) in &encoded.sidecars {
            let path = self.directory.join(file);
            let appended = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .and_then(|mut handle| handle.write_all(content.as_bytes()))
                .map_err(|error| error_from(&format!("append to {file}"), &error.to_string()));
            if let Err(error) = appended {
                return Err(self.poison(&mut state, error));
            }
        }
        if self.fsync {
            for (file, _) in &encoded.sidecars {
                let path = self.directory.join(file);
                let flushed = std::fs::OpenOptions::new()
                    .append(true)
                    .open(&path)
                    .and_then(|handle| handle.sync_all())
                    .map_err(|error| error_from(&format!("flush of {file}"), &error.to_string()));
                if let Err(error) = flushed {
                    return Err(self.poison(&mut state, error));
                }
            }
        }
        let marker = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.main_path)
            .and_then(|mut handle| handle.write_all(encoded.marker.as_bytes()))
            .map_err(|error| error_from(&format!("append to {MAIN_FILE}"), &error.to_string()));
        if let Err(error) = marker {
            return Err(self.poison(&mut state, error));
        }
        let seq = state.memory.apply_prepared(&mut prepared)?;
        adopt_sidecar_state(&mut state, writes);
        drop(state);
        self.reclaim_sidecars(&reclamations);
        Ok(seq)
    }

    fn mint_id(&self) -> Result<i64, StorageError> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.assert_usable(&state)?;
        state.memory.mint_id()
    }

    fn conversation(
        &self,
        id: i64,
        context: &Context,
    ) -> Result<Option<ConversationRecord>, StorageError> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.assert_usable(&state)?;
        state.memory.conversation(id, context)
    }

    fn scan_conversations(
        &self,
        query: ConversationQuery,
        limit: usize,
        cursor: Option<&Cursor>,
        context: &Context,
    ) -> Result<Page<ConversationRecord>, StorageError> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.assert_usable(&state)?;
        state
            .memory
            .scan_conversations(query, limit, cursor, context)
    }

    fn entry(
        &self,
        id: i64,
        context: &Context,
    ) -> Result<Option<EntryWithCommitSeq>, StorageError> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.assert_usable(&state)?;
        state.memory.entry(id, context)
    }

    fn entry_visible(
        &self,
        conversation_id: i64,
        id: i64,
        context: &Context,
    ) -> Result<Option<EntryWithCommitSeq>, StorageError> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.assert_usable(&state)?;
        state.memory.entry_visible(conversation_id, id, context)
    }

    fn find_latest_head_marker(
        &self,
        conversation_id: i64,
        at_or_before_entry_id: Option<i64>,
        context: &Context,
    ) -> Result<Option<EntryRecord>, StorageError> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.assert_usable(&state)?;
        state
            .memory
            .find_latest_head_marker(conversation_id, at_or_before_entry_id, context)
    }

    fn scan_entries(
        &self,
        query: EntryQuery,
        limit: usize,
        cursor: Option<&Cursor>,
        context: &Context,
    ) -> Result<Page<EntryRecord>, StorageError> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.assert_usable(&state)?;
        state.memory.scan_entries(query, limit, cursor, context)
    }

    fn task(&self, id: TaskId, context: &Context) -> Result<Option<TaskRecord>, StorageError> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.assert_usable(&state)?;
        state.memory.task(id, context)
    }

    fn scan_tasks(
        &self,
        query: TaskQuery,
        limit: usize,
        cursor: Option<&Cursor>,
        context: &Context,
    ) -> Result<Page<TaskRecord>, StorageError> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.assert_usable(&state)?;
        state.memory.scan_tasks(query, limit, cursor, context)
    }

    fn submission(
        &self,
        id: i64,
        context: &Context,
    ) -> Result<Option<SubmissionRecord>, StorageError> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.assert_usable(&state)?;
        state.memory.submission(id, context)
    }

    fn scan_submissions(
        &self,
        query: SubmissionQuery,
        limit: usize,
        cursor: Option<&Cursor>,
        context: &Context,
    ) -> Result<Page<SubmissionRecord>, StorageError> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.assert_usable(&state)?;
        state.memory.scan_submissions(query, limit, cursor, context)
    }

    fn submission_by_request(
        &self,
        conversation_id: i64,
        request_id: &str,
        context: &Context,
    ) -> Result<Option<SubmissionRecord>, StorageError> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.assert_usable(&state)?;
        state
            .memory
            .submission_by_request(conversation_id, request_id, context)
    }

    fn find_document(
        &self,
        address: &DocumentAddress,
        at: DocumentPoint,
        context: &Context,
    ) -> Result<Option<DocumentRecord>, StorageError> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.assert_usable(&state)?;
        state.memory.find_document(address, at, context)
    }

    fn document(
        &self,
        id: i64,
        at: DocumentPoint,
        context: &Context,
    ) -> Result<Option<StoredDocument>, StorageError> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.assert_usable(&state)?;
        state.memory.document(id, at, context)
    }

    fn scan_documents(
        &self,
        query: DocumentQuery,
        limit: usize,
        cursor: Option<&Cursor>,
        context: &Context,
    ) -> Result<Page<DocumentRecord>, StorageError> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.assert_usable(&state)?;
        state.memory.scan_documents(query, limit, cursor, context)
    }

    fn close(&self, context: &Context) -> Result<(), StorageError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.closed {
            return Ok(());
        }
        state.closed = true;
        state.memory.close(context)
    }
}

/// `adoptSidecarState` (`storage.ts:500-513`).
fn adopt_sidecar_state(state: &mut JsonlState, writes: &[StorageWrite]) {
    for write in writes {
        match write {
            StorageWrite::DocumentCreate { record, .. } => {
                if is_current_only(record) && !state.current_only_documents.contains(&record.id) {
                    state.current_only_documents.push(record.id);
                }
            }
            StorageWrite::Task { value } => {
                if value.status() == TaskStatus::Terminal {
                    state.live_task_sidecars.retain(|id| *id != value.id);
                } else if !state.live_task_sidecars.contains(&value.id) {
                    state.live_task_sidecars.push(value.id);
                }
            }
            _ => {}
        }
    }
}
