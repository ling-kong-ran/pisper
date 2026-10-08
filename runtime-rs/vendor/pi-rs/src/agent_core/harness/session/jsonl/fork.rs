//! Port of `packages/agent/src/harness/session/jsonl/fork.ts` (328 lines):
//! the streaming JSONL fork — index the source file (or reuse the legacy v3
//! scan), validate the requested fork against it, and stream selected
//! entries and current state into an atomically published format-4
//! destination without modifying the source.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::agent_core::chord_support::Context;
use crate::agent_core::harness::session::commit::{
    CommittedListWrite, CommittedValueWrite, CommittedWrite,
};
use crate::agent_core::harness::session::fork_policy::{
    project_fork_current_state_value, select_branch_fork, BranchForkSource, ForkCurrentStatePlan,
};
use crate::agent_core::harness::session::jsonl::io::{
    file_value, parse_jsonl_transaction, publish_jsonl, read_jsonl_header,
};
use crate::agent_core::harness::session::jsonl::legacy_v3::LegacyV3Source;
use crate::agent_core::harness::session::jsonl::types::{
    JsonlStorageHeader, JSONL_STORAGE_VERSION,
};
use crate::agent_core::harness::session::types::ForkOptions;
use crate::agent_core::harness::types::{FileSystem, TextLineReader};

/// Upstream `JsonlForkSourceMetadata` (`fork.ts:15-19`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonlForkSourceMetadata {
    pub id: String,
    pub cwd: String,
    pub path: String,
}

/// Upstream `physicalKey` (`fork.ts:21-23`).
fn physical_key(namespace: &str, key: &str) -> String {
    format!("{namespace}\0{key}")
}

/// Upstream `readJsonlForkHeader` (`fork.ts:25-42`).
async fn read_jsonl_fork_header(
    reader: &dyn TextLineReader,
    source: &JsonlForkSourceMetadata,
    context: Context,
) -> anyhow::Result<JsonlStorageHeader> {
    let parsed = read_jsonl_header(reader, &source.path, context).await?;
    let crate::agent_core::harness::session::jsonl::codec::JsonlParsedSessionHeader::V4 { header } =
        parsed
    else {
        anyhow::bail!(
            "Invalid JSONL storage {}: expected format 4 header",
            source.path
        );
    };
    if header.id != source.id || header.cwd != source.cwd {
        anyhow::bail!("Session identity does not match header: {}", source.id);
    }
    if header.storage_version != JSONL_STORAGE_VERSION {
        anyhow::bail!(
            "Session {} uses unsupported storage version {}",
            source.id,
            header.storage_version
        );
    }
    Ok(header)
}

/// Upstream `reachesForkBoundary` (`fork.ts:44-53`): whether the transaction
/// crosses (or errors crossing) the captured sequence boundary.
fn reaches_fork_boundary(
    writes: &[CommittedWrite],
    stop_before_seq: Option<i64>,
) -> anyhow::Result<bool> {
    let Some(stop_before_seq) = stop_before_seq else {
        return Ok(false);
    };
    if writes.is_empty() {
        return Ok(false);
    }
    let first = writes[0].seq();
    let last = writes[writes.len() - 1].seq();
    if first >= stop_before_seq {
        return Ok(true);
    }
    if last >= stop_before_seq {
        anyhow::bail!("JSONL transaction crosses fork sequence boundary {stop_before_seq}");
    }
    Ok(false)
}

/// Upstream `JsonlForkIndex` (`fork.ts:71-158`): the indexed source
/// metadata — entry parents, current-row sequences, lane inventory, and the
/// branch-scope selected entries.
#[derive(Debug, Default, Clone)]
struct JsonlForkIndex {
    current_scalar_seqs: HashMap<String, i64>,
    branch_tips: HashMap<String, Option<String>>,
    first_surviving_list_seqs: HashMap<String, i64>,
    entry_parents: HashMap<String, Option<String>>,
    copied_entry_ids: HashSet<String>,
    lane_configs: HashSet<String>,
    lane_states: HashSet<String>,
}

impl JsonlForkIndex {
    fn apply_entry(&mut self, id: &str, parent_id: Option<&str>) {
        self.entry_parents
            .insert(id.to_string(), parent_id.map(str::to_string));
    }

    fn apply_writes(&mut self, writes: &[CommittedWrite]) {
        for write in writes {
            match write {
                CommittedWrite::Entry { entry } => {
                    self.apply_entry(entry.id(), entry.parent_id());
                }
                CommittedWrite::Value(value_write) => {
                    let (namespace, key) = match value_write {
                        CommittedValueWrite::Set { namespace, key, .. }
                        | CommittedValueWrite::Delete { namespace, key, .. } => (namespace, key),
                    };
                    let physical = physical_key(namespace, key);
                    match value_write {
                        CommittedValueWrite::Set { seq, .. } => {
                            self.current_scalar_seqs.insert(physical, *seq);
                        }
                        CommittedValueWrite::Delete { .. } => {
                            self.current_scalar_seqs.remove(&physical);
                        }
                    }
                    self.apply_lane_value(namespace, key, value_write);
                }
                CommittedWrite::List(list_write) => {
                    let (namespace, key) = match list_write {
                        CommittedListWrite::Append { namespace, key, .. }
                        | CommittedListWrite::Delete { namespace, key, .. } => (namespace, key),
                    };
                    let physical = physical_key(namespace, key);
                    match list_write {
                        CommittedListWrite::Append { seq, .. } => {
                            self.first_surviving_list_seqs
                                .entry(physical)
                                .or_insert(*seq);
                        }
                        CommittedListWrite::Delete { .. } => {
                            self.first_surviving_list_seqs.remove(&physical);
                        }
                    }
                }
                CommittedWrite::Usage { .. } => {}
            }
        }
    }

    fn apply_lane_value(&mut self, namespace: &str, key: &str, write: &CommittedValueWrite) {
        let present = matches!(write, CommittedValueWrite::Set { .. });
        match namespace {
            "pi.branch.tip" => {
                if present {
                    let tip = match write {
                        CommittedValueWrite::Set { value, .. } => {
                            value.as_str().map(str::to_string)
                        }
                        _ => None,
                    };
                    self.branch_tips.insert(key.to_string(), tip);
                } else {
                    self.branch_tips.remove(key);
                }
            }
            "pi.lane.config" => {
                if present {
                    self.lane_configs.insert(key.to_string());
                } else {
                    self.lane_configs.remove(key);
                }
            }
            "pi.lane.state" => {
                if present {
                    self.lane_states.insert(key.to_string());
                } else {
                    self.lane_states.remove(key);
                }
            }
            _ => {}
        }
    }

    fn get_branch_tip(&self, branch: &str) -> Option<Option<String>> {
        self.branch_tips.get(branch).cloned()
    }

    fn has_complete_lane(&self, branch: &str) -> bool {
        self.lane_configs.contains(branch) && self.lane_states.contains(branch)
    }

    fn get_current_scalar_seq(&self, namespace: &str, key: &str) -> Option<i64> {
        self.current_scalar_seqs
            .get(&physical_key(namespace, key))
            .copied()
    }

    fn is_surviving_list_element(&self, namespace: &str, key: &str, seq: i64) -> bool {
        self.first_surviving_list_seqs
            .get(&physical_key(namespace, key))
            .is_some_and(|first_seq| seq >= *first_seq)
    }

    fn get_parent(&self, entry_id: &str) -> Option<Option<String>> {
        self.entry_parents.get(entry_id).cloned()
    }

    fn select_entry(&mut self, entry_id: &str) {
        self.copied_entry_ids.insert(entry_id.to_string());
    }

    fn is_entry_selected(&self, entry_id: &str) -> bool {
        self.copied_entry_ids.contains(entry_id)
    }
}

/// Upstream `selectJsonlFork` (`fork.ts:170-181`): validate the source lanes
/// and return the fork plan. Branch scope selects the destination tip and
/// its ancestors on the index.
fn select_jsonl_fork(
    index: &mut JsonlForkIndex,
    options: &ForkOptions,
) -> anyhow::Result<ForkCurrentStatePlan> {
    match options {
        ForkOptions::Tree { .. } => Ok(ForkCurrentStatePlan::Tree),
        ForkOptions::Branch { branch, .. } => {
            let tip = index.get_branch_tip(branch);
            let selected: std::cell::RefCell<Vec<String>> = std::cell::RefCell::new(Vec::new());
            let (branch, destination_tip) = select_branch_fork(
                options,
                BranchForkSource {
                    tip,
                    get_parent: |entry_id| index.get_parent(entry_id),
                    select_entry: &mut |entry_id| {
                        selected.borrow_mut().push(entry_id.to_string());
                    },
                },
            )?;
            for entry_id in selected.into_inner() {
                index.select_entry(&entry_id);
            }
            if !index.has_complete_lane(&branch) {
                anyhow::bail!("Source branch {branch:?} is not a configured AgentLane");
            }
            Ok(ForkCurrentStatePlan::Branch {
                branch,
                destination_tip,
            })
        }
    }
}

/// Upstream `projectJsonlForkWrite` (`fork.ts:185-205`): the copy-pass
/// filter and transformation for one source write.
fn project_jsonl_fork_write(
    write: &CommittedWrite,
    index: &JsonlForkIndex,
    plan: &ForkCurrentStatePlan,
    is_entry_copied: &dyn Fn(&str) -> bool,
) -> anyhow::Result<Option<CommittedWrite>> {
    match write {
        CommittedWrite::Entry { entry } => {
            if is_entry_copied(entry.id()) {
                Ok(Some(write.clone()))
            } else {
                Ok(None)
            }
        }
        CommittedWrite::Value(value_write) => {
            let CommittedValueWrite::Set {
                seq,
                namespace,
                key,
                value,
            } = value_write
            else {
                return Ok(None);
            };
            if index.get_current_scalar_seq(namespace, key) != Some(*seq) {
                return Ok(None);
            }
            Ok(project_fork_current_state_value(
                namespace,
                key,
                value.clone(),
                plan,
                is_entry_copied,
            )?
            .map(|(namespace, key, value)| {
                CommittedWrite::Value(CommittedValueWrite::Set {
                    seq: *seq,
                    namespace,
                    key,
                    value,
                })
            }))
        }
        CommittedWrite::List(list_write) => {
            let CommittedListWrite::Append {
                seq,
                namespace,
                key,
                value,
            } = list_write
            else {
                return Ok(None);
            };
            if !index.is_surviving_list_element(namespace, key, *seq) {
                return Ok(None);
            }
            Ok(project_fork_current_state_value(
                namespace,
                key,
                value.clone(),
                plan,
                is_entry_copied,
            )?
            .map(|(namespace, key, value)| {
                CommittedWrite::List(CommittedListWrite::Append {
                    seq: *seq,
                    namespace,
                    key,
                    value,
                })
            }))
        }
        CommittedWrite::Usage { .. } => Ok(None),
    }
}

/// Upstream `JsonlForkInput` (`fork.ts:208-211`).
#[derive(Clone)]
pub enum JsonlForkInput {
    /// An open format-4 source, stopped at the captured boundary.
    Open {
        metadata: JsonlForkSourceMetadata,
        next_seq: i64,
    },
    /// A closed format-4 source, scanned to EOF or a torn final line.
    Closed { metadata: JsonlForkSourceMetadata },
    /// An already-normalized legacy v3 source.
    LegacyV3 { normalized: Arc<LegacyV3Source> },
}

/// The complete transactions of a fork source pass.
async fn read_fork_transactions(
    reader: &dyn TextLineReader,
    path: &str,
    stop_before_seq: Option<i64>,
) -> anyhow::Result<Vec<Vec<CommittedWrite>>> {
    let mut transactions = Vec::new();
    loop {
        let line = file_value(
            reader.read_line(Context::background()).await,
            &format!("Failed to read JSONL fork source {path}"),
        )?;
        let Some(line) = line else { break };
        if !line.terminated {
            break;
        }
        let writes = parse_jsonl_transaction(&line.text)?;
        if reaches_fork_boundary(&writes, stop_before_seq)? {
            break;
        }
        transactions.push(writes);
    }
    Ok(transactions)
}

/// Upstream `indexForkInput` (`fork.ts:227-257`): build the index used to
/// select branch ancestry and identify current scalar/list writes.
async fn index_fork_input(
    input: &JsonlForkInput,
    file_system: &dyn FileSystem,
    context: Context,
) -> anyhow::Result<(JsonlForkIndex, i64)> {
    let mut index = JsonlForkIndex::default();
    if let JsonlForkInput::LegacyV3 { normalized } = input {
        for entry in normalized.entry_structures() {
            index.apply_entry(&entry.id, entry.parent_id.as_deref());
        }
        index.apply_writes(&normalized.values);
        return Ok((index, normalized.next_seq));
    }
    let metadata = match input {
        JsonlForkInput::Open { metadata, .. } | JsonlForkInput::Closed { metadata } => {
            metadata.clone()
        }
        JsonlForkInput::LegacyV3 { .. } => unreachable!("handled above"),
    };
    let reader = file_value(
        file_system
            .open_text_line_reader(&metadata.path, context.clone())
            .await,
        &format!("Failed to open JSONL fork source {}", metadata.path),
    )?;
    let result = async {
        let header = read_jsonl_fork_header(reader.as_ref(), &metadata, context.clone()).await?;
        let stop_before_seq = match input {
            JsonlForkInput::Open { next_seq, .. } => Some(*next_seq),
            _ => None,
        };
        let mut highest_complete_seq: i64 = 0;
        let transactions =
            read_fork_transactions(reader.as_ref(), &metadata.path, stop_before_seq).await?;
        for writes in transactions {
            if let Some(last) = writes.last() {
                highest_complete_seq = last.seq();
            }
            index.apply_writes(&writes);
        }
        let next_seq = match input {
            JsonlForkInput::Open { next_seq, .. } => *next_seq,
            _ => (header.next_seq.unwrap_or(1)).max(highest_complete_seq + 1),
        };
        Ok((index, next_seq))
    }
    .await;
    reader.close(context).await;
    result
}

/// Upstream `streamForkWrites` (`fork.ts:260-286`): yield source writes; the
/// caller owns final projection and filtering.
async fn stream_fork_writes(
    input: &JsonlForkInput,
    file_system: &dyn FileSystem,
    stop_before_seq: i64,
    is_entry_copied: &(dyn Fn(&str) -> bool + Send + Sync),
    context: Context,
) -> anyhow::Result<Vec<CommittedWrite>> {
    if let JsonlForkInput::LegacyV3 { normalized } = input {
        // V3 filters early to avoid rereading messages and reconstructing
        // unselected compaction tails.
        return normalized.writes(context, Some(&is_entry_copied)).await;
    }
    let metadata = match input {
        JsonlForkInput::Open { metadata, .. } | JsonlForkInput::Closed { metadata } => {
            metadata.clone()
        }
        JsonlForkInput::LegacyV3 { .. } => unreachable!("handled above"),
    };
    let reader = file_value(
        file_system
            .open_text_line_reader(&metadata.path, context.clone())
            .await,
        &format!("Failed to open JSONL fork source {}", metadata.path),
    )?;
    let result = async {
        read_jsonl_fork_header(reader.as_ref(), &metadata, context.clone()).await?;
        let transactions =
            read_fork_transactions(reader.as_ref(), &metadata.path, Some(stop_before_seq)).await?;
        Ok(transactions.into_iter().flatten().collect())
    }
    .await;
    reader.close(context).await;
    result
}

/// Upstream `runJsonlFork` (`fork.ts:295-328`): index the source, validate
/// the requested fork, and stream selected entries and current state into an
/// atomically published format-4 destination without modifying the source.
pub async fn run_jsonl_fork(options: JsonlForkOptions, context: Context) -> anyhow::Result<()> {
    let JsonlForkOptions {
        input,
        file_system,
        destination_path,
        destination_header,
        fork,
    } = options;
    let (mut index, next_seq) =
        index_fork_input(&input, file_system.as_ref(), context.clone()).await?;
    let mut fork = fork;
    if let (
        JsonlForkInput::LegacyV3 { normalized },
        ForkOptions::Branch {
            entry_id: Some(entry_id),
            ..
        },
    ) = (&input, &mut fork)
    {
        let translated = normalized.translate_fork_entry_id(entry_id)?;
        if let ForkOptions::Branch { entry_id, .. } = &mut fork {
            *entry_id = Some(translated);
        }
    }
    let plan = select_jsonl_fork(&mut index, &fork)?;
    let mut destination_header = destination_header;
    destination_header.next_seq = Some(next_seq);
    publish_jsonl(
        file_system.as_ref(),
        &destination_path,
        &destination_header,
        context.clone(),
        |append| {
            let input = input.clone();
            let plan = plan.clone();
            let index = index.clone();
            let file_system = file_system.clone();
            let context = context.clone();
            async move {
                let is_entry_copied = |entry_id: &str| match &plan {
                    ForkCurrentStatePlan::Tree => true,
                    ForkCurrentStatePlan::Branch { .. } => index.is_entry_selected(entry_id),
                };
                let source_writes = stream_fork_writes(
                    &input,
                    file_system.as_ref(),
                    next_seq,
                    &is_entry_copied,
                    context.clone(),
                )
                .await?;
                for write in source_writes {
                    let projected =
                        project_jsonl_fork_write(&write, &index, &plan, &is_entry_copied)?;
                    if let Some(projected) = projected {
                        append.append(std::slice::from_ref(&projected)).await?;
                    }
                }
                Ok(())
            }
        },
    )
    .await
}

/// Upstream `runJsonlFork` options object (`fork.ts:296-303`).
pub struct JsonlForkOptions {
    pub input: JsonlForkInput,
    pub file_system: Arc<dyn FileSystem>,
    pub destination_path: String,
    pub destination_header: JsonlStorageHeader,
    pub fork: ForkOptions,
}

/// The idle lane-state literal used by fork projection (re-exported for the
/// storage-state fork).
pub use crate::agent_core::harness::session::fork_policy::idle_lane_state_value as idle_lane_state;

#[cfg(test)]
mod tests;
