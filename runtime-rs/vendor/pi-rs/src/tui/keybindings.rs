//! Port of upstream `packages/tui/src/keybindings.ts`: the global keybinding
//! registry with user overrides and conflict detection.
//!
//! Disclosed substitutions: upstream's declaration-merging `Keybindings`
//! interface is a TypeScript typing feature; the binding identifiers are plain
//! strings here. `Object.entries` iteration order (insertion order) is
//! reproduced with ordered Vecs.

use std::sync::OnceLock;

use crate::tui::keys::matches_key;

/// Upstream `KeybindingDefinition`.
#[derive(Clone, Debug)]
pub struct KeybindingDefinition {
    pub default_keys: Vec<String>,
    pub description: Option<&'static str>,
}

/// Upstream `TUI_KEYBINDINGS`, in the same insertion order.
pub fn tui_keybindings() -> Vec<(&'static str, KeybindingDefinition)> {
    vec![
        ("tui.editor.cursorUp", def(&["up"], "Move cursor up")),
        ("tui.editor.cursorDown", def(&["down"], "Move cursor down")),
        (
            "tui.editor.historyPrevious",
            def(&[], "Select previous prompt history entry"),
        ),
        (
            "tui.editor.historyNext",
            def(&[], "Select next prompt history entry"),
        ),
        (
            "tui.editor.cursorLeft",
            def(&["left", "ctrl+b"], "Move cursor left"),
        ),
        (
            "tui.editor.cursorRight",
            def(&["right", "ctrl+f"], "Move cursor right"),
        ),
        (
            "tui.editor.cursorWordLeft",
            def(&["alt+left", "ctrl+left", "alt+b"], "Move cursor word left"),
        ),
        (
            "tui.editor.cursorWordRight",
            def(
                &["alt+right", "ctrl+right", "alt+f"],
                "Move cursor word right",
            ),
        ),
        (
            "tui.editor.cursorLineStart",
            def(&["home", "ctrl+home", "ctrl+a"], "Move to line start"),
        ),
        (
            "tui.editor.cursorLineEnd",
            def(&["end", "ctrl+end", "ctrl+e"], "Move to line end"),
        ),
        (
            "tui.editor.jumpForward",
            def(&["ctrl+]"], "Jump forward to character"),
        ),
        (
            "tui.editor.jumpBackward",
            def(&["ctrl+alt+]"], "Jump backward to character"),
        ),
        (
            "tui.editor.pageUp",
            def(&["pageUp", "ctrl+pageUp"], "Page up"),
        ),
        (
            "tui.editor.pageDown",
            def(&["pageDown", "ctrl+pageDown"], "Page down"),
        ),
        (
            "tui.editor.deleteCharBackward",
            def(&["backspace"], "Delete character backward"),
        ),
        (
            "tui.editor.deleteCharForward",
            def(&["delete", "ctrl+d"], "Delete character forward"),
        ),
        (
            "tui.editor.deleteWordBackward",
            def(&["ctrl+w", "alt+backspace"], "Delete word backward"),
        ),
        (
            "tui.editor.deleteWordForward",
            def(&["alt+d", "alt+delete"], "Delete word forward"),
        ),
        (
            "tui.editor.deleteToLineStart",
            def(&["ctrl+u"], "Delete to line start"),
        ),
        (
            "tui.editor.deleteToLineEnd",
            def(&["ctrl+k"], "Delete to line end"),
        ),
        ("tui.editor.yank", def(&["ctrl+y"], "Yank")),
        ("tui.editor.yankPop", def(&["alt+y"], "Yank pop")),
        ("tui.editor.undo", def(&["ctrl+-"], "Undo")),
        (
            "tui.input.newLine",
            def(&["shift+enter", "ctrl+j"], "Insert newline"),
        ),
        ("tui.input.submit", def(&["enter"], "Submit input")),
        ("tui.input.tab", def(&["tab"], "Tab / autocomplete")),
        ("tui.input.copy", def(&["ctrl+c"], "Copy selection")),
        ("tui.select.up", def(&["up"], "Move selection up")),
        ("tui.select.down", def(&["down"], "Move selection down")),
        ("tui.select.pageUp", def(&["pageUp"], "Selection page up")),
        (
            "tui.select.pageDown",
            def(&["pageDown"], "Selection page down"),
        ),
        ("tui.select.confirm", def(&["enter"], "Confirm selection")),
        (
            "tui.select.cancel",
            def(&["escape", "ctrl+c"], "Cancel selection"),
        ),
        // These intentionally shadow the unmodified editor bindings in fullscreen mode.
        (
            "tui.altScreen.pageUp",
            def(&["pageUp"], "Scroll viewport up one page"),
        ),
        (
            "tui.altScreen.pageDown",
            def(&["pageDown"], "Scroll viewport down one page"),
        ),
        (
            "tui.altScreen.halfPageUp",
            def(&[], "Scroll viewport up half a page"),
        ),
        (
            "tui.altScreen.halfPageDown",
            def(&[], "Scroll viewport down half a page"),
        ),
        (
            "tui.altScreen.lineUp",
            def(&[], "Scroll viewport up one line"),
        ),
        (
            "tui.altScreen.lineDown",
            def(&[], "Scroll viewport down one line"),
        ),
        (
            "tui.altScreen.previousPrompt",
            def(
                &["ctrl+shift+up", "ctrl+up"],
                "Jump to previous semantic prompt",
            ),
        ),
        (
            "tui.altScreen.nextPrompt",
            def(
                &["ctrl+shift+down", "ctrl+down"],
                "Jump to next semantic prompt",
            ),
        ),
        (
            "tui.altScreen.search",
            def(&["ctrl+shift+f"], "Search the primary scroll view"),
        ),
        (
            "tui.altScreen.searchNext",
            def(&["enter", "ctrl+g"], "Select the next search match"),
        ),
        (
            "tui.altScreen.searchPrevious",
            def(
                &["shift+enter", "ctrl+shift+g"],
                "Select the previous search match",
            ),
        ),
        (
            "tui.altScreen.searchClose",
            def(&["escape"], "Close transcript search"),
        ),
        (
            "tui.altScreen.top",
            def(&["home"], "Scroll viewport to top"),
        ),
        (
            "tui.altScreen.bottom",
            def(&["end"], "Scroll viewport to bottom"),
        ),
    ]
}

fn def(default_keys: &[&str], description: &'static str) -> KeybindingDefinition {
    KeybindingDefinition {
        default_keys: default_keys.iter().map(|key| key.to_string()).collect(),
        description: Some(description),
    }
}

/// Upstream `KeybindingConflict`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeybindingConflict {
    pub key: String,
    pub keybindings: Vec<String>,
}

/// Upstream `KeybindingsManager`.
pub struct KeybindingsManager {
    definitions: Vec<(String, KeybindingDefinition)>,
    user_bindings: Vec<(String, Vec<String>)>,
    keys_by_id: Vec<(String, Vec<String>)>,
    conflicts: Vec<KeybindingConflict>,
}

fn normalize_keys(keys: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut result = Vec::new();
    for key in keys {
        if !seen.contains(&key) {
            seen.push(key.clone());
            result.push(key);
        }
    }
    result
}

impl KeybindingsManager {
    pub fn new(
        definitions: &[(&str, KeybindingDefinition)],
        user_bindings: &[(&str, Vec<String>)],
    ) -> Self {
        let definitions: Vec<(String, KeybindingDefinition)> = definitions
            .iter()
            .map(|(id, definition)| (id.to_string(), definition.clone()))
            .collect();
        let user_bindings: Vec<(String, Vec<String>)> = user_bindings
            .iter()
            .map(|(id, keys)| (id.to_string(), keys.clone()))
            .collect();
        let mut manager = Self {
            definitions,
            user_bindings,
            keys_by_id: Vec::new(),
            conflicts: Vec::new(),
        };
        manager.rebuild();
        manager
    }

    fn rebuild(&mut self) {
        self.keys_by_id.clear();
        self.conflicts.clear();

        // Ordered claim map: (key id, claimant keybinding ids).
        let mut user_claims: Vec<(String, Vec<String>)> = Vec::new();
        for (keybinding, keys) in &self.user_bindings {
            if !self.definitions.iter().any(|(id, _)| id == keybinding) {
                continue;
            }
            for key in normalize_keys(keys.clone()) {
                if let Some(claimants) = user_claims.iter_mut().find(|(claimed, _)| *claimed == key)
                {
                    if !claimants.1.contains(keybinding) {
                        claimants.1.push(keybinding.clone());
                    }
                } else {
                    user_claims.push((key, vec![keybinding.clone()]));
                }
            }
        }

        for (key, keybindings) in &user_claims {
            if keybindings.len() > 1 {
                self.conflicts.push(KeybindingConflict {
                    key: key.clone(),
                    keybindings: keybindings.clone(),
                });
            }
        }

        for (id, definition) in &self.definitions {
            let user_keys = self
                .user_bindings
                .iter()
                .find(|(user_id, _)| user_id == id)
                .map(|(_, keys)| keys.clone());
            let keys = match user_keys {
                Some(keys) => normalize_keys(keys),
                None => normalize_keys(definition.default_keys.clone()),
            };
            self.keys_by_id.push((id.clone(), keys));
        }
    }

    pub fn matches(&self, data: &str, keybinding: &str) -> bool {
        self.keys_by_id
            .iter()
            .find(|(id, _)| id == keybinding)
            .map(|(_, keys)| keys)
            .unwrap_or(&Vec::new())
            .iter()
            .any(|key| matches_key(data, key))
    }

    pub fn get_keys(&self, keybinding: &str) -> Vec<String> {
        self.keys_by_id
            .iter()
            .find(|(id, _)| id == keybinding)
            .map(|(_, keys)| keys.clone())
            .unwrap_or_default()
    }

    pub fn get_definition(&self, keybinding: &str) -> Option<&KeybindingDefinition> {
        self.definitions
            .iter()
            .find(|(id, _)| id == keybinding)
            .map(|(_, definition)| definition)
    }

    pub fn get_conflicts(&self) -> Vec<KeybindingConflict> {
        self.conflicts.clone()
    }

    pub fn set_user_bindings(&mut self, user_bindings: &[(&str, Vec<String>)]) {
        self.user_bindings = user_bindings
            .iter()
            .map(|(id, keys)| (id.to_string(), keys.clone()))
            .collect();
        self.rebuild();
    }

    pub fn get_user_bindings(&self) -> Vec<(String, Vec<String>)> {
        self.user_bindings.clone()
    }

    pub fn get_resolved_bindings(&self) -> Vec<(String, Vec<String>)> {
        self.definitions
            .iter()
            .map(|(id, _)| {
                let keys = self
                    .keys_by_id
                    .iter()
                    .find(|(resolved_id, _)| resolved_id == id)
                    .map(|(_, keys)| keys.clone())
                    .unwrap_or_default();
                (id.clone(), keys)
            })
            .collect()
    }
}

static GLOBAL_KEYBINDINGS: OnceLock<std::sync::RwLock<Option<KeybindingsManager>>> =
    OnceLock::new();

/// Upstream `setKeybindings`: replace the process-global manager.
pub fn set_keybindings(keybindings: KeybindingsManager) {
    let lock = GLOBAL_KEYBINDINGS.get_or_init(|| std::sync::RwLock::new(None));
    if let Ok(mut guard) = lock.write() {
        *guard = Some(keybindings);
    }
}

fn default_manager() -> KeybindingsManager {
    KeybindingsManager::new(&tui_keybindings(), &[])
}

/// Upstream `getKeybindings`: run `f` with the process-global manager,
/// defaulting to `TUI_KEYBINDINGS` on first use.
pub fn with_keybindings<R>(f: impl FnOnce(&KeybindingsManager) -> R) -> R {
    let lock = GLOBAL_KEYBINDINGS.get_or_init(|| std::sync::RwLock::new(Some(default_manager())));
    let guard = match lock.read() {
        Ok(guard) if guard.is_some() => guard,
        guard => {
            drop(guard);
            if let Ok(mut guard) = lock.write() {
                if guard.is_none() {
                    *guard = Some(default_manager());
                }
            }
            lock.read().unwrap_or_else(|poisoned| poisoned.into_inner())
        }
    };
    f(guard.as_ref().expect("populated above"))
}
