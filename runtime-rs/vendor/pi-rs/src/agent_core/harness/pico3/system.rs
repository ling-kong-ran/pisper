//! Port of `packages/agent/src/harness/pico3/system.ts` (376 lines): the
//! managed system-instructions layer — sections (§12.1), the managed entry
//! (§12.3), the canonical-state fold (§12.4), the draft (§12.2), and
//! preparation (§12.5).
//!
//! Disclosed substitutions:
//! - Upstream `Canonical` is a JS `Map` whose insertion order is the
//!   canonical section order. The repo's `serde_json` build does not
//!   preserve object insertion order, so the port carries an explicit
//!   insertion-ordered vector ([`Canonical`]) — the order semantics are the
//!   ones upstream's §12 relies on ("existing key keeps position; new key
//!   appends").
//! - `structuredClone` copies are serde round-trips.
//! - Upstream `SystemSectionDraft` methods take section objects as key
//!   witnesses; the port takes the section keys directly — the objects carry
//!   nothing else the draft reads.
#![allow(clippy::type_complexity)]

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::sync::{Arc, RwLock};

use futures::future::BoxFuture;
use serde_json::Value;

use crate::agent_core::harness::pico3::session::Tx;
use crate::agent_core::harness::pico3::types::{ContextEdit, Entry, Id, SectionSeed};

use super::runtime::ToolDeclaration;

// ---------------------------------------------------------------------------
// Sections (pico §12.1). A section is a stable key plus a pure renderer. Only
// keys, payloads and rendered text are stored; never renderers.
// ---------------------------------------------------------------------------

/// Upstream `SystemSection` (`system.ts:22-25`).
#[derive(Clone)]
pub struct SystemSection {
    /// Upstream `key`.
    pub key: String,
    /// Upstream `render(value)`.
    pub render: Arc<dyn Fn(&Value) -> String + Send + Sync>,
}

impl std::fmt::Debug for SystemSection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SystemSection")
            .field("key", &self.key)
            .finish()
    }
}

/// Upstream `defineSystemSection` (`system.ts:26-31`).
pub fn define_system_section(
    key: impl Into<String>,
    render: impl Fn(&Value) -> String + Send + Sync + 'static,
) -> SystemSection {
    SystemSection {
        key: key.into(),
        render: Arc::new(render),
    }
}

/// Upstream `systemSections` (`system.ts:35-45`): the built-in sections, in
/// declaration order.
pub fn system_sections() -> Vec<SystemSection> {
    vec![
        define_system_section("identity", |value| {
            value.as_str().unwrap_or_default().to_owned()
        }),
        define_system_section("environment", |value| {
            let cwd = value.get("cwd").and_then(Value::as_str).unwrap_or_default();
            format!("Working directory: {cwd}")
        }),
        define_system_section("skills", |value| {
            value
                .as_array()
                .map(|skills| {
                    skills
                        .iter()
                        .map(|skill| {
                            let name = skill
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or_default();
                            let description = skill
                                .get("description")
                                .and_then(Value::as_str)
                                .unwrap_or_default();
                            format!("- {name}: {description}")
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default()
        }),
    ]
}

/// Upstream `sectionSeed(section, value)` (`system.ts:49-52`); the record
/// type lives in [`crate::agent_core::harness::pico3::types::SectionSeed`]
/// because `Conversation` stores it.
pub fn section_seed(section: &SystemSection, value: Value) -> SectionSeed {
    SectionSeed::Set {
        key: section.key.clone(),
        value,
    }
}

/// Upstream `removeSection` (`system.ts:53`).
pub fn remove_section(key: impl Into<String>) -> SectionSeed {
    SectionSeed::Remove { key: key.into() }
}

// ---------------------------------------------------------------------------
// The managed entry (§12.3)
// ---------------------------------------------------------------------------

/// Upstream `SectionRecord` (`system.ts:59-61`).
#[derive(Debug, Clone, PartialEq)]
pub struct SectionRecord {
    pub key: String,
    pub action: &'static str,
    pub value: Option<Value>,
    pub rendered: Option<String>,
}

impl SectionRecord {
    /// `{ key, action: "set", value, rendered }`.
    pub fn set(key: impl Into<String>, value: Value, rendered: impl Into<String>) -> SectionRecord {
        SectionRecord {
            key: key.into(),
            action: "set",
            value: Some(value),
            rendered: Some(rendered.into()),
        }
    }

    /// `{ key, action: "remove" }`.
    pub fn remove(key: impl Into<String>) -> SectionRecord {
        SectionRecord {
            key: key.into(),
            action: "remove",
            value: None,
            rendered: None,
        }
    }

    /// The wire shape.
    pub fn to_value(&self) -> Value {
        let mut object = serde_json::Map::new();
        object.insert("key".to_owned(), Value::String(self.key.clone()));
        object.insert("action".to_owned(), Value::String(self.action.to_owned()));
        if let Some(value) = &self.value {
            object.insert("value".to_owned(), value.clone());
        }
        if let Some(rendered) = &self.rendered {
            object.insert("rendered".to_owned(), Value::String(rendered.clone()));
        }
        Value::Object(object)
    }
}

/// Upstream `SystemEntryData` (`system.ts:62`): `{ baseline?: true, sections
/// }`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SystemEntryData {
    pub baseline: Option<bool>,
    pub sections: Vec<SectionRecord>,
}

impl SystemEntryData {
    /// The wire shape.
    pub fn to_value(&self) -> Value {
        let mut object = serde_json::Map::new();
        if self.baseline == Some(true) {
            object.insert("baseline".to_owned(), Value::Bool(true));
        }
        object.insert(
            "sections".to_owned(),
            Value::Array(self.sections.iter().map(SectionRecord::to_value).collect()),
        );
        Value::Object(object)
    }
}

// ---------------------------------------------------------------------------
// Canonical state (§12.4)
// ---------------------------------------------------------------------------

/// One canonical section's state (`system.ts:71`).
#[derive(Debug, Clone, PartialEq)]
pub struct SectionState {
    pub value: Value,
    pub rendered: String,
}

/// Upstream `Canonical` (`system.ts:72`): insertion order is canonical
/// order; the port carries the order explicitly (module docs).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Canonical {
    entries: Vec<(String, SectionState)>,
}

impl Canonical {
    pub fn new() -> Canonical {
        Canonical {
            entries: Vec::new(),
        }
    }

    /// Upstream `Map.get`.
    pub fn get(&self, key: &str) -> Option<&SectionState> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, s)| s)
    }

    /// Upstream `Map.set`: an existing key keeps its position; a new key
    /// appends.
    pub fn set(&mut self, key: impl Into<String>, state: SectionState) {
        let key = key.into();
        if let Some(slot) = self.entries.iter_mut().find(|(k, _)| *k == key) {
            slot.1 = state;
        } else {
            self.entries.push((key, state));
        }
    }

    /// Upstream `Map.delete`.
    pub fn delete(&mut self, key: &str) {
        self.entries.retain(|(k, _)| k != key);
    }

    /// Upstream `Map.keys()`.
    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.entries.iter().map(|(k, _)| k)
    }

    /// Upstream iteration (`for (const [key, s] of …)`).
    pub fn iter(&self) -> impl Iterator<Item = (&String, &SectionState)> {
        self.entries.iter().map(|(k, v)| (k, v))
    }
}

/// Upstream `foldCanonical` (`system.ts:74-106`): fold the fork-visible
/// managed entries from the newest baseline forward, using the kind scan.
pub async fn fold_canonical(
    tx: &mut Tx,
    conversation_id: Id,
) -> anyhow::Result<(Canonical, Option<Id>, Option<Id>)> {
    let mut managed: Vec<Entry> = Vec::new();
    let mut before: Option<Id> = None;
    let mut baseline: Option<Id> = None;
    'scan: loop {
        let page = tx
            .scan_entries(&crate::agent_core::harness::pico3::types::EntryScan {
                conversation_id,
                kind: Some("pi.system".to_owned()),
                with_head: false,
                limit: 64,
                before,
            })
            .await?;
        let page_len = page.len();
        if let Some(last) = page.last() {
            // `before = page[page.length - 1]!.id` (`system.ts:96`): the raw
            // page tail, whether or not the page held the baseline.
            before = Some(last.id);
        }
        for entry in page {
            let is_baseline = entry
                .data
                .as_ref()
                .and_then(|data| data.get("baseline"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            // Upstream pushes the baseline entry into `managed` before
            // breaking (`system.ts:88-93`).
            let is_baseline_entry = is_baseline;
            let entry_id = entry.id;
            managed.push(entry);
            if is_baseline_entry {
                baseline = Some(entry_id);
                break 'scan;
            }
        }
        if page_len < 64 {
            break;
        }
    }
    managed.reverse();
    let mut canonical = Canonical::new();
    for entry in &managed {
        let Some(data) = &entry.data else { continue };
        let Some(sections) = data.get("sections").and_then(Value::as_array) else {
            continue;
        };
        for record in sections {
            let key = record
                .get("key")
                .and_then(Value::as_str)
                .unwrap_or_default();
            match record.get("action").and_then(Value::as_str) {
                Some("remove") => canonical.delete(key),
                _ => {
                    let value = record.get("value").cloned().unwrap_or(Value::Null);
                    let rendered = record
                        .get("rendered")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    canonical.set(key, SectionState { value, rendered });
                }
            }
        }
    }
    let newest_managed = managed.last().map(|entry| entry.id);
    Ok((canonical, newest_managed, baseline))
}

// ---------------------------------------------------------------------------
// Draft (§12.2)
// ---------------------------------------------------------------------------

/// Upstream `SystemSectionDraft` (`system.ts:112-117`): what
/// `systemInstructions` handlers edit.
#[derive(Default)]
pub struct Draft {
    values: Vec<(String, Value)>,
    wrappers: HashMap<String, Vec<Arc<dyn Fn(String) -> String + Send + Sync>>>,
    touched: HashSet<String>,
}

impl Draft {
    /// Upstream `new Draft(seed)` (`system.ts:123-125`).
    pub fn new(seed: &Canonical) -> Draft {
        let mut draft = Draft::default();
        for (key, state) in seed.iter() {
            draft.values.push((key.clone(), state.value.clone()));
        }
        draft
    }

    /// Upstream `get` (`system.ts:126-129`): an owned copy.
    pub fn get(&self, key: &str) -> Option<Value> {
        self.values
            .iter()
            .find(|(existing, _)| existing == key)
            .map(|(_, value)| plain(value))
    }

    /// Upstream `set` (`system.ts:130-133`): an existing key keeps its
    /// position; a new key appends.
    pub fn set(&mut self, key: &str, value: Value) {
        if let Some(slot) = self.values.iter_mut().find(|(existing, _)| existing == key) {
            slot.1 = value;
        } else {
            self.values.push((key.to_owned(), value));
        }
        self.touched.insert(key.to_owned());
    }

    /// Upstream `delete` (`system.ts:134-138`).
    pub fn delete(&mut self, key: &str) {
        self.values.retain(|(existing, _)| existing != key);
        self.wrappers.remove(key);
        self.touched.insert(key.to_owned());
    }

    /// Upstream `wrap` (`system.ts:139-142`): preparation-local.
    pub fn wrap(
        &mut self,
        key: &str,
        transform: impl Fn(String) -> String + Send + Sync + 'static,
    ) {
        self.wrappers
            .entry(key.to_owned())
            .or_default()
            .push(Arc::new(transform));
        self.touched.insert(key.to_owned());
    }

    /// Upstream `snapshot` (`system.ts:143-149`).
    fn snapshot(&self) -> DraftSnapshot {
        DraftSnapshot {
            values: self.values.clone(),
            wrappers: self
                .wrappers
                .iter()
                .map(|(key, wraps)| (key.clone(), wraps.clone()))
                .collect(),
            touched: self.touched.clone(),
        }
    }

    /// Upstream `restore` (`system.ts:150-154`).
    fn restore(&mut self, snapshot: DraftSnapshot) {
        self.values = snapshot.values;
        self.wrappers = snapshot.wrappers;
        self.touched = snapshot.touched;
    }
}

struct DraftSnapshot {
    values: Vec<(String, Value)>,
    wrappers: HashMap<String, Vec<Arc<dyn Fn(String) -> String + Send + Sync>>>,
    touched: HashSet<String>,
}

/// Plain JSON copy (`structuredClone`).
fn plain(value: &Value) -> Value {
    serde_json::from_slice(&serde_json::to_vec(value).expect("values serialize"))
        .expect("values round-trip")
}

/// Upstream `freeze` (`system.ts:158-173`): render touched sections
/// (wrappers apply after rendering, in registration order); untouched keep
/// their stored text.
fn freeze(
    draft: &Draft,
    canonical: &Canonical,
    registry: &HashMap<String, SystemSection>,
) -> Canonical {
    let mut desired = Canonical::new();
    for (key, value) in &draft.values {
        if !draft.touched.contains(key) {
            if let Some(prev) = canonical.get(key) {
                desired.set(key.clone(), prev.clone());
            }
            continue;
        }
        let Some(def) = registry.get(key) else {
            continue; // unregistered: cannot re-render; treated as removed
        };
        let mut rendered = (def.render)(value);
        for wrapper in draft.wrappers.get(key).map(Vec::as_slice).unwrap_or(&[]) {
            rendered = wrapper(rendered);
        }
        desired.set(
            key.clone(),
            SectionState {
                value: value.clone(),
                rendered,
            },
        );
    }
    desired
}

// ---------------------------------------------------------------------------
// Preparation (§12.5)
// ---------------------------------------------------------------------------

/// Upstream `SectionRegistry` (`system.ts:179-182`).
#[derive(Clone)]
pub struct SectionRegistry {
    pub map: Arc<RwLock<HashMap<String, SystemSection>>>,
    pub revision: Arc<AtomicU64>,
}

impl SectionRegistry {
    pub fn new() -> SectionRegistry {
        SectionRegistry {
            map: Arc::new(RwLock::new(HashMap::new())),
            revision: Arc::new(AtomicU64::new(0)),
        }
    }

    /// The revision at this instant (upstream `get revision()`).
    pub fn current_revision(&self) -> u64 {
        self.revision.load(Ordering::SeqCst)
    }
}

impl Default for SectionRegistry {
    fn default() -> SectionRegistry {
        SectionRegistry::new()
    }
}

/// Upstream `ToolRegistry` (`system.ts:183-186`).
#[derive(Clone)]
pub struct ToolRegistry {
    pub map: Arc<RwLock<HashMap<String, Arc<ToolDeclaration>>>>,
    pub revision: Arc<AtomicU64>,
}

impl ToolRegistry {
    pub fn new() -> ToolRegistry {
        ToolRegistry {
            map: Arc::new(RwLock::new(HashMap::new())),
            revision: Arc::new(AtomicU64::new(0)),
        }
    }

    /// The revision at this instant (upstream `get revision()`).
    pub fn current_revision(&self) -> u64 {
        self.revision.load(Ordering::SeqCst)
    }
}

impl Default for ToolRegistry {
    fn default() -> ToolRegistry {
        ToolRegistry::new()
    }
}

/// Upstream `PreparationSnapshot` (`system.ts:188-196`).
#[derive(Debug, Clone, PartialEq)]
pub struct PreparationSnapshot {
    pub newest_managed: Option<Id>,
    pub newest_baseline: Option<Id>,
    pub newest_head: Option<Id>,
    pub settings: SettingsSnapshot,
    pub sections_rev: u64,
    pub tools_rev: u64,
}

/// Upstream `PreparationSnapshot["settings"]` (`system.ts:193`).
#[derive(Debug, Clone, PartialEq)]
pub struct SettingsSnapshot {
    pub model: Option<Value>,
    pub thinking_level: Value,
    pub selected_tools: Vec<String>,
    pub profile: Value,
}

/// Upstream `sameSnapshot` (`system.ts:197`): field-by-field equality
/// (upstream `JSON.stringify(a) === JSON.stringify(b)`; serde value equality
/// is the same modulo key order).
pub fn same_snapshot(a: &PreparationSnapshot, b: &PreparationSnapshot) -> bool {
    a == b
}

/// Upstream `takeSnapshot` (`system.ts:199-226`).
pub async fn take_snapshot(
    tx: &mut Tx,
    conversation_id: Id,
    sections: &SectionRegistry,
    tools: &ToolRegistry,
) -> anyhow::Result<(PreparationSnapshot, Canonical, Option<Vec<SectionSeed>>)> {
    let (canonical, newest_managed, newest_baseline) = fold_canonical(tx, conversation_id).await?;
    let head = tx.newest_entry(conversation_id, None, true).await?;
    // A plain copy: the snapshot outlives this transaction (`system.ts:207`).
    let rewindable = tx.snapshot(
        crate::agent_core::harness::pico3::types::DocRef::Rewindable { conversation_id },
    )?;
    let conv = tx.conversation(conversation_id).await?;
    let take = |key: &str| rewindable.get(key).cloned();
    let model = take("model").filter(|value| !value.is_null());
    let thinking_level = take("thinkingLevel").unwrap_or(Value::Null);
    let selected_tools = take("selectedTools")
        .and_then(|value| value.as_array().cloned())
        .map(|array| {
            array
                .into_iter()
                .filter_map(|name| name.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let profile = take("profile").unwrap_or(Value::Null);
    let snapshot = PreparationSnapshot {
        newest_managed,
        newest_baseline,
        newest_head: head.as_ref().map(|entry| entry.id),
        settings: SettingsSnapshot {
            model,
            thinking_level,
            selected_tools,
            profile,
        },
        sections_rev: sections.current_revision(),
        tools_rev: tools.current_revision(),
    };
    // §8.1: the seed applies while there is no local managed entry
    // (`system.ts:224`).
    let seed = if newest_managed.is_none() {
        conv.and_then(|conversation| conversation.sections)
    } else {
        None
    };
    Ok((snapshot, canonical, seed))
}

/// The `systemInstructions` handler type (`system.ts:230-238`): edit the
/// draft; optionally override the tool loadout (last override wins). A
/// throwing handler's edits are rolled back by [`prepare_draft`].
pub type SystemInstructionsFn = Arc<
    dyn for<'a> Fn(
            &'a Mutex<Draft>,
            &'a SettingsSnapshot,
            &'a [Arc<ToolDeclaration>],
            &'a super::runtime::HookApi,
        ) -> BoxFuture<'a, anyhow::Result<Option<Vec<Arc<ToolDeclaration>>>>>
        + Send
        + Sync,
>;

/// The `systemInstructions` hook bindings, already downcast by the caller
/// (the generation kind's runner): handler + binding api pairs.
pub type SystemInstructionsBindings = Vec<(SystemInstructionsFn, super::runtime::HookApi)>;

/// Upstream `prepareDraft` (`system.ts:242-277`): off the line — seed the
/// draft, run handlers, freeze. `tools_lookup` is upstream
/// `rt.tools.get(name)` (`system.ts:256`).
pub async fn prepare_draft(
    handlers: SystemInstructionsBindings,
    sections: &SectionRegistry,
    tools_lookup: &(dyn Fn(&str) -> Option<Arc<ToolDeclaration>> + Send + Sync),
    canonical: &Canonical,
    seed: Option<&[SectionSeed]>,
    settings: &SettingsSnapshot,
    on_report: &(dyn Fn(String) + Send + Sync),
) -> anyhow::Result<(Canonical, Vec<Arc<ToolDeclaration>>)> {
    let draft = Mutex::new(Draft::new(canonical));
    for s in seed.unwrap_or(&[]) {
        let mut draft_guard = draft.lock().expect("draft");
        match s {
            SectionSeed::Remove { key } => draft_guard.delete(key),
            SectionSeed::Set { key, value } => {
                // `draft.set({ key: s.key, render: () => "" }, s.value)`
                // (`system.ts:253`): the render fn is never consulted — the
                // freeze re-renders through the registry.
                draft_guard.set(key, plain(value));
            }
        }
    }

    let default_tools: Vec<Arc<ToolDeclaration>> = settings
        .selected_tools
        .iter()
        .filter_map(|name| tools_lookup(name))
        .collect();
    // Upstream reports `selected tool ${name} is not registered`
    // (`system.ts:256-258`).
    for name in &settings.selected_tools {
        if tools_lookup(name).is_none() {
            on_report(format!("selected tool {name} is not registered"));
        }
    }
    let mut tools: Vec<Arc<ToolDeclaration>> = default_tools.clone();

    for (handler, api) in &handlers {
        let before = draft.lock().expect("draft").snapshot();
        match handler(&draft, settings, &default_tools, api).await {
            Ok(Some(override_tools)) => {
                // `if (out?.tools) tools = out.tools` (`system.ts:269`): last
                // override wins.
                tools = override_tools;
            }
            Ok(None) => {}
            Err(error) => {
                // A throwing handler's edits are rolled back
                // (`system.ts:271-274`). Abort re-raises: the caller's
                // context is checked at the next line operation; upstream
                // rethrows only when the signal already fired.
                draft.lock().expect("draft").restore(before);
                on_report(format!("{error}"));
            }
        }
    }
    let registry = sections.map.read().expect("section registry").clone();
    let draft = draft.into_inner().expect("draft");
    Ok((freeze(&draft, canonical, &registry), tools))
}

/// Upstream `planManagedEntry`'s result (`system.ts:288`).
pub struct PlannedManagedEntry {
    pub data: SystemEntryData,
    /// `[]` for a metadata-only delta (`system.ts:64`).
    pub model: Vec<Value>,
    pub edits: Vec<ContextEdit>,
}

/// Upstream `planManagedEntry` (`system.ts:280-363`): on the line, in the
/// `prepared` commit — diff desired against canonical; baseline if a head
/// intervened.
pub async fn plan_managed_entry(
    tx: &mut Tx,
    conversation_id: Id,
    snapshot: &PreparationSnapshot,
    canonical: &Canonical,
    desired: &Canonical,
    tools: &[Arc<ToolDeclaration>],
    now: i64,
) -> anyhow::Result<Option<PlannedManagedEntry>> {
    // §12.4: no managed history at all → first preparation appends a full
    // baseline; and a usable baseline must follow the newest head
    // (`system.ts:289-294`).
    let need_baseline = snapshot.newest_managed.is_none()
        || (snapshot.newest_head.is_some()
            && (snapshot.newest_baseline.is_none()
                || snapshot.newest_head.unwrap_or(0) > snapshot.newest_baseline.unwrap_or(0)));

    // Previous effective tools: those declared by the newest managed entry
    // chain, recomputed from the projected messages (`system.ts:305-312`).
    let context = tx.context(conversation_id, None).await?;
    let previous = effective_tools(&context.messages);

    if need_baseline {
        let sections: Vec<SectionRecord> = desired
            .iter()
            .map(|(key, state)| {
                SectionRecord::set(key.clone(), state.value.clone(), state.rendered.clone())
            })
            .collect();
        let content = sections
            .iter()
            .map(|record| match (record.action, record.rendered.as_deref()) {
                ("set", Some(rendered)) => format!("## {}\n{rendered}", record.key),
                _ => String::new(),
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        // Omit managed entries above the newest head (`system.ts:322-325`).
        let entries = tx.context(conversation_id, None).await?.entries;
        let edits: Vec<ContextEdit> = entries
            .iter()
            .filter(|entry| {
                entry.kind == "pi.system" && entry.id > snapshot.newest_head.unwrap_or(0)
            })
            .map(|entry| ContextEdit {
                target: entry.id,
                action: "omit".to_owned(),
                messages: None,
            })
            .collect();
        let tools_added: Vec<Value> = tools.iter().map(|tool| tool_of(tool)).collect();
        let mut message = serde_json::Map::new();
        message.insert("role".to_owned(), Value::String("system".to_owned()));
        message.insert("content".to_owned(), Value::String(content));
        message.insert("toolsAdded".to_owned(), Value::Array(tools_added));
        message.insert(
            "timestamp".to_owned(),
            crate::agent_core::harness::pico3::types::number(now),
        );
        return Ok(Some(PlannedManagedEntry {
            data: SystemEntryData {
                baseline: Some(true),
                sections,
            },
            model: vec![Value::Object(message)],
            edits,
        }));
    }

    let mut changed: Vec<SectionRecord> = Vec::new();
    for (key, state) in desired.iter() {
        let differs = match canonical.get(key) {
            Some(prev) => {
                prev.rendered != state.rendered
                    || serde_json::to_string(&prev.value).unwrap_or_default()
                        != serde_json::to_string(&state.value).unwrap_or_default()
            }
            None => true,
        };
        if differs {
            changed.push(SectionRecord::set(
                key.clone(),
                state.value.clone(),
                state.rendered.clone(),
            ));
        }
    }
    for key in canonical.keys() {
        if desired.get(key).is_none() {
            changed.push(SectionRecord::remove(key.clone()));
        }
    }
    let added: Vec<Value> = tools
        .iter()
        .filter(|tool| {
            !previous
                .iter()
                .any(|old| old.get("name").and_then(Value::as_str) == Some(tool.name.as_str()))
        })
        .map(|tool| tool_of(tool))
        .collect();
    let removed: Vec<Value> = previous
        .iter()
        .filter(|previous_tool| {
            !tools.iter().any(|tool| {
                Some(tool.name.as_str()) == previous_tool.get("name").and_then(Value::as_str)
            })
        })
        .cloned()
        .collect();
    if changed.is_empty() && added.is_empty() && removed.is_empty() {
        return Ok(None);
    }

    let render_changed = changed.iter().any(|record| {
        record.action == "remove"
            || canonical
                .get(&record.key)
                .map(|prev| prev.rendered.as_str())
                != record.rendered.as_deref()
    });
    if !render_changed && added.is_empty() && removed.is_empty() {
        // Metadata-only delta (`system.ts:347`).
        return Ok(Some(PlannedManagedEntry {
            data: SystemEntryData {
                baseline: None,
                sections: changed,
            },
            model: Vec::new(),
            edits: Vec::new(),
        }));
    }
    let content = changed
        .iter()
        .map(|record| match (record.action, &record.rendered) {
            ("set", Some(rendered)) => format!("The {} section now reads:\n{rendered}", record.key),
            _ => format!("The {} section no longer applies.", record.key),
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let mut message = serde_json::Map::new();
    message.insert("role".to_owned(), Value::String("system".to_owned()));
    message.insert("content".to_owned(), Value::String(content));
    if !added.is_empty() {
        message.insert("toolsAdded".to_owned(), Value::Array(added));
    }
    if !removed.is_empty() {
        message.insert("toolsRemoved".to_owned(), Value::Array(removed));
    }
    message.insert(
        "timestamp".to_owned(),
        crate::agent_core::harness::pico3::types::number(now),
    );
    Ok(Some(PlannedManagedEntry {
        data: SystemEntryData {
            baseline: None,
            sections: changed,
        },
        model: vec![Value::Object(message)],
        edits: Vec::new(),
    }))
}

/// Upstream `toolOf` (`system.ts:295-303`): `{ name, description,
/// parameters: structuredClone(parameters) }`.
fn tool_of(tool: &ToolDeclaration) -> Value {
    serde_json::json!({
        "name": tool.name,
        "description": tool.description,
        "parameters": plain(&tool.parameters),
    })
}

/// Upstream `effectiveTools` (`system.ts:366-374`): fold
/// toolsAdded/toolsRemoved across a projection's SystemMessages.
pub fn effective_tools(messages: &[Value]) -> Vec<Value> {
    // JS Map preserves first insertion order; replacement keeps its position,
    // while removing and re-adding a name moves it to the end.
    let mut tools: Vec<Value> = Vec::new();
    for message in messages {
        if message.get("role").and_then(Value::as_str) != Some("system") {
            continue;
        }
        if let Some(removed) = message.get("toolsRemoved").and_then(Value::as_array) {
            for tool in removed {
                if let Some(name) = tool.get("name").and_then(Value::as_str) {
                    tools.retain(|current| {
                        current.get("name").and_then(Value::as_str) != Some(name)
                    });
                }
            }
        }
        if let Some(added) = message.get("toolsAdded").and_then(Value::as_array) {
            for tool in added {
                if let Some(name) = tool.get("name").and_then(Value::as_str) {
                    match tools
                        .iter_mut()
                        .find(|current| current.get("name").and_then(Value::as_str) == Some(name))
                    {
                        Some(current) => *current = tool.clone(),
                        None => tools.push(tool.clone()),
                    }
                }
            }
        }
    }
    tools
}

/// Unused shard of the module's public surface (keeps `JsonObject` imported
/// for the projected request shape).
pub type RequestMessage = Value;
