//! Port of upstream `coding-agent/src/core/keybindings.ts`.
//!
//! The app keybinding registry: the platform-adjusted `KEYBINDINGS` table
//! (`TUI_KEYBINDINGS` with four WSL/Windows-adjusted defaults plus the
//! `app.*` entries), legacy key-name migration, and the file-backed
//! [`KeybindingsManager`] extending the pi-tui manager
//! ([`crate::tui::keybindings::KeybindingsManager`], reused read-only).
//!
//! Seams / disclosed substitutions:
//! - Upstream declaration merging (`AppKeybindings` keys augmenting the tui
//!   `Keybindings` interface) is compile-time only; ids are plain strings.
//! - `defaultKeys: KeyId | KeyId[]` keeps the string-vs-array distinction
//!   where it is observable ([`UserKeys`], [`ResolvedKeys`]); the inner tui
//!   manager normalizes to key lists.
//! - `process.platform` maps to `std::env::consts::OS` with the node
//!   spellings (`win32`, `darwin`, …); `process.env` is read through a
//!   closure seam ([`use_windows_keybindings_for`]) so tests can inject the
//!   upstream suites' literal env objects.
//! - Upstream builds `KEYBINDINGS` once at module scope from the host
//!   platform; [`keybindings`] is the same pure function of
//!   (platform, windowsKeybindings), and host-platform helpers
//!   ([`use_windows_keybindings`]) feed it by default.
//! - `getAgentDir()` (upstream `../config.ts`, not yet ported) is the shared
//!   [`super::get_agent_dir`] seam.
//! - JS object key order is semantic for migration ordering and file
//!   rewrites, so configs are ordered `Vec`s of pairs (insertion order),
//!   with JS last-write-wins on duplicate keys.

use std::collections::HashMap;

use serde_json::Value;

use crate::coding_agent::utils::text::strip_bom;
use crate::tui::keybindings::{KeybindingDefinition, KeybindingsManager as TuiKeybindingsManager};

/// node's `process.platform` spelling for the host.
pub fn node_platform() -> &'static str {
    match std::env::consts::OS {
        "windows" => "win32",
        "macos" => "darwin",
        other => other,
    }
}

/// Reads a value from the process environment (node `process.env[name]`).
fn process_env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// Upstream `useWindowsKeybindings(platform = process.platform, env = process.env)`:
/// true on native Windows, or in WSL (without relying on Windows Terminal
/// detection). Empty env values are falsy, exactly like JS `Boolean("")`.
pub fn use_windows_keybindings() -> bool {
    use_windows_keybindings_for(node_platform(), &|name| process_env(name))
}

/// The explicit-argument form of [`use_windows_keybindings`] (the upstream
/// function's parameters). `platform` uses the node spelling (`win32`,
/// `linux`, `darwin`, …).
pub fn use_windows_keybindings_for(platform: &str, env: &dyn Fn(&str) -> Option<String>) -> bool {
    if platform == "win32" {
        return true;
    }
    if platform == "linux" {
        let wsl = env("WSL_DISTRO_NAME").is_some_and(|value| !value.is_empty())
            || env("WSL_INTEROP").is_some_and(|value| !value.is_empty());
        if wsl {
            return true;
        }
    }
    false
}

/// Convenience wrapper taking the upstream tests' `NodeJS.ProcessEnv`
/// literals.
pub fn use_windows_keybindings_with_env(platform: &str, env: &HashMap<String, String>) -> bool {
    use_windows_keybindings_for(platform, &|name| env.get(name).cloned())
}

fn definition(default_keys: Vec<String>, description: &'static str) -> KeybindingDefinition {
    KeybindingDefinition {
        default_keys,
        description: Some(description),
    }
}

fn keys(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

/// Upstream `KEYBINDINGS`: `TUI_KEYBINDINGS` spread first (the four adjusted
/// `tui.*` entries keep their original positions, like JS object spread),
/// then the `app.*` entries in declaration order.
pub fn keybindings() -> Vec<(&'static str, KeybindingDefinition)> {
    let platform = node_platform();
    let windows = use_windows_keybindings();
    keybindings_for(platform, windows)
}

/// The parameterized form of [`keybindings`] (upstream computes the table at
/// module scope from the process platform/env).
pub fn keybindings_for(
    platform: &str,
    windows_keybindings: bool,
) -> Vec<(&'static str, KeybindingDefinition)> {
    let windows = windows_keybindings;
    let mut entries: Vec<(&'static str, KeybindingDefinition)> =
        crate::tui::keybindings::tui_keybindings();
    // Overridden tui defaults; JS spread keeps each key's original position.
    let overrides: &[(&str, Vec<String>)] = &[
        // tui.editor.undo: win32 ? "ctrl+z" : windows ? "alt+z" : "ctrl+-"
        (
            "tui.editor.undo",
            if platform == "win32" {
                keys(&["ctrl+z"])
            } else if windows {
                keys(&["alt+z"])
            } else {
                keys(&["ctrl+-"])
            },
        ),
        // tui.altScreen.previousPrompt: windows ? "ctrl+up" : ["ctrl+shift+up", "ctrl+up"]
        (
            "tui.altScreen.previousPrompt",
            if windows {
                keys(&["ctrl+up"])
            } else {
                keys(&["ctrl+shift+up", "ctrl+up"])
            },
        ),
        // tui.altScreen.nextPrompt: windows ? "ctrl+down" : ["ctrl+shift+down", "ctrl+down"]
        (
            "tui.altScreen.nextPrompt",
            if windows {
                keys(&["ctrl+down"])
            } else {
                keys(&["ctrl+shift+down", "ctrl+down"])
            },
        ),
        // tui.altScreen.search: windows ? "ctrl+f" : "ctrl+shift+f"
        (
            "tui.altScreen.search",
            if windows {
                keys(&["ctrl+f"])
            } else {
                keys(&["ctrl+shift+f"])
            },
        ),
    ];
    for (id, default_keys) in overrides {
        if let Some(definition) = entries.iter_mut().find(|(entry_id, _)| entry_id == id) {
            definition.1.default_keys = default_keys.clone();
        }
    }

    let app_entries: Vec<(&'static str, KeybindingDefinition)> = vec![
        (
            "app.interrupt",
            definition(keys(&["escape"]), "Cancel or abort"),
        ),
        ("app.clear", definition(keys(&["ctrl+c"]), "Clear editor")),
        (
            "app.exit",
            definition(keys(&["ctrl+d"]), "Exit when editor is empty"),
        ),
        (
            "app.suspend",
            definition(
                if platform == "win32" {
                    Vec::new()
                } else {
                    keys(&["ctrl+z"])
                },
                "Suspend to background",
            ),
        ),
        (
            "app.thinking.cycle",
            definition(keys(&["shift+tab"]), "Cycle thinking level"),
        ),
        (
            "app.thinking.save",
            definition(keys(&["ctrl+s"]), "Save thinking level"),
        ),
        (
            "app.model.cycleForward",
            definition(keys(&["ctrl+p"]), "Cycle to next model"),
        ),
        (
            "app.model.cycleBackward",
            definition(
                if windows {
                    keys(&["alt+p"])
                } else {
                    keys(&["shift+ctrl+p"])
                },
                "Cycle to previous model",
            ),
        ),
        (
            "app.model.select",
            definition(keys(&["ctrl+l"]), "Open model selector"),
        ),
        (
            "app.tools.expand",
            definition(keys(&["ctrl+o"]), "Toggle tool output"),
        ),
        (
            "app.thinking.toggle",
            definition(keys(&["ctrl+t"]), "Toggle thinking blocks"),
        ),
        (
            "app.session.toggleNamedFilter",
            definition(keys(&["ctrl+n"]), "Toggle named session filter"),
        ),
        (
            "app.editor.external",
            definition(keys(&["ctrl+g"]), "Open external editor"),
        ),
        (
            "app.message.copy",
            definition(keys(&["ctrl+x"]), "Copy message to clipboard"),
        ),
        (
            "app.message.followUp",
            definition(
                if windows {
                    keys(&["ctrl+q"])
                } else {
                    keys(&["alt+enter"])
                },
                "Queue follow-up message",
            ),
        ),
        (
            "app.message.dequeue",
            definition(
                if windows {
                    keys(&["alt+q"])
                } else {
                    keys(&["alt+up"])
                },
                "Restore queued messages",
            ),
        ),
        (
            "app.clipboard.pasteImage",
            definition(
                if windows {
                    keys(&["alt+v"])
                } else {
                    keys(&["ctrl+v"])
                },
                "Paste image from clipboard (text fallback)",
            ),
        ),
        (
            "app.session.new",
            definition(Vec::new(), "Start a new session"),
        ),
        (
            "app.session.tree",
            definition(Vec::new(), "Open session tree"),
        ),
        (
            "app.session.fork",
            definition(Vec::new(), "Fork current session"),
        ),
        (
            "app.session.resume",
            definition(Vec::new(), "Resume a session"),
        ),
        (
            "app.tree.foldOrUp",
            definition(
                if platform == "darwin" {
                    keys(&["alt+left", "ctrl+left"])
                } else {
                    keys(&["ctrl+left", "alt+left"])
                },
                "Fold tree branch or move up",
            ),
        ),
        (
            "app.tree.unfoldOrDown",
            definition(
                if platform == "darwin" {
                    keys(&["alt+right", "ctrl+right"])
                } else {
                    keys(&["ctrl+right", "alt+right"])
                },
                "Unfold tree branch or move down",
            ),
        ),
        (
            "app.tree.editLabel",
            definition(keys(&["shift+l"]), "Edit tree label"),
        ),
        (
            "app.tree.toggleLabelTimestamp",
            definition(keys(&["shift+t"]), "Toggle tree label timestamps"),
        ),
        (
            "app.session.togglePath",
            definition(keys(&["ctrl+p"]), "Toggle session path display"),
        ),
        (
            "app.session.toggleSort",
            definition(keys(&["ctrl+s"]), "Toggle session sort mode"),
        ),
        (
            "app.session.rename",
            definition(keys(&["ctrl+r"]), "Rename session"),
        ),
        (
            "app.session.delete",
            definition(keys(&["ctrl+d"]), "Delete session"),
        ),
        (
            "app.session.deleteNoninvasive",
            definition(
                keys(&["ctrl+backspace"]),
                "Delete session when query is empty",
            ),
        ),
        (
            "app.models.save",
            definition(keys(&["ctrl+s"]), "Save model selection"),
        ),
        (
            "app.models.enableAll",
            definition(keys(&["ctrl+a"]), "Enable all models"),
        ),
        (
            "app.models.clearAll",
            definition(keys(&["ctrl+x"]), "Clear all models"),
        ),
        (
            "app.models.toggleProvider",
            definition(keys(&["ctrl+p"]), "Toggle all models for provider"),
        ),
        (
            "app.models.reorderUp",
            definition(keys(&["alt+up"]), "Move model up in order"),
        ),
        (
            "app.models.reorderDown",
            definition(keys(&["alt+down"]), "Move model down in order"),
        ),
        (
            "app.tree.filter.default",
            definition(keys(&["ctrl+d"]), "Tree filter: default view"),
        ),
        (
            "app.tree.filter.noTools",
            definition(keys(&["ctrl+t"]), "Tree filter: hide tool results"),
        ),
        (
            "app.tree.filter.userOnly",
            definition(keys(&["ctrl+u"]), "Tree filter: user messages only"),
        ),
        (
            "app.tree.filter.labeledOnly",
            definition(keys(&["ctrl+l"]), "Tree filter: labeled entries only"),
        ),
        (
            "app.tree.filter.all",
            definition(keys(&["ctrl+a"]), "Tree filter: show all entries"),
        ),
        (
            "app.tree.filter.cycleForward",
            definition(keys(&["ctrl+o"]), "Tree filter: cycle forward"),
        ),
        (
            "app.tree.filter.cycleBackward",
            definition(keys(&["shift+ctrl+o"]), "Tree filter: cycle backward"),
        ),
    ];
    entries.extend(app_entries);
    entries
}

/// Upstream `KEYBINDING_NAME_MIGRATIONS`, in declaration order.
pub const KEYBINDING_NAME_MIGRATIONS: &[(&str, &str)] = &[
    ("cursorUp", "tui.editor.cursorUp"),
    ("cursorDown", "tui.editor.cursorDown"),
    ("cursorLeft", "tui.editor.cursorLeft"),
    ("cursorRight", "tui.editor.cursorRight"),
    ("cursorWordLeft", "tui.editor.cursorWordLeft"),
    ("cursorWordRight", "tui.editor.cursorWordRight"),
    ("cursorLineStart", "tui.editor.cursorLineStart"),
    ("cursorLineEnd", "tui.editor.cursorLineEnd"),
    ("jumpForward", "tui.editor.jumpForward"),
    ("jumpBackward", "tui.editor.jumpBackward"),
    ("pageUp", "tui.editor.pageUp"),
    ("pageDown", "tui.editor.pageDown"),
    ("deleteCharBackward", "tui.editor.deleteCharBackward"),
    ("deleteCharForward", "tui.editor.deleteCharForward"),
    ("deleteWordBackward", "tui.editor.deleteWordBackward"),
    ("deleteWordForward", "tui.editor.deleteWordForward"),
    ("deleteToLineStart", "tui.editor.deleteToLineStart"),
    ("deleteToLineEnd", "tui.editor.deleteToLineEnd"),
    ("yank", "tui.editor.yank"),
    ("yankPop", "tui.editor.yankPop"),
    ("undo", "tui.editor.undo"),
    ("newLine", "tui.input.newLine"),
    ("submit", "tui.input.submit"),
    ("tab", "tui.input.tab"),
    ("copy", "tui.input.copy"),
    ("selectUp", "tui.select.up"),
    ("selectDown", "tui.select.down"),
    ("selectPageUp", "tui.select.pageUp"),
    ("selectPageDown", "tui.select.pageDown"),
    ("selectConfirm", "tui.select.confirm"),
    ("selectCancel", "tui.select.cancel"),
    ("interrupt", "app.interrupt"),
    ("clear", "app.clear"),
    ("exit", "app.exit"),
    ("suspend", "app.suspend"),
    ("cycleThinkingLevel", "app.thinking.cycle"),
    ("cycleModelForward", "app.model.cycleForward"),
    ("cycleModelBackward", "app.model.cycleBackward"),
    ("selectModel", "app.model.select"),
    ("expandTools", "app.tools.expand"),
    ("toggleThinking", "app.thinking.toggle"),
    ("toggleSessionNamedFilter", "app.session.toggleNamedFilter"),
    ("externalEditor", "app.editor.external"),
    ("followUp", "app.message.followUp"),
    ("dequeue", "app.message.dequeue"),
    ("pasteImage", "app.clipboard.pasteImage"),
    ("newSession", "app.session.new"),
    ("tree", "app.session.tree"),
    ("fork", "app.session.fork"),
    ("resume", "app.session.resume"),
    ("treeFoldOrUp", "app.tree.foldOrUp"),
    ("treeUnfoldOrDown", "app.tree.unfoldOrDown"),
    ("treeEditLabel", "app.tree.editLabel"),
    ("treeToggleLabelTimestamp", "app.tree.toggleLabelTimestamp"),
    ("toggleSessionPath", "app.session.togglePath"),
    ("toggleSessionSort", "app.session.toggleSort"),
    ("renameSession", "app.session.rename"),
    ("deleteSession", "app.session.delete"),
    ("deleteSessionNoninvasive", "app.session.deleteNoninvasive"),
];

fn is_legacy_keybinding_name(key: &str) -> Option<&'static str> {
    KEYBINDING_NAME_MIGRATIONS
        .iter()
        .find(|(legacy, _)| *legacy == key)
        .map(|(_, namespaced)| *namespaced)
}

/// JS `Array.prototype.sort()` default comparison (UTF-16 code units).
fn js_string_sort(strings: &mut [String]) {
    strings.sort_by(|a, b| js_utf16_compare(a, b));
}

fn js_utf16_compare(a: &str, b: &str) -> std::cmp::Ordering {
    let a_units: Vec<u16> = a.encode_utf16().collect();
    let b_units: Vec<u16> = b.encode_utf16().collect();
    for (a_unit, b_unit) in a_units.iter().zip(b_units.iter()) {
        match a_unit.cmp(b_unit) {
            std::cmp::Ordering::Equal => continue,
            ordering => return ordering,
        }
    }
    a_units.len().cmp(&b_units.len())
}

/// JS `{}`-as-map: insertion-ordered pairs where assigning an existing key
/// keeps its position but overwrites the value.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OrderedConfig(Vec<(String, Value)>);

impl OrderedConfig {
    pub fn new(entries: Vec<(String, Value)>) -> Self {
        let mut config = Self::default();
        for (key, value) in entries {
            config.set(key, value);
        }
        config
    }

    /// JS `config[key] = value` semantics.
    pub fn set(&mut self, key: String, value: Value) {
        match self.0.iter_mut().find(|(existing, _)| *existing == key) {
            Some(entry) => entry.1 = value,
            None => self.0.push((key, value)),
        }
    }

    /// JS `Object.hasOwn(config, key)`.
    pub fn has_own(&self, key: &str) -> bool {
        self.0.iter().any(|(existing, _)| existing == key)
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0
            .iter()
            .find(|(existing, _)| existing == key)
            .map(|(_, value)| value)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn entries(&self) -> &[(String, Value)] {
        &self.0
    }

    pub fn into_entries(self) -> Vec<(String, Value)> {
        self.0
    }

    /// JS `Object.keys(config)` order.
    pub fn keys(&self) -> Vec<&str> {
        self.0.iter().map(|(key, _)| key.as_str()).collect()
    }
}

/// Upstream `migrateKeybindingsConfig(rawConfig)`: rewrite legacy names to
/// namespaced ids, drop shadowed entries (the namespaced value wins), and
/// order the result like the upstream `JSON.stringify` rewrite does.
/// Returns `(config, migrated)`.
pub fn migrate_keybindings_config(raw_config: &[(String, Value)]) -> (OrderedConfig, bool) {
    let mut config = OrderedConfig::default();
    let mut migrated = false;

    for (key, value) in raw_config {
        let next_key = is_legacy_keybinding_name(key)
            .map(str::to_string)
            .unwrap_or_else(|| key.clone());
        if next_key != *key {
            migrated = true;
        }
        if next_key != *key && raw_config.iter().any(|(existing, _)| existing == &next_key) {
            migrated = true;
            continue;
        }
        config.set(next_key, value.clone());
    }

    (order_keybindings_config(&config), migrated)
}

/// Upstream `orderKeybindingsConfig`: known keybindings in `KEYBINDINGS`
/// order first, then unknown extras sorted (JS default string sort).
pub fn order_keybindings_config(config: &OrderedConfig) -> OrderedConfig {
    let mut ordered = OrderedConfig::default();
    for (id, _) in keybindings() {
        if config.has_own(id) {
            ordered.set(id.to_string(), config.get(id).unwrap().clone());
        }
    }

    let mut extras: Vec<String> = config
        .keys()
        .into_iter()
        .filter(|key| !ordered.has_own(key))
        .map(str::to_string)
        .collect();
    js_string_sort(&mut extras);
    for key in extras {
        let value = config.get(&key).unwrap().clone();
        ordered.set(key, value);
    }

    ordered
}

/// Upstream `KeybindingsConfig` values: `KeyId | KeyId[]`.
#[derive(Debug, Clone, PartialEq)]
pub enum UserKeys {
    /// A single key id (upstream `KeyId`).
    One(String),
    /// A list of key ids (upstream `KeyId[]`).
    Many(Vec<String>),
}

impl UserKeys {
    /// Normalized key list (upstream `normalizeKeys`).
    pub fn to_key_list(&self) -> Vec<String> {
        match self {
            Self::One(key) => vec![key.clone()],
            Self::Many(list) => list.clone(),
        }
    }
}

/// Upstream `KeybindingsConfig` (`Record<string, KeyId | KeyId[]>`), in
/// insertion order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct KeybindingsConfig(Vec<(String, UserKeys)>);

impl KeybindingsConfig {
    pub fn new(entries: Vec<(String, UserKeys)>) -> Self {
        Self(entries)
    }

    pub fn entries(&self) -> &[(String, UserKeys)] {
        &self.0
    }

    pub fn get(&self, key: &str) -> Option<&UserKeys> {
        self.0
            .iter()
            .find(|(existing, _)| existing == key)
            .map(|(_, keys)| keys)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Upstream `toKeybindingsConfig`: keep string values and all-string arrays,
/// drop everything else.
pub fn to_keybindings_config(value: &[(String, Value)]) -> KeybindingsConfig {
    let mut config = Vec::new();
    for (key, binding) in value {
        if let Some(key_id) = binding.as_str() {
            config.push((key.clone(), UserKeys::One(key_id.to_string())));
            continue;
        }
        if let Some(list) = binding.as_array() {
            if list.iter().all(|entry| entry.is_string()) {
                config.push((
                    key.clone(),
                    UserKeys::Many(
                        list.iter()
                            .map(|entry| entry.as_str().unwrap().to_string())
                            .collect(),
                    ),
                ));
            }
        }
    }
    KeybindingsConfig(config)
}

/// Upstream `loadRawConfig`: `undefined` when missing/malformed; arrays are
/// objects upstream, so they surface as their `Object.entries` form.
fn load_raw_config(path: &str) -> Option<OrderedConfig> {
    let Ok(metadata) = std::fs::metadata(path) else {
        return None;
    };
    if !metadata.is_file() {
        // upstream relies on readFileSync failing for directories inside the
        // try/catch; the JSON.parse of garbage fails the same way.
        return None;
    }
    let Ok(raw) = std::fs::read_to_string(path) else {
        return None;
    };
    let Ok(parsed) = serde_json::from_str::<Value>(strip_bom(&raw)) else {
        return None;
    };
    if parsed.is_null() {
        return None;
    }
    let entries: Vec<(String, Value)> = match parsed {
        Value::Object(map) => map.into_iter().collect(),
        // typeof [] === "object": JS reads it as Record with index keys.
        Value::Array(items) => items
            .into_iter()
            .enumerate()
            .map(|(index, value)| (index.to_string(), value))
            .collect(),
        _ => return None,
    };
    Some(OrderedConfig::new(entries))
}

/// Upstream `ResolvedKeys` (`KeyId | KeyId[]` as returned by
/// `getResolvedBindings`): a single key renders as a bare string upstream.
#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedKeys {
    One(String),
    Many(Vec<String>),
}

/// Upstream `KeybindingsManager extends TuiKeybindingsManager`: the app
/// defaults plus a `keybindings.json` path for [`Self::reload`].
pub struct KeybindingsManager {
    inner: TuiKeybindingsManager,
    config_path: Option<String>,
    user_bindings: KeybindingsConfig,
}

impl KeybindingsManager {
    /// Upstream `new KeybindingsManager(userBindings = {}, configPath?)`.
    pub fn new(user_bindings: KeybindingsConfig, config_path: Option<String>) -> Self {
        let definitions = keybindings();
        let user_pairs: Vec<(&str, Vec<String>)> = user_bindings
            .entries()
            .iter()
            .map(|(id, keys)| (id.as_str(), keys.to_key_list()))
            .collect();
        Self {
            inner: TuiKeybindingsManager::new(&definitions, &user_pairs),
            config_path,
            user_bindings,
        }
    }

    /// Upstream `KeybindingsManager.create(agentDir = getAgentDir())`.
    pub fn create(agent_dir: &str) -> Self {
        let config_path = super::path_join(agent_dir, "keybindings.json");
        let user_bindings = Self::load_from_file(&config_path);
        Self::new(user_bindings, Some(config_path))
    }

    /// Upstream `KeybindingsManager.create()` with the host agent dir.
    pub fn create_default() -> Self {
        Self::create(&super::get_agent_dir())
    }

    fn load_from_file(path: &str) -> KeybindingsConfig {
        let Some(raw_config) = load_raw_config(path) else {
            return KeybindingsConfig::default();
        };
        let (config, _) = migrate_keybindings_config(raw_config.entries());
        to_keybindings_config(config.entries())
    }

    /// Upstream `reload()`.
    pub fn reload(&mut self) {
        let Some(config_path) = &self.config_path else {
            return;
        };
        self.set_user_bindings(Self::load_from_file(config_path));
    }

    /// Upstream `setUserBindings` (used by [`Self::reload`]).
    pub fn set_user_bindings(&mut self, user_bindings: KeybindingsConfig) {
        let pairs: Vec<(&str, Vec<String>)> = user_bindings
            .entries()
            .iter()
            .map(|(id, keys)| (id.as_str(), keys.to_key_list()))
            .collect();
        self.inner = TuiKeybindingsManager::new(&keybindings(), &pairs);
        self.user_bindings = user_bindings;
    }

    /// Upstream `getUserBindings()`: the raw user config.
    pub fn get_user_bindings(&self) -> KeybindingsConfig {
        self.user_bindings.clone()
    }

    /// Upstream `getEffectiveConfig()` → `getResolvedBindings()`.
    pub fn get_effective_config(&self) -> Vec<(String, ResolvedKeys)> {
        self.inner
            .get_resolved_bindings()
            .into_iter()
            .map(|(id, keys)| {
                let resolved = if keys.len() == 1 {
                    ResolvedKeys::One(keys.into_iter().next().unwrap())
                } else {
                    ResolvedKeys::Many(keys)
                };
                (id, resolved)
            })
            .collect()
    }

    /// Upstream `matches` (delegated to the tui manager).
    pub fn matches(&self, data: &str, keybinding: &str) -> bool {
        self.inner.matches(data, keybinding)
    }

    /// Upstream `getKeys` (delegated to the tui manager).
    pub fn get_keys(&self, keybinding: &str) -> Vec<String> {
        self.inner.get_keys(keybinding)
    }
}

#[cfg(test)]
#[path = "keybindings_tests.rs"]
mod tests;
