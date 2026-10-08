//! The three-line transcript search component (`alt-screen-search.ts:197-327`).
//!
//! Owns and drives the real Input; no pre-rendered input or oracle data enters
//! production. The default environment reads the current global keybindings on
//! every render. An explicit service seam supports alternate host platforms and
//! dynamic navigation bindings without changing process-global state in tests.
//!
//! Like Component, widths are nonnegative cells and strings are valid UTF-8.
//! Result integers are exact within JS's safe-integer range. This is not a raw
//! UTF-16 Input, a reentrant callback API, or the TuiAltScreen host/event loop.
use crate::tui::component::Component;
use crate::tui::components::input::{Input, InputOptions};
use crate::tui::keybindings::with_keybindings;
use crate::tui::utils::{truncate_to_width, visible_width};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchNavigationDirection {
    Previous,
    Next,
}

/// Environment inputs only. Key selection/formatting/layout remain in the UI.
#[derive(Clone, Debug)]
pub struct SearchNavigationEnvironment {
    pub macos: bool,
    pub previous_keys: Vec<String>,
    pub next_keys: Vec<String>,
}

impl SearchNavigationEnvironment {
    pub fn current() -> Self {
        with_keybindings(|kb| Self {
            macos: cfg!(target_os = "macos"),
            previous_keys: kb.get_keys("tui.altScreen.searchPrevious"),
            next_keys: kb.get_keys("tui.altScreen.searchNext"),
        })
    }
}

type NavigationStyle = Box<dyn FnMut(&str, bool) -> String>;

pub struct AltScreenSearchComponent {
    input: Input,
    on_query_change: Box<dyn FnMut(&str)>,
    navigation_button_style: NavigationStyle,
    environment: Box<dyn FnMut() -> SearchNavigationEnvironment>,
    result_count: i64,
    result_index: i64,
    previous_button: Option<std::ops::Range<usize>>,
    next_button: Option<std::ops::Range<usize>>,
    hovered_navigation_direction: Option<SearchNavigationDirection>,
    focused: bool,
}

impl AltScreenSearchComponent {
    pub fn new(on_query_change: impl FnMut(&str) + 'static) -> Self {
        Self::with_navigation_button_style(on_query_change, |text, _| text.to_owned())
    }

    pub fn with_navigation_button_style(
        on_query_change: impl FnMut(&str) + 'static,
        navigation_button_style: impl FnMut(&str, bool) -> String + 'static,
    ) -> Self {
        Self::with_environment(
            on_query_change,
            navigation_button_style,
            SearchNavigationEnvironment::current,
        )
    }

    pub fn with_environment(
        on_query_change: impl FnMut(&str) + 'static,
        navigation_button_style: impl FnMut(&str, bool) -> String + 'static,
        environment: impl FnMut() -> SearchNavigationEnvironment + 'static,
    ) -> Self {
        Self {
            input: Input::new(InputOptions {
                prompt: Some(" ".to_owned()),
                placeholder: Some("Find in transcript".to_owned()),
                placeholder_style: Some(Arc::new(|text| format!("\x1b[2m{text}\x1b[22m"))),
            }),
            on_query_change: Box::new(on_query_change),
            navigation_button_style: Box::new(navigation_button_style),
            environment: Box::new(environment),
            result_count: 0,
            result_index: -1,
            previous_button: None,
            next_button: None,
            hovered_navigation_direction: None,
            focused: false,
        }
    }

    /// Does not recompute navigation hit bounds until the next render.
    pub fn set_result(&mut self, index: i64, count: i64) {
        self.result_index = index;
        self.result_count = count;
    }

    /// Zero-based local cells, half-open bounds from the most recent render.
    pub fn navigation_direction_at(
        &self,
        row: i64,
        column: i64,
    ) -> Option<SearchNavigationDirection> {
        if row != 2 {
            return None;
        }
        let column = usize::try_from(column).ok()?;
        if self
            .previous_button
            .as_ref()
            .is_some_and(|r| r.contains(&column))
        {
            Some(SearchNavigationDirection::Previous)
        } else if self
            .next_button
            .as_ref()
            .is_some_and(|r| r.contains(&column))
        {
            Some(SearchNavigationDirection::Next)
        } else {
            None
        }
    }

    pub fn set_hovered_navigation_direction(
        &mut self,
        direction: Option<SearchNavigationDirection>,
    ) -> bool {
        if direction == self.hovered_navigation_direction {
            return false;
        }
        self.hovered_navigation_direction = direction;
        true
    }

    pub fn handle_input(&mut self, data: &str) {
        let previous = self.input.value().to_owned();
        self.input.handle_input(data);
        let query = self.input.value();
        if query != previous {
            (self.on_query_change)(query);
        }
    }
}

fn format_key(key: Option<&String>, macos: bool) -> String {
    let Some(key) = key.filter(|key| !key.is_empty()) else {
        return "Unbound".to_owned();
    };
    key.split('+')
        .map(|part| {
            if macos && part.eq_ignore_ascii_case("alt") {
                return "Option".to_owned();
            }
            let Some(first) = part.chars().next() else {
                return String::new();
            };
            // JS charAt(0) is one UTF-16 unit: a leading surrogate cannot be
            // uppercased independently (not even for a Deseret lowercase pair).
            if first.len_utf16() == 2 {
                return part.to_owned();
            }
            first.to_uppercase().collect::<String>() + &part[first.len_utf8()..]
        })
        .collect::<Vec<_>>()
        .join("+")
}

impl Component for AltScreenSearchComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        let safe_width = width.max(1);
        let inner_width = safe_width.saturating_sub(2);
        let environment = (self.environment)();
        let previous_key = format_key(environment.previous_keys.first(), environment.macos);
        let next_key = format_key(environment.next_keys.first(), environment.macos);
        let result = if self.input.value().is_empty() {
            String::new()
        } else if self.result_count == 0 {
            "No matches".to_owned()
        } else {
            format!(
                "{}/{}",
                i128::from(self.result_index) + 1,
                self.result_count
            )
        };
        let visible_result = truncate_to_width(&result, inner_width.saturating_sub(3), "", false);
        let result_text = if visible_result.is_empty() {
            String::new()
        } else {
            format!("\x1b[2m {visible_result} \x1b[22m")
        };
        let input_width = inner_width.saturating_sub(visible_width(&result_text));
        let input_lines = self.input.render(input_width.max(1));
        let input_line = truncate_to_width(
            input_lines.first().map_or("", String::as_str),
            input_width,
            "",
            false,
        );
        let input_padding = " ".repeat(input_width.saturating_sub(visible_width(&input_line)));
        let content = format!("{input_line}{input_padding}{result_text}");

        let mut previous_button = format!("↑ {previous_key}");
        let mut next_button = format!("↓ {next_key}");
        let mut separator = " · ";
        let available_controls_width = inner_width.saturating_sub(3);
        let mut controls_width = visible_width(&previous_button)
            + visible_width(separator)
            + visible_width(&next_button);
        if controls_width > available_controls_width {
            previous_button = "↑".to_owned();
            next_button = "↓".to_owned();
            separator = " ";
            controls_width = visible_width(&previous_button)
                + visible_width(separator)
                + visible_width(&next_button);
        }
        let show_buttons = controls_width <= available_controls_width;
        let rendered_buttons = if show_buttons {
            let previous = (self.navigation_button_style)(
                &previous_button,
                self.hovered_navigation_direction == Some(SearchNavigationDirection::Previous),
            );
            let next = (self.navigation_button_style)(
                &next_button,
                self.hovered_navigation_direction == Some(SearchNavigationDirection::Next),
            );
            format!("{previous}{separator}{next}")
        } else {
            String::new()
        };
        let outer_gaps_width = if show_buttons { 2 } else { 0 };
        let right_rule_width = usize::from(
            !rendered_buttons.is_empty() && inner_width > controls_width + outer_gaps_width,
        );
        let left_rule_width = inner_width.saturating_sub(
            (if show_buttons { controls_width } else { 0 }) + outer_gaps_width + right_rule_width,
        );
        let previous_start = 1 + left_rule_width + 1;
        // Publish unstyled bounds only after Input render and painter callbacks.
        self.previous_button =
            show_buttons.then(|| previous_start..previous_start + visible_width(&previous_button));
        self.next_button = self.previous_button.as_ref().map(|previous| {
            let start = previous.end + visible_width(separator);
            start..start + visible_width(&next_button)
        });
        // Upstream still performs all the work above at widths 0 and 1.
        if safe_width == 1 {
            return vec!["┌".to_owned(), "│".to_owned(), "└".to_owned()];
        }
        let gap = if rendered_buttons.is_empty() { "" } else { " " };
        vec![
            format!("┌{}┐", "─".repeat(inner_width)),
            format!("│{content}│"),
            format!(
                "└{}{gap}{rendered_buttons}{gap}{}┘",
                "─".repeat(left_rule_width),
                "─".repeat(right_rule_width),
            ),
        ]
    }

    fn handle_input(&mut self, data: &str) {
        Self::handle_input(self, data);
    }

    fn invalidate(&mut self) {
        self.input.invalidate();
    }

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

#[cfg(test)]
#[path = "tests/alt_screen_search_component.rs"]
mod tests;
