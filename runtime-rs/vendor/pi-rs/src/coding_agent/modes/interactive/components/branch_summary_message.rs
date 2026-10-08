//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Port of `modes/interactive/components/branch-summary-message.ts` (67 lines,
//! sha256 `32525e00f5fd9c4e6706aeb77b0647d20d4fa2e35f6384a8b6d4d2fbcd0bb041`)
//! — a branch summary message with a collapsed/expanded state, rendered in a
//! `customMessageBg` box.
//!
//! Disclosed substitution (S19.6, shared by the expandable message
//! components): upstream wraps the content in a `MouseRegion` whose click
//! handler calls `setExpanded`; the port implements the same left-click toggle
//! as the component's `handle_mouse` (the region spans the whole content, so
//! the observable behavior is identical), and the content `Container` is
//! mounted directly instead of inside a region wrapper.

use std::sync::Arc;

use crate::coding_agent::modes::interactive::components::model_selector::theme_fg;
use crate::coding_agent::modes::interactive::components::support::{
    plain_markdown_theme, MessageBgBox,
};
use crate::coding_agent::modes::interactive::theme::Theme;
use crate::tui::component::{
    Component, TuiMouseButton, TuiMouseEvent, TuiMouseEventResult, TuiMouseEventType,
};
use crate::tui::component_mouse::ComponentHandle;
use crate::tui::components::container::Container;
use crate::tui::components::layout_widgets::Spacer;
use crate::tui::components::markdown::{DefaultTextStyle, Markdown, MarkdownTheme};
use crate::tui::components::text::Text;

use super::support::theme_text_style;

/// Upstream `BranchSummaryMessage` payload used by the component.
#[derive(Clone, Debug, Default)]
pub struct BranchSummaryMessage {
    pub summary: String,
}

/// Upstream `BranchSummaryMessageComponent`.
pub struct BranchSummaryMessageComponent {
    box_: MessageBgBox,
    expanded: bool,
    message: BranchSummaryMessage,
    markdown_theme: MarkdownTheme,
    theme: Arc<Theme>,
}

impl BranchSummaryMessageComponent {
    /// Upstream constructor.
    pub fn new(
        theme: Arc<Theme>,
        message: BranchSummaryMessage,
        markdown_theme: Option<MarkdownTheme>,
    ) -> Self {
        let markdown_theme = markdown_theme.unwrap_or_else(plain_markdown_theme);
        let mut component = Self {
            box_: MessageBgBox::new(
                1,
                1,
                Some({
                    let bg_theme = Arc::clone(&theme);
                    Arc::new(move |t: &str| {
                        bg_theme.bg("customMessageBg", t).expect("customMessageBg")
                    })
                }),
            ),
            expanded: false,
            message,
            markdown_theme,
            theme,
        };
        component.update_display();
        component
    }

    /// Upstream `setExpanded`.
    pub fn set_expanded(&mut self, expanded: bool) {
        self.expanded = expanded;
        self.update_display();
    }

    pub fn is_expanded(&self) -> bool {
        self.expanded
    }

    fn update_display(&mut self) {
        self.box_ = MessageBgBox::new(
            1,
            1,
            Some({
                let bg_theme = Arc::clone(&self.theme);
                Arc::new(move |t: &str| bg_theme.bg("customMessageBg", t).expect("customMessageBg"))
            }),
        );
        let mut content = Container::default();
        let label = theme_fg(&self.theme, "customMessageLabel", "\x1b[1m[branch]\x1b[22m");
        content.add_child(ComponentHandle::new(Text::with_options(&label, 0, 0, None)));
        content.add_child(ComponentHandle::new(Spacer::new(1)));

        if self.expanded {
            let header = "**Branch Summary**\n\n";
            content.add_child(ComponentHandle::new(Markdown::new(
                format!("{header}{}", self.message.summary),
                0,
                0,
                self.markdown_theme.clone(),
                Some(DefaultTextStyle {
                    color: Some(theme_text_style(&self.theme, "customMessageText")),
                    ..DefaultTextStyle::default()
                }),
                None,
            )));
        } else {
            let text = theme_fg(&self.theme, "customMessageText", "Branch summary (")
                + &theme_fg(&self.theme, "dim", &key_text("app.tools.expand"))
                + &theme_fg(&self.theme, "customMessageText", " to expand)");
            content.add_child(ComponentHandle::new(Text::with_options(&text, 0, 0, None)));
        }

        self.box_.add_child(Box::new(content));
    }
}

fn key_text(keybinding: &str) -> String {
    crate::coding_agent::modes::interactive::components::model_selector::key_text(keybinding)
}

impl Component for BranchSummaryMessageComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.box_.render(width)
    }

    fn invalidate(&mut self) {
        self.update_display();
    }

    fn handle_mouse(&mut self, event: &TuiMouseEvent) -> Option<TuiMouseEventResult> {
        if event.event_type == TuiMouseEventType::Click && event.button == TuiMouseButton::Left {
            self.set_expanded(!self.expanded);
            return Some(TuiMouseEventResult {
                handled: true,
                ..TuiMouseEventResult::default()
            });
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding_agent::modes::interactive::theme::{load_builtin_theme, ColorMode};

    fn dark() -> Arc<Theme> {
        Arc::new(load_builtin_theme("dark", Some(ColorMode::Truecolor)).expect("dark"))
    }

    /// Oracle: tests/fixtures/interactive_r19_oracle scenario `message_components`
    /// (`branchCollapsed` / `branchToggled` structures and the collapsed label
    /// bytes; the expanded face feeds `**Branch Summary**` into Markdown).
    #[test]
    fn collapsed_label_matches_oracle_bytes() {
        let theme = dark();
        let mut component = BranchSummaryMessageComponent::new(
            theme,
            BranchSummaryMessage {
                summary: "Did **stuff**".to_string(),
            },
            None,
        );
        assert!(!component.is_expanded());
        let mut rendered = component.render(40);
        assert!(!rendered.is_empty());
        let first = rendered.remove(0);
        // top pad row: full-width customMessageBg
        assert!(first.contains("\x1b[48;2;58;48;85m"), "customMessageBg row");
        let content = rendered.join("\n");
        assert!(content.contains("\x1b[38;2;167;152;215m\x1b[1m[branch]\x1b[22m\x1b[39m"));
        assert!(content.contains("Branch summary ("));
        assert!(content.contains("ctrl+o"));
        assert!(content.contains(" to expand)"));

        // click toggles to expanded
        let event = TuiMouseEvent {
            event_type: TuiMouseEventType::Click,
            button: TuiMouseButton::Left,
            x: 1,
            y: 2,
            screen_x: 1,
            screen_y: 2,
            width: 40,
            height: 10,
            shift: false,
            alt: false,
            ctrl: false,
            wheel_delta: None,
            click_count: Some(1),
        };
        let result = component.handle_mouse(&event);
        assert!(result.expect("handled").handled);
        assert!(component.is_expanded());
        let expanded = component.render(40);
        assert!(expanded.join("\n").contains("Branch Summary"));
    }
}
