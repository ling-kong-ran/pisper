//! Port of `packages/agent/src/harness/session/in-memory-storage-state.ts`
//! (349 lines): the complete materialized session state shared by
//! [`super::memory::MemoryStorage`] and [`super::jsonl::storage::JsonlStorage`]
//! — entry tree, current values, lists, the usage ledger, and stats, with
//! prepare/validate/apply commit staging and the in-memory fork plan.
//!
//! Disclosed substitutions:
//! - Upstream `Map` preserves insertion order; the port keeps `entries_by_seq`
//!   for sequence-ordered scans and sorts value scans by key (upstream sorts
//!   them the same way), so observation order matches everywhere the oracle
//!   tests pin it. `get_value`-style lookups are order-free hashes.
//! - Upstream `compareKeys` compares UTF-16 code points; `str`'s `Ord`
//!   compares UTF-8 bytes, which is the same code-point ordering.
//! - `addUsage` is the shared harness util (upstream `harness/utils/usage.ts`,
//!   ported with the compaction module).

use std::collections::{HashMap, HashSet};

use crate::ai::types::primitives::Usage;

use super::commit::{
    prepare_storage_commit, validate_committed_writes, CommitValidationState, CommittedWrite,
    PreparedCommit,
};
use super::fork_policy::{
    idle_lane_state_value, project_value_set, select_branch_fork, BranchForkSource,
    ForkCurrentStatePlan,
};
use super::types::{
    AscDescOrder, BranchScanOrder, Entry, EntryScan, EntryStructure, ForkOptions, SessionStats,
    StorageBranchScan, UsageRow, UsageScan, Write,
};
use super::values::{
    branch_tip, lane_config, lane_state, list, resolve_list_read_options, value, ListElement,
    ListReadOptions, StoredValue, ValueAddress,
};

/// Upstream `MemoryForkPlan` (`in-memory-storage-state.ts:53-55`).
#[derive(Debug, Clone)]
enum MemoryForkPlan {
    Tree,
    Branch {
        branch: String,
        destination_tip: Option<String>,
        entry_ids: HashSet<String>,
    },
}

/// Upstream `physicalKey(namespace, key)` (`in-memory-storage-state.ts:57-59`).
fn physical_key(namespace: &str, key: &str) -> String {
    format!("{namespace}\0{key}")
}

/// Upstream `compareKeys` (`in-memory-storage-state.ts:61-70`): UTF-8 `str`
/// ordering equals the upstream code-point ordering (see module docs).
fn compare_keys(left: &str, right: &str) -> std::cmp::Ordering {
    left.cmp(right)
}

/// Upstream `StoredListSnapshot` (`in-memory-storage-state.ts:48-51`).
#[derive(Debug, Clone)]
struct StoredListSnapshot {
    #[allow(dead_code)]
    address: ValueAddress,
    elements: Vec<ListElement>,
}

/// Upstream `InMemoryStorageState` (`in-memory-storage-state.ts:78-349`):
/// intentionally unsuitable for database backends, like upstream.
#[derive(Debug)]
pub struct InMemoryStorageState {
    entries: HashMap<String, Entry>,
    entries_by_seq: Vec<Entry>,
    scalar_values: HashMap<String, StoredValue>,
    list_values: HashMap<String, StoredListSnapshot>,
    usage: HashMap<String, UsageRow>,
    stats: SessionStats,
    next_seq: i64,
}

impl Default for InMemoryStorageState {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryStorageState {
    pub fn new() -> Self {
        InMemoryStorageState {
            entries: HashMap::new(),
            entries_by_seq: Vec::new(),
            scalar_values: HashMap::new(),
            list_values: HashMap::new(),
            usage: HashMap::new(),
            stats: SessionStats {
                message_count: 0,
                usage: Usage::default(),
            },
            next_seq: 1,
        }
    }

    /// Upstream `prepareCommit(writes, timestamp)` (`:97-101`).
    pub fn prepare_commit(
        &mut self,
        writes: Vec<Write>,
        timestamp: i64,
    ) -> anyhow::Result<PreparedCommit> {
        let next_seq = self.next_seq;
        let prepared = prepare_storage_commit(writes, next_seq, timestamp);
        self.validate_committed(&prepared.writes)?;
        Ok(prepared)
    }

    /// Upstream `validateCommitted(writes)` (`:103-108`).
    pub fn validate_committed(&self, writes: &[CommittedWrite]) -> anyhow::Result<()> {
        let next_seq = self.next_seq;
        validate_committed_writes(writes, next_seq, &ValidationState { state: self })
    }

    /// Upstream `applyValidated(writes)` (`:110-139`): apply writes already
    /// accepted by [`Self::validate_committed`] and return the post-apply
    /// totals.
    pub fn apply_validated(&mut self, writes: Vec<CommittedWrite>) -> SessionStats {
        for write in writes {
            let seq = write.seq();
            match write {
                CommittedWrite::Entry { entry } => {
                    let entry = *entry;
                    let is_message = entry.entry_type() == super::types::EntryType::Message;
                    self.entries.insert(entry.id().to_string(), entry.clone());
                    self.entries_by_seq.push(entry);
                    if is_message {
                        self.stats.message_count += 1;
                    }
                }
                CommittedWrite::Usage { row } => {
                    self.stats.usage = crate::agent_core::harness::compaction::utils::add_usage(
                        self.stats.usage,
                        row.usage,
                    );
                    self.usage.insert(row.id.clone(), row);
                }
                CommittedWrite::Value(value_write) => match value_write {
                    super::commit::CommittedValueWrite::Set {
                        seq,
                        namespace,
                        key,
                        value,
                    } => {
                        self.apply_value_set(seq, &namespace, &key, value);
                    }
                    super::commit::CommittedValueWrite::Delete {
                        seq: _,
                        namespace,
                        key,
                    } => {
                        self.scalar_values.remove(&physical_key(&namespace, &key));
                    }
                },
                CommittedWrite::List(list_write) => match list_write {
                    super::commit::CommittedListWrite::Append {
                        seq,
                        namespace,
                        key,
                        value,
                    } => {
                        self.apply_list_append(seq, &namespace, &key, value);
                    }
                    super::commit::CommittedListWrite::Delete {
                        seq: _,
                        namespace,
                        key,
                    } => {
                        self.list_values.remove(&physical_key(&namespace, &key));
                    }
                },
            }
            self.next_seq = seq + 1;
        }
        self.stats.clone()
    }

    /// Upstream `applyValueSetOrListAppend` (`:213-234`) value half.
    fn apply_value_set(&mut self, seq: i64, namespace: &str, key: &str, stored: serde_json::Value) {
        self.scalar_values.insert(
            physical_key(namespace, key),
            StoredValue {
                address: value(namespace, key),
                value: stored,
                seq,
            },
        );
    }

    /// Upstream `applyValueSetOrListAppend` (`:213-234`) list half.
    fn apply_list_append(
        &mut self,
        seq: i64,
        namespace: &str,
        key: &str,
        element_value: serde_json::Value,
    ) {
        let element = ListElement {
            seq,
            value: element_value,
        };
        match self.list_values.get_mut(&physical_key(namespace, key)) {
            Some(stored) => stored.elements.push(element),
            None => {
                self.list_values.insert(
                    physical_key(namespace, key),
                    StoredListSnapshot {
                        address: list(namespace, key),
                        elements: vec![element],
                    },
                );
            }
        }
    }

    /// Upstream `createFork(options)` (`:141-193`).
    pub fn create_fork(&self, options: &ForkOptions) -> anyhow::Result<InMemoryStorageState> {
        let plan = self.select_fork_plan(options)?;

        let is_entry_copied = |entry_id: &str| match &plan {
            MemoryForkPlan::Tree => true,
            MemoryForkPlan::Branch { entry_ids, .. } => entry_ids.contains(entry_id),
        };
        let mut destination = InMemoryStorageState::new();
        let mut message_count = 0u64;
        for entry in &self.entries_by_seq {
            if !is_entry_copied(entry.id()) {
                continue;
            }
            destination
                .entries
                .insert(entry.id().to_string(), entry.clone());
            destination.entries_by_seq.push(entry.clone());
            if entry.entry_type() == super::types::EntryType::Message {
                message_count += 1;
            }
        }
        destination.stats.message_count = message_count;

        for stored in self.scalar_values.values() {
            let projected = project_value_set(
                stored.seq,
                &stored.address.namespace,
                &stored.address.key,
                stored.value.clone(),
                &plan_fork_plan(&plan),
                &is_entry_copied,
            )?;
            if let Some(super::commit::CommittedValueWrite::Set {
                seq,
                namespace,
                key,
                value,
            }) = projected
            {
                destination.apply_value_set(seq, &namespace, &key, value);
            }
        }

        for stored in self.list_values.values() {
            for element in &stored.elements {
                let projected = project_value_set(
                    element.seq,
                    &stored.address.namespace,
                    &stored.address.key,
                    element.value.clone(),
                    &plan_fork_plan(&plan),
                    &is_entry_copied,
                )?;
                if let Some(super::commit::CommittedValueWrite::Set {
                    seq,
                    namespace,
                    key,
                    value,
                }) = projected
                {
                    destination.apply_list_append(seq, &namespace, &key, value);
                }
            }
        }
        destination.next_seq = self.next_seq;
        Ok(destination)
    }

    /// Upstream `selectForkPlan(options)` (`:195-211`).
    fn select_fork_plan(&self, options: &ForkOptions) -> anyhow::Result<MemoryForkPlan> {
        match options {
            ForkOptions::Tree { .. } => Ok(MemoryForkPlan::Tree),
            ForkOptions::Branch { branch, .. } => {
                let mut entry_ids = HashSet::new();
                // Tip shape: `None` = unknown branch (upstream `undefined`),
                // `Some(None)` = null tip, `Some(Some(id))` = tip id.
                let tip = match self.get_value(&branch_tip(branch)) {
                    None => None,
                    Some(stored) => Some(match stored.value {
                        serde_json::Value::String(id) => Some(id),
                        _ => None,
                    }),
                };
                let (branch_name, destination_tip) = select_branch_fork(
                    options,
                    BranchForkSource {
                        tip,
                        get_parent: |entry_id| {
                            // `Some(None)` = null root parent, `None` =
                            // missing entry (upstream `undefined`).
                            self.entries
                                .get(entry_id)
                                .map(|entry| entry.parent_id().map(str::to_string))
                        },
                        select_entry: &mut |entry_id| {
                            entry_ids.insert(entry_id.to_string());
                        },
                    },
                )?;
                if self.get_value(&lane_config(branch)).is_none()
                    || self.get_value(&lane_state(branch)).is_none()
                {
                    anyhow::bail!("Source branch {branch:?} is not a configured AgentLane");
                }
                Ok(MemoryForkPlan::Branch {
                    branch: branch_name,
                    destination_tip,
                    entry_ids,
                })
            }
        }
    }

    /// Upstream `advanceNextSeq(nextSeq)` (`:236-241`).
    pub fn advance_next_seq(&mut self, next_seq: i64) -> anyhow::Result<()> {
        if next_seq < 1 {
            anyhow::bail!("Invalid storage sequence high-water mark: {next_seq}");
        }
        self.next_seq = self.next_seq.max(next_seq);
        Ok(())
    }

    /// Upstream `getEntries(ids)` (`:243-250`).
    pub fn get_entries(&self, ids: &[String]) -> HashMap<String, Entry> {
        let mut found = HashMap::new();
        for id in ids {
            if let Some(entry) = self.entries.get(id) {
                found.insert(id.clone(), entry.clone());
            }
        }
        found
    }

    /// Upstream `getValue(address)` (`:252-254`).
    pub fn get_value(&self, address: &ValueAddress) -> Option<StoredValue> {
        self.scalar_values
            .get(&physical_key(&address.namespace, &address.key))
            .cloned()
    }

    /// Upstream `scanValues(prefix)` (`:256-260`).
    pub fn scan_values(&self, prefix: &ValueAddress) -> Vec<StoredValue> {
        let mut matched: Vec<StoredValue> = self
            .scalar_values
            .values()
            .filter(|stored| {
                stored.address.namespace == prefix.namespace
                    && stored.address.key.starts_with(&prefix.key)
            })
            .cloned()
            .collect();
        matched.sort_by(|left, right| compare_keys(&left.address.key, &right.address.key));
        matched
    }

    /// Upstream `readList(address, options)` (`:262-271`).
    pub fn read_list(
        &self,
        address: &ValueAddress,
        options: Option<&ListReadOptions>,
    ) -> anyhow::Result<Vec<ListElement>> {
        let resolved = resolve_list_read_options(options)?;
        let empty: Vec<ListElement> = Vec::new();
        let elements = self
            .list_values
            .get(&physical_key(&address.namespace, &address.key))
            .map(|stored| &stored.elements[..])
            .unwrap_or(&empty[..]);
        let filtered: Vec<ListElement> = elements
            .iter()
            .filter(|element| match resolved.cursor {
                None => true,
                Some(cursor) => match resolved.order {
                    AscDescOrder::Asc => element.seq > cursor.seq,
                    AscDescOrder::Desc => element.seq < cursor.seq,
                },
            })
            .cloned()
            .collect();
        let mut ordered = filtered;
        if resolved.order == AscDescOrder::Desc {
            ordered.reverse();
        }
        ordered.truncate(resolved.limit as usize);
        Ok(ordered)
    }

    /// Upstream `scanBranch(query)` (`:273-301`).
    pub fn scan_branch(&self, query: &StorageBranchScan) -> anyhow::Result<Vec<Entry>> {
        let start = self
            .entries
            .get(&query.start)
            .ok_or_else(|| anyhow::anyhow!("Unknown branch start: {}", query.start))?;

        let mut path: Vec<Entry> = Vec::new();
        let mut current: Option<&Entry> = Some(start);
        while let Some(entry) = current {
            path.push(entry.clone());
            let Some(parent_id) = entry.parent_id() else {
                break;
            };
            current = Some(
                self.entries
                    .get(parent_id)
                    .ok_or_else(|| anyhow::anyhow!("Corrupt branch: missing parent"))?,
            );
        }
        if query.order == Some(BranchScanOrder::OldestFirst) {
            path.reverse();
        }

        let mut stopped: Vec<Entry> = Vec::new();
        for candidate in path {
            let stop_after = query.stop_at_id.as_deref() == Some(candidate.id())
                || query.stop_at_type == Some(candidate.entry_type());
            stopped.push(candidate);
            if stop_after {
                break;
            }
        }
        let filtered: Vec<Entry> = stopped
            .into_iter()
            .filter(|candidate| {
                query.scan_type.is_none() || query.scan_type == Some(candidate.entry_type())
            })
            .filter(|candidate| {
                query
                    .custom_type
                    .as_deref()
                    .is_none_or(|custom_type| candidate.custom_type() == Some(custom_type))
            })
            .filter(|candidate| match query.cursor {
                None => true,
                Some(cursor) => match query.order {
                    Some(BranchScanOrder::OldestFirst) => candidate.seq() > cursor.seq,
                    _ => candidate.seq() < cursor.seq,
                },
            })
            .collect();
        Ok(match query.limit {
            None => filtered,
            Some(limit) => filtered.into_iter().take(limit as usize).collect(),
        })
    }

    /// Upstream `scanBranchStructure(query)` (`:303-312`).
    pub fn scan_branch_structure(
        &self,
        query: &StorageBranchScan,
    ) -> anyhow::Result<Vec<EntryStructure>> {
        Ok(self
            .scan_branch(query)?
            .into_iter()
            .map(|entry| EntryStructure {
                id: entry.id().to_string(),
                parent_id: entry.parent_id().map(str::to_string),
                seq: entry.seq(),
                timestamp: entry.timestamp(),
                entry_type: entry.entry_type(),
                custom_type: entry.custom_type().map(str::to_string),
            })
            .collect())
    }

    /// Upstream `scanEntries(query)` (`:314-332`).
    pub fn scan_entries(&self, query: &EntryScan) -> Vec<Entry> {
        let limit = query.limit.unwrap_or(u64::MAX) as usize;
        let mut entries: Vec<Entry> = Vec::new();
        let descending = query.order == Some(AscDescOrder::Desc);
        let mut index = if descending {
            self.entries_by_seq.len() as i64 - 1
        } else {
            0
        };
        while index >= 0 && (index as usize) < self.entries_by_seq.len() && entries.len() < limit {
            let entry = &self.entries_by_seq[index as usize];
            if (query.scan_type.is_none() || query.scan_type == Some(entry.entry_type()))
                && query
                    .custom_type
                    .as_deref()
                    .is_none_or(|custom_type| entry.custom_type() == Some(custom_type))
                && query
                    .from_seq
                    .is_none_or(|from_seq| entry.seq() >= from_seq)
                && query.to_seq.is_none_or(|to_seq| entry.seq() <= to_seq)
            {
                entries.push(entry.clone());
            }
            index += if descending { -1 } else { 1 };
        }
        entries
    }

    /// Upstream `scanUsage(query)` (`:334-340`).
    pub fn scan_usage(&self, query: &UsageScan) -> Vec<UsageRow> {
        let mut rows: Vec<UsageRow> = self
            .usage
            .values()
            .filter(|row| query.from_seq.is_none_or(|from_seq| row.seq >= from_seq))
            .filter(|row| query.to_seq.is_none_or(|to_seq| row.seq <= to_seq))
            .cloned()
            .collect();
        rows.sort_by(|left, right| {
            if query.order == Some(AscDescOrder::Desc) {
                right.seq.cmp(&left.seq)
            } else {
                left.seq.cmp(&right.seq)
            }
        });
        match query.limit {
            None => rows,
            Some(limit) => rows.into_iter().take(limit as usize).collect(),
        }
    }

    /// Upstream `getStats()` (`:342-344`).
    pub fn get_stats(&self) -> SessionStats {
        self.stats.clone()
    }

    /// Upstream `getNextSeq()` (`:346-348`).
    pub fn get_next_seq(&self) -> i64 {
        self.next_seq
    }

    /// The idle lane-state literal the fork writes (upstream
    /// `fork-policy.ts:57` via `projectForkCurrentStateWrite`).
    pub fn idle_lane_state() -> serde_json::Value {
        idle_lane_state_value()
    }
}

/// Adapt the internal fork plan to the policy-layer plan.
fn plan_fork_plan(plan: &MemoryForkPlan) -> ForkCurrentStatePlan {
    match plan {
        MemoryForkPlan::Tree => ForkCurrentStatePlan::Tree,
        MemoryForkPlan::Branch {
            branch,
            destination_tip,
            ..
        } => ForkCurrentStatePlan::Branch {
            branch: branch.clone(),
            destination_tip: destination_tip.clone(),
        },
    }
}

/// Upstream's inline `CommitValidationState` implementation
/// (`in-memory-storage-state.ts:104-108`).
struct ValidationState<'a> {
    state: &'a InMemoryStorageState,
}

impl CommitValidationState for ValidationState<'_> {
    fn has_entry_or_usage_id(&self, id: &str) -> bool {
        self.state.entries.contains_key(id) || self.state.usage.contains_key(id)
    }

    fn has_entry_id(&self, id: &str) -> bool {
        self.state.entries.contains_key(id)
    }
}

#[cfg(test)]
mod tests;
