//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Port of `modes/interactive/components/keybinding-hints.ts` (48 lines, sha256
//! `76b13ee8bfc6e49d5b2b5496eac13523bd9aad95770cbfa4a239336131f94aae`).
//!
//! The helpers landed with the selector slice
//! ([`super::model_selector`]) because that slice predates the r19
//! `components/mod.rs` unfreeze (`formatKeyText`/`keyText`/`keyDisplayText`/
//! `keyHint`/`rawKeyHint` → [`format_key_text`]/[`key_text`]/[`key_display_text`]/[`key_hint`]/[`raw_key_hint`]);
//! this module documents the coverage and re-exports the surface under the
//! upstream file's name. The darwin `alt → option` rewrite is
//! `process.platform`-conditional upstream and platform-conditional via
//! [`crate::coding_agent::core::keybindings::node_platform`] here.

pub(crate) use super::model_selector::format_key_text as format_key_text_impl;

/// Upstream `formatKeyText(key, options)` (no options → uncapitalized).
pub fn format_key_text(key: &str, capitalize: bool) -> String {
    format_key_text_impl(key, capitalize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding_agent::modes::interactive::components::model_selector::{
        key_display_text, key_text,
    };

    /// Oracle: tests/fixtures/interactive_r19_oracle scenario `keybinding_hints`
    /// (win32 host → no darwin rewrite; merged registry keys).
    #[test]
    fn keybinding_hints_match_oracle() {
        assert_eq!(format_key_text("ctrl+o", false), "ctrl+o");
        assert_eq!(format_key_text("ctrl+o", true), "Ctrl+O");
        assert_eq!(
            format_key_text("alt+enter/ctrl+q", false),
            "alt+enter/ctrl+q"
        );
        assert_eq!(format_key_text("alt+enter", true), "Alt+Enter");
        assert_eq!(format_key_text("", false), "");
        assert_eq!(format_key_text("a", false), "a");
        assert_eq!(key_text("tui.select.cancel"), "escape/ctrl+c");
        assert_eq!(key_display_text("tui.select.confirm"), "Enter");
        assert_eq!(key_text("app.tools.expand"), "ctrl+o");
        assert_eq!(key_text("app.interrupt"), "escape");
        assert_eq!(key_display_text("app.thinking.save"), "Ctrl+S");
        assert_eq!(key_display_text("app.thinking.cycle"), "Shift+Tab");
    }
}
