//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Port of `modes/interactive/tui-renderer.ts` (79 lines, sha256
//! `98e233fa33fd3ca08a2307db860e7594ef8515da328e92b816ce15020a7dd6cb`).
//!
//! Ported surface: [`InteractiveTuiOptions`] and the deterministic
//! search/scroll stylers the fullscreen composition root installs
//! ([`search_match_style`], [`search_current_match_style`],
//! [`search_navigation_button_style`], [`scroll_to_end_indicator`]), plus the
//! options-resolution face of [`create_interactive_tui`].
//!
//! # Seams
//!
//! - **D8 — screen-class construction** (`tui-renderer.ts:22-47`): upstream
//!   builds `TuiAltScreen`/`TuiMainScreen` over a `ProcessTerminal` and wires
//!   `openBrowser`, `copyToClipboard` and the right-click-paste hooks. The
//!   vendored tui slice has neither screen class nor clipboard/browser
//!   transports yet, so [`create_interactive_tui`] resolves the options into a
//!   mode-specific [`InteractiveTuiPlan`] (carrying the prebuilt styles) and
//!   the terminal/transport choreography stays unported.
//! - **D9 — `createInteractiveTuiReference` Proxy** (`tui-renderer.ts:51-79`):
//!   the JS Proxy forwarding every property to the *current* TUI instance is
//!   metaprogramming with no Rust equivalent; the ported interactive shell
//!   threads the live TUI explicitly (r18 seam S5), so the indirection is not
//!   needed here.

use std::sync::Arc;

use crate::coding_agent::modes::interactive::theme::Theme;

/// Upstream `InteractiveTuiOptions`.
#[derive(Clone, Debug, Default)]
pub struct InteractiveTuiOptions {
    pub tui_mode: TuiMode,
    pub show_hardware_cursor: bool,
    pub log_directory: String,
    pub on_right_click_paste: bool,
    pub fullscreen_copy_on_select: bool,
}

/// Upstream `tuiMode`: `"regular" | "fullscreen"`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TuiMode {
    #[default]
    Regular,
    Fullscreen,
}

/// Upstream `styleSearchMatch` (`tui-renderer.ts:24`) — the un-underlined base.
pub fn style_search_match(theme: &Theme, text: &str) -> String {
    theme
        .bg(
            "searchMatchBg",
            &theme.fg("searchMatchText", text).expect("searchMatchText"),
        )
        .expect("searchMatchBg")
}

/// Upstream `searchMatchStyle` option (`tui-renderer.ts:26`).
pub fn search_match_style(theme: &Theme, text: &str) -> String {
    theme.underline(&style_search_match(theme, text))
}

/// Upstream `searchCurrentMatchStyle` option (`tui-renderer.ts:27`) — bold +
/// inverse over the *un-underlined* match style.
pub fn search_current_match_style(theme: &Theme, text: &str) -> String {
    theme.bold(&theme.inverse(&style_search_match(theme, text)))
}

/// Upstream `searchNavigationButtonStyle` option (`tui-renderer.ts:28`).
pub fn search_navigation_button_style(theme: &Theme, text: &str, hovered: bool) -> String {
    if hovered {
        theme.underline(text)
    } else {
        text.to_string()
    }
}

/// Upstream `scrollToEndIndicator` option (`tui-renderer.ts:29-33`).
pub fn scroll_to_end_indicator(theme: &Theme, shortcut: Option<&str>) -> String {
    let label = match shortcut {
        Some(shortcut) if !shortcut.is_empty() => {
            format!(" ↓ Jump to latest message · {shortcut} ")
        }
        _ => " ↓ Jump to latest message".to_string() + " ",
    };
    theme
        .bg("selectedBg", &theme.fg("text", &label).expect("text"))
        .expect("selectedBg")
}

/// The mode-resolved construction plan (upstream returns the constructed
/// screen; see seam D8). `Fullscreen` carries the prebuilt style closures.
pub enum InteractiveTuiPlan {
    /// Upstream `new TuiAltScreen(terminal, …)` call shape.
    Fullscreen {
        options: InteractiveTuiOptions,
        search_match_style: SearchStyle,
        search_current_match_style: SearchStyle,
        search_navigation_button_style: NavigationStyle,
    },
    /// Upstream `new TuiMainScreen(terminal, …)` call shape.
    Regular { options: InteractiveTuiOptions },
}

pub type SearchStyle = Arc<dyn Fn(&str) -> String + Send + Sync>;
pub type NavigationStyle = Arc<dyn Fn(&str, bool) -> String + Send + Sync>;

/// Upstream `createInteractiveTui` (options-resolution face; the terminal and
/// screen-class construction are seam D8).
pub fn create_interactive_tui(
    theme: Arc<Theme>,
    options: InteractiveTuiOptions,
) -> InteractiveTuiPlan {
    if options.tui_mode == TuiMode::Fullscreen {
        let theme_for_match = Arc::clone(&theme);
        let theme_for_current = Arc::clone(&theme);
        let theme_for_button = Arc::clone(&theme);
        InteractiveTuiPlan::Fullscreen {
            search_match_style: Arc::new(move |text| search_match_style(&theme_for_match, text)),
            search_current_match_style: Arc::new(move |text| {
                search_current_match_style(&theme_for_current, text)
            }),
            search_navigation_button_style: Arc::new(move |text, hovered| {
                search_navigation_button_style(&theme_for_button, text, hovered)
            }),
            options,
        }
    } else {
        InteractiveTuiPlan::Regular { options }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding_agent::modes::interactive::theme::{load_builtin_theme, ColorMode};
    use std::sync::Arc;

    fn dark() -> Arc<Theme> {
        Arc::new(load_builtin_theme("dark", Some(ColorMode::Truecolor)).expect("dark theme"))
    }

    /// Oracle: tests/fixtures/interactive_r19_oracle/tui_renderer_oracle.json —
    /// byte-identical stylers over the r17-verified truecolor dark theme.
    #[test]
    fn stylers_match_oracle_bytes() {
        let theme = dark();
        let probes = ["match", "", "multi word match", "↑"];
        let expected_match = [
            "\x1b[4m\x1b[48;2;78;47;27m\x1b[38;2;157;165;169mmatch\x1b[39m\x1b[49m\x1b[24m",
            "\x1b[4m\x1b[48;2;78;47;27m\x1b[38;2;157;165;169m\x1b[39m\x1b[49m\x1b[24m",
            "\x1b[4m\x1b[48;2;78;47;27m\x1b[38;2;157;165;169mmulti word match\x1b[39m\x1b[49m\x1b[24m",
            "\x1b[4m\x1b[48;2;78;47;27m\x1b[38;2;157;165;169m↑\x1b[39m\x1b[49m\x1b[24m",
        ];
        for (probe, expected) in probes.iter().zip(expected_match.iter()) {
            assert_eq!(&search_match_style(&theme, probe), expected);
        }
        let expected_current = [
            "\x1b[1m\x1b[7m\x1b[48;2;78;47;27m\x1b[38;2;157;165;169mmatch\x1b[39m\x1b[49m\x1b[27m\x1b[22m",
            "\x1b[1m\x1b[7m\x1b[48;2;78;47;27m\x1b[38;2;157;165;169m\x1b[39m\x1b[49m\x1b[27m\x1b[22m",
            "\x1b[1m\x1b[7m\x1b[48;2;78;47;27m\x1b[38;2;157;165;169mmulti word match\x1b[39m\x1b[49m\x1b[27m\x1b[22m",
            "\x1b[1m\x1b[7m\x1b[48;2;78;47;27m\x1b[38;2;157;165;169m↑\x1b[39m\x1b[49m\x1b[27m\x1b[22m",
        ];
        for (probe, expected) in probes.iter().zip(expected_current.iter()) {
            assert_eq!(&search_current_match_style(&theme, probe), expected);
        }
        assert_eq!(search_navigation_button_style(&theme, "btn", false), "btn");
        assert_eq!(
            search_navigation_button_style(&theme, "btn", true),
            "\x1b[4mbtn\x1b[24m"
        );
    }

    /// Oracle `scrollToEndIndicator`: `keyDisplayText("tui.altScreen.bottom")`
    /// resolves through the merged keybindings registry ("End").
    #[test]
    fn scroll_to_end_indicator_matches_oracle_bytes() {
        let theme = dark();
        let shortcut =
            crate::coding_agent::modes::interactive::components::model_selector::key_display_text(
                "tui.altScreen.bottom",
            );
        let expected = "\x1b[48;2;33;59;73m\x1b[38;2;222;224;225m ↓ Jump to latest message · End \x1b[39m\x1b[49m";
        assert_eq!(scroll_to_end_indicator(&theme, Some(&shortcut)), expected);
        // No shortcut: the label keeps its trailing space but drops the separator.
        assert_eq!(
            scroll_to_end_indicator(&theme, None),
            "\x1b[48;2;33;59;73m\x1b[38;2;222;224;225m ↓ Jump to latest message \x1b[39m\x1b[49m"
        );
    }

    #[test]
    fn create_interactive_tui_resolves_mode() {
        let theme = dark();
        let fullscreen = create_interactive_tui(
            Arc::clone(&theme),
            InteractiveTuiOptions {
                tui_mode: TuiMode::Fullscreen,
                show_hardware_cursor: true,
                log_directory: "/logs".to_string(),
                ..InteractiveTuiOptions::default()
            },
        );
        assert!(matches!(fullscreen, InteractiveTuiPlan::Fullscreen { .. }));
        let regular = create_interactive_tui(
            Arc::clone(&theme),
            InteractiveTuiOptions {
                tui_mode: TuiMode::Regular,
                ..InteractiveTuiOptions::default()
            },
        );
        assert!(matches!(regular, InteractiveTuiPlan::Regular { .. }));
    }
}
