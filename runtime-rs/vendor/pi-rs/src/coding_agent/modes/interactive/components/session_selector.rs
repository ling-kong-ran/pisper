//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Port of `modes/interactive/components/session-selector.ts` (1031 lines,
//! sha256
//! `b6d2f348e6eb68e17914a4936433912155271f0c3199441e14c419cbab21be10`): the
//! `/resume` session picker — threaded/fuzzy/recent sorting, scope toggle,
//! search, rename, delete-with-trash and the header/status surface. The
//! search-query machinery comes from the already-landed
//! [`super::session_selector_search`] (upstream `session-selector-search.ts`,
//! S19.9).
//!
//! Slice conventions: see [`super::model_selector`] (theme seam, inline
//! composite rendering) and [`super::tree_selector`] (event queue). Further
//! disclosed substitutions:
//! - **Async scheduling (S20.1)**: upstream fire-and-forget promises become
//!   explicit futures — the constructor's initial load is
//!   [`SessionSelectorComponent::initial_load`], and the async flows the list
//!   triggers (session deletion, rename submit) queue
//!   [`SessionSelectorEvent`]s that the caller drives through
//!   [`SessionSelectorComponent::run_pending_work`]. Event order matches the
//!   upstream closure dispatches. The header status auto-hide
//!   `setTimeout` becomes the explicit [`SessionSelectorComponent::fire_status_timeout`]
//!   tick.
//! - **Loader errors**: upstream loaders reject; the ported
//!   [`SessionsLoader`] resolves `Result<Vec<SessionInfo>, String>` and
//!   `load_scope` reproduces the `Failed to load sessions: {message}` branch.
//! - **`node:fs` error strings (S20.2)**: [`delete_session_file`] re-states
//!   node's ENOENT/EPERM unlink messages and the `spawn trash ENOENT` spawn
//!   error for the mapped [`std::io::ErrorKind`]s; other kinds carry the
//!   `io::Error` display.
//! - **Clock seam (S20.5)**: `formatSessionDate` reads `new Date()` upstream;
//!   the port renders against an injectable clock (default `SystemTime`, the
//!   oracle pins `Date` to the same FIXED_NOW).
//! - **Rename default keybindings**: upstream defaults to
//!   `KeybindingsManager.create()` (reads the agent dir); the port defaults
//!   to the in-memory defaults (`KeybindingsManager::new(default, None)`).

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use crate::coding_agent::core::keybindings::KeybindingsManager;
use crate::coding_agent::modes::interactive::theme::Theme;
use crate::coding_agent::session_manager::SessionInfo;
use crate::coding_agent::utils::paths::canonicalize_path;
use crate::tui::component::Component;
use crate::tui::components::input::{Input, InputOptions};
use crate::tui::components::text::Text;
use crate::tui::utils::{truncate_to_width, visible_width};

use super::model_selector::{
    key_hint, key_text, keybindings_match, spacer_lines, theme_fg, DynamicBorder,
};

/// Shared request-render callback (upstream closes over `requestRender`).
pub type RenderFn = Arc<std::sync::Mutex<Box<dyn FnMut() + Send>>>;
use super::session_selector_search::{
    filter_and_sort_sessions, has_session_name, NameFilter, SortMode,
};

/// Upstream `SessionScope` ("current" | "all").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionScope {
    Current,
    All,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn homedir() -> String {
    if cfg!(windows) {
        std::env::var("USERPROFILE").unwrap_or_default()
    } else {
        std::env::var("HOME").unwrap_or_default()
    }
}

/// Upstream `shortenPath` (`os.homedir()` prefix → `~`).
fn shorten_path(path: &str) -> String {
    let home = homedir();
    if path.is_empty() {
        return path.to_string();
    }
    if !home.is_empty() {
        if let Some(rest) = path.strip_prefix(&home) {
            return format!("~{rest}");
        }
    }
    path.to_string()
}

/// Upstream `formatSessionDate` against the injectable clock.
fn format_session_date_at(date_ms: i64, now: i64) -> String {
    let diff_ms = now - date_ms;
    let diff_mins = diff_ms / 60_000;
    let diff_hours = diff_ms / 3_600_000;
    let diff_days = diff_ms / 86_400_000;

    if diff_mins < 1 {
        return "now".to_string();
    }
    if diff_mins < 60 {
        return format!("{diff_mins}m");
    }
    if diff_hours < 24 {
        return format!("{diff_hours}h");
    }
    if diff_days < 7 {
        return format!("{diff_days}d");
    }
    if diff_days < 30 {
        return format!("{}w", diff_days / 7);
    }
    if diff_days < 365 {
        return format!("{}mo", diff_days / 30);
    }
    format!("{}y", diff_days / 365)
}

/// Upstream `canonicalizePath` wrapper (undefined passthrough).
fn canonicalize_optional(path: Option<&str>) -> Option<String> {
    path.map(canonicalize_path)
}

/// Upstream `SessionListProgress` callback payload.
pub type SessionListProgress = Arc<dyn Fn(usize, usize) + Send + Sync>;

/// Upstream `SessionsLoader`: resolves the session list, optionally reporting
/// progress. Errors surface through the `Result` (upstream promise rejection,
/// S20.1).
pub type SessionsLoader = Arc<
    dyn Fn(
            Option<SessionListProgress>,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<SessionInfo>, String>> + Send>>
        + Send
        + Sync,
>;

/// Upstream `renameSession` option: `(sessionPath, currentName) => Promise`.
pub type RenameSessionFn =
    Arc<dyn Fn(String, String) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// A session tree node for hierarchical display (upstream local
/// `SessionTreeNode`).
pub(crate) struct SessionTreeNode {
    pub(crate) session: SessionInfo,
    pub(crate) children: Vec<usize>,
    pub(crate) latest_activity: i64,
}

/// Flattened node for display with tree structure info (upstream
/// `FlatSessionNode`).
#[derive(Clone, Debug)]
struct FlatSessionNode {
    session: SessionInfo,
    depth: usize,
    is_last: bool,
    /// For each ancestor level, whether there are more siblings after it.
    ancestor_continues: Vec<bool>,
}

/// Build a tree structure from sessions based on parentSessionPath; roots
/// sorted by latest subtree activity (upstream `buildSessionTree`), returned
/// as the materialized arena + root indices.
pub(crate) fn build_session_tree_full(sessions: &[SessionInfo]) -> SessionTree {
    let mut arena: Vec<SessionTreeNode> = Vec::new();
    let mut by_path: HashMap<String, usize> = HashMap::new();
    let mut roots: Vec<usize> = Vec::new();

    for session in sessions {
        let session_path =
            canonicalize_optional(Some(&session.path)).unwrap_or_else(|| session.path.clone());
        let index = arena.len();
        arena.push(SessionTreeNode {
            session: session.clone(),
            children: Vec::new(),
            latest_activity: session.modified,
        });
        // Upstream keeps the LAST node for a duplicate path.
        by_path.insert(session_path, index);
    }
    for session in sessions {
        let session_path =
            canonicalize_optional(Some(&session.path)).unwrap_or_else(|| session.path.clone());
        let node = by_path[&session_path];
        let parent_path = canonicalize_optional(session.parent_session_path.as_deref());
        match parent_path
            .as_deref()
            .and_then(|parent| by_path.get(parent))
        {
            Some(&parent) => arena[parent].children.push(node),
            None => roots.push(node),
        }
    }

    // updateLatestActivity: post-order max (upstream recursion).
    fn update_latest_activity(arena: &mut [SessionTreeNode], index: usize) -> i64 {
        let children = arena[index].children.clone();
        let mut latest = arena[index].session.modified;
        for child in children {
            latest = latest.max(update_latest_activity(arena, child));
        }
        arena[index].latest_activity = latest;
        latest
    }
    for &root in &roots {
        update_latest_activity(&mut arena, root);
    }

    // Sort children and roots by latest activity in each subtree (descending;
    // JS Array#sort is stable, Rust sort_by is stable too).
    fn sort_nodes(arena: &mut [SessionTreeNode], indices: &mut [usize]) {
        indices.sort_by(|a, b| arena[*b].latest_activity.cmp(&arena[*a].latest_activity));
        for &node in indices.iter() {
            let mut children = std::mem::take(&mut arena[node].children);
            sort_nodes(arena, &mut children);
            arena[node].children = children;
        }
    }
    sort_nodes(&mut arena, &mut roots);

    SessionTree { arena, roots }
}

/// The tree plus its arena (the port's materialized `buildSessionTree`
/// result).
pub(crate) struct SessionTree {
    pub(crate) arena: Vec<SessionTreeNode>,
    pub(crate) roots: Vec<usize>,
}

/// Flatten the tree into the display list with tree structure metadata
/// (upstream `flattenSessionTree`).
fn flatten_session_tree(tree: &SessionTree) -> Vec<FlatSessionNode> {
    let mut result: Vec<FlatSessionNode> = Vec::new();
    fn walk(
        tree: &SessionTree,
        index: usize,
        depth: usize,
        ancestor_continues: Vec<bool>,
        is_last: bool,
        result: &mut Vec<FlatSessionNode>,
    ) {
        result.push(FlatSessionNode {
            session: tree.arena[index].session.clone(),
            depth,
            is_last,
            ancestor_continues: ancestor_continues.clone(),
        });
        let children = tree.arena[index].children.clone();
        for (i, &child) in children.iter().enumerate() {
            let child_is_last = i == children.len() - 1;
            // Only show continuation line for non-root ancestors
            let continues = depth > 0 && !is_last;
            let mut continues_vec = ancestor_continues.clone();
            continues_vec.push(continues);
            walk(tree, child, depth + 1, continues_vec, child_is_last, result);
        }
    }
    for (i, &root) in tree.roots.iter().enumerate() {
        walk(
            tree,
            root,
            0,
            Vec::new(),
            i == tree.roots.len() - 1,
            &mut result,
        );
    }
    result
}

// ===========================================================================
// SessionSelectorHeader
// ===========================================================================

/// A status message (upstream `{ type: "info" | "error"; message }`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusMessage {
    pub is_error: bool,
    pub message: String,
}

/// Upstream `SessionSelectorHeader`.
pub struct SessionSelectorHeader {
    scope: SessionScope,
    sort_mode: SortMode,
    name_filter: NameFilter,
    request_render: RenderFn,
    loading: bool,
    load_progress: Option<(usize, usize)>,
    show_path: bool,
    confirming_delete_path: Option<String>,
    status_message: Option<StatusMessage>,
    /// S20.1 timer seam: pending auto-hide delay in ms.
    status_autohide_ms: Option<u64>,
    show_rename_hint: bool,
    theme: Arc<Theme>,
}

impl SessionSelectorHeader {
    pub fn new(
        scope: SessionScope,
        sort_mode: SortMode,
        name_filter: NameFilter,
        request_render: RenderFn,
        theme: Arc<Theme>,
    ) -> Self {
        Self {
            scope,
            sort_mode,
            name_filter,
            request_render,
            loading: false,
            load_progress: None,
            show_path: false,
            confirming_delete_path: None,
            status_message: None,
            status_autohide_ms: None,
            show_rename_hint: false,
            theme,
        }
    }

    pub fn set_scope(&mut self, scope: SessionScope) {
        self.scope = scope;
    }

    pub fn set_sort_mode(&mut self, sort_mode: SortMode) {
        self.sort_mode = sort_mode;
    }

    pub fn set_name_filter(&mut self, name_filter: NameFilter) {
        self.name_filter = name_filter;
    }

    pub fn set_loading(&mut self, loading: bool) {
        self.loading = loading;
        // Progress is scoped to the current load; clear whenever the loading
        // state is set
        self.load_progress = None;
    }

    pub fn set_progress(&mut self, loaded: usize, total: usize) {
        self.load_progress = Some((loaded, total));
    }

    pub fn set_show_path(&mut self, show_path: bool) {
        self.show_path = show_path;
    }

    pub fn set_show_rename_hint(&mut self, show: bool) {
        self.show_rename_hint = show;
    }

    pub fn set_confirming_delete_path(&mut self, path: Option<String>) {
        self.confirming_delete_path = path;
    }

    /// Upstream `setStatusMessage` with the auto-hide delay captured (the
    /// timer itself is the explicit S20.1 tick).
    pub fn set_status_message(&mut self, msg: Option<StatusMessage>, auto_hide_ms: Option<u64>) {
        self.status_autohide_ms = None;
        self.status_message = msg;
        if self.status_message.is_none() {
            return;
        }
        if let Some(auto_hide_ms) = auto_hide_ms.filter(|ms| *ms != 0) {
            self.status_autohide_ms = Some(auto_hide_ms);
        }
    }

    /// The S20.1 explicit tick: clears the status when the auto-hide timer is
    /// pending and re-renders. Returns whether the status was cleared.
    pub fn fire_status_timeout(&mut self) -> bool {
        if self.status_autohide_ms.take().is_some() {
            self.status_message = None;
            (self.request_render.lock().expect("render fn"))();
            return true;
        }
        false
    }

    /// Upstream `render`.
    pub fn render(&self, width: usize) -> Vec<String> {
        let theme = &self.theme;
        let title = match self.scope {
            SessionScope::Current => "Resume Session (Current Folder)",
            SessionScope::All => "Resume Session (All)",
        };
        let left_text = theme.bold(title);

        let sort_label = match self.sort_mode {
            SortMode::Threaded => "Threaded",
            SortMode::Recent => "Recent",
            SortMode::Relevance => "Fuzzy",
        };
        let sort_text = theme_fg(theme, "muted", "Sort: ") + &theme_fg(theme, "accent", sort_label);

        let name_label = if self.name_filter == NameFilter::All {
            "All"
        } else {
            "Named"
        };
        let name_text = theme_fg(theme, "muted", "Name: ") + &theme_fg(theme, "accent", name_label);

        let scope_text = if self.loading {
            let progress_text = match &self.load_progress {
                Some((loaded, total)) => format!("{loaded}/{total}"),
                None => "...".to_string(),
            };
            format!(
                "{}{}",
                theme_fg(theme, "muted", "○ Current Folder | "),
                theme_fg(theme, "accent", &format!("Loading {progress_text}"))
            )
        } else {
            match self.scope {
                SessionScope::Current => format!(
                    "{}{}",
                    theme_fg(theme, "accent", "◉ Current Folder"),
                    theme_fg(theme, "muted", " | ○ All")
                ),
                SessionScope::All => format!(
                    "{}{}",
                    theme_fg(theme, "muted", "○ Current Folder | "),
                    theme_fg(theme, "accent", "◉ All")
                ),
            }
        };

        let right_text = truncate_to_width(
            &format!("{scope_text}  {name_text}  {sort_text}"),
            width,
            "",
            false,
        );
        let available_left = width
            .saturating_sub(visible_width(&right_text))
            .saturating_sub(1);
        let left = truncate_to_width(&left_text, available_left, "", false);
        let spacing = width
            .saturating_sub(visible_width(&left))
            .saturating_sub(visible_width(&right_text));

        // Build hint lines - changes based on state (all branches truncate to width)
        let (hint_line1, hint_line2) =
            if let Some(confirming_delete_path) = &self.confirming_delete_path {
                let _ = confirming_delete_path;
                let confirm_hint = format!(
                    "Delete session? {} · {}",
                    key_hint(theme, "tui.select.confirm", "confirm"),
                    key_hint(theme, "tui.select.cancel", "cancel")
                );
                (
                    theme_fg(
                        theme,
                        "error",
                        &truncate_to_width(&confirm_hint, width, "…", false),
                    ),
                    String::new(),
                )
            } else if let Some(status) = &self.status_message {
                let color = if status.is_error { "error" } else { "accent" };
                (
                    theme_fg(
                        theme,
                        color,
                        &truncate_to_width(&status.message, width, "…", false),
                    ),
                    String::new(),
                )
            } else {
                let path_state = if self.show_path { "(on)" } else { "(off)" };
                let sep = theme_fg(theme, "muted", " · ");
                let hint1 = key_hint(theme, "tui.input.tab", "scope")
                    + &sep
                    + &theme_fg(theme, "muted", "re:<pattern> regex · \"phrase\" exact");
                let mut hint2_parts = vec![
                    key_hint(theme, "app.session.toggleSort", "sort"),
                    key_hint(theme, "app.session.toggleNamedFilter", "named"),
                    key_hint(theme, "app.session.delete", "delete"),
                    key_hint(
                        theme,
                        "app.session.togglePath",
                        &format!("path {path_state}"),
                    ),
                ];
                if self.show_rename_hint {
                    hint2_parts.push(key_hint(theme, "app.session.rename", "rename"));
                }
                let hint2 = hint2_parts.join(&sep);
                (
                    truncate_to_width(&hint1, width, "…", false),
                    truncate_to_width(&hint2, width, "…", false),
                )
            };

        vec![
            format!("{left}{}{right_text}", " ".repeat(spacing)),
            hint_line1,
            hint_line2,
        ]
    }
}

// ===========================================================================
// SessionList
// ===========================================================================

/// Events the list reports to its owner (upstream: the closure callbacks,
/// delivered through the shared queue — see the S20.1 seam note).
pub(crate) enum SessionSelectorEvent {
    Select(String),
    Cancel,
    Exit,
    ToggleScope,
    ToggleSort,
    ToggleNameFilter,
    TogglePath(bool),
    DeleteConfirmation(Option<String>),
    Rename(String),
    Error(String),
}

pub(crate) type SessionEventQueue = Arc<Mutex<Vec<SessionSelectorEvent>>>;

fn push_event(queue: &SessionEventQueue, event: SessionSelectorEvent) {
    queue
        .lock()
        .expect("session selector event queue")
        .push(event);
}

/// Upstream `SessionList`.
pub struct SessionList {
    all_sessions: Vec<SessionInfo>,
    filtered_sessions: Vec<FlatSessionNode>,
    selected_index: usize,
    /// Upstream `selectionTouched`: set on the first navigation key; before
    /// that, fresh session lists reset the selection to the top.
    selection_touched: bool,
    search_input: Input,
    show_cwd: bool,
    sort_mode: SortMode,
    name_filter: NameFilter,
    keybindings: KeybindingsManager,
    show_path: bool,
    confirming_delete_path: Option<String>,
    current_session_canonical_path: Option<String>,
    max_visible: usize,
    focused: bool,
    clock: fn() -> i64,
    theme: Arc<Theme>,

    pub on_select: Option<Box<dyn FnMut(&str) + Send>>,
    pub on_cancel: Option<Box<dyn FnMut() + Send>>,
    pub on_exit: Option<Box<dyn FnMut() + Send>>,
    pub on_toggle_scope: Option<Box<dyn FnMut() + Send>>,
    pub on_toggle_sort: Option<Box<dyn FnMut() + Send>>,
    pub on_toggle_name_filter: Option<Box<dyn FnMut() + Send>>,
    pub on_toggle_path: Option<Box<dyn FnMut(bool) + Send>>,
    pub on_delete_confirmation_change: Option<Box<dyn FnMut(Option<&str>) + Send>>,
    pub on_delete_session: Option<Box<dyn FnMut(&str) + Send>>,
    pub on_rename_session: Option<Box<dyn FnMut(&str) + Send>>,
    pub on_error: Option<Box<dyn FnMut(&str) + Send>>,
}

impl SessionList {
    /// Upstream constructor.
    pub fn new(
        sessions: Vec<SessionInfo>,
        show_cwd: bool,
        sort_mode: SortMode,
        name_filter: NameFilter,
        keybindings: KeybindingsManager,
        current_session_file_path: Option<&str>,
        theme: Arc<Theme>,
    ) -> Self {
        let mut list = Self {
            all_sessions: sessions,
            filtered_sessions: Vec::new(),
            selected_index: 0,
            selection_touched: false,
            search_input: Input::new(InputOptions::default()),
            show_cwd,
            sort_mode,
            name_filter,
            keybindings,
            show_path: false,
            confirming_delete_path: None,
            current_session_canonical_path: canonicalize_optional(current_session_file_path),
            max_visible: 10,
            focused: false,
            clock: now_ms,
            theme,
            on_select: None,
            on_cancel: None,
            on_exit: None,
            on_toggle_scope: None,
            on_toggle_sort: None,
            on_toggle_name_filter: None,
            on_toggle_path: None,
            on_delete_confirmation_change: None,
            on_delete_session: None,
            on_rename_session: None,
            on_error: None,
        };
        list.filter_sessions("");
        list
    }

    /// S20.5 clock seam (test injection).
    pub fn set_clock(&mut self, clock: fn() -> i64) {
        self.clock = clock;
    }

    /// Upstream `getSelectedSessionPath`.
    pub fn get_selected_session_path(&self) -> Option<&str> {
        self.filtered_sessions
            .get(self.selected_index)
            .map(|node| node.session.path.as_str())
    }

    pub fn set_sort_mode(&mut self, sort_mode: SortMode) {
        self.sort_mode = sort_mode;
        let query = self.search_input.value().to_string();
        self.filter_sessions(&query);
    }

    pub fn set_name_filter(&mut self, name_filter: NameFilter) {
        self.name_filter = name_filter;
        let query = self.search_input.value().to_string();
        self.filter_sessions(&query);
    }

    pub fn set_sessions(&mut self, sessions: Vec<SessionInfo>, show_cwd: bool) {
        let selected_path = if self.selection_touched {
            self.get_selected_session_path().map(str::to_string)
        } else {
            None
        };
        self.all_sessions = sessions;
        self.show_cwd = show_cwd;
        let query = self.search_input.value().to_string();
        self.filter_sessions(&query);
        if !self.selection_touched {
            self.selected_index = 0;
        } else if let Some(selected_path) = selected_path {
            let selected_index = self
                .filtered_sessions
                .iter()
                .position(|node| node.session.path == selected_path);
            if let Some(selected_index) = selected_index {
                self.selected_index = selected_index;
            }
        }
    }

    /// Upstream `filterSessions`.
    fn filter_sessions(&mut self, query: &str) {
        let name_filtered: Vec<SessionInfo> = if self.name_filter == NameFilter::All {
            self.all_sessions.clone()
        } else {
            self.all_sessions
                .iter()
                .filter(|session| has_session_name(session))
                .cloned()
                .collect()
        };

        let trimmed = query.trim();
        if self.sort_mode == SortMode::Threaded && trimmed.is_empty() {
            // Threaded mode without search: show tree structure
            let tree = build_session_tree_full(&name_filtered);
            self.filtered_sessions = flatten_session_tree(&tree);
        } else {
            // Other modes or with search: flat list
            let filtered =
                filter_and_sort_sessions(&name_filtered, query, self.sort_mode, self.name_filter);
            self.filtered_sessions = filtered
                .into_iter()
                .map(|session| FlatSessionNode {
                    session,
                    depth: 0,
                    is_last: true,
                    ancestor_continues: Vec::new(),
                })
                .collect();
        }
        self.selected_index = self
            .selected_index
            .min(self.filtered_sessions.len().saturating_sub(1));
    }

    /// Upstream `setConfirmingDeletePath`.
    fn set_confirming_delete_path(&mut self, path: Option<String>) {
        self.confirming_delete_path = path.clone();
        if let Some(on_change) = &mut self.on_delete_confirmation_change {
            on_change(path.as_deref());
        }
    }

    /// Upstream `startDeleteConfirmationForSelectedSession`.
    fn start_delete_confirmation_for_selected_session(&mut self) {
        let Some(selected) = self.filtered_sessions.get(self.selected_index) else {
            return;
        };
        let path = selected.session.path.clone();

        // Prevent deleting current session
        if self.is_current_session_path(&path) {
            if let Some(on_error) = &mut self.on_error {
                on_error("Cannot delete the currently active session");
            }
            return;
        }

        self.set_confirming_delete_path(Some(path));
    }

    /// Upstream `isCurrentSessionPath`.
    fn is_current_session_path(&self, path: &str) -> bool {
        match &self.current_session_canonical_path {
            None => false,
            Some(current) => {
                canonicalize_optional(Some(path)).unwrap_or_else(|| path.to_string()) == *current
            }
        }
    }

    /// The search query (test/status surface).
    pub fn search_query(&self) -> &str {
        self.search_input.value()
    }

    /// Focus propagation (upstream `focused` setter → the search input, for
    /// IME cursor positioning).
    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        self.search_input.set_focused(focused);
    }

    /// Upstream `render`.
    pub fn render_list(&mut self, width: usize) -> Vec<String> {
        let theme = Arc::clone(&self.theme);
        let mut lines: Vec<String> = Vec::new();

        // Render search input
        lines.extend(self.search_input.render(width));
        lines.push(String::new()); // Blank line after search

        if self.filtered_sessions.is_empty() {
            let empty_message = if self.name_filter == NameFilter::Named {
                let toggle_key = key_text("app.session.toggleNamedFilter");
                if self.show_cwd {
                    format!("  No named sessions found. Press {toggle_key} to show all.")
                } else {
                    format!(
                        "  No named sessions in current folder. Press {toggle_key} to show all, or Tab to view all."
                    )
                }
            } else if self.show_cwd {
                // "All" scope - no sessions anywhere that match filter
                "  No sessions found".to_string()
            } else {
                // "Current folder" scope - hint to try "all"
                "  No sessions in current folder. Press Tab to view all.".to_string()
            };
            lines.push(theme_fg(
                &theme,
                "muted",
                &truncate_to_width(&empty_message, width, "…", false),
            ));
            return lines;
        }

        // Calculate visible range with scrolling
        let start_index = self
            .selected_index
            .saturating_sub(self.max_visible / 2)
            .min(
                self.filtered_sessions
                    .len()
                    .saturating_sub(self.max_visible),
            );
        let end_index = (start_index + self.max_visible).min(self.filtered_sessions.len());

        // Render visible sessions (one line each with tree structure)
        for i in start_index..end_index {
            let node = &self.filtered_sessions[i];
            let session = &node.session;
            let is_selected = i == self.selected_index;
            let is_confirming_delete =
                Some(session.path.as_str()) == self.confirming_delete_path.as_deref();
            let is_current = self.is_current_session_path(&session.path);

            // Build tree prefix
            let prefix = Self::build_tree_prefix(node);

            // Session display text (name or first message)
            let has_name = session.name.is_some();
            let display_text = session
                .name
                .clone()
                .unwrap_or_else(|| session.first_message.clone());
            let normalized_message = normalize_control_chars(&display_text);

            // Right side: message count and age
            let age = format_session_date_at(session.modified, (self.clock)());
            let msg_count = session.message_count.to_string();
            let mut right_part = format!("{msg_count} {age}");
            if self.show_cwd && !session.cwd.is_empty() {
                right_part = format!("{} {right_part}", shorten_path(&session.cwd));
            }
            if self.show_path {
                right_part = format!("{} {right_part}", shorten_path(&session.path));
            }

            // Cursor
            let cursor = if is_selected {
                theme_fg(&theme, "accent", "› ")
            } else {
                "  ".to_string()
            };

            // Calculate available width for message
            let prefix_width = visible_width(&prefix);
            let right_width = visible_width(&right_part) + 2; // +2 for spacing
            let available_for_msg = width
                .saturating_sub(2)
                .saturating_sub(prefix_width)
                .saturating_sub(right_width); // -2 for cursor

            let truncated_msg =
                truncate_to_width(&normalized_message, 10.max(available_for_msg), "…", false);

            // Style message
            let message_color: Option<&str> = if is_confirming_delete {
                Some("error")
            } else if is_current {
                Some("accent")
            } else if has_name {
                Some("warning")
            } else {
                None
            };
            let mut styled_msg = match message_color {
                Some(color) => theme_fg(&theme, color, &truncated_msg),
                None => truncated_msg,
            };
            if is_selected {
                styled_msg = theme.bold(&styled_msg);
            }

            // Build line
            let left_part = cursor + &theme_fg(&theme, "dim", &prefix) + &styled_msg;
            let left_width = visible_width(&left_part);
            let spacing = 1.max(
                width
                    .saturating_sub(left_width)
                    .saturating_sub(visible_width(&right_part)),
            );
            let styled_right = theme_fg(
                &theme,
                if is_confirming_delete { "error" } else { "dim" },
                &right_part,
            );

            let mut line = format!("{left_part}{}{styled_right}", " ".repeat(spacing));
            if is_selected {
                line = theme.bg("selectedBg", &line).expect("selectedBg");
            }
            lines.push(truncate_to_width(&line, width, "...", false));
        }

        // Add scroll indicator if needed
        if start_index > 0 || end_index < self.filtered_sessions.len() {
            let scroll_text = format!(
                "  ({}/{})",
                self.selected_index + 1,
                self.filtered_sessions.len()
            );
            let scroll_info = theme_fg(
                &theme,
                "muted",
                &truncate_to_width(&scroll_text, width, "", false),
            );
            lines.push(scroll_info);
        }

        lines
    }

    /// Upstream `buildTreePrefix`.
    fn build_tree_prefix(node: &FlatSessionNode) -> String {
        if node.depth == 0 {
            return String::new();
        }
        let parts: String = node
            .ancestor_continues
            .iter()
            .map(|&continues| if continues { "│  " } else { "   " })
            .collect();
        let branch = if node.is_last { "└─ " } else { "├─ " };
        parts + branch
    }

    /// Upstream `handleInput`.
    pub fn handle_input(&mut self, data: &str) {
        // Handle delete confirmation state first - intercept all keys
        if self.confirming_delete_path.is_some() {
            if keybindings_match(data, "tui.select.confirm") {
                let path_to_delete = self.confirming_delete_path.take();
                self.set_confirming_delete_path(None);
                if let Some(path) = path_to_delete {
                    if let Some(on_delete) = &mut self.on_delete_session {
                        on_delete(&path);
                    }
                }
                return;
            }
            if keybindings_match(data, "tui.select.cancel") {
                self.set_confirming_delete_path(None);
                return;
            }
            // Ignore all other keys while confirming
            return;
        }

        if keybindings_match(data, "tui.input.tab") {
            if let Some(on_toggle_scope) = &mut self.on_toggle_scope {
                on_toggle_scope();
            }
            return;
        }

        if keybindings_match(data, "app.session.toggleSort") {
            if let Some(on_toggle_sort) = &mut self.on_toggle_sort {
                on_toggle_sort();
            }
            return;
        }

        // The injected manager handles `app.session.toggleNamedFilter`
        // (upstream `this.keybindings.matches`).
        if self
            .keybindings
            .matches(data, "app.session.toggleNamedFilter")
        {
            if let Some(on_toggle_name_filter) = &mut self.on_toggle_name_filter {
                on_toggle_name_filter();
            }
            return;
        }

        // Ctrl+P: toggle path display
        if keybindings_match(data, "app.session.togglePath") {
            self.show_path = !self.show_path;
            if let Some(on_toggle_path) = &mut self.on_toggle_path {
                on_toggle_path(self.show_path);
            }
            return;
        }

        // Ctrl+D: initiate delete confirmation (useful on terminals that
        // don't distinguish Ctrl+Backspace from Backspace)
        if keybindings_match(data, "app.session.delete") {
            self.start_delete_confirmation_for_selected_session();
            return;
        }

        // Rename selected session
        if keybindings_match(data, "app.session.rename") {
            let selected = self
                .filtered_sessions
                .get(self.selected_index)
                .map(|node| node.session.path.clone());
            if let Some(path) = selected {
                if let Some(on_rename) = &mut self.on_rename_session {
                    on_rename(&path);
                }
            }
            return;
        }

        // Ctrl+Backspace: non-invasive convenience alias for delete. Only
        // triggers deletion when the query is empty; otherwise forwarded to
        // the input.
        if keybindings_match(data, "app.session.deleteNoninvasive") {
            if !self.search_input.value().is_empty() {
                self.search_input.handle_input(data);
                let query = self.search_input.value().to_string();
                self.filter_sessions(&query);
                return;
            }
            self.start_delete_confirmation_for_selected_session();
            return;
        }

        self.selection_touched = true;
        // Up arrow
        if keybindings_match(data, "tui.select.up") {
            self.selected_index = self.selected_index.saturating_sub(1);
        }
        // Down arrow
        else if keybindings_match(data, "tui.select.down") {
            self.selected_index =
                (self.selected_index + 1).min(self.filtered_sessions.len().saturating_sub(1));
        }
        // Page up - jump up by maxVisible items
        else if keybindings_match(data, "tui.select.pageUp") {
            self.selected_index = self.selected_index.saturating_sub(self.max_visible);
        }
        // Page down - jump down by maxVisible items
        else if keybindings_match(data, "tui.select.pageDown") {
            self.selected_index = (self.selected_index + self.max_visible)
                .min(self.filtered_sessions.len().saturating_sub(1));
        }
        // Enter
        else if keybindings_match(data, "tui.select.confirm") {
            let selected = self
                .filtered_sessions
                .get(self.selected_index)
                .map(|node| node.session.path.clone());
            if let (Some(path), Some(on_select)) = (selected, &mut self.on_select) {
                on_select(&path);
            }
        }
        // Escape - cancel
        else if keybindings_match(data, "tui.select.cancel") {
            if let Some(on_cancel) = &mut self.on_cancel {
                on_cancel();
            }
        }
        // Pass everything else to search input
        else {
            self.search_input.handle_input(data);
            let query = self.search_input.value().to_string();
            self.filter_sessions(&query);
        }
    }
}

/// `displayText.replace(/[\x00-\x1f\x7f]/g, " ").trim()`.
fn normalize_control_chars(text: &str) -> String {
    let replaced: String = text
        .chars()
        .map(|c| {
            let code = c as u32;
            if code <= 0x1f || code == 0x7f {
                ' '
            } else {
                c
            }
        })
        .collect();
    replaced.trim().to_string()
}

// ===========================================================================
// deleteSessionFile
// ===========================================================================

/// Upstream `method: "trash" | "unlink"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeleteMethod {
    Trash,
    Unlink,
}

/// Upstream `deleteSessionFile` result.
#[derive(Clone, Debug)]
pub struct DeleteResult {
    pub ok: bool,
    pub method: DeleteMethod,
    pub error: Option<String>,
}

/// node's `spawnSync` `error.message` for the mapped kinds (S20.2).
fn spawn_error_message(error: &std::io::Error) -> String {
    match error.kind() {
        std::io::ErrorKind::NotFound => "spawn trash ENOENT".to_string(),
        std::io::ErrorKind::PermissionDenied => "spawn trash EACCES".to_string(),
        _ => error.to_string(),
    }
}

/// node's `unlink` error message for the mapped kinds (S20.2).
fn unlink_error_message(error: &std::io::Error, path: &str) -> String {
    match error.kind() {
        std::io::ErrorKind::NotFound => {
            format!("ENOENT: no such file or directory, unlink '{path}'")
        }
        std::io::ErrorKind::PermissionDenied => {
            format!("EPERM: operation not permitted, unlink '{path}'")
        }
        _ => error.to_string(),
    }
}

/// Delete a session file, trying the `trash` CLI first, then falling back to
/// unlink (upstream `deleteSessionFile`).
pub fn delete_session_file(session_path: &str) -> DeleteResult {
    // Try `trash` first (if installed)
    let trash_args: Vec<&str> = if session_path.starts_with('-') {
        vec!["--", session_path]
    } else {
        vec![session_path]
    };
    let trash_result = std::process::Command::new("trash")
        .args(&trash_args)
        .output();
    let mut trash_error_message: Option<String> = None;
    let mut trash_stderr = String::new();
    let mut trash_status_zero = false;
    match trash_result {
        Ok(output) => {
            trash_stderr = String::from_utf8_lossy(&output.stderr).to_string();
            trash_status_zero = output.status.success();
        }
        Err(error) => {
            trash_error_message = Some(spawn_error_message(&error));
        }
    }

    let get_trash_error_hint = || -> Option<String> {
        let mut parts: Vec<String> = Vec::new();
        if let Some(message) = &trash_error_message {
            parts.push(message.clone());
        }
        let stderr = trash_stderr.trim();
        if !stderr.is_empty() {
            let first_line = stderr.split('\n').next().unwrap_or(stderr);
            parts.push(first_line.to_string());
        }
        if parts.is_empty() {
            return None;
        }
        let joined: String = parts.join(" · ");
        let clipped: String = joined.chars().take(200).collect();
        Some(format!("trash: {clipped}"))
    };

    // If trash reports success, or the file is gone afterwards, treat it as
    // successful
    if trash_status_zero || !std::path::Path::new(session_path).exists() {
        return DeleteResult {
            ok: true,
            method: DeleteMethod::Trash,
            error: None,
        };
    }

    // Fallback to permanent deletion
    match std::fs::remove_file(session_path) {
        Ok(()) => DeleteResult {
            ok: true,
            method: DeleteMethod::Unlink,
            error: None,
        },
        Err(error) => {
            let unlink_error = unlink_error_message(&error, session_path);
            let error = match get_trash_error_hint() {
                Some(hint) => format!("{unlink_error} ({hint})"),
                None => unlink_error,
            };
            DeleteResult {
                ok: false,
                method: DeleteMethod::Unlink,
                error: Some(error),
            }
        }
    }
}

// ===========================================================================
// SessionSelectorComponent
// ===========================================================================

/// Work queued for the async flows (upstream: fire-and-forget promise bodies;
/// S20.1).
pub(crate) enum PendingWork {
    DeleteSession(String),
    RenameSubmit(String),
}

/// The in-flight scope load: the loader future plus the identifiers the
/// result application re-checks (upstream: the live promise and its closure
/// captures).
struct ActiveLoad {
    scope: SessionScope,
    reason: LoadReason,
    token: u64,
    future: Pin<Box<dyn Future<Output = Result<Vec<SessionInfo>, String>> + Send>>,
    progress_cell: Arc<Mutex<Option<(usize, usize)>>>,
}

/// Body mode (upstream: the base layout content).
#[derive(Clone, Copy, PartialEq, Eq)]
enum BodyMode {
    List,
    Rename,
}

/// Upstream `SessionSelectorComponent`.
pub struct SessionSelectorComponent {
    can_rename: bool,
    session_list: SessionList,
    header: SessionSelectorHeader,
    scope: SessionScope,
    sort_mode: SortMode,
    name_filter: NameFilter,
    current_sessions: Option<Vec<SessionInfo>>,
    all_sessions: Option<Vec<SessionInfo>>,
    current_sessions_loader: SessionsLoader,
    all_sessions_loader: SessionsLoader,
    request_render: RenderFn,
    rename_session: Option<RenameSessionFn>,
    /// Upstream `currentLoad`/`allLoad` (an `AbortController | null`): a
    /// `Some(token)` marks an in-flight load for the scope. The token pins
    /// the load so a result completing after `cancelLoads` is discarded.
    current_load: Option<u64>,
    all_load: Option<u64>,
    load_generation: u64,
    mode: BodyMode,
    rename_input: Input,
    rename_target_path: Option<String>,
    focused: bool,
    events: SessionEventQueue,
    pending: Arc<Mutex<Vec<PendingWork>>>,
    /// Rename requests staged from the event queue for `run_pending_work`.
    pending_renames: Vec<String>,
    /// The started-but-unfinished scope load (upstream: the in-flight
    /// promise); polled and re-stored by `run_pending_work` so dropping the
    /// awaiting stack never loses the load.
    active_load: Option<ActiveLoad>,
    theme: Arc<Theme>,

    pub on_select: Option<Box<dyn FnMut(&str) + Send>>,
    pub on_cancel: Option<Box<dyn FnMut() + Send>>,
    pub on_exit: Option<Box<dyn FnMut() + Send>>,
}

impl SessionSelectorComponent {
    /// Upstream constructor. The initial current-scope load must be driven by
    /// the caller via [`Self::initial_load`] (S20.1).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        current_sessions_loader: SessionsLoader,
        all_sessions_loader: SessionsLoader,
        on_select: Box<dyn FnMut(&str) + Send>,
        on_cancel: Box<dyn FnMut() + Send>,
        on_exit: Box<dyn FnMut() + Send>,
        request_render: Box<dyn FnMut() + Send>,
        options: Option<SessionSelectorOptions>,
        current_session_file_path: Option<&str>,
        theme: Arc<Theme>,
    ) -> Self {
        super::model_selector::set_default_theme(Arc::clone(&theme));
        let mut options = options.unwrap_or_default();
        let request_render: RenderFn = Arc::new(Mutex::new(Box::new(request_render)));

        let rename_session = options.rename_session;
        let can_rename = rename_session.is_some();
        let show_rename_hint = options.show_rename_hint.unwrap_or(can_rename);

        let events: SessionEventQueue = Arc::new(Mutex::new(Vec::new()));
        let pending: Arc<Mutex<Vec<PendingWork>>> = Arc::new(Mutex::new(Vec::new()));

        // Create session list (starts empty, will be populated after load)
        let list_keybindings = options
            .keybindings
            .take()
            .unwrap_or_else(|| KeybindingsManager::new(Default::default(), None));
        let mut session_list = SessionList::new(
            Vec::new(),
            false,
            SortMode::Threaded,
            NameFilter::All,
            list_keybindings,
            current_session_file_path,
            Arc::clone(&theme),
        );
        session_list.set_clock(now_ms);

        // The list reports through the shared queue; the component drains it
        // in `handle_input` (upstream wires closures over `this`).
        let install = |list: &mut SessionList,
                       events: &SessionEventQueue,
                       pending: &Arc<Mutex<Vec<PendingWork>>>| {
            list.on_select = Some({
                let events = Arc::clone(events);
                Box::new(move |path: &str| {
                    push_event(&events, SessionSelectorEvent::Select(path.to_string()));
                })
            });
            list.on_cancel = Some({
                let events = Arc::clone(events);
                Box::new(move || {
                    push_event(&events, SessionSelectorEvent::Cancel);
                })
            });
            list.on_exit = Some({
                let events = Arc::clone(events);
                Box::new(move || {
                    push_event(&events, SessionSelectorEvent::Exit);
                })
            });
            list.on_toggle_scope = Some({
                let events = Arc::clone(events);
                Box::new(move || {
                    push_event(&events, SessionSelectorEvent::ToggleScope);
                })
            });
            list.on_toggle_sort = Some({
                let events = Arc::clone(events);
                Box::new(move || {
                    push_event(&events, SessionSelectorEvent::ToggleSort);
                })
            });
            list.on_toggle_name_filter = Some({
                let events = Arc::clone(events);
                Box::new(move || {
                    push_event(&events, SessionSelectorEvent::ToggleNameFilter);
                })
            });
            list.on_toggle_path = Some({
                let events = Arc::clone(events);
                Box::new(move |show_path: bool| {
                    push_event(&events, SessionSelectorEvent::TogglePath(show_path));
                })
            });
            list.on_delete_confirmation_change = Some({
                let events = Arc::clone(events);
                Box::new(move |path: Option<&str>| {
                    push_event(
                        &events,
                        SessionSelectorEvent::DeleteConfirmation(path.map(str::to_string)),
                    );
                })
            });
            list.on_delete_session = Some({
                let events = Arc::clone(events);
                let pending = Arc::clone(pending);
                Box::new(move |path: &str| {
                    pending
                        .lock()
                        .expect("pending work")
                        .push(PendingWork::DeleteSession(path.to_string()));
                    // The upstream fires the async body immediately; the
                    // caller drives it via `run_pending_work`.
                    let _ = &events;
                })
            });
            list.on_rename_session = Some({
                let events = Arc::clone(events);
                Box::new(move |path: &str| {
                    push_event(&events, SessionSelectorEvent::Rename(path.to_string()));
                })
            });
            list.on_error = Some({
                let events = Arc::clone(events);
                Box::new(move |message: &str| {
                    push_event(&events, SessionSelectorEvent::Error(message.to_string()));
                })
            });
        };
        install(&mut session_list, &events, &pending);

        let mut header = SessionSelectorHeader::new(
            SessionScope::Current,
            SortMode::Threaded,
            NameFilter::All,
            Arc::clone(&request_render),
            Arc::clone(&theme),
        );
        header.set_show_rename_hint(show_rename_hint);

        let mut rename_input = Input::new(InputOptions::default());
        rename_input.set_focused(false);
        {
            let pending = Arc::clone(&pending);
            rename_input.on_submit(move |value: &str| {
                pending
                    .lock()
                    .expect("pending work")
                    .push(PendingWork::RenameSubmit(value.to_string()));
            });
        }

        let mut component = Self {
            can_rename,
            session_list,
            header,
            scope: SessionScope::Current,
            sort_mode: SortMode::Threaded,
            name_filter: NameFilter::All,
            current_sessions: None,
            all_sessions: None,
            current_sessions_loader,
            all_sessions_loader,
            request_render,
            rename_session,
            current_load: None,
            all_load: None,
            load_generation: 0,
            mode: BodyMode::List,
            rename_input,
            rename_target_path: None,
            focused: false,
            events,
            pending,
            pending_renames: Vec::new(),
            active_load: None,
            theme,
            on_select: Some(on_select),
            on_cancel: Some(on_cancel),
            on_exit: Some(on_exit),
        };

        // Ensure header status timeouts are cleared when leaving the selector
        // (upstream `clearStatusMessage` in the list event closures) happens
        // during event draining; nothing else to wire here.
        let _ = &mut component;
        component
    }

    /// S20.5 clock seam (test injection for the session ages).
    pub fn set_clock(&mut self, clock: fn() -> i64) {
        self.session_list.set_clock(clock);
    }

    /// The initial `loadScope("current", "initial")` future (S20.1).
    pub fn initial_load(&mut self) -> Pin<Box<dyn Future<Output = ()> + '_>> {
        Box::pin(self.load_scope(SessionScope::Current, LoadReason::Initial))
    }

    /// The list accessor (`getSessionList`).
    pub fn session_list(&self) -> &SessionList {
        &self.session_list
    }

    pub fn session_list_mut(&mut self) -> &mut SessionList {
        &mut self.session_list
    }

    /// Upstream `handleInput` plus the synchronous event forwarding.
    pub fn handle_input(&mut self, data: &str) {
        if self.mode == BodyMode::Rename {
            if keybindings_match(data, "tui.select.cancel") {
                self.exit_rename_mode();
                return;
            }
            self.rename_input.handle_input(data);
            return;
        }

        self.session_list.handle_input(data);
        self.drain_events();
    }

    /// Drain the queued list events (upstream's synchronous closure bodies).
    fn drain_events(&mut self) {
        let events: Vec<SessionSelectorEvent> =
            std::mem::take(&mut *self.events.lock().expect("session selector event queue"));
        for event in events {
            match event {
                SessionSelectorEvent::Select(path) => {
                    self.header.set_status_message(None, None);
                    self.cancel_loads();
                    if let Some(on_select) = &mut self.on_select {
                        on_select(&path);
                    }
                }
                SessionSelectorEvent::Cancel => {
                    self.header.set_status_message(None, None);
                    self.cancel_loads();
                    if let Some(on_cancel) = &mut self.on_cancel {
                        on_cancel();
                    }
                }
                SessionSelectorEvent::Exit => {
                    self.header.set_status_message(None, None);
                    self.cancel_loads();
                    if let Some(on_exit) = &mut self.on_exit {
                        on_exit();
                    }
                }
                SessionSelectorEvent::ToggleScope => self.toggle_scope(),
                SessionSelectorEvent::ToggleSort => self.toggle_sort_mode(),
                SessionSelectorEvent::ToggleNameFilter => self.toggle_name_filter(),
                SessionSelectorEvent::TogglePath(show_path) => {
                    self.header.set_show_path(show_path);
                    (self.request_render.lock().expect("render fn"))();
                }
                SessionSelectorEvent::DeleteConfirmation(path) => {
                    self.header.set_confirming_delete_path(path);
                    (self.request_render.lock().expect("render fn"))();
                }
                SessionSelectorEvent::Rename(session_path) => {
                    // Gating + rename-mode entry run in `run_pending_work`
                    // (they read the loading state, like upstream's closure).
                    self.pending_renames.push(session_path);
                }
                SessionSelectorEvent::Error(message) => {
                    self.header.set_status_message(
                        Some(StatusMessage {
                            is_error: true,
                            message,
                        }),
                        Some(3000),
                    );
                    (self.request_render.lock().expect("render fn"))();
                }
            }
        }
    }

    /// Drive the queued async flows (upstream: the fire-and-forget promise
    /// bodies started by the list closures — S20.1). Must be awaited after
    /// every [`Self::handle_input`].
    pub async fn run_pending_work(&mut self) {
        let work: Vec<PendingWork> =
            std::mem::take(&mut *self.pending.lock().expect("pending work"));
        for item in work {
            match item {
                PendingWork::DeleteSession(session_path) => {
                    // Upstream `sessionList.onDeleteSession` body.
                    let result = delete_session_file(&session_path);

                    if result.ok {
                        if let Some(current) = &mut self.current_sessions {
                            current.retain(|s| s.path != session_path);
                        }
                        if let Some(all) = &mut self.all_sessions {
                            all.retain(|s| s.path != session_path);
                        }

                        let sessions = match self.scope {
                            SessionScope::All => self.all_sessions.clone().unwrap_or_default(),
                            SessionScope::Current => {
                                self.current_sessions.clone().unwrap_or_default()
                            }
                        };
                        let show_cwd = self.scope == SessionScope::All;
                        self.session_list.set_sessions(sessions, show_cwd);

                        let msg = if result.method == DeleteMethod::Trash {
                            "Session moved to trash"
                        } else {
                            "Session deleted"
                        };
                        self.header.set_status_message(
                            Some(StatusMessage {
                                is_error: false,
                                message: msg.to_string(),
                            }),
                            Some(2000),
                        );
                        self.refresh_sessions_after_mutation().await;
                    } else {
                        let error_message =
                            result.error.unwrap_or_else(|| "Unknown error".to_string());
                        self.header.set_status_message(
                            Some(StatusMessage {
                                is_error: true,
                                message: format!("Failed to delete: {error_message}"),
                            }),
                            Some(3000),
                        );
                    }

                    (self.request_render.lock().expect("render fn"))();
                }
                PendingWork::RenameSubmit(value) => {
                    self.confirm_rename(&value).await;
                }
            }
        }

        // The rename requests (gating + rename-mode entry) are synchronous
        // but must run after the list's key handling; they ride the event
        // queue into `pending_renames`.
        let renames = std::mem::take(&mut self.pending_renames);
        for session_path in renames {
            if !self.can_rename {
                continue;
            }
            // Upstream: `if (this.scope === "current" ? this.currentLoad :
            // this.allLoad) return;`
            let load_in_flight = if self.scope == SessionScope::Current {
                self.current_load
            } else {
                self.all_load
            };
            if load_in_flight.is_some() {
                continue;
            }

            let sessions = match self.scope {
                SessionScope::All => self.all_sessions.clone().unwrap_or_default(),
                SessionScope::Current => self.current_sessions.clone().unwrap_or_default(),
            };
            let current_name = sessions
                .iter()
                .find(|s| s.path == session_path)
                .and_then(|s| s.name.clone());
            self.enter_rename_mode(&session_path, current_name.as_deref());
        }

        // The in-flight scope load (upstream: the live promise from
        // `void this.loadScope("all", "toggle")`) — polled and re-stored so a
        // dropped awaiting stack never loses it.
        while let Some((scope, reason, seq, result)) = self.poll_active_load() {
            self.apply_load_result(scope, reason, seq, result);
        }
    }

    /// The header status auto-hide tick (S20.1).
    pub fn fire_status_timeout(&mut self) -> bool {
        self.header.fire_status_timeout()
    }

    /// Upstream `enterRenameMode`.
    fn enter_rename_mode(&mut self, session_path: &str, current_name: Option<&str>) {
        self.mode = BodyMode::Rename;
        self.rename_target_path = Some(session_path.to_string());
        self.rename_input.set_value(current_name.unwrap_or(""));
        self.rename_input.set_focused(true);

        (self.request_render.lock().expect("render fn"))();
    }

    /// Upstream `exitRenameMode`.
    fn exit_rename_mode(&mut self) {
        self.mode = BodyMode::List;
        self.rename_target_path = None;

        (self.request_render.lock().expect("render fn"))();
    }

    /// Upstream `confirmRename`.
    async fn confirm_rename(&mut self, value: &str) {
        let next = value.trim().to_string();
        if next.is_empty() {
            return;
        }
        let target = self.rename_target_path.clone();
        let Some(target) = target else {
            self.exit_rename_mode();
            return;
        };

        // Find current name for callback
        let Some(rename_session) = self.rename_session.clone() else {
            self.exit_rename_mode();
            return;
        };

        rename_session(target, next).await;
        self.refresh_sessions_after_mutation().await;
        self.exit_rename_mode();
    }

    /// Upstream `loadScope`: starts the load and drives it to completion
    /// (initial/refresh loads use immediate loaders in the component's own
    /// flow; the toggle path goes through [`Self::active_load`] so dropping
    /// the awaiting stack never loses the load).
    async fn load_scope(&mut self, scope: SessionScope, reason: LoadReason) {
        self.start_load(scope, reason);
        loop {
            if let Some((scope, reason, seq, result)) = self.poll_active_load() {
                self.apply_load_result(scope, reason, seq, result);
                return;
            }
            // Load still pending: the state lives in `active_load`, so the
            // awaiting stack can be dropped and resumed via
            // `run_pending_work` (S20.1).
            std::future::pending::<()>().await;
        }
    }

    /// The synchronous half of `loadScope` upstream: mark loading, mint the
    /// load token, sync the header, call the loader (the returned future is
    /// stored on the component — see [`Self::poll_active_load`]).
    fn start_load(&mut self, scope: SessionScope, reason: LoadReason) -> Option<u64> {
        // Upstream `if (scope === "current" ? this.currentLoad : this.allLoad) return;`
        let slot_in_use = match scope {
            SessionScope::Current => self.current_load,
            SessionScope::All => self.all_load,
        };
        if slot_in_use.is_some() {
            return None;
        }
        let show_cwd = scope == SessionScope::All;
        let _ = show_cwd;

        // Mark loading (the token is the port's `AbortController` identity).
        self.load_generation += 1;
        let token = self.load_generation;
        match scope {
            SessionScope::Current => self.current_load = Some(token),
            SessionScope::All => self.all_load = Some(token),
        }
        self.header.set_scope(scope);
        self.header.set_loading(true);
        (self.request_render.lock().expect("render fn"))();

        // The progress callback fires during the load; the port stages the
        // values in a shared cell that `apply_progress` flushes into the
        // header under the same scope/token guards (S20.1 explicit progress
        // drain).
        let progress_cell: Arc<Mutex<Option<(usize, usize)>>> = Arc::new(Mutex::new(None));
        let on_progress: SessionListProgress = {
            let progress_cell = Arc::clone(&progress_cell);
            Arc::new(move |loaded: usize, total: usize| {
                *progress_cell.lock().expect("progress cell") = Some((loaded, total));
            })
        };

        let future = match scope {
            SessionScope::Current => (self.current_sessions_loader)(Some(on_progress)),
            SessionScope::All => (self.all_sessions_loader)(Some(on_progress)),
        };
        self.active_load = Some(ActiveLoad {
            scope,
            reason,
            token,
            future,
            progress_cell,
        });
        Some(token)
    }

    /// Upstream `cancelLoads`: abort both loads and drop their cached
    /// sessions. The port aborts by dropping the in-flight future and
    /// clearing the scope slots; a load polled afterwards is discarded by
    /// the token check.
    fn cancel_loads(&mut self) {
        if self.current_load.take().is_some() {
            self.current_sessions = None;
        }
        if self.all_load.take().is_some() {
            self.all_sessions = None;
        }
        self.active_load = None;
    }

    /// Whether the load carrying `token` for `scope` is still the scope's
    /// active load (upstream `isActive()`).
    fn load_is_active(&self, scope: SessionScope, token: u64) -> bool {
        match scope {
            SessionScope::Current => self.current_load == Some(token),
            SessionScope::All => self.all_load == Some(token),
        }
    }

    /// Poll the stored load once, re-storing it when still pending (upstream:
    /// the promise lives independently of the awaiting stack — S20.1).
    /// Returns the result when the load completed.
    fn poll_active_load(
        &mut self,
    ) -> Option<(
        SessionScope,
        LoadReason,
        u64,
        Result<Vec<SessionInfo>, String>,
    )> {
        let mut active = self.active_load.take()?;
        let waker = futures::task::noop_waker();
        let mut cx = std::task::Context::from_waker(&waker);
        match active.future.as_mut().poll(&mut cx) {
            std::task::Poll::Ready(result) => {
                Some((active.scope, active.reason, active.token, result))
            }
            std::task::Poll::Pending => {
                self.active_load = Some(active);
                None
            }
        }
    }

    /// The post-await half of `loadScope` upstream (guards, session store,
    /// header sync, error path).
    fn apply_load_result(
        &mut self,
        scope: SessionScope,
        reason: LoadReason,
        token: u64,
        result: Result<Vec<SessionInfo>, String>,
    ) {
        let _ = reason;
        let show_cwd = scope == SessionScope::All;
        if !self.load_is_active(scope, token) {
            // Upstream `if (!isActive()) return;` — the load was cancelled.
            return;
        }
        match result {
            Ok(sessions) => {
                match scope {
                    SessionScope::Current => {
                        self.current_sessions = Some(sessions.clone());
                        self.current_load = None;
                    }
                    SessionScope::All => {
                        self.all_sessions = Some(sessions.clone());
                        self.all_load = None;
                    }
                }

                if scope != self.scope {
                    return;
                }

                self.header.set_loading(false);
                self.session_list.set_sessions(sessions, show_cwd);
                (self.request_render.lock().expect("render fn"))();
            }
            Err(message) => {
                match scope {
                    SessionScope::Current => {
                        self.current_load = None;
                        self.current_sessions = None;
                    }
                    SessionScope::All => {
                        self.all_load = None;
                        self.all_sessions = None;
                    }
                }

                if scope != self.scope {
                    return;
                }

                self.header.set_loading(false);
                self.header.set_status_message(
                    Some(StatusMessage {
                        is_error: true,
                        message: format!("Failed to load sessions: {message}"),
                    }),
                    Some(4000),
                );

                self.session_list.set_sessions(Vec::new(), show_cwd);
                (self.request_render.lock().expect("render fn"))();
            }
        }
    }

    /// Flush progress reported by the in-flight loader into the header
    /// (upstream: the loader's `onProgress` callback runs inline; S20.1
    /// explicit progress drain). Returns whether progress was applied.
    pub fn apply_progress(&mut self) -> bool {
        let Some(active) = &self.active_load else {
            return false;
        };
        let Some((loaded, total)) = active.progress_cell.lock().expect("progress cell").take()
        else {
            return false;
        };
        let scope_unchanged = active.scope == self.scope;
        let token_current = self.load_is_active(active.scope, active.token);
        if scope_unchanged && token_current {
            self.header.set_progress(loaded, total);
            (self.request_render.lock().expect("render fn"))();
            return true;
        }
        false
    }

    /// Upstream `toggleSortMode` (threaded → recent → relevance → threaded).
    fn toggle_sort_mode(&mut self) {
        self.sort_mode = match self.sort_mode {
            SortMode::Threaded => SortMode::Recent,
            SortMode::Recent => SortMode::Relevance,
            SortMode::Relevance => SortMode::Threaded,
        };
        self.header.set_sort_mode(self.sort_mode);
        self.session_list.set_sort_mode(self.sort_mode);
        (self.request_render.lock().expect("render fn"))();
    }

    /// Upstream `toggleNameFilter`.
    fn toggle_name_filter(&mut self) {
        self.name_filter = if self.name_filter == NameFilter::All {
            NameFilter::Named
        } else {
            NameFilter::All
        };
        self.header.set_name_filter(self.name_filter);
        self.session_list.set_name_filter(self.name_filter);
        (self.request_render.lock().expect("render fn"))();
    }

    /// Upstream `refreshSessionsAfterMutation`.
    async fn refresh_sessions_after_mutation(&mut self) {
        self.cancel_loads();
        self.current_sessions = None;
        self.all_sessions = None;
        self.load_scope(self.scope, LoadReason::Refresh).await;
    }

    /// Upstream `toggleScope` (capture-verbatim: current→all with a cached
    /// all list swaps it in and clears loading; without a cache the list
    /// keeps showing the previous scope's rows under the loading header and
    /// the toggle load is started only when one is not already in flight).
    fn toggle_scope(&mut self) {
        if self.scope == SessionScope::Current {
            self.scope = SessionScope::All;
            self.header.set_scope(self.scope);

            if let Some(sessions) = self.all_sessions.clone() {
                self.header.set_loading(false);
                self.session_list.set_sessions(sessions, true);
                (self.request_render.lock().expect("render fn"))();
                return;
            }

            if self.all_load.is_none() {
                // Upstream fires `void this.loadScope("all", "toggle")`; the
                // started load is stored and polled through
                // `run_pending_work` (S20.1).
                self.start_load(self.scope, LoadReason::Toggle);
            }
            return;
        }

        self.scope = SessionScope::Current;
        self.header.set_scope(self.scope);
        let loading = self.current_load.is_some();
        self.header.set_loading(loading);
        let sessions = self.current_sessions.clone().unwrap_or_default();
        self.session_list.set_sessions(sessions, false);
        (self.request_render.lock().expect("render fn"))();
    }

    /// Whether the rename panel is active (test surface).
    pub fn is_rename_mode(&self) -> bool {
        self.mode == BodyMode::Rename
    }

    /// The rename input value (test surface).
    pub fn rename_value(&self) -> &str {
        self.rename_input.value()
    }
}

/// Upstream constructor `options` tail.
#[derive(Default)]
pub struct SessionSelectorOptions {
    pub rename_session: Option<RenameSessionFn>,
    pub show_rename_hint: Option<bool>,
    pub keybindings: Option<KeybindingsManager>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LoadReason {
    Initial,
    Refresh,
    Toggle,
}

impl Component for SessionSelectorComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        // buildBaseLayout: Spacer(1), DynamicBorder(accent), Spacer(1),
        // [header, Spacer(1)], content, Spacer(1), DynamicBorder(accent)
        let accent_border = DynamicBorder::new(Some(Box::new({
            let theme = Arc::clone(&self.theme);
            move |s: &str| theme_fg(&theme, "accent", s)
        })));
        lines.extend(spacer_lines(1));
        lines.extend(accent_border.render(width));
        lines.extend(spacer_lines(1));
        match self.mode {
            BodyMode::List => {
                lines.extend(self.header.render(width));
                lines.extend(spacer_lines(1));
                lines.extend(self.session_list.render_list(width));
            }
            BodyMode::Rename => {
                let theme = Arc::clone(&self.theme);
                let mut title = Text::with_options(&theme.bold("Rename Session"), 1, 0, None);
                lines.extend(title.render(width));
                lines.extend(spacer_lines(1));
                lines.extend(self.rename_input.render(width));
                lines.extend(spacer_lines(1));
                let hint = format!(
                    "{} to save · {} to cancel",
                    key_text("tui.select.confirm"),
                    key_text("tui.select.cancel")
                );
                let mut hint_text =
                    Text::with_options(&theme_fg(&theme, "muted", &hint), 1, 0, None);
                lines.extend(hint_text.render(width));
            }
        }
        lines.extend(spacer_lines(1));
        let accent_border = DynamicBorder::new(Some(Box::new({
            let theme = Arc::clone(&self.theme);
            move |s: &str| theme_fg(&theme, "accent", s)
        })));
        lines.extend(accent_border.render(width));
        lines
    }

    fn handle_input(&mut self, data: &str) {
        SessionSelectorComponent::handle_input(self, data);
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
        self.session_list.set_focused(focused);
        self.rename_input.set_focused(focused);
    }
}

/// Test helper: poll a future to completion with the noop waker (immediate
/// loaders resolve on the first poll).
#[cfg(test)]
#[allow(dead_code)] // replay driver used by later scenario batches
pub(crate) fn drive_future(fut: &mut Pin<Box<dyn Future<Output = ()> + '_>>) {
    use futures::task::noop_waker_ref;
    let waker = noop_waker_ref();
    let mut cx = std::task::Context::from_waker(waker);
    let _ = fut.as_mut().poll(&mut cx);
}

#[cfg(test)]
#[path = "session_selector_tests.rs"]
mod tests;
