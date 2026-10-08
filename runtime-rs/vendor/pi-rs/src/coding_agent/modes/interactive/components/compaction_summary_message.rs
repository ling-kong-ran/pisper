//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Port of `modes/interactive/components/compaction-summary-message.ts`
//! (68 lines, sha256
//! `90f5d197364755431678229959f233527e2b07076d2e7d8ffde83da09b77905a`) — a
//! compaction message with a collapsed/expanded state, rendered in a
//! `customMessageBg` box (same click-toggle substitution as
//! [`super::branch_summary_message`], seam S19.6).
//!
//! `tokensBefore.toLocaleString()` is locale-dependent upstream; the port
//! fixes node's default en-US grouping (seam S19.5).

use std::sync::Arc;

use crate::coding_agent::modes::interactive::components::model_selector::theme_fg;
use crate::coding_agent::modes::interactive::components::support::{
    plain_markdown_theme, theme_text_style, MessageBgBox,
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

/// Upstream `CompactionSummaryMessage` payload used by the component.
#[derive(Clone, Debug, Default)]
pub struct CompactionSummaryMessage {
    pub summary: String,
    pub tokens_before: u64,
}

/// `toLocaleString()` under node's default en-US locale.
pub fn format_number_with_separators(value: u64) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    let len = digits.len();
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (len - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    grouped
}

/// Upstream `CompactionSummaryMessageComponent`.
pub struct CompactionSummaryMessageComponent {
    box_: MessageBgBox,
    expanded: bool,
    message: CompactionSummaryMessage,
    markdown_theme: MarkdownTheme,
    theme: Arc<Theme>,
}

impl CompactionSummaryMessageComponent {
    /// Upstream constructor.
    pub fn new(
        theme: Arc<Theme>,
        message: CompactionSummaryMessage,
        markdown_theme: Option<MarkdownTheme>,
    ) -> Self {
        let markdown_theme = markdown_theme.unwrap_or_else(plain_markdown_theme);
        let mut component = Self {
            box_: bg_box(&theme),
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
        self.box_ = bg_box(&self.theme);
        let mut content = Container::default();
        let token_str = format_number_with_separators(self.message.tokens_before);
        let label = theme_fg(
            &self.theme,
            "customMessageLabel",
            "\x1b[1m[compaction]\x1b[22m",
        );
        content.add_child(ComponentHandle::new(Text::with_options(&label, 0, 0, None)));
        content.add_child(ComponentHandle::new(Spacer::new(1)));

        if self.expanded {
            let header = format!("**Compacted from {token_str} tokens**\n\n");
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
            let text = theme_fg(
                &self.theme,
                "customMessageText",
                &format!("Compacted from {token_str} tokens ("),
            ) + &theme_fg(&self.theme, "dim", &key_text("app.tools.expand"))
                + &theme_fg(&self.theme, "customMessageText", " to expand)");
            content.add_child(ComponentHandle::new(Text::with_options(&text, 0, 0, None)));
        }

        self.box_.add_child(Box::new(content));
    }
}

fn bg_box(theme: &Arc<Theme>) -> MessageBgBox {
    MessageBgBox::new(
        1,
        1,
        Some({
            let bg_theme = Arc::clone(theme);
            Arc::new(move |t: &str| bg_theme.bg("customMessageBg", t).expect("customMessageBg"))
        }),
    )
}

fn key_text(keybinding: &str) -> String {
    crate::coding_agent::modes::interactive::components::model_selector::key_text(keybinding)
}

impl Component for CompactionSummaryMessageComponent {
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

    #[test]
    fn locale_grouping_matches_node_default() {
        assert_eq!(format_number_with_separators(0), "0");
        assert_eq!(format_number_with_separators(999), "999");
        assert_eq!(format_number_with_separators(1_000), "1,000");
        assert_eq!(format_number_with_separators(1_234_567), "1,234,567");
    }

    /// Oracle scenario `message_components` (`compactionCollapsed` /
    /// `compactionExpanded`): collapsed label bytes + expanded header feed.
    #[test]
    fn compaction_matches_oracle() {
        let theme = dark();
        let mut component = CompactionSummaryMessageComponent::new(
            theme,
            CompactionSummaryMessage {
                summary: "compact summary".to_string(),
                tokens_before: 1_234_567,
            },
            None,
        );
        // render wide enough that the 50-visible-column collapsed line is not
        // truncated to the box's inner width (the oracle captures the untruncated
        // child text: "Compacted from 1,234,567 tokens (" + dim "ctrl+o" + " to expand)")
        let collapsed = component.render(80).join("\n");
        assert!(collapsed.contains("\x1b[1m[compaction]\x1b[22m"));
        assert!(collapsed.contains("Compacted from 1,234,567 tokens ("));
        // oracle compactionCollapsed bytes: dim "ctrl+o" then customMessageText " to expand)"
        assert!(collapsed
            .contains("\x1b[38;2;126;136;142mctrl+o\x1b[39m\x1b[38;2;157;165;169m to expand)"));

        component.set_expanded(true);
        let expanded = component.render(80).join("\n");
        assert!(expanded.contains("[compaction]"));
    }

    #[test]
    fn click_toggles_expanded() {
        let theme = dark();
        let mut component = CompactionSummaryMessageComponent::new(
            theme,
            CompactionSummaryMessage::default(),
            None,
        );
        let event = TuiMouseEvent {
            event_type: TuiMouseEventType::Click,
            button: TuiMouseButton::Left,
            x: 0,
            y: 0,
            screen_x: 0,
            screen_y: 0,
            width: 50,
            height: 10,
            shift: false,
            alt: false,
            ctrl: false,
            wheel_delta: None,
            click_count: Some(1),
        };
        assert!(component.handle_mouse(&event).expect("handled").handled);
        assert!(component.is_expanded());
    }
}
