//! Port of `src/harness/agent.ts` (v1.0.0): the built-in `pi.agent` document
//! token, the built-in retry and compaction policies, host-settings
//! resolution, agent-state changes, and agent resolution against a registry
//! snapshot — the deterministic core the harness run loop resolves requests
//! with.
//!
//! Divergences (structural, disclosed):
//! - **D-types (erased payloads).** Upstream `Extension`/`ToolRegistration`/
//!   `PromptSection` carry executable closures; the port's resolution core is
//!   phrased over the erased wire shapes (`ExtensionSpec`, `ToolSpec`,
//!   `SectionSpec`) whose tool/section payloads are JSON-erased, so resolution
//!   order and state are pinned byte-for-byte while execution stays with the
//!   registry's erased handlers.
//! - Upstream mutates a Chord `Draft<AgentState>` inside the creating commit
//!   (`configure`, `addTools`, `createAgent`); the port exposes the same
//!   transformations as pure [`AgentState`] edits the transaction layer
//!   applies to its document draft.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::types::{
    CompactionPolicy, ConversationRetryPolicy, ConversationStreamOptions, QueueMode,
    ToolExecutionMode,
};

/// Upstream `DEFAULT_RETRY_POLICY`.
pub fn default_retry_policy() -> ConversationRetryPolicy {
    ConversationRetryPolicy {
        enabled: true,
        max_retries: 3,
        base_delay_ms: 2000.0,
        max_agent_delay_ms: Some(60000.0),
    }
}

/// Upstream `DEFAULT_COMPACTION_POLICY`.
pub fn default_compaction_policy() -> CompactionPolicy {
    CompactionPolicy {
        enabled: true,
        reserve_tokens: 16384,
        keep_recent_tokens: 20000,
        background_tokens: 32768,
    }
}

/// The reserved section key of the agent's `instructions` (upstream
/// `INSTRUCTIONS_KEY`).
pub const INSTRUCTIONS_KEY: &str = "instructions";

/// The `pi.agent` document definition (upstream `AgentDoc`): rewindable so
/// forks start from the agent at their fork entry.
pub const AGENT_DOC_KIND: &str = "pi.agent";
/// `AgentDoc.definition.version`.
pub const AGENT_DOC_VERSION: i64 = 1;

/// Stored choices of one conversation; names, not objects (upstream
/// `AgentState`). Unset fields follow the host.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelRefSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<String>,
    /// An array selects exactly these extensions, in order. An object edits
    /// the host default selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<ExtensionSelection>,
    /// Filters the selected extensions' tools. An array offers exactly these,
    /// in order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolSelection>,
    /// Rendered after every extension section, as the section
    /// `instructions`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// Directory within the environment's file system, passed to
    /// `HarnessOptions.env`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

/// Upstream `ModelRef`: provider and model ID resolved through pi-ai
/// `Models`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRefSpec {
    pub provider: String,
    #[serde(rename = "modelId")]
    pub model_id: String,
}

/// Upstream `AgentState.extensions`: an exact list or an edit of the default
/// selection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ExtensionSelection {
    List(Vec<String>),
    Edit {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        add: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        remove: Option<Vec<String>>,
    },
}

/// Upstream `AgentState.tools`: an exact list or a removal filter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolSelection {
    List(Vec<String>),
    Remove { remove: Vec<String> },
}

/// Upstream `AgentChange`: a given field replaces the stored one, `None`
/// clears it, `undefined` (here [`Change::Keep`]) changes nothing.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AgentChange {
    pub model: Change<ModelRefSpec>,
    pub thinking_level: Change<String>,
    /// Extensions resolve to stored names (an array) or an edit (object).
    pub extensions: Change<ExtensionSelection>,
    pub tools: Change<ToolSelection>,
    pub instructions: Change<String>,
    pub cwd: Change<String>,
}

/// Upstream's `field?: T | null` tri-state.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Change<T> {
    #[default]
    Keep,
    Set(T),
    Clear,
}

impl<T> Change<T> {
    /// Upstream `undefined`.
    pub fn is_keep(&self) -> bool {
        matches!(self, Change::Keep)
    }
}

/// Upstream `Extension` (erased): a named bundle of code; installed in a
/// registry and selected by conversations by name.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ExtensionSpec {
    pub name: String,
    /// Erased tool registrations, in order.
    pub tools: Vec<ToolSpec>,
    /// Erased prompt sections, in order.
    pub sections: Vec<SectionSpec>,
    /// Hook registrations: `(task name, handlers)` pairs in order.
    pub hooks: Vec<HookRegistrationSpec>,
    /// Applied where this extension is selected, in order.
    pub wraps: Vec<WrapSpec>,
    /// Names of the task definitions this extension contributes.
    pub tasks: Vec<String>,
}

/// Upstream `ToolRegistration` (erased): enough to resolve and order tools.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    /// Erased registration payload (schema, hooks, execute) held by the
    /// registry.
    pub payload: Value,
}

/// Upstream `PromptSection` (erased).
#[derive(Debug, Clone, PartialEq)]
pub struct SectionSpec {
    pub key: String,
    /// Default true: wrap the text as `<key>\n...\n</key>`.
    pub tag: bool,
    pub payload: Value,
}

/// Upstream `HookRegistration`.
#[derive(Debug, Clone, PartialEq)]
pub struct HookRegistrationSpec {
    pub task: String,
    /// Erased handlers object.
    pub handlers: Value,
}

/// Upstream `Wrap`: targets a tool name or a section key. Wrappers are pure.
#[derive(Debug, Clone, PartialEq)]
pub enum WrapSpec {
    Tool {
        tool: String,
        /// Erased wrapper payload.
        payload: Value,
    },
    Section {
        section: String,
        payload: Value,
    },
}

/// Resolved settings (upstream `Settings`): every field over its built-in
/// default, object fields merged.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// Absent: every installed extension, in install order.
    pub extensions: Option<Vec<ExtensionSpec>>,
    pub stream: ConversationStreamOptions,
    pub retry: ConversationRetryPolicy,
    pub compaction: CompactionPolicy,
    pub tool_execution: ToolExecutionMode,
    pub steering_mode: QueueMode,
    pub follow_up_mode: QueueMode,
}

/// Harness-wide run policy (upstream `HarnessSettings`). Read at every
/// resolution and never copied; getters are fine.
#[derive(Debug, Clone, Default)]
pub struct HarnessSettings {
    /// Default extension selection; absent: every installed extension, in
    /// install order.
    pub extensions: Option<Vec<ExtensionSpec>>,
    pub stream: ConversationStreamOptions,
    pub retry: Option<ConversationRetryPolicy>,
    pub compaction: Option<CompactionPolicy>,
    pub tool_execution: Option<ToolExecutionMode>,
    pub steering_mode: Option<QueueMode>,
    pub follow_up_mode: Option<QueueMode>,
}

/// A conversation's agent resolved against a registry snapshot and the
/// settings (upstream `Agent`, erased).
#[derive(Debug, Clone, PartialEq)]
pub struct Agent {
    pub model: Option<ModelRefSpec>,
    /// Upstream defaults the thinking level to `"off"`.
    pub thinking_level: String,
    pub extensions: Vec<ExtensionSpec>,
    /// The tools a request offers, in order.
    pub tools: Vec<ToolSpec>,
    /// Extension sections, then `instructions` when set.
    pub sections: Vec<SectionSpec>,
    pub instructions: Option<String>,
    pub cwd: Option<String>,
}

/// A change to `pi.agent` where the requested extensions/tools arrive as
/// objects (upstream accepts registration objects and stores their names).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AgentChangeFromObjects {
    pub model: Change<ModelRefSpec>,
    pub thinking_level: Change<String>,
    pub extensions: Change<Vec<ExtensionSpec>>,
    pub extensions_edit: Change<ExtensionSelection>,
    pub tools: Change<Vec<ToolSpec>>,
    pub tools_remove: Change<Vec<String>>,
    pub instructions: Change<String>,
    pub cwd: Change<String>,
}

/// Resolve the host settings (upstream `resolveSettings`): every field over
/// its built-in default, object fields merged.
pub fn resolve_settings(settings: Option<&HarnessSettings>) -> Settings {
    Settings {
        extensions: settings.and_then(|settings| settings.extensions.clone()),
        stream: settings
            .map(|settings| settings.stream.clone())
            .unwrap_or_default(),
        retry: merge_retry(settings.and_then(|settings| settings.retry.clone())),
        compaction: merge_compaction(settings.and_then(|settings| settings.compaction.clone())),
        tool_execution: settings
            .and_then(|settings| settings.tool_execution)
            .unwrap_or(ToolExecutionMode::Parallel),
        steering_mode: settings
            .and_then(|settings| settings.steering_mode)
            .unwrap_or(QueueMode::OneAtATime),
        follow_up_mode: settings
            .and_then(|settings| settings.follow_up_mode)
            .unwrap_or(QueueMode::OneAtATime),
    }
}

fn merge_retry(partial: Option<ConversationRetryPolicy>) -> ConversationRetryPolicy {
    let mut policy = default_retry_policy();
    if let Some(partial) = partial {
        policy.enabled = partial.enabled;
        policy.max_retries = partial.max_retries;
        policy.base_delay_ms = partial.base_delay_ms;
        // An absent optional field keeps the default (upstream
        //  skips keys the partial omits).
        policy.max_agent_delay_ms = partial.max_agent_delay_ms.or(policy.max_agent_delay_ms);
    }
    policy
}

fn merge_compaction(partial: Option<CompactionPolicy>) -> CompactionPolicy {
    let mut policy = default_compaction_policy();
    if let Some(partial) = partial {
        policy.enabled = partial.enabled;
        policy.reserve_tokens = partial.reserve_tokens;
        policy.keep_recent_tokens = partial.keep_recent_tokens;
        policy.background_tokens = partial.background_tokens;
    }
    policy
}

/// Apply one change to `pi.agent` (upstream `applyChange` behind
/// `configure`): a given field replaces the stored one, [`Change::Clear`]
/// deletes it, [`Change::Keep`] changes nothing.
pub fn apply_change(state: &mut AgentState, change: &AgentChange) {
    set(&mut state.model, &change.model);
    set(&mut state.thinking_level, &change.thinking_level);
    set(&mut state.extensions, &change.extensions);
    set(&mut state.tools, &change.tools);
    set(&mut state.instructions, &change.instructions);
    set(&mut state.cwd, &change.cwd);
}

fn set<T: Clone>(slot: &mut Option<T>, change: &Change<T>) {
    match change {
        Change::Keep => {}
        Change::Clear => *slot = None,
        Change::Set(value) => *slot = Some(value.clone()),
    }
}

/// Upstream `configure`: read-modify-write of `pi.agent`; the transaction
/// layer supplies the stored state draft and persists it.
pub fn configure(state: &mut AgentState, change: &AgentChange) {
    apply_change(state, change);
}

/// `addTools` of a tool round (upstream `addTools`): an array gets each name
/// it lacks appended, `{ remove }` loses the names, and unset tools already
/// offer every tool, so nothing is written. Returns whether the state changed
/// (upstream writes only then).
pub fn add_tools(state: &mut AgentState, added: &[String]) -> bool {
    let Some(tools) = state.tools.as_mut() else {
        return false;
    };
    match tools {
        ToolSelection::List(list) => {
            let mut changed = false;
            for name in added {
                if !list.iter().any(|existing| existing == name) {
                    list.push(name.clone());
                    changed = true;
                }
            }
            changed
        }
        ToolSelection::Remove { remove } => {
            let before = remove.len();
            remove.retain(|name| !added.contains(name));
            if remove.len() != before {
                // An empty removal filter is kept (upstream assigns the
                // filtered object back unconditionally on change).
                true
            } else {
                false
            }
        }
    }
}

/// Built-in part of every Harness commit that creates or forks a conversation
/// (upstream `createAgent`): a fork keeps its `asOf` copy (`parent` set); a
/// new task-owned conversation copies the stored agent of its owner task's
/// conversation; a new ownerless one starts empty. `parent` and `owner`
/// arrive resolved from the conversation record.
pub fn create_agent(
    state: &mut AgentState,
    parent: Option<()>,
    owner_agent: Option<&AgentState>,
) -> bool {
    if parent.is_some() {
        return false;
    }
    match owner_agent {
        None => false,
        Some(owner) => {
            *state = owner.clone();
            true
        }
    }
}

/// Handlers of the selected extensions' hooks for a task name, in extension
/// order (upstream `agentHooks`).
pub fn agent_hooks(agent: &Agent, task_name: &str) -> Vec<Value> {
    let mut handlers = Vec::new();
    for extension in &agent.extensions {
        for hook in &extension.hooks {
            if hook.task == task_name {
                handlers.push(hook.handlers.clone());
            }
        }
    }
    handlers
}

/// Resolve an agent from its stored state (absent: every field unset), a
/// registry snapshot, and resolved settings (upstream `resolveAgent`). A
/// wrapper that throws or renames drops its target and is reported; a
/// wrapper without a target does nothing. The erased wrappers are applied by
/// the caller through [`apply_wraps`]; this function composes, filters, and
/// orders.
pub fn resolve_agent(
    state: Option<&AgentState>,
    installed: &[ExtensionSpec],
    settings: &Settings,
) -> Agent {
    let extensions = select_extensions(
        state.and_then(|state| state.extensions.as_ref()),
        installed,
        settings,
    );

    // Compose tools and sections in extension order, later duplicates
    // replacing earlier ones in position.
    let mut composed: HashMap<String, ToolSpec> = HashMap::new();
    let mut tool_order: Vec<String> = Vec::new();
    for extension in &extensions {
        for tool in &extension.tools {
            if !composed.contains_key(&tool.name) {
                tool_order.push(tool.name.clone());
            }
            composed.insert(tool.name.clone(), tool.clone());
        }
    }
    let mut sections: HashMap<String, SectionSpec> = HashMap::new();
    let mut section_order: Vec<String> = Vec::new();
    for extension in &extensions {
        for section in &extension.sections {
            if !sections.contains_key(&section.key) {
                section_order.push(section.key.clone());
            }
            sections.insert(section.key.clone(), section.clone());
        }
    }

    let filter = state.and_then(|state| state.tools.as_ref());
    let tools: Vec<ToolSpec> = match filter {
        None => tool_order
            .iter()
            .filter_map(|name| composed.get(name).cloned())
            .collect(),
        Some(ToolSelection::List(names)) => {
            let mut seen = HashSet::new();
            let mut tools = Vec::new();
            for name in names {
                if seen.insert(name.clone()) {
                    if let Some(tool) = composed.get(name) {
                        tools.push(tool.clone());
                    }
                }
            }
            tools
        }
        Some(ToolSelection::Remove { remove }) => {
            let removed: HashSet<&String> = remove.iter().collect();
            tool_order
                .iter()
                .filter(|name| !removed.contains(*name))
                .filter_map(|name| composed.get(name).cloned())
                .collect()
        }
    };

    let instructions = state.and_then(|state| state.instructions.clone());
    let mut agent_sections: Vec<SectionSpec> = section_order
        .iter()
        .filter_map(|key| sections.get(key).cloned())
        .collect();
    if let Some(instructions) = &instructions {
        agent_sections.push(SectionSpec {
            key: INSTRUCTIONS_KEY.to_string(),
            tag: true,
            payload: Value::String(instructions.clone()),
        });
    }

    Agent {
        model: state.and_then(|state| state.model.clone()),
        thinking_level: state
            .and_then(|state| state.thinking_level.clone())
            .unwrap_or_else(|| "off".to_string()),
        extensions,
        tools,
        sections: agent_sections,
        instructions,
        cwd: state.and_then(|state| state.cwd.clone()),
    }
}

/// The wrap application the caller drives between composition and filtering
/// (upstream's inline wrap loop): looks the target up, applies `wrap`, drops
/// a renamed result or a throwing wrapper (reported), and keeps insertion
/// order.
pub fn apply_wrap<T>(
    items: &mut Vec<(String, T)>,
    target: &str,
    wrap: impl FnOnce(T) -> Result<T, String>,
    name_of: impl Fn(&T) -> String,
    report: &mut dyn FnMut(&str),
) {
    let Some(index) = items.iter().position(|(key, _)| key == target) else {
        return;
    };
    let item = items.remove(index).1;
    match wrap(item) {
        Ok(wrapped) => {
            if name_of(&wrapped) != target {
                report(&format!(
                    "Wrapper renamed {target} to {}",
                    name_of(&wrapped)
                ));
                return;
            }
            items.insert(index, (target.to_string(), wrapped));
        }
        Err(error) => {
            report(&error);
        }
    }
}

/// Selected installed extensions (upstream `selectExtensions`): the stored
/// array, or the default selection edited by `{ add, remove }`. The default
/// selection is the settings' list, else every installed extension in install
/// order. First occurrence of each name wins.
pub fn select_extensions(
    stored: Option<&ExtensionSelection>,
    installed: &[ExtensionSpec],
    settings: &Settings,
) -> Vec<ExtensionSpec> {
    let selected: Vec<String> = match stored {
        Some(ExtensionSelection::List(names)) => names.clone(),
        stored => {
            let base: Vec<String> = match settings.extensions {
                Some(ref extensions) => extensions
                    .iter()
                    .map(|extension| extension.name.clone())
                    .collect(),
                None => installed
                    .iter()
                    .map(|extension| extension.name.clone())
                    .collect(),
            };
            let removed: HashSet<&String> = match stored {
                Some(ExtensionSelection::Edit {
                    remove: Some(remove),
                    ..
                }) => remove.iter().collect(),
                _ => HashSet::new(),
            };
            let added: Vec<String> = match stored {
                Some(ExtensionSelection::Edit { add: Some(add), .. }) => add.clone(),
                _ => Vec::new(),
            };
            base.into_iter()
                .chain(added)
                .filter(|name| !removed.contains(name))
                .collect()
        }
    };
    let mut seen = HashSet::new();
    let mut extensions = Vec::new();
    for name in selected {
        if seen.insert(name.clone()) {
            if let Some(extension) = installed.iter().find(|extension| extension.name == name) {
                extensions.push(extension.clone());
            }
        }
    }
    extensions
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str) -> ToolSpec {
        ToolSpec {
            name: name.to_string(),
            payload: Value::Null,
        }
    }

    fn extension(name: &str, tools: &[&str]) -> ExtensionSpec {
        ExtensionSpec {
            name: name.to_string(),
            tools: tools.iter().map(|name| tool(name)).collect(),
            ..ExtensionSpec::default()
        }
    }

    #[test]
    fn resolve_settings_applies_builtin_defaults() {
        let settings = resolve_settings(None);
        assert_eq!(settings.retry, default_retry_policy());
        assert_eq!(settings.compaction, default_compaction_policy());
        assert_eq!(settings.tool_execution, ToolExecutionMode::Parallel);
        assert_eq!(settings.steering_mode, QueueMode::OneAtATime);
        assert_eq!(settings.follow_up_mode, QueueMode::OneAtATime);
        assert!(settings.extensions.is_none());
    }

    #[test]
    fn resolve_settings_merges_partials() {
        let settings = resolve_settings(Some(&HarnessSettings {
            retry: Some(ConversationRetryPolicy {
                enabled: false,
                max_retries: 1,
                base_delay_ms: 5.0,
                max_agent_delay_ms: None,
            }),
            tool_execution: Some(ToolExecutionMode::Sequential),
            ..HarnessSettings::default()
        }));
        assert!(!settings.retry.enabled);
        assert_eq!(settings.retry.max_retries, 1);
        assert_eq!(settings.retry.base_delay_ms, 5.0);
        // Unset partial fields fall back to the default.
        assert_eq!(settings.retry.max_agent_delay_ms, Some(60000.0));
        assert_eq!(settings.tool_execution, ToolExecutionMode::Sequential);
        assert_eq!(settings.compaction, default_compaction_policy());
    }

    #[test]
    fn resolve_agent_composes_filters_and_defaults() {
        let installed = vec![
            extension("a", &["read", "edit"]),
            extension("b", &["bash", "read"]),
        ];
        let settings = resolve_settings(None);
        let agent = resolve_agent(None, &installed, &settings);
        // Without a default selection: every installed extension, in install
        // order; later duplicates replace earlier ones in position.
        assert_eq!(
            agent
                .tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            vec!["read", "edit", "bash"]
        );
        assert_eq!(agent.thinking_level, "off");
        assert!(agent.model.is_none());
        assert!(agent.instructions.is_none());

        // Exact tool list, in list order, first occurrence only.
        let state = AgentState {
            extensions: Some(ExtensionSelection::List(vec![
                "b".to_string(),
                "a".to_string(),
                "b".to_string(),
            ])),
            tools: Some(ToolSelection::List(vec![
                "bash".to_string(),
                "read".to_string(),
                "bash".to_string(),
                "missing".to_string(),
            ])),
            instructions: Some("be brief".into()),
            ..AgentState::default()
        };
        let agent = resolve_agent(Some(&state), &installed, &settings);
        assert_eq!(
            agent
                .tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            vec!["bash", "read"]
        );
        // Selected extensions dedupe to first occurrences.
        assert_eq!(
            agent
                .extensions
                .iter()
                .map(|extension| extension.name.as_str())
                .collect::<Vec<_>>(),
            vec!["b", "a"]
        );
        // Instructions become the trailing reserved section.
        assert_eq!(
            agent.sections.last().map(|section| section.key.as_str()),
            Some(INSTRUCTIONS_KEY)
        );
        assert_eq!(agent.instructions.as_deref(), Some("be brief"));

        // Removal filter keeps composition order.
        let state = AgentState {
            tools: Some(ToolSelection::Remove {
                remove: vec!["edit".into()],
            }),
            ..AgentState::default()
        };
        let agent = resolve_agent(Some(&state), &installed, &settings);
        assert_eq!(
            agent
                .tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            vec!["read", "bash"]
        );
    }

    #[test]
    fn select_extensions_edits_the_default_selection() {
        let installed = vec![
            extension("a", &[]),
            extension("b", &[]),
            extension("c", &[]),
        ];
        let mut settings = resolve_settings(None);
        settings.extensions = Some(vec![extension("b", &[]), extension("a", &[])]);
        let edit = ExtensionSelection::Edit {
            add: Some(vec!["c".into(), "missing".into()]),
            remove: Some(vec!["a".into(), "ghost".into()]),
        };
        let selected = select_extensions(Some(&edit), &installed, &settings);
        assert_eq!(
            selected
                .iter()
                .map(|extension| extension.name.as_str())
                .collect::<Vec<_>>(),
            vec!["b", "c"]
        );
    }

    #[test]
    fn apply_change_tri_state() {
        let mut state = AgentState::default();
        apply_change(
            &mut state,
            &AgentChange {
                model: Change::Set(ModelRefSpec {
                    provider: "p".into(),
                    model_id: "m".into(),
                }),
                thinking_level: Change::Set("high".into()),
                cwd: Change::Clear,
                ..AgentChange::default()
            },
        );
        assert_eq!(state.model.as_ref().unwrap().provider, "p");
        assert_eq!(state.thinking_level.as_deref(), Some("high"));
        assert!(state.cwd.is_none());
        apply_change(
            &mut state,
            &AgentChange {
                model: Change::Clear,
                thinking_level: Change::Keep,
                ..AgentChange::default()
            },
        );
        assert!(state.model.is_none());
        assert_eq!(state.thinking_level.as_deref(), Some("high"));
    }

    #[test]
    fn add_tools_edits_both_selection_shapes() {
        // Exact list gains the missing names in order.
        let mut state = AgentState {
            tools: Some(ToolSelection::List(vec!["read".into()])),
            ..AgentState::default()
        };
        assert!(add_tools(
            &mut state,
            &["read".to_string(), "edit".to_string()]
        ));
        assert_eq!(
            state.tools,
            Some(ToolSelection::List(vec!["read".into(), "edit".into()]))
        );
        // Removal filter loses the added names.
        let mut state = AgentState {
            tools: Some(ToolSelection::Remove {
                remove: vec!["edit".into(), "bash".into()],
            }),
            ..AgentState::default()
        };
        assert!(add_tools(&mut state, &["edit".to_string()]));
        assert_eq!(
            state.tools,
            Some(ToolSelection::Remove {
                remove: vec!["bash".into()]
            })
        );
        // Unset tools already offer everything.
        let mut state = AgentState::default();
        assert!(!add_tools(&mut state, &["edit".to_string()]));
        assert!(state.tools.is_none());
    }

    #[test]
    fn create_agent_copies_the_owner_agent() {
        let owner = AgentState {
            instructions: Some("inherited".into()),
            ..AgentState::default()
        };
        let mut state = AgentState::default();
        // Fork (parent set): keeps its asOf copy — nothing to do.
        assert!(!create_agent(&mut state, Some(()), Some(&owner)));
        assert!(state.instructions.is_none());
        // Task-owned: copies the owner's stored agent.
        assert!(create_agent(&mut state, None, Some(&owner)));
        assert_eq!(state.instructions.as_deref(), Some("inherited"));
        // Ownerless: starts empty.
        let mut state = AgentState::default();
        assert!(!create_agent(&mut state, None, None));
        assert_eq!(state, AgentState::default());
    }

    #[test]
    fn agent_hooks_collect_in_extension_order() {
        let hook = |task: &str, tag: &str| HookRegistrationSpec {
            task: task.to_string(),
            handlers: Value::String(tag.to_string()),
        };
        let mut a = extension("a", &[]);
        a.hooks = vec![hook("pi.generation", "a-gen"), hook("pi.tool", "a-tool")];
        let mut b = extension("b", &[]);
        b.hooks = vec![hook("pi.generation", "b-gen")];
        let settings = resolve_settings(None);
        let agent = resolve_agent(None, &[a, b], &settings);
        let hooks = agent_hooks(&agent, "pi.generation");
        assert_eq!(
            hooks,
            vec![Value::String("a-gen".into()), Value::String("b-gen".into())]
        );
        assert_eq!(
            agent_hooks(&agent, "pi.tool"),
            vec![Value::String("a-tool".into())]
        );
        assert!(agent_hooks(&agent, "other").is_empty());
    }

    #[test]
    fn apply_wrap_replaces_in_place_and_drops_renames() {
        let mut items = vec![("one".to_string(), 1), ("two".to_string(), 2)];
        let mut reported: Vec<String> = Vec::new();
        apply_wrap(
            &mut items,
            "two",
            |value| Ok(value * 10),
            |_value| "two".to_string(),
            &mut |error| reported.push(error.to_string()),
        );
        assert_eq!(items, vec![("one".to_string(), 1), ("two".to_string(), 20)]);
        // A renamed result drops the target and reports.
        apply_wrap(
            &mut items,
            "one",
            Ok,
            |_| "other".to_string(),
            &mut |error| reported.push(error.to_string()),
        );
        assert_eq!(items, vec![("two".to_string(), 20)]);
        assert_eq!(reported.len(), 1);
        // A throwing wrapper drops the target and reports.
        apply_wrap(
            &mut items,
            "two",
            |_| Err("boom".to_string()),
            |_| "two".to_string(),
            &mut |error| reported.push(error.to_string()),
        );
        assert!(items.is_empty());
        assert_eq!(reported.len(), 2);
        // A wrapper without a target does nothing.
        apply_wrap(
            &mut items,
            "ghost",
            |value: i32| Ok(value),
            |_| "ghost".to_string(),
            &mut |error| reported.push(error.to_string()),
        );
        assert_eq!(reported.len(), 2);
    }
}
