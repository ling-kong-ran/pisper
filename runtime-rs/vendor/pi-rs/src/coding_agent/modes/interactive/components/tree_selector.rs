//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Port of `modes/interactive/components/tree-selector.ts` (1427 lines,
//! sha256 `731ce9fe6c07d0b1a55a11ce2987f21771376b7651e28355c98fbdf1f053a931`):
//! the session-tree selector with fold state, filter modes, search, label
//! editing and the horizontal viewport.
//!
//! Slice conventions: see [`super::model_selector`] (theme seam, inline
//! composite rendering, `key_text`/`key_hint`/merged keybinding registry).
//! Further disclosed substitutions:
//! - **Node arena**: upstream `TreeList` holds `SessionTreeNode` objects and
//!   `FlatNode` references them. The port flattens the caller's tree into an
//!   arena (`Vec<ArenaNode>`, preorder) so labels can be mutated through
//!   `update_node_label` without self-referential borrows; `FlatNode` stores
//!   arena indices. Every parent/child walk and iteration order mirrors the
//!   upstream index arithmetic.
//! - **JS sort stability**: `Array#sort` is stable; the ported
//!   contains-active root sort uses Rust's stable `sort_by` (ties keep the
//!   caller's root order, like upstream).
//! - **UTF-16 slicing (S19.2-class seam, see `diff.rs`)**: `String#slice`
//!   display/copy truncations (`slice(0,80)`, `slice(0,200)`, the JSON arg
//!   previews) are char-based here; the oracle scenarios are ASCII so the
//!   byte comparisons are unaffected.
//! - **Clock seam (S20.5)**: `formatLabelTimestamp` reads `new Date()`
//!   upstream; the port renders label timestamps from the UTC fields of an
//!   injectable clock (`set_clock`, default `SystemTime` — the oracle pins
//!   `Date` to the same FIXED_NOW and reads UTC fields), and
//!   `new Date().toISOString()` default label stamps use the same clock. The
//!   empty-tree constructor's `setTimeout(onCancel, 100)` becomes the explicit
//!   `take_auto_cancel` drain.
//! - **Event queue**: the tree list reports `onSelect`/`onCancel`/`onCopy`/
//!   `onLabelEdit` through a shared queue that [`TreeSelectorComponent`]
//!   drains after `handle_input` and forwards to its own callbacks (upstream
//!   forwards synchronously through closures over `this`; the queue preserves
//!   the exact dispatch order without aliasing `self`).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use crate::agent_core::harness::session::jsonl::iso8601::{format_iso8601_utc, parse_iso8601_utc};
use crate::agent_core::types::AgentMessage;
use crate::ai::types::{AssistantBlock, StopReason, StringOrBlocks, TextOrImageBlock};
use crate::coding_agent::core::messages::CustomMessageContent;
use crate::coding_agent::modes::interactive::theme::Theme;
use crate::coding_agent::session_manager::{SessionEntry, SessionTreeNode};
use crate::tui::component::Component;
use crate::tui::components::input::{Input, InputOptions};
use crate::tui::components::text::Text;
use crate::tui::utils::{slice_by_column, truncate_to_width, visible_width, wrap_text_with_ansi};

use crate::tui::keybindings::with_keybindings;

use super::model_selector::{
    format_key_text, key_hint, keybindings_match, spacer_lines, theme_bg, theme_fg, DynamicBorder,
};

/// Local mirror of `model_selector::registry_keys` (private there; this
/// slice must not touch that file): the global registry lookup with the
/// merged coding-agent app.* fallback.
fn registry_keys(keybinding: &str) -> Vec<String> {
    let keys = with_keybindings(|kb| kb.get_keys(keybinding));
    if !keys.is_empty() {
        return keys;
    }
    crate::coding_agent::core::keybindings::keybindings()
        .into_iter()
        .find(|(id, _)| *id == keybinding)
        .map(|(_, definition)| definition.default_keys)
        .unwrap_or_default()
}

const TREE_GUTTER_WIDTH: usize = 2;
const MIN_VISIBLE_ANCHOR_CONTENT_WIDTH: usize = 4;
const MAX_VISIBLE_ANCHOR_CONTENT_WIDTH: usize = 20;
const MIN_ANCHOR_CONTEXT_WIDTH: usize = 2;
const MAX_ANCHOR_CONTEXT_WIDTH: usize = 12;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Upstream `new Date().toISOString()` against the injectable clock.
fn iso_now(clock: fn() -> i64) -> String {
    format_iso8601_utc(clock())
}

/// The five local-time fields `formatLabelTimestamp` reads
/// (`getFullYear/getMonth/getDate/getHours/getMinutes`) resolved to the UTC
/// fields of the instant (S20.5 clock seam).
fn civil_fields(epoch_ms: i64) -> (i64, i64, i64, i64, i64) {
    let iso = format_iso8601_utc(epoch_ms);
    // "YYYY-MM-DDTHH:MM:SS.mmmZ"
    let year: i64 = iso[0..4].parse().unwrap_or(1970);
    let month: i64 = iso[5..7].parse().unwrap_or(1);
    let day: i64 = iso[8..10].parse().unwrap_or(1);
    let hour: i64 = iso[11..13].parse().unwrap_or(0);
    let minute: i64 = iso[14..16].parse().unwrap_or(0);
    (year, month, day, hour, minute)
}

/// Upstream `FilterMode`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FilterMode {
    #[default]
    Default,
    NoTools,
    UserOnly,
    LabeledOnly,
    All,
}

impl FilterMode {
    /// The upstream union literal (also the oracle JSON spelling).
    pub fn as_str(self) -> &'static str {
        match self {
            FilterMode::Default => "default",
            FilterMode::NoTools => "no-tools",
            FilterMode::UserOnly => "user-only",
            FilterMode::LabeledOnly => "labeled-only",
            FilterMode::All => "all",
        }
    }

    /// Cycle order used by the `app.tree.filter.cycle*` bindings.
    const CYCLE: [FilterMode; 5] = [
        FilterMode::Default,
        FilterMode::NoTools,
        FilterMode::UserOnly,
        FilterMode::LabeledOnly,
        FilterMode::All,
    ];
}

/// Gutter info: position (displayIndent where connector was) and whether to
/// show `│` (upstream `GutterInfo`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GutterInfo {
    position: usize,
    show: bool,
}

/// Flattened tree node for navigation (upstream `FlatNode`); `node` is an
/// arena index instead of a shared object reference.
#[derive(Clone, Debug)]
struct FlatNode {
    node: usize,
    /// Indentation level (each level = 3 chars).
    indent: usize,
    /// Whether to show connector (`├─` or `└─`) - true if parent has multiple
    /// children.
    show_connector: bool,
    /// If showConnector, true = last sibling (`└─`), false = not last (`├─`).
    is_last: bool,
    /// Gutter info for each ancestor branch point.
    gutters: Vec<GutterInfo>,
    /// True if this node is a root under a virtual branching root (multiple
    /// roots).
    is_virtual_root_child: bool,
}

struct HorizontalViewportRow {
    gutter: String,
    body: String,
    anchor_col: usize,
    body_width: usize,
    is_selected: bool,
}

/// Render tree rows into a horizontally clipped viewport (upstream
/// `renderHorizontalViewport`).
fn render_horizontal_viewport(rows: &[HorizontalViewportRow], width: usize) -> Vec<String> {
    let viewport_width = width.saturating_sub(TREE_GUTTER_WIDTH);
    let max_body_width = rows.iter().map(|row| row.body_width).max().unwrap_or(0);
    let max_horizontal_scroll = max_body_width.saturating_sub(viewport_width);
    let selected_row = rows.iter().find(|row| row.is_selected);

    // Only pan horizontally when needed to keep enough selected-row content
    // visible after its anchor.
    let mut horizontal_scroll = 0;
    if let Some(selected_row) = selected_row {
        if max_horizontal_scroll > 0 {
            let min_visible_anchor_content_width = MAX_VISIBLE_ANCHOR_CONTENT_WIDTH
                .min(MIN_VISIBLE_ANCHOR_CONTENT_WIDTH.max(viewport_width / 3));
            if selected_row.anchor_col > viewport_width - min_visible_anchor_content_width {
                let anchor_context_width =
                    MAX_ANCHOR_CONTEXT_WIDTH.min(MIN_ANCHOR_CONTEXT_WIDTH.max(viewport_width / 4));
                horizontal_scroll = max_horizontal_scroll
                    .min(selected_row.anchor_col.saturating_sub(anchor_context_width));
            }
        }
    }

    // Clip only the body; the fixed-width gutter remains visible as navigation
    // context.
    rows.iter()
        .map(|row| {
            let line = if horizontal_scroll > 0 {
                format!(
                    "{}{}\u{1b}[0m",
                    row.gutter,
                    slice_by_column(&row.body, horizontal_scroll, viewport_width, true)
                )
            } else {
                format!("{}{}", row.gutter, row.body)
            };
            truncate_to_width(&line, width, "", false)
        })
        .collect()
}

/// Tool call info for lookup (upstream `ToolCallInfo`).
#[derive(Clone, Debug)]
struct ToolCallInfo {
    name: String,
    arguments: serde_json::Value,
}

/// One arena node: the mutable upstream `SessionTreeNode` object graph.
#[derive(Debug, Clone)]
pub struct ArenaNode {
    entry: SessionEntry,
    parent: Option<usize>,
    children: Vec<usize>,
    pub label: Option<String>,
    pub label_timestamp: Option<String>,
}

impl ArenaNode {
    pub fn id(&self) -> Option<&str> {
        self.entry.id()
    }
}

/// Events the tree list emits through the shared queue (upstream: the
/// `onSelect`/`onCancel`/`onCopy`/`onLabelEdit` closure calls).
pub(crate) enum TreeSelectorEvent {
    Select(String),
    Cancel,
    Copy(Option<String>),
    LabelEdit {
        entry_id: String,
        current_label: Option<String>,
    },
}

pub(crate) type EventQueue = Arc<Mutex<Vec<TreeSelectorEvent>>>;

fn push_event(queue: &EventQueue, event: TreeSelectorEvent) {
    queue.lock().expect("tree selector event queue").push(event);
}

/// Upstream `TreeList`: the tree rows, fold state, filters and key handling.
pub struct TreeList {
    arena: Vec<ArenaNode>,
    roots: Vec<usize>,
    flat_nodes: Vec<FlatNode>,
    filtered_nodes: Vec<usize>,
    selected_index: usize,
    current_leaf_id: Option<String>,
    max_visible_lines: usize,
    filter_mode: FilterMode,
    search_query: String,
    tool_call_map: HashMap<String, ToolCallInfo>,
    multiple_roots: bool,
    show_label_timestamps: bool,
    active_path_ids: HashSet<String>,
    /// arena index -> visible parent arena index (`None` for visible roots).
    visible_parent_map: HashMap<usize, Option<usize>>,
    /// `None`-keyed root bucket plus per-node children, in filtered order.
    visible_children_map: HashMap<Option<usize>, Vec<usize>>,
    last_selected_id: Option<String>,
    folded_nodes: HashSet<String>,
    /// S20.5 clock seam: the `new Date()` read points.
    clock: fn() -> i64,
    theme: Arc<Theme>,

    on_select: Option<Box<dyn FnMut(&str) + Send>>,
    on_cancel: Option<Box<dyn FnMut() + Send>>,
    on_copy: Option<Box<dyn FnMut(Option<&str>) + Send>>,
    on_label_edit: Option<Box<dyn FnMut(&str, Option<&str>) + Send>>,
}

impl TreeList {
    /// Upstream constructor. `tree` is borrowed: the arena is built from it.
    pub fn new(
        tree: &[SessionTreeNode],
        current_leaf_id: Option<&str>,
        max_visible_lines: usize,
        initial_selected_id: Option<&str>,
        initial_filter_mode: Option<FilterMode>,
        theme: Arc<Theme>,
    ) -> Self {
        let mut list = Self {
            arena: Vec::new(),
            roots: Vec::new(),
            flat_nodes: Vec::new(),
            filtered_nodes: Vec::new(),
            selected_index: 0,
            current_leaf_id: current_leaf_id.map(str::to_string),
            max_visible_lines,
            filter_mode: initial_filter_mode.unwrap_or_default(),
            search_query: String::new(),
            tool_call_map: HashMap::new(),
            multiple_roots: false,
            show_label_timestamps: false,
            active_path_ids: HashSet::new(),
            visible_parent_map: HashMap::new(),
            visible_children_map: HashMap::new(),
            last_selected_id: None,
            folded_nodes: HashSet::new(),
            clock: now_ms,
            theme,
            on_select: None,
            on_cancel: None,
            on_copy: None,
            on_label_edit: None,
        };
        list.build_arena(tree);
        list.multiple_roots = tree.len() > 1;
        list.flat_nodes = list.flatten_tree();
        list.build_active_path();
        list.apply_filter();

        // Start with initialSelectedId if provided, otherwise current leaf
        let target_id = initial_selected_id.or(current_leaf_id);
        list.selected_index = list.find_nearest_visible_index(target_id);
        list.last_selected_id = list
            .filtered_nodes
            .get(list.selected_index)
            .and_then(|&flat| list.arena[flat].id().map(str::to_string));
        list
    }

    /// S20.5 clock seam (test injection).
    pub fn set_clock(&mut self, clock: fn() -> i64) {
        self.clock = clock;
    }

    /// The event sink overrides (used by [`TreeSelectorComponent`]).
    pub(crate) fn set_callbacks(
        &mut self,
        on_select: Option<Box<dyn FnMut(&str) + Send>>,
        on_cancel: Option<Box<dyn FnMut() + Send>>,
        on_copy: Option<Box<dyn FnMut(Option<&str>) + Send>>,
        on_label_edit: Option<Box<dyn FnMut(&str, Option<&str>) + Send>>,
    ) {
        self.on_select = on_select;
        self.on_cancel = on_cancel;
        self.on_copy = on_copy;
        self.on_label_edit = on_label_edit;
    }

    /// Expand the caller's tree into the preorder arena (roots keep the
    /// caller's order; parent links are arena indices).
    fn build_arena(&mut self, roots: &[SessionTreeNode]) {
        self.arena.clear();
        self.roots.clear();
        // Reverse push keeps the pop order (and therefore the preorder arena
        // numbering) in the caller's root order.
        let mut stack: Vec<(&SessionTreeNode, Option<usize>)> =
            roots.iter().rev().map(|root| (root, None)).collect();
        while let Some((node, parent)) = stack.pop() {
            let index = self.arena.len();
            self.arena.push(ArenaNode {
                entry: node.entry.clone(),
                parent,
                children: Vec::new(),
                label: node.label.clone(),
                label_timestamp: node.label_timestamp.clone(),
            });
            if let Some(parent) = parent {
                self.arena[parent].children.push(index);
            } else {
                self.roots.push(index);
            }
            // Reverse push keeps preorder numbering left-to-right.
            for child in node.children.iter().rev() {
                stack.push((child, Some(index)));
            }
        }
    }

    /// The selected entry id (`getSelectedNode()?.entry.id`).
    pub fn selected_entry_id(&self) -> Option<&str> {
        self.filtered_nodes
            .get(self.selected_index)
            .and_then(|&flat| self.arena[flat].id())
    }

    /// The selected arena node (`getSelectedNode()`); `None` on an empty
    /// filtered list.
    pub fn selected_node(&self) -> Option<&ArenaNode> {
        self.filtered_nodes
            .get(self.selected_index)
            .map(|&flat| &self.arena[flat])
    }

    /// `getSearchQuery()`.
    pub fn search_query(&self) -> &str {
        &self.search_query
    }

    /// The active filter mode (status-suffix surface).
    pub fn filter_mode(&self) -> FilterMode {
        self.filter_mode
    }

    /// Number of visible (filtered) nodes.
    pub fn filtered_len(&self) -> usize {
        self.filtered_nodes.len()
    }

    /// Whether label timestamps are currently shown.
    pub fn show_label_timestamps(&self) -> bool {
        self.show_label_timestamps
    }

    /// The filtered arena ids in display order (debug/test surface).
    #[cfg(test)]
    pub(crate) fn filtered_ids(&self) -> Vec<String> {
        self.filtered_nodes
            .iter()
            .filter_map(|&flat| self.arena[flat].id().map(str::to_string))
            .collect()
    }

    /// Upstream `findNearestVisibleIndex`.
    fn find_nearest_visible_index(&self, entry_id: Option<&str>) -> usize {
        if self.filtered_nodes.is_empty() {
            return 0;
        }
        let Some(entry_id) = entry_id else {
            return self.filtered_nodes.len() - 1;
        };

        // Visible entry ids to their positions in filteredNodes
        let visible_id_to_index: HashMap<&str, usize> = self
            .filtered_nodes
            .iter()
            .enumerate()
            .filter_map(|(i, &flat)| self.arena[flat].id().map(|id| (id, i)))
            .collect();

        // Walk from entryId up to root, looking for a visible entry
        let mut current_id = entry_id.to_string();
        loop {
            if let Some(&index) = visible_id_to_index.get(current_id.as_str()) {
                return index;
            }
            let node = self
                .arena
                .iter()
                .find(|node| node.id() == Some(current_id.as_str()));
            let Some(node) = node else {
                break;
            };
            match node
                .parent
                .map(|parent| &self.arena[parent])
                .and_then(|parent| parent.id())
            {
                Some(parent_id) => current_id = parent_id.to_string(),
                None => break,
            }
        }

        // Fallback: last visible entry
        self.filtered_nodes.len() - 1
    }

    /// Upstream `buildActivePath`: the entry ids on the root-to-leaf path.
    fn build_active_path(&mut self) {
        self.active_path_ids.clear();
        let Some(mut current_id) = self.current_leaf_id.clone() else {
            return;
        };
        loop {
            self.active_path_ids.insert(current_id.clone());
            let node = self
                .arena
                .iter()
                .find(|node| node.id() == Some(current_id.as_str()));
            let Some(node) = node else {
                break;
            };
            match node
                .parent
                .map(|parent| &self.arena[parent])
                .and_then(|parent| parent.id())
            {
                Some(parent_id) => current_id = parent_id.to_string(),
                None => break,
            }
        }
    }

    /// Whether each subtree contains the active leaf (upstream
    /// `containsActive`, computed with the same post-order effect).
    fn compute_contains_active(&self) -> HashMap<usize, bool> {
        let mut contains_active: HashMap<usize, bool> = HashMap::new();
        let leaf_id = self.current_leaf_id.as_deref();
        // The arena is in preorder (parents before children), so a reverse
        // scan processes children before parents.
        for index in (0..self.arena.len()).rev() {
            let mut has = leaf_id == self.arena[index].id();
            for &child in &self.arena[index].children {
                if contains_active.get(&child).copied().unwrap_or(false) {
                    has = true;
                }
            }
            contains_active.insert(index, has);
        }
        contains_active
    }

    /// Upstream `flattenTree`: preorder flat list plus tool-call extraction.
    fn flatten_tree(&mut self) -> Vec<FlatNode> {
        let mut result: Vec<FlatNode> = Vec::new();
        self.tool_call_map.clear();

        let contains_active = self.compute_contains_active();

        // Add roots in reverse order, prioritizing the one containing the
        // active leaf. If multiple roots, treat them as children of a virtual
        // root that branches.
        let multiple_roots = self.roots.len() > 1;
        let mut ordered_roots = self.roots.clone();
        ordered_roots.sort_by(|a, b| {
            let a_active = contains_active.get(a).copied().unwrap_or(false);
            let b_active = contains_active.get(b).copied().unwrap_or(false);
            b_active.cmp(&a_active) // stable: ties keep the caller's order
        });
        let mut stack: Vec<(usize, usize, bool, bool, bool, Vec<GutterInfo>, bool)> = Vec::new();
        for i in (0..ordered_roots.len()).rev() {
            let is_last = i == ordered_roots.len() - 1;
            stack.push((
                ordered_roots[i],
                if multiple_roots { 1 } else { 0 },
                multiple_roots,
                multiple_roots,
                is_last,
                Vec::new(),
                multiple_roots,
            ));
        }

        while let Some((
            node,
            indent,
            just_branched,
            show_connector,
            is_last,
            gutters,
            is_virtual_root_child,
        )) = stack.pop()
        {
            // Extract tool calls from assistant messages for later lookup
            if let SessionEntry::Message(message) = &self.arena[node].entry {
                if let AgentMessage::Assistant(assistant) = &message.message {
                    for block in &assistant.content {
                        if let AssistantBlock::ToolCall(tool_call) = block {
                            self.tool_call_map.insert(
                                tool_call.id.clone(),
                                ToolCallInfo {
                                    name: tool_call.name.clone(),
                                    arguments: tool_call.arguments.clone(),
                                },
                            );
                        }
                    }
                }
            }

            result.push(FlatNode {
                node,
                indent,
                show_connector,
                is_last,
                gutters: gutters.clone(),
                is_virtual_root_child,
            });

            let children = self.arena[node].children.clone();
            let multiple_children = children.len() > 1;

            // Order children so the branch containing the active leaf comes first
            let mut prioritized: Vec<usize> = Vec::new();
            let mut rest: Vec<usize> = Vec::new();
            for &child in &children {
                if contains_active.get(&child).copied().unwrap_or(false) {
                    prioritized.push(child);
                } else {
                    rest.push(child);
                }
            }
            prioritized.extend(rest);
            let ordered_children = prioritized;

            // Calculate child indent: parent branches (and the first
            // generation after a branch) shift +1 for visual grouping;
            // single-child chains stay flat
            let child_indent = if multiple_children || (just_branched && indent > 0) {
                indent + 1
            } else {
                indent
            };

            // Build gutters for children. If this node showed a connector, add
            // a gutter entry for descendants; only add a gutter if the
            // connector is actually displayed (not suppressed for virtual root
            // children). The connector sits at position (displayIndent - 1).
            let connector_displayed = show_connector && !is_virtual_root_child;
            let current_display_indent = if self.multiple_roots {
                indent.saturating_sub(1)
            } else {
                indent
            };
            let connector_position = current_display_indent.saturating_sub(1);
            let mut child_gutters = gutters;
            if connector_displayed {
                child_gutters.push(GutterInfo {
                    position: connector_position,
                    show: !is_last,
                });
            }

            // Add children in reverse order
            for i in (0..ordered_children.len()).rev() {
                let child_is_last = i == ordered_children.len() - 1;
                stack.push((
                    ordered_children[i],
                    child_indent,
                    multiple_children,
                    multiple_children,
                    child_is_last,
                    child_gutters.clone(),
                    false,
                ));
            }
        }

        result
    }

    /// Upstream `applyFilter`: filter modes, search tokens, folded subtrees,
    /// visual structure recalculation and selection restoration.
    fn apply_filter(&mut self) {
        // Update lastSelectedId only when we have a valid selection
        // (non-empty list); preserves the selection across empty filters.
        if !self.filtered_nodes.is_empty() {
            if let Some(&flat) = self.filtered_nodes.get(self.selected_index) {
                if let Some(id) = self.arena[flat].id() {
                    self.last_selected_id = Some(id.to_string());
                }
            }
        }

        let search_tokens: Vec<String> = self
            .search_query
            .to_lowercase()
            .split_whitespace()
            .filter(|token| !token.is_empty())
            .map(str::to_string)
            .collect();

        let mut filtered: Vec<usize> = Vec::new();
        for (flat_index, flat_node) in self.flat_nodes.iter().enumerate() {
            let arena_node = &self.arena[flat_node.node];
            let Some(entry_id) = arena_node.id() else {
                // Unparsed entries have no id upstream either; they never
                // join the filtered view.
                continue;
            };
            // Upstream hides `usage` entries from every filter mode before
            // any other check.
            if matches!(arena_node.entry, SessionEntry::Usage(_)) {
                continue;
            }
            let is_current_leaf = Some(entry_id) == self.current_leaf_id.as_deref();

            // Skip assistant messages with only tool calls (no text) unless
            // error/aborted. Always show the current leaf so the active
            // position is visible.
            if let SessionEntry::Message(message) = &arena_node.entry {
                if let AgentMessage::Assistant(assistant) = &message.message {
                    if !is_current_leaf {
                        let has_text = assistant_has_text_content(&assistant.content);
                        let is_error_or_aborted = !matches!(
                            assistant.stop_reason,
                            StopReason::Stop | StopReason::ToolUse
                        );
                        // Only hide if no text AND not an error/aborted message
                        if !has_text && !is_error_or_aborted {
                            continue;
                        }
                    }
                }
            }

            // Apply filter mode. Entry types hidden in the default view
            // (settings/bookkeeping).
            let is_settings_entry = matches!(
                arena_node.entry,
                SessionEntry::Label(_)
                    | SessionEntry::ContextEdit(_)
                    | SessionEntry::Custom(_)
                    | SessionEntry::ModelChange(_)
                    | SessionEntry::ThinkingLevelChange(_)
                    | SessionEntry::SessionInfo(_)
            );

            let passes_filter = match self.filter_mode {
                FilterMode::UserOnly => matches!(
                    &arena_node.entry,
                    SessionEntry::Message(message)
                        if matches!(&message.message, AgentMessage::User(_))
                ),
                FilterMode::NoTools => {
                    !is_settings_entry
                        && !matches!(
                            &arena_node.entry,
                            SessionEntry::Message(message)
                                if matches!(&message.message, AgentMessage::ToolResult(_))
                        )
                }
                FilterMode::LabeledOnly => arena_node.label.is_some(),
                FilterMode::All => true,
                FilterMode::Default => !is_settings_entry,
            };
            if !passes_filter {
                continue;
            }

            // Apply search filter
            if !search_tokens.is_empty() {
                let node_text = self.get_searchable_text(flat_node.node).to_lowercase();
                if !search_tokens
                    .iter()
                    .all(|token| node_text.contains(token.as_str()))
                {
                    continue;
                }
            }

            filtered.push(flat_index);
        }
        self.filtered_nodes = filtered;

        // Filter out descendants of folded nodes.
        if !self.folded_nodes.is_empty() {
            let mut skip_set: HashSet<String> = HashSet::new();
            for flat_node in &self.flat_nodes {
                let arena_node = &self.arena[flat_node.node];
                if let (Some(id), Some(parent)) = (arena_node.id(), arena_node.parent) {
                    if let Some(parent_id) = self.arena[parent].id() {
                        if self.folded_nodes.contains(parent_id) || skip_set.contains(parent_id) {
                            skip_set.insert(id.to_string());
                        }
                    }
                }
            }
            self.filtered_nodes.retain(|&flat| {
                self.arena[self.flat_nodes[flat].node]
                    .id()
                    .map(|id| !skip_set.contains(id))
                    .unwrap_or(true)
            });
        }

        // Recalculate visual structure (indent, connectors, gutters)
        self.recalculate_visual_structure();

        // Try to preserve cursor on the same node, or find the nearest visible
        // ancestor.
        if let Some(last_selected_id) = self.last_selected_id.clone() {
            self.selected_index = self.find_nearest_visible_index(Some(&last_selected_id));
        } else if self.selected_index >= self.filtered_nodes.len() {
            // Clamp index if out of bounds
            self.selected_index = self.filtered_nodes.len().saturating_sub(1);
        }

        // Update lastSelectedId to the actual selection (may have moved to the
        // nearest visible ancestor).
        if !self.filtered_nodes.is_empty() {
            if let Some(&flat) = self.filtered_nodes.get(self.selected_index) {
                if let Some(id) = self.arena[flat].id() {
                    self.last_selected_id = Some(id.to_string());
                }
            }
        }
    }

    /// Upstream `recalculateVisualStructure`: recompute indentation,
    /// connectors and gutters for the filtered view.
    fn recalculate_visual_structure(&mut self) {
        if self.filtered_nodes.is_empty() {
            return;
        }

        let visible_ids: HashSet<usize> = self.filtered_nodes.iter().copied().collect();

        // Nearest visible ancestor for a node (arena index), `None` for roots.
        let find_visible_ancestor = |arena: &[ArenaNode], mut current: usize| -> Option<usize> {
            while let Some(parent) = arena[current].parent {
                if visible_ids.contains(&parent) {
                    return Some(parent);
                }
                current = parent;
            }
            None
        };

        // Build the visible tree structure.
        let mut visible_parent: HashMap<usize, Option<usize>> = HashMap::new();
        let mut visible_children: HashMap<Option<usize>, Vec<usize>> = HashMap::new();
        visible_children.insert(None, Vec::new()); // root-level nodes

        for &flat in &self.filtered_nodes {
            let node = self.flat_nodes[flat].node;
            let ancestor = find_visible_ancestor(&self.arena, node);
            visible_parent.insert(node, ancestor);
            visible_children.entry(ancestor).or_default().push(node);
        }

        // Update multipleRoots based on visible roots
        self.multiple_roots = visible_children
            .get(&None)
            .map(|roots| roots.len() > 1)
            .unwrap_or(false);

        // DFS over the visible tree using flattenTree() indentation
        // semantics; roots are pushed in reverse so they pop forward.
        let mut stack: Vec<(usize, usize, bool, bool, bool, Vec<GutterInfo>, bool)> = Vec::new();
        let visible_roots = visible_children.get(&None).cloned().unwrap_or_default();
        for i in (0..visible_roots.len()).rev() {
            let is_last = i == visible_roots.len() - 1;
            stack.push((
                visible_roots[i],
                if self.multiple_roots { 1 } else { 0 },
                self.multiple_roots,
                self.multiple_roots,
                is_last,
                Vec::new(),
                self.multiple_roots,
            ));
        }

        while let Some((
            node,
            indent,
            just_branched,
            show_connector,
            is_last,
            gutters,
            is_virtual_root_child,
        )) = stack.pop()
        {
            // Update this node's visual properties
            if let Some(flat_node) = self
                .flat_nodes
                .iter_mut()
                .find(|flat_node| flat_node.node == node)
            {
                flat_node.indent = indent;
                flat_node.show_connector = show_connector;
                flat_node.is_last = is_last;
                flat_node.gutters = gutters.clone();
                flat_node.is_virtual_root_child = is_virtual_root_child;
            }

            // Visible children of this node
            let children = visible_children
                .get(&Some(node))
                .cloned()
                .unwrap_or_default();
            let multiple_children = children.len() > 1;

            // Child indent follows flattenTree(): branch points (and first
            // generation after a branch) shift +1
            let child_indent = if multiple_children || (just_branched && indent > 0) {
                indent + 1
            } else {
                indent
            };

            // Child gutters follow flattenTree() connector/gutter rules
            let connector_displayed = show_connector && !is_virtual_root_child;
            let current_display_indent = if self.multiple_roots {
                indent.saturating_sub(1)
            } else {
                indent
            };
            let connector_position = current_display_indent.saturating_sub(1);
            let mut child_gutters = gutters;
            if connector_displayed {
                child_gutters.push(GutterInfo {
                    position: connector_position,
                    show: !is_last,
                });
            }

            // Add children in reverse order (processed forward via the stack)
            for i in (0..children.len()).rev() {
                let child_is_last = i == children.len() - 1;
                stack.push((
                    children[i],
                    child_indent,
                    multiple_children,
                    multiple_children,
                    child_is_last,
                    child_gutters.clone(),
                    false,
                ));
            }
        }

        // Store the visible tree maps for ancestor/descendant lookups
        self.visible_parent_map = visible_parent;
        self.visible_children_map = visible_children;
    }

    /// Upstream `getSearchableText`.
    fn get_searchable_text(&self, node: usize) -> String {
        let arena_node = &self.arena[node];
        let mut parts: Vec<String> = Vec::new();

        if let Some(label) = &arena_node.label {
            parts.push(label.clone());
        }

        match &arena_node.entry {
            SessionEntry::Message(message) => {
                let msg = &message.message;
                parts.push(msg.role().to_string());
                let text = match msg {
                    AgentMessage::User(user) => extract_string_or_blocks(&user.content),
                    AgentMessage::Assistant(assistant) => {
                        extract_assistant_full(&assistant.content)
                    }
                    AgentMessage::ToolResult(tool_result) => {
                        extract_blocks_text(&tool_result.content)
                    }
                    AgentMessage::System(system) => extract_string_or_blocks(&system.content),
                    _ => String::new(),
                };
                if !text.is_empty() {
                    parts.push(extract_content(&text));
                }
                if let AgentMessage::Custom(custom) = msg {
                    if custom.role == "bashExecution" {
                        if let Some(command) = custom.data.get("command").and_then(|c| c.as_str()) {
                            parts.push(command.to_string());
                        }
                    }
                }
            }
            SessionEntry::CustomMessage(custom) => {
                parts.push(custom.custom_type.clone());
                if let Some(content) = &custom.content {
                    match content {
                        CustomMessageContent::Text(text) => parts.push(text.clone()),
                        CustomMessageContent::Blocks(blocks) => {
                            parts.push(extract_content(&extract_blocks_text(blocks)));
                        }
                    }
                }
            }
            SessionEntry::Compaction(_) => parts.push("compaction".to_string()),
            SessionEntry::BranchSummary(branch) => {
                parts.push("branch summary".to_string());
                parts.push(branch.summary.clone());
            }
            SessionEntry::SessionInfo(info) => {
                parts.push("title".to_string());
                if let Some(name) = &info.name {
                    parts.push(name.clone());
                }
            }
            SessionEntry::ModelChange(change) => {
                parts.push("model".to_string());
                parts.push(change.model_id.clone());
            }
            SessionEntry::ThinkingLevelChange(change) => {
                parts.push("thinking".to_string());
                parts.push(change.thinking_level.clone());
            }
            SessionEntry::Custom(custom) => {
                parts.push("custom".to_string());
                parts.push(custom.custom_type.clone());
            }
            SessionEntry::Label(label) => {
                parts.push("label".to_string());
                parts.push(label.label.clone().unwrap_or_default());
            }
            SessionEntry::ContextEdit(edit) => {
                parts.push("context edit".to_string());
                parts.push(
                    if edit.replacement.is_none() {
                        "omit"
                    } else {
                        "replace"
                    }
                    .to_string(),
                );
                parts.push(edit.target_id.clone());
            }
            // State-only entries (usage) carry no searchable display text.
            SessionEntry::Usage(_) => {}
            SessionEntry::Unparsed(_) => {}
        }

        parts.join(" ")
    }

    /// Upstream `copySelected`.
    pub fn copy_selected(&mut self) {
        let text = self
            .selected_node()
            .and_then(|node| self.get_entry_copy_text(node));
        if let Some(on_copy) = &mut self.on_copy {
            on_copy(text.as_deref());
        }
    }

    /// Upstream `updateNodeLabel`.
    pub fn update_node_label(
        &mut self,
        entry_id: &str,
        label: Option<&str>,
        label_timestamp: Option<&str>,
    ) {
        for node in &mut self.arena {
            if node.id() == Some(entry_id) {
                node.label = label.map(str::to_string);
                node.label_timestamp = if label.is_some() {
                    Some(
                        label_timestamp
                            .map(str::to_string)
                            .unwrap_or_else(|| iso_now(self.clock)),
                    )
                } else {
                    None
                };
                break;
            }
        }
    }

    /// The status suffix labels (`[no-tools]`, `[+label time]`, …).
    fn get_status_labels(&self) -> String {
        let mut labels = String::new();
        match self.filter_mode {
            FilterMode::NoTools => labels.push_str(" [no-tools]"),
            FilterMode::UserOnly => labels.push_str(" [user]"),
            FilterMode::LabeledOnly => labels.push_str(" [labeled]"),
            FilterMode::All => labels.push_str(" [all]"),
            FilterMode::Default => {}
        }
        if self.show_label_timestamps {
            labels.push_str(" [+label time]");
        }
        labels
    }

    /// Upstream `render`.
    pub fn render_list(&mut self, width: usize) -> Vec<String> {
        let theme = Arc::clone(&self.theme);
        let mut lines: Vec<String> = Vec::new();

        if self.filtered_nodes.is_empty() {
            lines.push(truncate_to_width(
                &theme_fg(&theme, "muted", "  No entries found"),
                width,
                "",
                false,
            ));
            let status = format!("  (0/0){}", self.get_status_labels());
            lines.push(truncate_to_width(
                &theme_fg(&theme, "muted", &status),
                width,
                "",
                false,
            ));
            return lines;
        }

        let start_index = self
            .selected_index
            .saturating_sub(self.max_visible_lines / 2)
            .min(
                self.filtered_nodes
                    .len()
                    .saturating_sub(self.max_visible_lines),
            );
        let end_index = (start_index + self.max_visible_lines).min(self.filtered_nodes.len());

        let mut rendered_rows: Vec<HorizontalViewportRow> = Vec::new();
        for i in start_index..end_index {
            let flat_node = &self.flat_nodes[self.filtered_nodes[i]];
            let arena_node = &self.arena[flat_node.node];
            let Some(entry_id) = arena_node.id() else {
                continue;
            };
            let is_selected = i == self.selected_index;

            // Build line: cursor + prefix + path marker + label + content
            let cursor = if is_selected {
                theme_fg(&theme, "accent", "› ")
            } else {
                "  ".to_string()
            };

            // If multiple roots, shift display (roots at 0, not 1)
            let display_indent = if self.multiple_roots {
                flat_node.indent.saturating_sub(1)
            } else {
                flat_node.indent
            };

            // Connector + fold indicator placement
            let connector = if flat_node.show_connector && !flat_node.is_virtual_root_child {
                if flat_node.is_last {
                    "└─ "
                } else {
                    "├─ "
                }
            } else {
                ""
            };
            let connector_position = if connector.is_empty() {
                None
            } else {
                Some(display_indent - 1)
            };

            // Build prefix char by char, placing gutters and connector at
            // their positions
            let total_chars = display_indent * 3;
            let is_folded = self.folded_nodes.contains(entry_id);
            let mut prefix_chars: Vec<char> = Vec::with_capacity(total_chars);
            for i in 0..total_chars {
                let level = i / 3;
                let pos_in_level = i % 3;

                // Check if there's a gutter at this level
                if let Some(gutter) = flat_node.gutters.iter().find(|g| g.position == level) {
                    if pos_in_level == 0 {
                        prefix_chars.push(if gutter.show { '│' } else { ' ' });
                    } else {
                        prefix_chars.push(' ');
                    }
                } else if connector_position == Some(level) {
                    // Connector at this level, with fold indicator
                    if pos_in_level == 0 {
                        prefix_chars.push(if flat_node.is_last { '└' } else { '├' });
                    } else if pos_in_level == 1 {
                        let foldable = self.is_foldable(entry_id);
                        prefix_chars.push(if is_folded {
                            '⊞'
                        } else if foldable {
                            '⊟'
                        } else {
                            '─'
                        });
                    } else {
                        prefix_chars.push(' ');
                    }
                } else {
                    prefix_chars.push(' ');
                }
            }
            let prefix: String = prefix_chars.into_iter().collect();

            // Fold marker for nodes without connectors (roots)
            let shows_fold_in_connector =
                flat_node.show_connector && !flat_node.is_virtual_root_child;
            let fold_marker = if is_folded && !shows_fold_in_connector {
                theme_fg(&theme, "accent", "⊞ ")
            } else {
                String::new()
            };

            // Active path marker - shown right before the entry text
            let is_on_active_path = self.active_path_ids.contains(entry_id);
            let path_marker = if is_on_active_path {
                theme_fg(&theme, "accent", "• ")
            } else {
                String::new()
            };

            let label = match &arena_node.label {
                Some(label) => theme_fg(&theme, "warning", &format!("[{label}] ")),
                None => String::new(),
            };
            let label_timestamp = match (
                self.show_label_timestamps,
                &arena_node.label,
                &arena_node.label_timestamp,
            ) {
                (true, Some(_), Some(timestamp)) => theme_fg(
                    &theme,
                    "muted",
                    &format!("{} ", self.format_label_timestamp(timestamp)),
                ),
                _ => String::new(),
            };
            let content = self.get_entry_display_text(flat_node.node, is_selected);
            let prefix_part = theme_fg(&theme, "dim", &prefix) + &fold_marker + &path_marker;
            let anchor_col = visible_width(&prefix_part);
            let mut gutter = cursor;
            let mut body = format!("{prefix_part}{label}{label_timestamp}{content}");
            if is_selected {
                gutter = theme_bg(&theme, "selectedBg", &gutter);
                body = theme_bg(&theme, "selectedBg", &body);
            }
            let body_width = visible_width(&body);
            rendered_rows.push(HorizontalViewportRow {
                gutter,
                body,
                anchor_col,
                body_width,
                is_selected,
            });
        }

        lines.extend(render_horizontal_viewport(&rendered_rows, width));
        let status = format!(
            "  ({}/{}){}",
            self.selected_index + 1,
            self.filtered_nodes.len(),
            self.get_status_labels()
        );
        lines.push(truncate_to_width(
            &theme_fg(&theme, "muted", &status),
            width,
            "",
            false,
        ));

        lines
    }

    /// Upstream `getEntryDisplayText`.
    fn get_entry_display_text(&self, node: usize, is_selected: bool) -> String {
        let theme = Arc::clone(&self.theme);
        let arena_node = &self.arena[node];
        let entry = &arena_node.entry;
        let normalize = |s: &str| normalize_newlines_tabs(s);

        let result: String = match entry {
            SessionEntry::Message(message) => {
                let msg = &message.message;
                match msg {
                    AgentMessage::User(user) => {
                        let content = normalize(&extract_string_or_blocks(&user.content));
                        theme_fg(&theme, "accent", "user: ") + &content
                    }
                    AgentMessage::Assistant(assistant) => {
                        let text_content = normalize(&extract_assistant_full(&assistant.content));
                        if !text_content.is_empty() {
                            theme_fg(&theme, "success", "assistant: ") + &text_content
                        } else if assistant.stop_reason == StopReason::Aborted {
                            theme_fg(&theme, "success", "assistant: ")
                                + &theme_fg(&theme, "muted", "(aborted)")
                        } else if let Some(error_message) = &assistant.error_message {
                            let err_msg: String =
                                normalize(error_message).chars().take(80).collect();
                            theme_fg(&theme, "success", "assistant: ")
                                + &theme_fg(&theme, "error", &err_msg)
                        } else {
                            theme_fg(&theme, "success", "assistant: ")
                                + &theme_fg(&theme, "muted", "(no content)")
                        }
                    }
                    AgentMessage::ToolResult(tool_result) => {
                        let tool_call = self.tool_call_map.get(&tool_result.tool_call_id);
                        match tool_call {
                            Some(tool_call) => theme_fg(
                                &theme,
                                "muted",
                                &self.format_tool_call(&tool_call.name, &tool_call.arguments),
                            ),
                            None => {
                                theme_fg(&theme, "muted", &format!("[{}]", tool_result.tool_name))
                            }
                        }
                    }
                    AgentMessage::Custom(custom) if custom.role == "bashExecution" => {
                        let command = custom
                            .data
                            .get("command")
                            .and_then(|c| c.as_str())
                            .unwrap_or("");
                        theme_fg(&theme, "dim", &format!("[bash]: {}", normalize(command)))
                    }
                    other => theme_fg(&theme, "dim", &format!("[{}]", other.role())),
                }
            }
            SessionEntry::CustomMessage(custom) => {
                let content = match &custom.content {
                    Some(CustomMessageContent::Text(text)) => text.clone(),
                    Some(CustomMessageContent::Blocks(blocks)) => blocks
                        .iter()
                        .filter_map(|block| match block {
                            TextOrImageBlock::Text(text) => Some(text.text.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join(""),
                    None => String::new(),
                };
                theme_fg(
                    &theme,
                    "customMessageLabel",
                    &format!("[{}]: ", custom.custom_type),
                ) + &normalize(&content)
            }
            SessionEntry::Compaction(compaction) => {
                let tokens = ((compaction.tokens_before as f64) / 1000.0).round() as i64;
                theme_fg(
                    &theme,
                    "borderAccent",
                    &format!("[compaction: {tokens}k tokens]"),
                )
            }
            SessionEntry::BranchSummary(branch) => {
                theme_fg(&theme, "warning", "[branch summary]: ") + &normalize(&branch.summary)
            }
            SessionEntry::ModelChange(change) => {
                theme_fg(&theme, "dim", &format!("[model: {}]", change.model_id))
            }
            SessionEntry::ThinkingLevelChange(change) => theme_fg(
                &theme,
                "dim",
                &format!("[thinking: {}]", change.thinking_level),
            ),
            SessionEntry::Custom(custom) => {
                theme_fg(&theme, "dim", &format!("[custom: {}]", custom.custom_type))
            }
            SessionEntry::Label(label) => theme_fg(
                &theme,
                "dim",
                &format!("[label: {}]", label.label.as_deref().unwrap_or("(cleared)")),
            ),
            SessionEntry::ContextEdit(edit) => theme_fg(
                &theme,
                "dim",
                &format!(
                    "[context {}: {}]",
                    if edit.replacement.is_none() {
                        "omit"
                    } else {
                        "replace"
                    },
                    edit.target_id
                ),
            ),
            SessionEntry::SessionInfo(info) => match &info.name {
                Some(name) => {
                    theme_fg(&theme, "dim", "[title: ")
                        + &theme_fg(&theme, "dim", name)
                        + &theme_fg(&theme, "dim", "]")
                }
                None => {
                    theme_fg(&theme, "dim", "[title: ")
                        + &theme.italic(&theme_fg(&theme, "dim", "empty"))
                        + &theme_fg(&theme, "dim", "]")
                }
            },
            SessionEntry::Usage(_) => String::new(),
            SessionEntry::Unparsed(_) => String::new(),
        };

        if is_selected {
            theme.bold(&result)
        } else {
            result
        }
    }

    /// Upstream `formatLabelTimestamp` against the injected clock.
    fn format_label_timestamp(&self, timestamp: &str) -> String {
        self.format_label_timestamp_at(timestamp, (self.clock)())
    }

    /// The pure form of `formatLabelTimestamp` (S20.5 clock seam).
    fn format_label_timestamp_at(&self, timestamp: &str, now: i64) -> String {
        let Some(parsed) = parse_iso8601_utc(timestamp) else {
            return String::new();
        };
        let (year, month, day, hour, minute) = civil_fields(parsed);
        let time = format!("{hour:02}:{minute:02}");

        let (now_year, now_month, now_day, _, _) = civil_fields(now);
        if (year, month, day) == (now_year, now_month, now_day) {
            return time;
        }

        if year == now_year {
            return format!("{month}/{day} {time}");
        }

        let short_year = year.abs() % 100;
        format!("{short_year:02}/{month}/{day} {time}")
    }

    /// Upstream `getEntryCopyText`.
    fn get_entry_copy_text(&self, node: &ArenaNode) -> Option<String> {
        let entry = &node.entry;
        let text: Option<String> = match entry {
            SessionEntry::Message(message) => match &message.message {
                AgentMessage::Custom(custom) if custom.role == "bashExecution" => custom
                    .data
                    .get("command")
                    .and_then(|c| c.as_str())
                    .map(str::to_string),
                AgentMessage::User(user) => Some(extract_string_or_blocks(&user.content)),
                AgentMessage::Assistant(assistant) => {
                    let mut text = extract_assistant_full(&assistant.content);
                    if text.is_empty() {
                        if let Some(error_message) = &assistant.error_message {
                            text = error_message.clone();
                        }
                    }
                    Some(text)
                }
                AgentMessage::ToolResult(tool_result) => {
                    Some(extract_blocks_text(&tool_result.content))
                }
                _ => None,
            },
            SessionEntry::CustomMessage(custom) => {
                custom.content.as_ref().map(|content| match content {
                    CustomMessageContent::Text(text) => text.clone(),
                    CustomMessageContent::Blocks(blocks) => {
                        extract_content(&extract_blocks_text(blocks))
                    }
                })
            }
            SessionEntry::Compaction(compaction) => Some(compaction.summary.clone()),
            SessionEntry::BranchSummary(branch) => Some(branch.summary.clone()),
            _ => None,
        };

        text.filter(|text| !text.trim().is_empty())
    }

    /// Upstream `formatToolCall`.
    fn format_tool_call(&self, name: &str, args: &serde_json::Value) -> String {
        let shorten_path = |p: &str| -> String {
            let home = std::env::var("HOME")
                .or_else(|_| std::env::var("USERPROFILE"))
                .unwrap_or_default();
            if !home.is_empty() {
                if let Some(rest) = p.strip_prefix(&home) {
                    return format!("~{rest}");
                }
            }
            p.to_string()
        };
        // `String(args.x || args.y || …)`: the first truthy value (empty
        // strings are falsy upstream), or "" when none matches.
        let arg_str = |keys: &[&str]| -> String {
            for key in keys {
                match args.get(*key) {
                    Some(serde_json::Value::String(value)) if !value.is_empty() => {
                        return value.clone();
                    }
                    Some(serde_json::Value::Number(value)) => return value.to_string(),
                    Some(serde_json::Value::Bool(value)) => return value.to_string(),
                    _ => {}
                }
            }
            String::new()
        };
        // `args.path || "."` for the path-taking tools.
        let arg_path = |default: &str| -> String {
            let path = arg_str(&["path"]);
            if path.is_empty() {
                default.to_string()
            } else {
                path
            }
        };

        match name {
            "read" => {
                let path = shorten_path(&arg_str(&["path", "file_path"]));
                let offset = args.get("offset").and_then(|v| v.as_i64());
                let limit = args.get("limit").and_then(|v| v.as_i64());
                let mut display = path;
                if offset.is_some() || limit.is_some() {
                    let start = offset.unwrap_or(1);
                    let end = limit.map(|limit| (start + limit - 1).to_string());
                    display += &format!(":{start}");
                    if let Some(end) = end {
                        display += &format!("-{end}");
                    }
                }
                format!("[read: {display}]")
            }
            "write" => {
                let path = shorten_path(&arg_str(&["path", "file_path"]));
                format!("[write: {path}]")
            }
            "edit" => {
                let path = shorten_path(&arg_str(&["path", "file_path"]));
                format!("[edit: {path}]")
            }
            "bash" => {
                let raw_cmd = arg_str(&["command"]);
                let cmd: String = normalize_newlines_tabs(&raw_cmd)
                    .trim()
                    .chars()
                    .take(50)
                    .collect();
                let suffix = if raw_cmd.chars().count() > 50 {
                    "..."
                } else {
                    ""
                };
                format!("[bash: {cmd}{suffix}]")
            }
            "grep" => {
                let pattern = arg_str(&["pattern"]);
                let path = shorten_path(&arg_path("."));
                format!("[grep: /{pattern}/ in {path}]")
            }
            "find" => {
                let pattern = arg_str(&["pattern"]);
                let path = shorten_path(&arg_path("."));
                format!("[find: {pattern} in {path}]")
            }
            "ls" => {
                let path = shorten_path(&arg_path("."));
                format!("[ls: {path}]")
            }
            _ => {
                // Custom tool - show name and truncated JSON args
                let args_json = serde_json::to_string(args).unwrap_or_default();
                let args_str: String = args_json.chars().take(40).collect();
                let suffix = if args_json.chars().count() > 40 {
                    "..."
                } else {
                    ""
                };
                format!("[{name}: {args_str}{suffix}]")
            }
        }
    }

    /// Upstream `handleInput`.
    pub fn handle_input(&mut self, data: &str) {
        if keybindings_match(data, "tui.select.up") {
            self.selected_index = if self.selected_index == 0 {
                self.filtered_nodes.len().saturating_sub(1)
            } else {
                self.selected_index - 1
            };
        } else if keybindings_match(data, "tui.select.down") {
            self.selected_index = if self.selected_index == self.filtered_nodes.len() - 1 {
                0
            } else {
                self.selected_index + 1
            };
        } else if keybindings_match(data, "app.tree.foldOrUp") {
            let current_id = self
                .filtered_nodes
                .get(self.selected_index)
                .and_then(|&flat| self.arena[flat].id().map(str::to_string));
            if let Some(current_id) = current_id {
                if self.is_foldable(&current_id) && !self.folded_nodes.contains(&current_id) {
                    self.folded_nodes.insert(current_id);
                    self.apply_filter();
                    return;
                }
            }
            self.selected_index = self.find_branch_segment_start(BranchDirection::Up);
        } else if keybindings_match(data, "app.tree.unfoldOrDown") {
            let current_id = self
                .filtered_nodes
                .get(self.selected_index)
                .and_then(|&flat| self.arena[flat].id().map(str::to_string));
            if let Some(current_id) = current_id {
                if self.folded_nodes.remove(&current_id) {
                    self.apply_filter();
                    return;
                }
            }
            self.selected_index = self.find_branch_segment_start(BranchDirection::Down);
        } else if keybindings_match(data, "tui.editor.cursorLeft")
            || keybindings_match(data, "tui.select.pageUp")
        {
            // Page up
            self.selected_index = self.selected_index.saturating_sub(self.max_visible_lines);
        } else if keybindings_match(data, "tui.editor.cursorRight")
            || keybindings_match(data, "tui.select.pageDown")
        {
            // Page down
            self.selected_index = (self.selected_index + self.max_visible_lines)
                .min(self.filtered_nodes.len().saturating_sub(1));
        } else if keybindings_match(data, "tui.select.confirm") {
            let selected_id = self
                .filtered_nodes
                .get(self.selected_index)
                .and_then(|&flat| self.arena[flat].id().map(str::to_string));
            if let (Some(selected_id), Some(on_select)) = (selected_id, &mut self.on_select) {
                on_select(&selected_id);
            }
        } else if keybindings_match(data, "app.message.copy") {
            self.copy_selected();
        } else if keybindings_match(data, "tui.select.cancel") {
            if !self.search_query.is_empty() {
                self.search_query.clear();
                self.folded_nodes.clear();
                self.apply_filter();
            } else if let Some(on_cancel) = &mut self.on_cancel {
                on_cancel();
            }
        } else if keybindings_match(data, "app.tree.filter.default") {
            // Direct filter: default
            self.filter_mode = FilterMode::Default;
            self.folded_nodes.clear();
            self.apply_filter();
        } else if keybindings_match(data, "app.tree.filter.noTools") {
            // Toggle filter: no-tools ↔ default
            self.filter_mode = if self.filter_mode == FilterMode::NoTools {
                FilterMode::Default
            } else {
                FilterMode::NoTools
            };
            self.folded_nodes.clear();
            self.apply_filter();
        } else if keybindings_match(data, "app.tree.filter.userOnly") {
            // Toggle filter: user-only ↔ default
            self.filter_mode = if self.filter_mode == FilterMode::UserOnly {
                FilterMode::Default
            } else {
                FilterMode::UserOnly
            };
            self.folded_nodes.clear();
            self.apply_filter();
        } else if keybindings_match(data, "app.tree.filter.labeledOnly") {
            // Toggle filter: labeled-only ↔ default
            self.filter_mode = if self.filter_mode == FilterMode::LabeledOnly {
                FilterMode::Default
            } else {
                FilterMode::LabeledOnly
            };
            self.folded_nodes.clear();
            self.apply_filter();
        } else if keybindings_match(data, "app.tree.filter.all") {
            // Toggle filter: all ↔ default
            self.filter_mode = if self.filter_mode == FilterMode::All {
                FilterMode::Default
            } else {
                FilterMode::All
            };
            self.folded_nodes.clear();
            self.apply_filter();
        } else if keybindings_match(data, "app.tree.filter.cycleBackward") {
            // Cycle filter backwards
            let modes = FilterMode::CYCLE;
            self.filter_mode = match modes.iter().position(|mode| *mode == self.filter_mode) {
                Some(index) => modes[(index + modes.len() - 1) % modes.len()],
                None => FilterMode::Default,
            };
            self.folded_nodes.clear();
            self.apply_filter();
        } else if keybindings_match(data, "app.tree.filter.cycleForward") {
            // Cycle filter forwards: default → no-tools → user-only →
            // labeled-only → all → default
            let modes = FilterMode::CYCLE;
            self.filter_mode = match modes.iter().position(|mode| *mode == self.filter_mode) {
                Some(index) => modes[(index + 1) % modes.len()],
                None => FilterMode::Default,
            };
            self.folded_nodes.clear();
            self.apply_filter();
        } else if keybindings_match(data, "tui.editor.deleteCharBackward") {
            if !self.search_query.is_empty() {
                self.search_query.pop();
                self.folded_nodes.clear();
                self.apply_filter();
            }
        } else if keybindings_match(data, "app.tree.editLabel") {
            let selected = self
                .filtered_nodes
                .get(self.selected_index)
                .map(|&flat| &self.arena[flat]);
            if let Some(selected) = selected {
                let entry_id = selected.id().map(str::to_string);
                let label = selected.label.clone();
                if let (Some(entry_id), Some(on_label_edit)) = (entry_id, &mut self.on_label_edit) {
                    on_label_edit(&entry_id, label.as_deref());
                }
            }
        } else if keybindings_match(data, "app.tree.toggleLabelTimestamp") {
            self.show_label_timestamps = !self.show_label_timestamps;
        } else {
            let has_control_chars = data.chars().any(|ch| {
                let code = ch as u32;
                code < 32 || code == 0x7f || (0x80..=0x9f).contains(&code)
            });
            if !has_control_chars && !data.is_empty() {
                self.search_query.push_str(data);
                self.folded_nodes.clear();
                self.apply_filter();
            }
        }
    }

    /// Whether a node can be folded: it has visible children and is either a
    /// root (no visible parent) or a segment start (visible parent has
    /// multiple visible children). Upstream `isFoldable`.
    fn is_foldable(&self, entry_id: &str) -> bool {
        let Some(node) = self
            .arena
            .iter()
            .position(|node| node.id() == Some(entry_id))
        else {
            return false;
        };
        let Some(children) = self.visible_children_map.get(&Some(node)) else {
            return false;
        };
        if children.is_empty() {
            return false;
        }
        match self.visible_parent_map.get(&node) {
            None | Some(None) => true,
            Some(Some(parent)) => self
                .visible_children_map
                .get(&Some(*parent))
                .map(|siblings| siblings.len() > 1)
                .unwrap_or(false),
        }
    }

    /// Upstream `findBranchSegmentStart`.
    fn find_branch_segment_start(&self, direction: BranchDirection) -> usize {
        let selected_id = self
            .filtered_nodes
            .get(self.selected_index)
            .and_then(|&flat| self.arena[flat].id().map(str::to_string));
        let Some(selected_id) = selected_id else {
            return self.selected_index;
        };

        // arena index -> filtered position
        let index_by_node: HashMap<usize, usize> = self
            .filtered_nodes
            .iter()
            .enumerate()
            .map(|(i, &flat)| (self.flat_nodes[flat].node, i))
            .collect();
        let Some(mut current_node) = self
            .arena
            .iter()
            .position(|node| node.id() == Some(selected_id.as_str()))
        else {
            return self.selected_index;
        };

        if direction == BranchDirection::Down {
            // Walks visible children, always following the first child.
            loop {
                let children = self
                    .visible_children_map
                    .get(&Some(current_node))
                    .cloned()
                    .unwrap_or_default();
                if children.is_empty() {
                    return index_by_node
                        .get(&current_node)
                        .copied()
                        .unwrap_or(self.selected_index);
                }
                if children.len() > 1 {
                    return index_by_node
                        .get(&children[0])
                        .copied()
                        .unwrap_or(self.selected_index);
                }
                current_node = children[0];
            }
        }

        // direction === "up": walk the visible parent chain.
        loop {
            let parent = self
                .visible_parent_map
                .get(&current_node)
                .copied()
                .flatten();
            let Some(parent) = parent else {
                return index_by_node
                    .get(&current_node)
                    .copied()
                    .unwrap_or(self.selected_index);
            };
            let children = self
                .visible_children_map
                .get(&Some(parent))
                .cloned()
                .unwrap_or_default();
            if children.len() > 1 {
                if let Some(segment_start) = index_by_node.get(&current_node).copied() {
                    if segment_start < self.selected_index {
                        return segment_start;
                    }
                }
            }
            current_node = parent;
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BranchDirection {
    Up,
    Down,
}

/// `s.replace(/[\n\t]/g, " ").trim()`.
fn normalize_newlines_tabs(s: &str) -> String {
    let replaced = s.replace(['\n', '\t'], " ");
    replaced.trim().to_string()
}

/// Upstream `extractContent`: full content sliced to 200 chars.
fn extract_content(content: &str) -> String {
    content.chars().take(200).collect()
}

/// Upstream `extractFullContent` over `string | TextContent[]`.
fn extract_string_or_blocks(content: &StringOrBlocks) -> String {
    match content {
        StringOrBlocks::Text(text) => text.clone(),
        StringOrBlocks::Blocks(blocks) => extract_blocks_text(blocks),
    }
}

/// Text-only concatenation over `TextContent | ImageContent` blocks.
fn extract_blocks_text(blocks: &[TextOrImageBlock]) -> String {
    let mut result = String::new();
    for block in blocks {
        if let TextOrImageBlock::Text(text) = block {
            result.push_str(&text.text);
        }
    }
    result
}

/// Text-only concatenation over assistant blocks (`extractFullContent`).
fn extract_assistant_full(content: &[AssistantBlock]) -> String {
    let mut result = String::new();
    for block in content {
        if let AssistantBlock::Text(text) = block {
            result.push_str(&text.text);
        }
    }
    result
}

/// Assistant-message text detection for the default filter (`hasTextContent`).
fn assistant_has_text_content(content: &[AssistantBlock]) -> bool {
    content
        .iter()
        .any(|block| matches!(block, AssistantBlock::Text(text) if !text.text.trim().is_empty()))
}

/// Upstream `SearchLine`.
struct SearchLine<'a> {
    tree_list: &'a TreeList,
}

impl<'a> SearchLine<'a> {
    fn render(&self, width: usize) -> Vec<String> {
        let theme = Arc::clone(&self.tree_list.theme);
        let query = self.tree_list.search_query();
        let line = if query.is_empty() {
            format!("  {}", theme_fg(&theme, "muted", "Type to search:"))
        } else {
            format!(
                "  {} {}",
                theme_fg(&theme, "muted", "Type to search:"),
                theme_fg(&theme, "accent", query)
            )
        };
        vec![truncate_to_width(&line, width, "", false)]
    }
}

/// Upstream `TREE_HELP_ITEMS`: `(bindings, label, labelFirst)`.
const TREE_HELP_ITEMS: &[(&[&str], &str, bool)] = &[
    (&["tui.select.up", "tui.select.down"], "move", false),
    (
        &["tui.editor.cursorLeft", "tui.editor.cursorRight"],
        "page",
        false,
    ),
    (
        &["app.tree.foldOrUp", "app.tree.unfoldOrDown"],
        "branch",
        false,
    ),
    (&["app.message.copy"], "copy", false),
    (&["app.tree.editLabel"], "label", false),
    (&["app.tree.toggleLabelTimestamp"], "label time", false),
    (
        &[
            "app.tree.filter.default",
            "app.tree.filter.noTools",
            "app.tree.filter.userOnly",
            "app.tree.filter.labeledOnly",
            "app.tree.filter.all",
        ],
        "filters",
        true,
    ),
    (
        &[
            "app.tree.filter.cycleForward",
            "app.tree.filter.cycleBackward",
        ],
        "cycle",
        true,
    ),
];

/// Upstream `formatHelpKeys` including the word-boundary key renames.
fn format_help_keys(keybindings: &[&str]) -> String {
    let mut keys: Vec<String> = Vec::new();
    for keybinding in keybindings {
        if let Some(key) = registry_keys(keybinding).first() {
            keys.push(key.clone());
        }
    }
    if keys.is_empty() {
        return String::new();
    }

    format_key_text(&compact_raw_keys(&keys), false)
}

/// `\b<word>\b` replacement over `+`/`/`-separated key ids (the upstream
/// regex chain applies pageUp/pageDown first, then the arrow words).
fn replace_word(text: &str, word: &str, replacement: &str) -> String {
    let is_word_char = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < text.len() {
        if text[i..].starts_with(word) {
            let before_ok = i == 0 || !is_word_char(bytes[i - 1] as char);
            let after = i + word.len();
            let after_ok = after == text.len() || !is_word_char(bytes[after] as char);
            if before_ok && after_ok {
                out.push_str(replacement);
                i = after;
                continue;
            }
        }
        let ch = text[i..].chars().next().expect("non-empty suffix");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// The full `formatHelpKeys` rename chain (`pageUp→pgup`, `pageDown→pgdn`,
/// `up→↑`, `down→↓`, `left→←`, `right→→`).
fn rename_help_keys(text: &str) -> String {
    let text = replace_word(text, "pageUp", "pgup");
    let text = replace_word(&text, "pageDown", "pgdn");
    let text = replace_word(&text, "up", "\u{2191}");
    let text = replace_word(&text, "down", "\u{2193}");
    let text = replace_word(&text, "left", "\u{2190}");
    replace_word(&text, "right", "\u{2192}")
}

/// Upstream `compactRawKeys`.
fn compact_raw_keys(keys: &[String]) -> String {
    if keys.len() == 1 {
        return keys[0].clone();
    }

    let parts: Vec<(&str, &str)> = keys
        .iter()
        .map(|key| match key.rfind('+') {
            Some(separator_index) => (&key[..separator_index + 1], &key[separator_index + 1..]),
            None => ("", key.as_str()),
        })
        .collect();
    let prefix = parts[0].0;
    if !prefix.is_empty() && parts.iter().all(|part| part.0 == prefix) {
        let suffixes: Vec<&str> = parts.iter().map(|part| part.1).collect();
        format!("{prefix}{}", suffixes.join("/"))
    } else {
        keys.join("/")
    }
}

/// Upstream `TreeHelp`.
struct TreeHelp;

impl TreeHelp {
    fn render(&self, width: usize, theme: &Theme) -> Vec<String> {
        let items: Vec<String> = TREE_HELP_ITEMS
            .iter()
            .map(|(keys, label, label_first)| {
                let text = rename_help_keys(&format_help_keys(keys));
                if text.is_empty() {
                    return (*label).to_string();
                }
                if *label_first {
                    format!("{label} {text}")
                } else {
                    format!("{text} {label}")
                }
            })
            .collect();

        let available_width = width.max(1);
        let indent = "  ";
        let separator = " · ";
        let mut lines: Vec<String> = Vec::new();
        let mut current_line = String::new();

        for item in &items {
            let candidate = if !current_line.is_empty() {
                format!("{current_line}{separator}{item}")
            } else if visible_width(&format!("{indent}{item}")) <= available_width {
                format!("{indent}{item}")
            } else {
                (*item).clone()
            };
            if current_line.is_empty() || visible_width(&candidate) <= available_width {
                current_line = candidate;
                continue;
            }

            lines.extend(wrap_text_with_ansi(
                current_line.trim_end(),
                available_width,
            ));
            current_line = if visible_width(&format!("{indent}{item}")) <= available_width {
                format!("{indent}{item}")
            } else {
                (*item).clone()
            };
        }

        if !current_line.is_empty() {
            lines.extend(wrap_text_with_ansi(
                current_line.trim_end(),
                available_width,
            ));
        }

        lines
            .into_iter()
            .map(|line| theme_fg(theme, "muted", line.as_str()))
            .collect()
    }
}

/// Upstream `LabelInput`. Key handling for confirm/cancel lives in
/// [`TreeSelectorComponent::handle_input`] (the upstream closures over `this`
/// become component-level submits, see the event-queue seam note).
pub struct LabelInput {
    input: Input,
    entry_id: String,
    focused: bool,
    theme: Arc<Theme>,
}

impl LabelInput {
    pub fn new(entry_id: &str, current_label: Option<&str>, theme: Arc<Theme>) -> Self {
        let mut input = Input::new(InputOptions::default());
        if let Some(current_label) = current_label {
            input.set_value(current_label);
        }
        Self {
            input,
            entry_id: entry_id.to_string(),
            focused: false,
            theme,
        }
    }

    /// The trimmed value about to be submitted (`input.getValue().trim()`).
    pub fn value(&self) -> &str {
        self.input.value()
    }

    pub fn entry_id(&self) -> &str {
        &self.entry_id
    }

    /// Upstream `render`.
    pub fn render_lines(&mut self, width: usize) -> Vec<String> {
        let theme = Arc::clone(&self.theme);
        let mut lines: Vec<String> = Vec::new();
        let indent = "  ";
        let available_width = width - indent.len();
        lines.push(truncate_to_width(
            &format!(
                "{indent}{}",
                theme_fg(&theme, "muted", "Label (empty to remove):")
            ),
            width,
            "",
            false,
        ));
        for line in self.input.render(available_width) {
            lines.push(truncate_to_width(
                &format!("{indent}{line}"),
                width,
                "",
                false,
            ));
        }
        lines.push(truncate_to_width(
            &format!(
                "{indent}{}  {}",
                key_hint(&theme, "tui.select.confirm", "save"),
                key_hint(&theme, "tui.select.cancel", "cancel")
            ),
            width,
            "",
            false,
        ));
        lines
    }

    /// The non-confirm, non-cancel keys go to the inner input (upstream
    /// `LabelInput.handleInput` fall-through).
    pub fn forward_input(&mut self, data: &str) {
        self.input.handle_input(data);
    }
}

impl Component for LabelInput {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.render_lines(width)
    }

    fn handle_input(&mut self, data: &str) {
        self.forward_input(data);
    }

    fn invalidate(&mut self) {}

    fn is_focusable(&self) -> bool {
        true
    }

    fn focused(&self) -> bool {
        self.focused
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        self.input.set_focused(focused);
    }
}
/// Display mode of the tree selector body (upstream: which container holds
/// the tree list vs the label input).
enum BodyMode {
    Tree,
    Label(Box<LabelInput>),
}

/// Upstream `TreeSelectorComponent`.
pub struct TreeSelectorComponent {
    tree_list: TreeList,
    body: BodyMode,
    on_label_change: Option<Box<dyn FnMut(&str, Option<&str>) + Send>>,
    pub on_copy: Option<Box<dyn FnMut(Option<&str>) + Send>>,
    pub on_select: Option<Box<dyn FnMut(&str) + Send>>,
    pub on_cancel: Option<Box<dyn FnMut() + Send>>,
    focused: bool,
    /// Set by the constructor when the tree is empty (upstream schedules
    /// `onCancel` via `setTimeout(…, 100)`; S20.5 timer seam drains it).
    auto_cancel_pending: bool,
    events: EventQueue,
    theme: Arc<Theme>,
}

impl TreeSelectorComponent {
    /// Upstream constructor.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        tree: &[SessionTreeNode],
        current_leaf_id: Option<&str>,
        terminal_height: usize,
        on_select: Box<dyn FnMut(&str) + Send>,
        on_cancel: Box<dyn FnMut() + Send>,
        on_label_change: Option<Box<dyn FnMut(&str, Option<&str>) + Send>>,
        initial_selected_id: Option<&str>,
        initial_filter_mode: Option<FilterMode>,
        theme: Arc<Theme>,
    ) -> Self {
        super::model_selector::set_default_theme(Arc::clone(&theme));
        let max_visible_lines = 5.max(terminal_height / 2);

        let events: EventQueue = Arc::new(Mutex::new(Vec::new()));

        let mut tree_list = TreeList::new(
            tree,
            current_leaf_id,
            max_visible_lines,
            initial_selected_id,
            initial_filter_mode,
            Arc::clone(&theme),
        );
        // The list emits through the shared queue; the component drains it in
        // `handle_input` and forwards to its own callbacks (upstream closes
        // over `this`).
        tree_list.set_callbacks(
            {
                let push_queue = Arc::clone(&events);
                Some(Box::new(move |entry_id: &str| {
                    push_event(&push_queue, TreeSelectorEvent::Select(entry_id.to_string()));
                }))
            },
            {
                let push_queue = Arc::clone(&events);
                Some(Box::new(move || {
                    push_event(&push_queue, TreeSelectorEvent::Cancel);
                }))
            },
            {
                let push_queue = Arc::clone(&events);
                Some(Box::new(move |text: Option<&str>| {
                    push_event(
                        &push_queue,
                        TreeSelectorEvent::Copy(text.map(str::to_string)),
                    );
                }))
            },
            {
                let push_queue = Arc::clone(&events);
                Some(Box::new(
                    move |entry_id: &str, current_label: Option<&str>| {
                        push_event(
                            &push_queue,
                            TreeSelectorEvent::LabelEdit {
                                entry_id: entry_id.to_string(),
                                current_label: current_label.map(str::to_string),
                            },
                        );
                    },
                ))
            },
        );

        Self {
            tree_list,
            body: BodyMode::Tree,
            on_label_change,
            on_copy: None,
            on_select: Some(on_select),
            on_cancel: Some(on_cancel),
            focused: false,
            auto_cancel_pending: tree.is_empty(),
            events,
            theme,
        }
    }

    /// `showLabelInput` (upstream private).
    pub fn show_label_input(&mut self, entry_id: &str, current_label: Option<&str>) {
        let mut label_input = Box::new(LabelInput::new(
            entry_id,
            current_label,
            Arc::clone(&self.theme),
        ));
        label_input.set_focused(self.focused);
        self.body = BodyMode::Label(label_input);
    }

    /// `hideLabelInput` (upstream private).
    pub fn hide_label_input(&mut self) {
        self.body = BodyMode::Tree;
    }

    /// Upstream `handleInput` plus the synchronous event forwarding.
    pub fn handle_input(&mut self, data: &str) {
        match &mut self.body {
            BodyMode::Label(label_input) => {
                if keybindings_match(data, "tui.select.confirm") {
                    // LabelInput.handleInput confirm branch
                    let value = label_input.value().trim().to_string();
                    let label = if value.is_empty() { None } else { Some(value) };
                    let entry_id = label_input.entry_id().to_string();
                    self.tree_list
                        .update_node_label(&entry_id, label.as_deref(), None);
                    if let Some(on_label_change) = &mut self.on_label_change {
                        on_label_change(&entry_id, label.as_deref());
                    }
                    self.hide_label_input();
                } else if keybindings_match(data, "tui.select.cancel") {
                    self.hide_label_input();
                } else {
                    label_input.forward_input(data);
                }
            }
            BodyMode::Tree => self.tree_list.handle_input(data),
        }
        self.drain_events();
    }

    /// Forward the queued list events to the component callbacks (upstream's
    /// synchronous closure forwards, in the same order).
    fn drain_events(&mut self) {
        let events: Vec<TreeSelectorEvent> =
            std::mem::take(&mut *self.events.lock().expect("tree selector event queue"));
        for event in events {
            match event {
                TreeSelectorEvent::Select(entry_id) => {
                    if let Some(on_select) = &mut self.on_select {
                        on_select(&entry_id);
                    }
                }
                TreeSelectorEvent::Cancel => {
                    if let Some(on_cancel) = &mut self.on_cancel {
                        on_cancel();
                    }
                }
                TreeSelectorEvent::Copy(text) => {
                    if let Some(on_copy) = &mut self.on_copy {
                        on_copy(text.as_deref());
                    }
                }
                TreeSelectorEvent::LabelEdit {
                    entry_id,
                    current_label,
                } => self.show_label_input(&entry_id, current_label.as_deref()),
            }
        }
    }

    /// The tree list accessors (`getTreeList`).
    pub fn tree_list(&self) -> &TreeList {
        &self.tree_list
    }

    pub fn tree_list_mut(&mut self) -> &mut TreeList {
        &mut self.tree_list
    }

    /// The empty-tree auto-cancel flag (upstream `setTimeout(onCancel, 100)`).
    pub fn auto_cancel_pending(&self) -> bool {
        self.auto_cancel_pending
    }

    /// Drain the auto-cancel: reports whether the constructor saw an empty
    /// tree (the caller then invokes its cancel path; upstream fires the same
    /// `onCancel` closure after 100ms — S20.5 timer seam).
    pub fn take_auto_cancel(&mut self) -> bool {
        std::mem::replace(&mut self.auto_cancel_pending, false)
    }
}

impl Component for TreeSelectorComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        // Spacer(1)
        lines.extend(spacer_lines(1));
        // DynamicBorder
        lines.extend(DynamicBorder::default().render(width));
        // Text(theme.bold("  Session Tree"), 1, 0)
        let mut title = Text::with_options(&self.theme.bold("  Session Tree"), 1, 0, None);
        lines.extend(title.render(width));
        // TreeHelp
        lines.extend(TreeHelp.render(width, &self.theme));
        // SearchLine
        lines.extend(
            SearchLine {
                tree_list: &self.tree_list,
            }
            .render(width),
        );
        // DynamicBorder
        lines.extend(DynamicBorder::default().render(width));
        // Spacer(1)
        lines.extend(spacer_lines(1));
        // treeContainer / labelInputContainer
        match &mut self.body {
            BodyMode::Tree => lines.extend(self.tree_list.render_list(width)),
            BodyMode::Label(label_input) => lines.extend(label_input.render_lines(width)),
        }
        // Spacer(1)
        lines.extend(spacer_lines(1));
        // DynamicBorder
        lines.extend(DynamicBorder::default().render(width));
        lines
    }

    fn handle_input(&mut self, data: &str) {
        TreeSelectorComponent::handle_input(self, data);
    }

    fn invalidate(&mut self) {}

    fn is_focusable(&self) -> bool {
        true
    }

    fn focused(&self) -> bool {
        self.focused
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        if let BodyMode::Label(label_input) = &mut self.body {
            label_input.set_focused(focused);
        }
    }
}

#[cfg(test)]
#[path = "tree_selector_tests.rs"]
mod tests;
