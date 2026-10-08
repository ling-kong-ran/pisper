//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Port of `modes/interactive/components/model-selector.ts` (421 lines,
//! sha256 `92d70b9faffc9febf2ce0c519c7c086938bddfc5da9ad28d819f408d54e3637e`).
//!
//! Slice conventions shared by the six selector components in this directory:
//! - **Theme**: upstream reads the process-global `theme` singleton; the port
//!   threads an explicit `Arc<Theme>` through constructors (the global is the
//!   disclosed unported seam D1 in `modes/interactive/mod.rs`).
//! - **Composite rendering**: upstream mounts a `Container` child tree
//!   (`DynamicBorder`, `Spacer`, `Text`, `Input`, list container); the port
//!   renders the identical child sequence inline through the same widget
//!   primitives. Byte-identical for these keyboard-driven selectors (upstream
//!   wires input by focus; none of the six implements mouse selection).
//! - **`tui: TUI`**: the constructor's render-request hook becomes an explicit
//!   `Box<dyn FnMut()>`.
//! - **Background refresh**: upstream fires `void this.refreshModels()` and
//!   mutates state when the promise settles; the abort controller, 15s timer
//!   and promise scheduling are presentation seams. The deterministic
//!   post-await body is [`ModelSelectorComponent::apply_refresh_result`].
//! - The `keybinding-hints.ts` helpers and `DynamicBorder` live here as
//!   `pub(crate)` items reused by the sibling selector modules
//!   (`components/mod.rs` is frozen, so no separate support module exists).
//! - `searchInput.onSubmit` is upstream-dead through the component's key
//!   routing (`tui.select.confirm` intercepts Enter before the input sees it),
//!   so the port omits the callback (the confirm branch calls
//!   [`Self::handle_select`]).

use std::sync::Arc;

use crate::ai::models::ModelsRefreshResult;
use crate::ai::types::Model;
use crate::coding_agent::core::model_resolver::models_are_equal;
use crate::coding_agent::modes::interactive::model_search::{
    get_model_selector_search_text, ModelSearchItem,
};
use crate::coding_agent::modes::interactive::theme::Theme;
use crate::tui::component::Component;
use crate::tui::components::input::{Input, InputOptions};
use crate::tui::components::text::Text;
use crate::tui::fuzzy::fuzzy_filter;
use crate::tui::keybindings::with_keybindings;

// ===========================================================================
// Shared selector plumbing (DynamicBorder + keybinding hints)
// ===========================================================================

thread_local! {
    static DEFAULT_THEME: std::cell::RefCell<Option<Arc<Theme>>> = const { std::cell::RefCell::new(None) };
}

/// Register the fallback theme used by [`DynamicBorder`]'s default painter
/// (upstream captures the global `theme` at construction time).
pub(crate) fn set_default_theme(theme: Arc<Theme>) {
    DEFAULT_THEME.with(|slot| *slot.borrow_mut() = Some(theme));
}

fn default_theme() -> Arc<Theme> {
    DEFAULT_THEME
        .with(|slot| slot.borrow().clone())
        .expect("default theme not initialized; call set_default_theme")
}

/// `theme.fg(color, text)` with upstream's unknown-color panic semantics.
pub(crate) fn theme_fg(theme: &Theme, color: &str, text: &str) -> String {
    theme.fg(color, text).expect("theme fg color")
}

/// `theme.bg(color, text)` (wired into the interactive shell in r19+).
#[allow(dead_code)]
pub(crate) fn theme_bg(theme: &Theme, color: &str, text: &str) -> String {
    theme.bg(color, text).expect("theme bg color")
}

fn host_is_darwin() -> bool {
    crate::coding_agent::core::keybindings::node_platform() == "darwin"
}

/// Upstream `formatKeyText` (`keybinding-hints.ts`).
pub(crate) fn format_key_text(key: &str, capitalize: bool) -> String {
    key.split('/')
        .map(|part_key| {
            part_key
                .split('+')
                .map(|part| {
                    let display = if host_is_darwin() && part.eq_ignore_ascii_case("alt") {
                        "option".to_string()
                    } else {
                        part.to_string()
                    };
                    if capitalize {
                        let mut chars = display.chars();
                        match chars.next() {
                            Some(first) => {
                                first.to_uppercase().collect::<String>() + chars.as_str()
                            }
                            None => display,
                        }
                    } else {
                        display
                    }
                })
                .collect::<Vec<_>>()
                .join("+")
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn format_keys(keys: Vec<String>, capitalize: bool) -> String {
    if keys.is_empty() {
        return String::new();
    }
    format_key_text(&keys.join("/"), capitalize)
}

/// Upstream `getKeybindings().getKeys(id)`: the coding agent replaces the
/// process-global manager at startup with `KeybindingsManager.create()`, whose
/// table merges the coding-agent `app.*` entries into `TUI_KEYBINDINGS`. The
/// port's global defaults to the tui registry only, so fall back to the merged
/// coding-agent table for ids it does not carry (same resolution as the r19
/// oracle's `getKeybindings` layer: `app.*` ids never collide with tui ids).
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

/// Upstream `keyText`.
pub(crate) fn key_text(keybinding: &str) -> String {
    format_keys(registry_keys(keybinding), false)
}

/// Upstream `keyDisplayText`.
pub(crate) fn key_display_text(keybinding: &str) -> String {
    format_keys(registry_keys(keybinding), true)
}

/// Upstream `getKeybindings().matches(data, id)` (same merged-registry
/// resolution as [`registry_keys`]).
pub(crate) fn keybindings_match(data: &str, keybinding: &str) -> bool {
    if with_keybindings(|kb| !kb.get_keys(keybinding).is_empty()) {
        return with_keybindings(|kb| kb.matches(data, keybinding));
    }
    registry_keys(keybinding)
        .iter()
        .any(|key| crate::tui::keys::matches_key(data, key))
}

/// Upstream `keyHint`.
pub(crate) fn key_hint(theme: &Theme, keybinding: &str, description: &str) -> String {
    theme_fg(theme, "dim", &key_text(keybinding))
        + &theme_fg(theme, "muted", &format!(" {description}"))
}

/// Upstream `rawKeyHint`.
pub(crate) fn raw_key_hint(theme: &Theme, key: &str, description: &str) -> String {
    theme_fg(theme, "dim", &format_key_text(key, false))
        + &theme_fg(theme, "muted", &format!(" {description}"))
}

/// Upstream `DynamicBorder` (`components/dynamic-border.ts`): a full-width
/// `─` rule painted with `theme.fg("border", …)` unless overridden.
#[derive(Default)]
pub(crate) struct DynamicBorder {
    color: Option<Box<dyn Fn(&str) -> String + Send>>,
}

impl DynamicBorder {
    #[allow(dead_code)] // wired into the interactive shell in r19+
    pub(crate) fn new(color: Option<Box<dyn Fn(&str) -> String + Send>>) -> Self {
        Self { color }
    }

    pub(crate) fn render(&self, width: usize) -> Vec<String> {
        let line = match &self.color {
            Some(color) => color(&"\u{2500}".repeat(width.max(1))),
            None => theme_fg(&default_theme(), "border", &"\u{2500}".repeat(width.max(1))),
        };
        vec![line]
    }
}

/// Upstream `Spacer(lines)` renders `lines` empty lines.
pub(crate) fn spacer_lines(lines: usize) -> Vec<String> {
    vec![String::new(); lines]
}

// ===========================================================================
// ModelSelectorComponent
// ===========================================================================

/// Upstream `ScopedModelItem`.
#[derive(Clone, Debug)]
pub struct ScopedModelItem {
    pub model: Model,
    pub thinking_level: Option<String>,
}

/// Upstream `DefaultModelReference`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DefaultModelReference {
    pub provider: String,
    pub id: String,
}

/// Upstream `ModelScope`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ModelScope {
    All,
    Scoped,
}

#[derive(Clone, Debug)]
struct ModelItem {
    provider: String,
    id: String,
    model: Model,
}

/// One entry of the rebuilt list container: a styled `Text` line or a spacer.
enum ListChild {
    Text(Text),
    Spacer(usize),
}

/// The runtime surface model-selector reads (`core/model-runtime.ts` subset).
/// Implemented by [`crate::coding_agent::core::model_runtime::ModelRuntime`].
pub trait ModelSelectorRuntime: Send + Sync {
    fn get_available_snapshot(&self) -> Vec<Model>;
    fn get_model(&self, provider: &str, id: &str) -> Option<Model>;
    fn get_error(&self) -> Option<String>;
}

/// Callbacks (upstream `onSelect` / `onSelectAsDefault?` / `onCancel`).
pub struct ModelSelectorCallbacks {
    pub on_select: Box<dyn FnMut(&Model) + Send>,
    pub on_select_as_default: Option<Box<dyn FnMut(&Model) + Send>>,
    pub on_cancel: Box<dyn FnMut() + Send>,
}

/// Upstream `ModelSelectorComponent`.
pub struct ModelSelectorComponent {
    theme: Arc<Theme>,
    search_input: Input,
    focused: bool,
    list_lines: Vec<ListChild>,
    all_models: Vec<ModelItem>,
    scoped_models: Vec<ScopedModelItem>,
    scoped_model_items: Vec<ModelItem>,
    active_models: Vec<ModelItem>,
    filtered_models: Vec<ModelItem>,
    selected_index: usize,
    current_model: Option<Model>,
    model_runtime: Arc<dyn ModelSelectorRuntime>,
    callbacks: Option<ModelSelectorCallbacks>,
    error_message: Option<String>,
    refresh_status_message: String,
    refresh_status_success: bool,
    request_render: Box<dyn FnMut() + Send>,
    default_model: Option<DefaultModelReference>,
    scope: ModelScope,
    /// Upstream `scopeText` / `scopeHintText` (scoped models present) or the
    /// static provider hint (none present).
    scope_text: Option<Text>,
    scope_hint_text: Option<Text>,
    provider_hint: Option<String>,
    closed: bool,
}

impl ModelSelectorComponent {
    /// Upstream constructor. `initial_search_input` seeds the search field.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        theme: Arc<Theme>,
        current_model: Option<Model>,
        model_runtime: Arc<dyn ModelSelectorRuntime>,
        scoped_models: Vec<ScopedModelItem>,
        callbacks: ModelSelectorCallbacks,
        request_render: Box<dyn FnMut() + Send>,
        initial_search_input: Option<&str>,
        default_model: Option<DefaultModelReference>,
    ) -> Self {
        set_default_theme(Arc::clone(&theme));
        let scope = if scoped_models.is_empty() {
            ModelScope::All
        } else {
            ModelScope::Scoped
        };

        let mut search_input = Input::new(InputOptions::default());
        if let Some(initial) = initial_search_input {
            search_input.set_value(initial);
        }

        let mut component = Self {
            theme,
            search_input,
            focused: false,
            list_lines: Vec::new(),
            all_models: Vec::new(),
            scoped_models,
            scoped_model_items: Vec::new(),
            active_models: Vec::new(),
            filtered_models: Vec::new(),
            selected_index: 0,
            current_model,
            model_runtime,
            callbacks: Some(callbacks),
            error_message: None,
            refresh_status_message: "Refreshing model catalogs…".to_string(),
            refresh_status_success: false,
            request_render,
            default_model,
            scope,
            scope_text: None,
            scope_hint_text: None,
            provider_hint: None,
            closed: false,
        };

        if !component.scoped_models.is_empty() {
            component.scope_text =
                Some(Text::with_options(&component.get_scope_text(), 0, 0, None));
            component.scope_hint_text = Some(Text::with_options(
                &component.get_scope_hint_text(),
                0,
                0,
                None,
            ));
        } else {
            let hint =
                "Only showing models from configured providers. Use /login to add providers.";
            component.provider_hint = Some(theme_fg(&component.theme, "warning", hint));
        }

        // Render the current snapshot immediately, then refresh in the
        // background (the host drives `apply_refresh_result`).
        component.load_models_from_snapshot();
        if initial_search_input.is_some() {
            let query = component.search_input.value().to_string();
            component.filter_models(&query);
        } else {
            component.update_list();
        }
        (component.request_render)();
        component
    }

    /// Upstream `loadModelsFromSnapshot`.
    pub fn load_models_from_snapshot(&mut self) {
        let models = self
            .model_runtime
            .get_available_snapshot()
            .into_iter()
            .map(|model| ModelItem {
                provider: model.provider.clone(),
                id: model.id.clone(),
                model,
            })
            .collect();
        self.all_models = self.sort_models(models);
        for scoped in &mut self.scoped_models {
            if let Some(refreshed) = self
                .model_runtime
                .get_model(&scoped.model.provider, &scoped.model.id)
            {
                scoped.model = refreshed;
            }
        }
        self.scoped_model_items = self
            .scoped_models
            .iter()
            .map(|scoped| ModelItem {
                provider: scoped.model.provider.clone(),
                id: scoped.model.id.clone(),
                model: scoped.model.clone(),
            })
            .collect();
        self.active_models = match self.scope {
            ModelScope::Scoped => self.scoped_model_items.clone(),
            ModelScope::All => self.all_models.clone(),
        };
        self.filtered_models = self.active_models.clone();
        let current_index = self.current_model.as_ref().and_then(|current| {
            self.filtered_models
                .iter()
                .position(|item| models_are_equal(current, &item.model))
        });
        self.selected_index = match current_index {
            Some(index) => index,
            None => self
                .selected_index
                .min(self.filtered_models.len().saturating_sub(1)),
        };
    }

    /// Upstream `refreshModels()` post-await body (deterministic core; the
    /// promise/abort choreography is a presentation seam). `timed_out` mirrors
    /// the upstream 15s timeout flag.
    pub fn apply_refresh_result(
        &mut self,
        result: &Result<ModelsRefreshResult, String>,
        timed_out: bool,
    ) {
        if self.closed {
            return;
        }
        match result {
            Ok(result) => {
                self.refresh_status_message = String::new();
                if result.aborted && timed_out {
                    self.error_message =
                        Some("Model refresh timed out; showing cached models.".to_string());
                } else if result.errors.len() == 1 {
                    let provider = result.errors.keys().next().cloned().unwrap_or_default();
                    self.error_message = Some(format!(
                        "Could not refresh {provider}; showing cached models."
                    ));
                } else if !result.errors.is_empty() {
                    let providers = result.errors.keys().cloned().collect::<Vec<_>>().join(", ");
                    self.error_message = Some(format!(
                        "Could not refresh {} model catalogs ({providers}); showing cached models.",
                        result.errors.len()
                    ));
                } else {
                    self.error_message = self.model_runtime.get_error();
                    if self.error_message.is_none() {
                        self.refresh_status_message = "Model catalogs refreshed.".to_string();
                        self.refresh_status_success = true;
                    }
                }
                self.load_models_from_snapshot();
                let query = self.search_input.value().to_string();
                self.filter_models(&query);
                (self.request_render)();
            }
            Err(_error) => {
                self.refresh_status_message = String::new();
                self.error_message = Some(if timed_out {
                    "Model refresh timed out; showing cached models.".to_string()
                } else {
                    // Upstream interpolates `error.message`; the host passes the
                    // transport error string through the seam payload.
                    "Could not refresh model catalogs: transport error".to_string()
                });
                self.update_list();
                (self.request_render)();
            }
        }
    }

    /// Upstream `dispose`.
    pub fn dispose(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
    }

    /// Upstream `sortModels`: current model first, default model second, then
    /// by provider (stable sort with the upstream comparator).
    fn sort_models(&self, models: Vec<ModelItem>) -> Vec<ModelItem> {
        let mut sorted = models;
        sorted.sort_by(|a, b| {
            let a_is_current = self
                .current_model
                .as_ref()
                .is_some_and(|m| models_are_equal(m, &a.model));
            let b_is_current = self
                .current_model
                .as_ref()
                .is_some_and(|m| models_are_equal(m, &b.model));
            if a_is_current && !b_is_current {
                return std::cmp::Ordering::Less;
            }
            if !a_is_current && b_is_current {
                return std::cmp::Ordering::Greater;
            }
            let a_is_default = self.is_default_model(&a.model);
            let b_is_default = self.is_default_model(&b.model);
            if a_is_default && !b_is_default {
                return std::cmp::Ordering::Less;
            }
            if !a_is_default && b_is_default {
                return std::cmp::Ordering::Greater;
            }
            a.provider.cmp(&b.provider)
        });
        sorted
    }

    fn get_scope_text(&self) -> String {
        let theme = &self.theme;
        let all_text = match self.scope {
            ModelScope::All => theme_fg(theme, "accent", "all"),
            ModelScope::Scoped => theme_fg(theme, "muted", "all"),
        };
        let scoped_text = match self.scope {
            ModelScope::All => theme_fg(theme, "muted", "scoped"),
            ModelScope::Scoped => theme_fg(theme, "accent", "scoped"),
        };
        format!(
            "{}{all_text}{}{scoped_text}",
            theme_fg(theme, "muted", "Scope: "),
            theme_fg(theme, "muted", " | ")
        )
    }

    fn get_scope_hint_text(&self) -> String {
        key_hint(&self.theme, "tui.input.tab", "scope")
            + &theme_fg(&self.theme, "muted", " (all/scoped)")
    }

    fn is_default_model(&self, model: &Model) -> bool {
        match &self.default_model {
            Some(default) => default.provider == model.provider && default.id == model.id,
            None => false,
        }
    }

    fn is_default_search(&self, query: &str) -> bool {
        let normalized = query.trim().to_lowercase();
        !normalized.is_empty() && "default".starts_with(&normalized)
    }

    fn set_scope(&mut self, scope: ModelScope) {
        if self.scope == scope {
            return;
        }
        self.scope = scope;
        self.active_models = match self.scope {
            ModelScope::Scoped => self.scoped_model_items.clone(),
            ModelScope::All => self.all_models.clone(),
        };
        let current_index = self.active_models.iter().position(|item| {
            self.current_model
                .as_ref()
                .is_some_and(|m| models_are_equal(m, &item.model))
        });
        self.selected_index = current_index.unwrap_or(0);
        let query = self.search_input.value().to_string();
        self.filter_models(&query);
        let scope_line = self.get_scope_text();
        if let Some(scope_text) = &mut self.scope_text {
            scope_text.set_text(&scope_line);
        }
    }

    /// Upstream `filterModels`.
    fn filter_models(&mut self, query: &str) {
        if !query.is_empty() {
            let default_model = self.default_model.clone();
            let is_default = |item: &ModelItem| match &default_model {
                Some(reference) => {
                    reference.provider == item.model.provider && reference.id == item.model.id
                }
                None => false,
            };
            // `fuzzy_filter` borrows the text out of the item, so pair each
            // item with its prebuilt search text (`searchText` + " default").
            let search_texts: Vec<(ModelItem, String)> = self
                .active_models
                .iter()
                .map(|item| {
                    let default_text = if is_default(item) { " default" } else { "" };
                    let search_text = get_model_selector_search_text(&ModelSearchItem {
                        id: item.id.clone(),
                        provider: item.provider.clone(),
                        name: (!item.model.name.is_empty()).then(|| item.model.name.clone()),
                    });
                    (item.clone(), format!("{search_text}{default_text}"))
                })
                .collect();
            let filtered = fuzzy_filter(search_texts, query, |pair: &(ModelItem, String)| {
                pair.1.as_str()
            })
            .into_iter()
            .map(|(item, _)| item)
            .collect::<Vec<_>>();
            if self.is_default_search(query) {
                let mut default_items: Vec<ModelItem> = self
                    .active_models
                    .iter()
                    .filter(|item| is_default(item))
                    .cloned()
                    .collect();
                let default_keys: Vec<String> = default_items
                    .iter()
                    .map(|item| format!("{}\u{0}{}", item.provider, item.id))
                    .collect();
                default_items.extend(filtered.into_iter().filter(|item| {
                    !default_keys.contains(&format!("{}\u{0}{}", item.provider, item.id))
                }));
                self.filtered_models = default_items;
            } else {
                self.filtered_models = filtered;
            }
        } else {
            self.filtered_models = self.active_models.clone();
        }
        self.selected_index = if !query.is_empty() {
            0
        } else {
            self.selected_index
                .min(self.filtered_models.len().saturating_sub(1))
        };
        self.update_list();
    }

    /// Upstream `updateList` — rebuilds the list-container children.
    fn update_list(&mut self) {
        let mut children: Vec<ListChild> = Vec::new();

        let max_visible: usize = 10;
        let start_index = self
            .selected_index
            .saturating_sub(max_visible / 2)
            .min(self.filtered_models.len().saturating_sub(max_visible));
        let end_index = (start_index + max_visible).min(self.filtered_models.len());

        let theme = Arc::clone(&self.theme);
        for i in start_index..end_index {
            let Some(item) = self.filtered_models.get(i) else {
                continue;
            };
            let is_selected = i == self.selected_index;
            let is_current = self
                .current_model
                .as_ref()
                .is_some_and(|m| models_are_equal(m, &item.model));
            let is_default = self.is_default_model(&item.model);
            let default_badge = if is_default {
                theme_fg(&theme, "muted", " · default")
            } else {
                String::new()
            };

            let cursor = if is_selected {
                theme_fg(&theme, "accent", "→ ")
            } else {
                "  ".to_string()
            };
            let current_marker = if is_current {
                theme_fg(&theme, "accent", "✓ ")
            } else {
                "  ".to_string()
            };
            let model_text = if is_selected {
                theme_fg(&theme, "accent", &item.id)
            } else {
                item.id.clone()
            };
            let provider_badge = theme_fg(&theme, "muted", &format!("[{}]", item.provider));
            let line =
                format!("{cursor}{current_marker}{model_text} {provider_badge}{default_badge}");
            children.push(ListChild::Text(Text::with_options(&line, 0, 0, None)));
        }

        if start_index > 0 || end_index < self.filtered_models.len() {
            let scroll_info = theme_fg(
                &theme,
                "muted",
                &format!(
                    "  ({}/{})",
                    self.selected_index + 1,
                    self.filtered_models.len()
                ),
            );
            children.push(ListChild::Text(Text::with_options(
                &scroll_info,
                0,
                0,
                None,
            )));
        }

        if let Some(error) = &self.error_message {
            for line in error.split('\n') {
                children.push(ListChild::Text(Text::with_options(
                    &theme_fg(&theme, "error", line),
                    0,
                    0,
                    None,
                )));
            }
        } else if self.filtered_models.is_empty() {
            children.push(ListChild::Text(Text::with_options(
                &theme_fg(&theme, "muted", "  No matching models"),
                0,
                0,
                None,
            )));
        } else {
            let selected = &self.filtered_models[self.selected_index];
            children.push(ListChild::Spacer(1));
            let model_name = format!("  Model Name: {}", selected.model.name);
            children.push(ListChild::Text(Text::with_options(
                &theme_fg(&theme, "muted", &model_name),
                0,
                0,
                None,
            )));
        }
        if !self.refresh_status_message.is_empty() {
            children.push(ListChild::Spacer(1));
            let status = format!("  {}", self.refresh_status_message);
            let painted = if self.refresh_status_success {
                theme_fg(&theme, "success", &status)
            } else {
                theme_fg(&theme, "muted", &status)
            };
            children.push(ListChild::Text(Text::with_options(&painted, 0, 0, None)));
        }

        self.list_lines = children;
    }

    fn handle_select(&mut self, model: &Model) {
        self.dispose();
        if let Some(callbacks) = &mut self.callbacks {
            (callbacks.on_select)(model);
        }
    }

    /// Upstream `handleInput`.
    pub fn handle_input(&mut self, key_data: &str) {
        if with_keybindings(|kb| kb.matches(key_data, "tui.input.tab")) {
            if !self.scoped_model_items.is_empty() {
                let next_scope = match self.scope {
                    ModelScope::All => ModelScope::Scoped,
                    ModelScope::Scoped => ModelScope::All,
                };
                self.set_scope(next_scope);
                let hint_line = self.get_scope_hint_text();
                if let Some(scope_hint_text) = &mut self.scope_hint_text {
                    scope_hint_text.set_text(&hint_line);
                }
            }
            return;
        }
        if with_keybindings(|kb| kb.matches(key_data, "tui.select.up")) {
            if self.filtered_models.is_empty() {
                return;
            }
            self.selected_index = if self.selected_index == 0 {
                self.filtered_models.len() - 1
            } else {
                self.selected_index - 1
            };
            self.update_list();
        } else if with_keybindings(|kb| kb.matches(key_data, "tui.select.down")) {
            if self.filtered_models.is_empty() {
                return;
            }
            self.selected_index = if self.selected_index == self.filtered_models.len() - 1 {
                0
            } else {
                self.selected_index + 1
            };
            self.update_list();
        } else if with_keybindings(|kb| kb.matches(key_data, "tui.select.confirm")) {
            if let Some(selected) = self.filtered_models.get(self.selected_index).cloned() {
                self.handle_select(&selected.model);
            }
        } else if with_keybindings(|kb| kb.matches(key_data, "tui.select.cancel")) {
            self.dispose();
            if let Some(callbacks) = &mut self.callbacks {
                (callbacks.on_cancel)();
            }
        } else if with_keybindings(|kb| kb.matches(key_data, "app.models.save"))
            && self
                .callbacks
                .as_ref()
                .is_some_and(|callbacks| callbacks.on_select_as_default.is_some())
        {
            if let Some(selected) = self.filtered_models.get(self.selected_index).cloned() {
                self.dispose();
                if let Some(callbacks) = &mut self.callbacks {
                    if let Some(on_select_as_default) = &mut callbacks.on_select_as_default {
                        (on_select_as_default)(&selected.model);
                    }
                }
            }
        } else {
            self.search_input.handle_input(key_data);
            let query = self.search_input.value().to_string();
            self.filter_models(&query);
        }
    }

    /// Upstream `getSearchInput`.
    pub fn search_input(&mut self) -> &mut Input {
        &mut self.search_input
    }

    /// Test seam: the currently selected model id.
    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn selected_model_id(&self) -> Option<String> {
        self.filtered_models
            .get(self.selected_index)
            .map(|item| item.id.clone())
    }
}

impl Component for ModelSelectorComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        // DynamicBorder
        lines.extend(DynamicBorder::default().render(width));
        // Spacer(1)
        lines.extend(spacer_lines(1));
        // Scope rows / provider hint
        if let Some(scope_text) = &mut self.scope_text {
            lines.extend(scope_text.render(width));
        }
        if let Some(scope_hint_text) = &mut self.scope_hint_text {
            lines.extend(scope_hint_text.render(width));
        }
        if let Some(hint) = &self.provider_hint {
            lines.extend(Text::with_options(hint, 0, 0, None).render(width));
        }
        // Spacer(1)
        lines.extend(spacer_lines(1));
        // Search input
        lines.extend(self.search_input.render(width));
        // Spacer(1)
        lines.extend(spacer_lines(1));
        // List container children
        for child in &mut self.list_lines {
            match child {
                ListChild::Text(text) => lines.extend(text.render(width)),
                ListChild::Spacer(count) => lines.extend(spacer_lines(*count)),
            }
        }
        // Spacer(1)
        lines.extend(spacer_lines(1));
        // Hint (only when a save-as-default callback exists)
        if self
            .callbacks
            .as_ref()
            .is_some_and(|callbacks| callbacks.on_select_as_default.is_some())
        {
            let hint = format!(
                "  {} to select · {} to set as default · {} to cancel",
                key_display_text("tui.select.confirm"),
                key_display_text("app.models.save"),
                key_display_text("tui.select.cancel"),
            );
            lines.extend(
                Text::with_options(&theme_fg(&self.theme, "dim", &hint), 0, 0, None).render(width),
            );
        }
        // DynamicBorder
        lines.extend(DynamicBorder::default().render(width));
        lines
    }

    fn is_focusable(&self) -> bool {
        true
    }

    fn focused(&self) -> bool {
        self.focused
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        self.search_input.set_focused(focused);
    }
}
