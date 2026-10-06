//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Port of `modes/interactive/components/dynamic-border.ts` (25 lines, sha256
//! `1f7fa398a861906e3dbbd217434f290f09d23c5e561ab7d6ef24808db4353cb0`).
//!
//! `DynamicBorder` landed with the selector slice ([`super::model_selector`],
//! same frozen-mod reason as `keybinding-hints`); this module documents the
//! coverage and mirrors its constructors. The optional color function defaults
//! to `theme.fg("border", …)` — the explicit-color constructor seam mirrors
//! upstream's jiti note.

use crate::coding_agent::modes::interactive::components::support::Border;
use crate::coding_agent::modes::interactive::theme::Theme;
use std::sync::Arc;

/// Upstream `new DynamicBorder()` (default `theme.fg("border", …)` painter).
// Constructor seams for the shell's border rows; wired into the interactive
// shell in r19+.
#[allow(dead_code)]
pub(crate) fn new_border() -> Border {
    Border::new(None)
}

/// Upstream `new DynamicBorder(colorFn)`.
#[allow(dead_code)]
pub(crate) fn new_border_with_color(color: Box<dyn Fn(&str) -> String + Send>) -> Border {
    Border::new(Some(color))
}

/// The default painter (`(str) => theme.fg("border", str)`).
pub fn border_painter(theme: &Arc<Theme>) -> impl Fn(&str) -> String + Send + Sync {
    let theme = Arc::clone(theme);
    move |text: &str| theme.fg("border", text).expect("border color")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding_agent::modes::interactive::theme::{load_builtin_theme, ColorMode};

    /// Oracle: tests/fixtures/interactive_r19_oracle scenario `keybinding_hints`
    /// (`border` rows) — a full-width `─` rule in the border color.
    #[test]
    fn dynamic_border_matches_oracle() {
        let theme = Arc::new(load_builtin_theme("dark", Some(ColorMode::Truecolor)).expect("dark"));
        // the default painter reads the process-global theme seam
        super::super::model_selector::set_default_theme(Arc::clone(&theme));
        let default_border = new_border();
        let lines = default_border.0.render(10);
        assert_eq!(
            lines,
            vec!["\x1b[38;2;95;168;204m──────────\x1b[39m".to_string()]
        );
        let accent_border = new_border_with_color(Box::new({
            let theme = Arc::clone(&theme);
            move |s: &str| theme.fg("accent", s).expect("accent")
        }));
        let lines = accent_border.0.render(4);
        assert_eq!(
            lines,
            vec!["\x1b[38;2;167;152;215m────\x1b[39m".to_string()]
        );
    }
}
