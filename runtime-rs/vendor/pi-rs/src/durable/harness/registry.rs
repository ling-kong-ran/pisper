//! Port of `src/harness/registry.ts`: the application-owned registry and its
//! published snapshot states, plus the built-in conversation setup every
//! registry starts with.
//!
//! Divergences (structural, disclosed):
//! - **D26 (generics erased).** Upstream `Registry<Tool extends
//!   ToolRegistration>` is generic; the port keeps the erased forms (D13),
//!   so the registry and its snapshot carry
//!   [`Arc<ToolRegistration>`](ToolRegistration) /
//!   [`TaskToken`](TaskToken) values directly.
//! - **D27 (batch callbacks).** Upstream `batch(register)` re-entrantly
//!   rejects a returned thenable ("callbacks must be synchronous"); the
//!   port's batch callback is a synchronous [`FnOnce`] by construction, so
//!   the thenable check vanishes. Staged-then-disposed records, nested-batch
//!   rejection, and the position keying behave identically.
//! - The typed `tools.add` / `hooks.add` / `tasks.add` /
//!   `conversations.setup` / `systemPrompt.*` object surfaces become methods
//!   on [`Registry`].

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use serde_json::Value;

use super::super::errors::PlainError;
use super::super::harness::config::conversation_config;
use super::super::harness::inbox::inbox_doc;
use super::super::harness::live::live_doc;
use super::super::harness::types::{
    ConversationSetup, HookHandlerFn, HookRegistration, HookScope, PromptSection,
    PromptSectionWrapper, RegistryFailure, RegistryFailureKind, RegistryReaderLike,
    RegistrySnapshotLike, SectionRenderFn, ToolRegistration, ToolWrapper,
};
use super::super::harness::usage::usage_doc;
use super::super::session::transaction::Transaction;
use super::super::tasks::TaskToken;
use super::super::types::ConversationRecord;
use super::generation::generation_task;
use super::tool::tool_task;

/// A section key (`SECTION_KEY`): `/^[a-z][a-z0-9_-]*$/`.
fn section_key_valid(key: &str) -> bool {
    let mut characters = key.chars();
    match characters.next() {
        Some(first) if first.is_ascii_lowercase() => {}
        _ => return false,
    }
    characters.all(|character| {
        character.is_ascii_lowercase()
            || character.is_ascii_digit()
            || character == '_'
            || character == '-'
    })
}

fn require_section_key(key: &str) -> Result<(), PlainError> {
    if !section_key_valid(key) {
        return Err(PlainError::new(format!(
            "Section key {} must match /^[a-z][a-z0-9_-]*$/",
            serde_json::to_string(key).unwrap_or_else(|_| key.to_owned())
        )));
    }
    Ok(())
}

/// Built-in task definitions every registry starts with; they cannot be
/// disposed or replaced (`BUILTIN_TASKS`).
pub fn builtin_tasks() -> Vec<TaskToken> {
    vec![generation_task(), tool_task()]
}

/// The built-in conversation setup key (`BUILTIN_SETUP_KEY`).
pub const BUILTIN_SETUP_KEY: &str = "pi";

/// Built-in documents: empty `pi.live`, `pi.inbox`, and `pi.usage`, a fresh
/// `pi.provider` identity, and for a new conversation the default
/// configuration with every registered tool active (`builtinSetup`) (D29:
/// sync over the port's transaction).
pub fn builtin_setup(
    tx: &Transaction,
    conversation: &ConversationRecord,
    registry: Arc<dyn RegistrySnapshotLike>,
) -> Result<(), PlainError> {
    let live = live_doc();
    tx.doc(&live.definition, Some(conversation.id), None, None)?;
    let inbox = inbox_doc();
    tx.doc(&inbox.definition, Some(conversation.id), None, None)?;
    let usage = usage_doc();
    tx.doc(&usage.definition, Some(conversation.id), None, None)?;
    let provider = super::provider::provider_doc();
    tx.doc(&provider.definition, Some(conversation.id), None, None)?;
    if conversation.parent.is_some() {
        return Ok(());
    }
    let config = conversation_config();
    let draft = tx.doc(&config.definition, Some(conversation.id), None, None)?;
    let mut state =
        super::super::harness::config::ConversationConfigState::from_json(&read_value(&draft)?)
            .unwrap_or_else(|_| super::super::harness::config::ConversationConfigState::initial());
    state.active_tools = registry.tool_names();
    write_value(
        &draft,
        &serde_json::to_value(&state).map_err(|error| PlainError::new(error.to_string()))?,
    )
}

/// Read a draft document value as an object.
fn read_value(
    draft: &super::super::session::transaction::DocumentDraft,
) -> Result<serde_json::Map<String, Value>, PlainError> {
    Ok(draft
        .read(&[])
        .map_err(|error| PlainError::new(error.message()))?
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default())
}

/// Replace a draft document value wholesale.
fn write_value(
    draft: &super::super::session::transaction::DocumentDraft,
    value: &Value,
) -> Result<(), PlainError> {
    draft
        .set(&[], value.clone())
        .map_err(|error| PlainError::new(error.message()))
}

/// One registration slot (`Slot`).
enum Slot {
    Tool(Arc<ToolRegistration>),
    ToolWrap {
        name: String,
        wrapper: ToolWrapper,
    },
    Hook {
        task_name: String,
        hook: HookRegistration,
    },
    Task(TaskToken),
    Setup {
        key: String,
        setup: ConversationSetup,
    },
    Section(PromptSection),
    SectionWrap {
        key: String,
        wrapper: PromptSectionWrapper,
    },
}

/// One registration and its lifecycle (`RegistryRecord`).
struct RegistryRecord {
    slot: Slot,
    /// Uniqueness and position key; absent for keyless hooks, which always
    /// append.
    key: Option<String>,
    /// Human-readable identity for duplicate errors.
    label: String,
    position: i64,
    status: RecordStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecordStatus {
    Staged,
    Published,
    Disposed,
    // Upstream stages-then-disposes records inside a batch; the port filters
    // disposed staged records at publish instead.
    #[allow(dead_code)]
    Rejected,
}

#[derive(Default)]
struct Batch {
    added: Vec<Arc<Mutex<RegistryRecord>>>,
    disposed: Vec<Arc<Mutex<RegistryRecord>>>,
}

struct Composition {
    tools: Vec<Arc<ToolRegistration>>,
    tools_by_name: BTreeMap<String, Arc<ToolRegistration>>,
    sections: Vec<PromptSection>,
    failures: Vec<RegistryFailure>,
}

/// Immutable published registry state; wrappers are composed lazily once per
/// state (`RegistryState`).
struct RegistryState {
    records: Vec<Arc<Mutex<RegistryRecord>>>,
    composition: Mutex<Option<Arc<Composition>>>,
}

impl RegistryState {
    fn composed(&self) -> Arc<Composition> {
        {
            let composition = self
                .composition
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(composition) = composition.as_ref() {
                return Arc::clone(composition);
            }
        }
        let mut failures: Vec<RegistryFailure> = Vec::new();
        let mut tool_wraps: BTreeMap<String, Vec<ToolWrapper>> = BTreeMap::new();
        let mut section_wraps: BTreeMap<String, Vec<PromptSectionWrapper>> = BTreeMap::new();
        for record in &self.records {
            let record = record
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match &record.slot {
                Slot::ToolWrap { name, wrapper } => {
                    tool_wraps
                        .entry(name.clone())
                        .or_default()
                        .push(Arc::clone(wrapper));
                }
                Slot::SectionWrap { key, wrapper } => {
                    section_wraps
                        .entry(key.clone())
                        .or_default()
                        .push(Arc::clone(wrapper));
                }
                _ => {}
            }
        }
        let mut tools: Vec<Arc<ToolRegistration>> = Vec::new();
        let mut tools_by_name = BTreeMap::new();
        let mut sections: Vec<PromptSection> = Vec::new();
        for record in &self.records {
            let record = record
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match &record.slot {
                Slot::Tool(tool) => {
                    let name = tool.name().to_owned();
                    let mut tool = Arc::clone(tool);
                    let mut failed = false;
                    for wrap in tool_wraps.get(&name).into_iter().flatten() {
                        tool = wrap(Arc::clone(&tool));
                        if tool.name() != name {
                            failures.push(RegistryFailure {
                                kind: RegistryFailureKind::Tool,
                                name: name.clone(),
                                error: PlainError::new(format!(
                                    "Tool wrapper renamed {name} to {}",
                                    tool.name()
                                )),
                            });
                            failed = true;
                            break;
                        }
                    }
                    if !failed {
                        tools.push(Arc::clone(&tool));
                        tools_by_name.insert(name, Arc::clone(&tool));
                    }
                }
                Slot::Section(section) => {
                    let key = section.key.clone();
                    let mut section = section.clone();
                    let mut failed = false;
                    for wrap in section_wraps.get(&key).into_iter().flatten() {
                        section = wrap(section);
                        if section.key != key {
                            failures.push(RegistryFailure {
                                kind: RegistryFailureKind::Section,
                                name: key.clone(),
                                error: PlainError::new(format!(
                                    "Section wrapper renamed {key} to {}",
                                    section.key
                                )),
                            });
                            failed = true;
                            break;
                        }
                    }
                    if !failed {
                        sections.push(section);
                    }
                }
                _ => {}
            }
        }
        let composition = Arc::new(Composition {
            tools,
            tools_by_name,
            sections,
            failures,
        });
        *self
            .composition
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Arc::clone(&composition));
        composition
    }
}

impl RegistrySnapshotLike for RegistryState {
    fn tools(&self) -> Vec<Arc<ToolRegistration>> {
        self.composed().tools.clone()
    }

    fn tool(&self, name: &str) -> Option<Arc<ToolRegistration>> {
        self.composed().tools_by_name.get(name).cloned()
    }

    fn tool_names(&self) -> Vec<String> {
        let mut names = Vec::new();
        for record in &self.records {
            let record = record
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Slot::Tool(tool) = &record.slot {
                names.push(tool.name().to_owned());
            }
        }
        names
    }

    fn task(&self, name: &str) -> Option<TaskToken> {
        for record in &self.records {
            let record = record
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Slot::Task(task) = &record.slot {
                if task.definition.name == name {
                    return Some(task.clone());
                }
            }
        }
        None
    }

    fn hooks(&self, task_name: &str) -> Vec<HookRegistration> {
        let mut hooks = Vec::new();
        for record in &self.records {
            let record = record
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Slot::Hook {
                task_name: registered,
                hook,
            } = &record.slot
            {
                // Registration was typed by a token with this name; matching
                // by name lets hooks survive a task reload.
                if registered == task_name {
                    hooks.push(hook.clone());
                }
            }
        }
        hooks
    }

    fn sections(&self) -> Vec<PromptSection> {
        self.composed().sections.clone()
    }

    fn failures(&self) -> Vec<RegistryFailure> {
        self.composed().failures.clone()
    }

    fn conversation_setups(&self) -> Vec<super::super::harness::types::ConversationSetupEntry> {
        let mut setups = Vec::new();
        for record in &self.records {
            let record = record
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Slot::Setup { key, setup } = &record.slot {
                setups.push(super::super::harness::types::ConversationSetupEntry {
                    key: key.clone(),
                    setup: Arc::clone(setup),
                });
            }
        }
        setups
    }
}

struct RegistryCore {
    current: Arc<RegistryState>,
    batch: Option<Batch>,
    /// First position of every key ever published; re-registered keys keep
    /// it.
    positions: HashMap<String, i64>,
    next_position: i64,
}

/// A publication listener (`Registry.subscribe`).
pub type RegistryListener = Box<dyn Fn() + Send + Sync>;

/// The application-owned registry (`RegistryImpl`).
pub struct Registry {
    core: Mutex<RegistryCore>,
    listeners: Arc<Mutex<Vec<RegistryListener>>>,
}

impl Registry {
    /// An empty registry (`constructor`); [`create_registry`] installs the
    /// built-ins.
    pub fn new() -> Arc<Registry> {
        Arc::new(Registry {
            listeners: Arc::new(Mutex::new(Vec::new())),
            core: Mutex::new(RegistryCore {
                current: Arc::new(RegistryState {
                    records: Vec::new(),
                    composition: Mutex::new(None),
                }),
                batch: None,
                positions: HashMap::new(),
                next_position: 0,
            }),
        })
    }

    /// Register a tool (`tools.add`).
    pub fn add_tool(
        &self,
        tool: Arc<ToolRegistration>,
    ) -> Result<super::super::harness::types::Registration, PlainError> {
        let name = tool.name().to_owned();
        self.register(
            Slot::Tool(tool),
            Some(format!("tool\0{name}")),
            format!("Tool {name}"),
        )
    }

    /// Wrap a tool's composed form (`tools.wrap`).
    pub fn wrap_tool(
        &self,
        name: impl Into<String>,
        key: impl Into<String>,
        wrapper: ToolWrapper,
    ) -> Result<super::super::harness::types::Registration, PlainError> {
        let name = name.into();
        let key = key.into();
        self.register(
            Slot::ToolWrap {
                name: name.clone(),
                wrapper,
            },
            Some(format!("toolWrap\0{name}\0{key}")),
            format!("Tool wrapper {key} for {name}"),
        )
    }

    /// Register hook handlers for a task (`hooks.add`); a key scopes the
    /// registration's identity, a scope restricts the conversations it runs
    /// for.
    pub fn add_hook(
        &self,
        task: &TaskToken,
        handlers: BTreeMap<String, HookHandlerFn>,
        scope: Option<HookScope>,
        key: Option<String>,
    ) -> Result<super::super::harness::types::Registration, PlainError> {
        let task_name = task.definition.name.clone();
        let hook = HookRegistration { handlers, scope };
        self.register(
            Slot::Hook {
                task_name: task_name.clone(),
                hook,
            },
            key.as_ref().map(|key| format!("hook\0{task_name}\0{key}")),
            format!("Hook {} for {}", key.as_deref().unwrap_or(""), task_name),
        )
    }

    /// Register an executable task definition (`tasks.add`).
    pub fn add_task(
        &self,
        task: TaskToken,
    ) -> Result<super::super::harness::types::Registration, PlainError> {
        let name = task.definition.name.clone();
        self.register(
            Slot::Task(task),
            Some(format!("task\0{name}")),
            format!("Task {name}"),
        )
    }

    /// Stage a conversation setup (`conversations.setup`).
    pub fn setup(
        &self,
        key: impl Into<String>,
        setup: ConversationSetup,
    ) -> Result<super::super::harness::types::Registration, PlainError> {
        let key = key.into();
        self.register(
            Slot::Setup {
                key: key.clone(),
                setup,
            },
            Some(format!("setup\0{key}")),
            format!("Setup {key}"),
        )
    }

    /// Register a system prompt section (`systemPrompt.section`).
    pub fn section(
        &self,
        key: impl Into<String>,
        render: SectionRenderFn,
        tag: Option<bool>,
    ) -> Result<super::super::harness::types::Registration, PlainError> {
        let key = key.into();
        require_section_key(&key)?;
        self.register(
            Slot::Section(PromptSection {
                key: key.clone(),
                render,
                tag,
            }),
            Some(format!("section\0{key}")),
            format!("Section {key}"),
        )
    }

    /// Wrap a section's composed form (`systemPrompt.wrap`).
    pub fn wrap_section(
        &self,
        key: impl Into<String>,
        wrapper_key: impl Into<String>,
        wrapper: PromptSectionWrapper,
    ) -> Result<super::super::harness::types::Registration, PlainError> {
        let key = key.into();
        let wrapper_key = wrapper_key.into();
        require_section_key(&key)?;
        self.register(
            Slot::SectionWrap {
                key: key.clone(),
                wrapper,
            },
            Some(format!("sectionWrap\0{key}\0{wrapper_key}")),
            format!("Section wrapper {wrapper_key} for {key}"),
        )
    }

    /// Register everything inside one published batch (`batch`); the
    /// callback is synchronous by construction (D27).
    pub fn batch(
        &self,
        register: impl FnOnce(&Registry),
    ) -> Result<super::super::harness::types::Registration, PlainError> {
        {
            let mut core = self
                .core
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if core.batch.is_some() {
                return Err(PlainError::new("Registry batches cannot be nested"));
            }
            core.batch = Some(Batch::default());
        }
        (register)(self);
        let core = self
            .core
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut core = core;
        let mut batch = core.batch.take().unwrap_or_default();
        let added = batch.added.clone();
        publish(self, &mut core, &mut batch)?;
        Ok(super::super::harness::types::Registration::new(Arc::new(
            move || {
                for record in &added {
                    dispose_record(record);
                }
            },
        )))
    }

    /// Validate the final staged state, then publish it synchronously
    /// (`#publish`).
    fn publish_batch(
        &self,
        added: Vec<Arc<Mutex<RegistryRecord>>>,
        disposed: Vec<Arc<Mutex<RegistryRecord>>>,
    ) -> Result<super::super::harness::types::Registration, PlainError> {
        let mut core = self
            .core
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut batch = Batch { added, disposed };
        let staged = batch.added.clone();
        publish(self, &mut core, &mut batch)?;
        Ok(super::super::harness::types::Registration::new(Arc::new(
            move || {
                for record in &staged {
                    dispose_record(record);
                }
            },
        )))
    }

    fn register(
        &self,
        slot: Slot,
        key: Option<String>,
        label: String,
    ) -> Result<super::super::harness::types::Registration, PlainError> {
        let record = Arc::new(Mutex::new(RegistryRecord {
            slot,
            key,
            label,
            position: -1,
            status: RecordStatus::Staged,
        }));
        let batch_exists = {
            let mut core = self
                .core
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(batch) = core.batch.as_mut() {
                batch.added.push(Arc::clone(&record));
                true
            } else {
                false
            }
        };
        if batch_exists {
            let staged = Arc::clone(&record);
            return Ok(super::super::harness::types::Registration::new(Arc::new(
                move || {
                    dispose_record(&staged);
                },
            )));
        }
        self.publish_batch(vec![Arc::clone(&record)], Vec::new())
    }

    /// Immutable view of the whole current registry (`snapshot`).
    pub fn snapshot(&self) -> Arc<dyn RegistrySnapshotLike> {
        let current = Arc::clone(
            &self
                .core
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .current,
        );
        current as Arc<dyn RegistrySnapshotLike>
    }

    /// Called synchronously after every publication (`subscribe`); returns
    /// the unsubscribe closure.
    pub fn subscribe(&self, listener: RegistryListener) -> Box<dyn FnOnce() + Send> {
        let listeners = Arc::clone(&self.listeners);
        let index = {
            let mut queue = listeners
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            queue.push(listener);
            queue.len() - 1
        };
        Box::new(move || {
            let mut queue = listeners
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if index < queue.len() {
                drop(queue.remove(index));
            }
        })
    }
}

impl RegistryReaderLike for Registry {
    fn snapshot(&self) -> Arc<dyn RegistrySnapshotLike> {
        Registry::snapshot(self)
    }

    fn subscribe(&self, listener: RegistryListener) -> Box<dyn FnOnce() + Send> {
        Registry::subscribe(self, listener)
    }
}

/// Dispose one staged or published record (`#dispose`).
fn dispose_record(record: &Arc<Mutex<RegistryRecord>>) {
    let status = record
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .status;
    match status {
        RecordStatus::Staged => {
            // Removing from a live batch needs the registry; the batch keeps
            // the record and validation rejects the duplicate key instead.
            // Upstream splices the staged record out; the port marks it
            // disposed and `publish` filters disposed staged records.
            record
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .status = RecordStatus::Disposed;
        }
        RecordStatus::Published => {
            record
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .status = RecordStatus::Disposed;
        }
        RecordStatus::Disposed | RecordStatus::Rejected => {}
    }
}

/// Validate the final staged state, then publish it synchronously
/// (`#publish`).
fn publish(
    registry: &Registry,
    core: &mut RegistryCore,
    batch: &mut Batch,
) -> Result<(), PlainError> {
    let added = batch
        .added
        .iter()
        .filter(|record| {
            record
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .status
                != RecordStatus::Disposed
        })
        .cloned()
        .collect::<Vec<_>>();
    let disposed = batch.disposed.clone();
    if added.is_empty() && disposed.is_empty() {
        return Ok(());
    }
    let mut records: Vec<Arc<Mutex<RegistryRecord>>> = core
        .current
        .records
        .iter()
        .filter(|record| {
            let record_id = Arc::as_ptr(record);
            !disposed
                .iter()
                .any(|disposed| Arc::as_ptr(disposed) == record_id)
        })
        .cloned()
        .collect();
    records.extend(added.iter().cloned());
    let mut keys: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for record in &records {
        let record = record
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(key) = &record.key else { continue };
        if !keys.insert(key.clone()) {
            let label = record.label.clone();
            return Err(PlainError::new(format!("{label} is already registered")));
        }
    }
    for record in &added {
        let mut record = record
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(key) = record.key.clone() else {
            record.position = core.next_position;
            core.next_position += 1;
            continue;
        };
        let position = match core.positions.get(&key) {
            Some(position) => *position,
            None => {
                let position = core.next_position;
                core.positions.insert(key, position);
                core.next_position += 1;
                position
            }
        };
        record.position = position;
    }
    records.sort_by_key(|record| {
        record
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .position
    });
    for record in &added {
        record
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .status = RecordStatus::Published;
    }
    for record in &disposed {
        record
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .status = RecordStatus::Disposed;
    }
    core.current = Arc::new(RegistryState {
        records,
        composition: Mutex::new(None),
    });
    let listeners = Arc::clone(&registry.listeners);
    let queue = listeners
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for listener in queue.iter() {
        listener();
    }
    Ok(())
}

/// Create an application-owned registry holding only the built-ins
/// (`createRegistry`). Their registrations are dropped, so nothing can
/// dispose them.
pub fn create_registry() -> Arc<Registry> {
    let registry = Registry::new();
    for task in builtin_tasks() {
        let _ = registry.add_task(task);
    }
    let setup: ConversationSetup = Arc::new(builtin_setup);
    let _ = registry.setup(BUILTIN_SETUP_KEY, setup);
    registry
}
