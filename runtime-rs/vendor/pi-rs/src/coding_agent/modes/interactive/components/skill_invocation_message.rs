//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Port of `modes/interactive/components/skill-invocation-message.ts` (64
//! lines, sha256
//! `7ec5c3e95413187aab7937740062cfa3c4dbac4754afbfa78810b6ad89ff0b27`) —
//! renders a skill invocation block with a collapsed/expanded state (same
//! click-toggle substitution as [`super::branch_summary_message`], S19.6).

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
use crate::tui::components::markdown::{DefaultTextStyle, Markdown, MarkdownTheme};
use crate::tui::components::text::Text;

/// Upstream `ParsedSkillBlock` payload used by the component.
#[derive(Clone, Debug, Default)]
pub struct ParsedSkillBlock {
    pub name: String,
    pub content: String,
}

/// Upstream `SkillInvocationMessageComponent`.
pub struct SkillInvocationMessageComponent {
    box_: MessageBgBox,
    expanded: bool,
    skill_block: ParsedSkillBlock,
    markdown_theme: MarkdownTheme,
    theme: Arc<Theme>,
}

impl SkillInvocationMessageComponent {
    /// Upstream constructor.
    pub fn new(
        theme: Arc<Theme>,
        skill_block: ParsedSkillBlock,
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
            skill_block,
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

        if self.expanded {
            // Expanded: label + skill name header + full content
            let label = theme_fg(&self.theme, "customMessageLabel", "\x1b[1m[skill]\x1b[22m");
            content.add_child(ComponentHandle::new(Text::with_options(&label, 0, 0, None)));
            let header = format!("**{}**\n\n", self.skill_block.name);
            content.add_child(ComponentHandle::new(Markdown::new(
                format!("{header}{}", self.skill_block.content),
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
            // Collapsed: single line - [skill] name (hint to expand)
            let line = theme_fg(&self.theme, "customMessageLabel", "\x1b[1m[skill]\x1b[22m ")
                + &theme_fg(&self.theme, "customMessageText", &self.skill_block.name)
                + &theme_fg(
                    &self.theme,
                    "dim",
                    &format!(" ({} to expand)", key_text("app.tools.expand")),
                );
            content.add_child(ComponentHandle::new(Text::with_options(&line, 0, 0, None)));
        }

        self.box_.add_child(Box::new(content));
    }
}

fn key_text(keybinding: &str) -> String {
    crate::coding_agent::modes::interactive::components::model_selector::key_text(keybinding)
}

impl Component for SkillInvocationMessageComponent {
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

    /// Oracle scenario `message_components` (`skillCollapsed` /
    /// `skillExpanded` structure + collapsed single-line bytes).
    #[test]
    fn skill_invocation_matches_oracle() {
        let theme = dark();
        let mut component = SkillInvocationMessageComponent::new(
            theme,
            ParsedSkillBlock {
                name: "my-skill".to_string(),
                content: "skill body".to_string(),
            },
            None,
        );
        let collapsed = component.render(40).join("\n");
        // oracle skillCollapsed row bytes (fg reset between the label space and name)
        assert!(collapsed.contains("\x1b[1m[skill]\x1b[22m \x1b[39m\x1b[38;2;157;165;169mmy-skill"));
        assert!(collapsed.contains(" (ctrl+o to expand)"));

        component.set_expanded(true);
        let expanded = component.render(40).join("\n");
        assert!(expanded.contains("[skill]"));
    }
}
