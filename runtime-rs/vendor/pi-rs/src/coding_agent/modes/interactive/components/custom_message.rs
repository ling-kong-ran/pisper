//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Port of `modes/interactive/components/custom-message.ts` (113 lines, sha256
//! `321af8afdff8da4924a7dbf5623b5637eb160ea7d73a70e6662d8125249bde9b`) —
//! renders a custom message entry from extensions with an optional
//! extension-owned renderer (distinct styling vs user messages).

use std::sync::Arc;

use crate::coding_agent::modes::interactive::components::model_selector::theme_fg;
use crate::coding_agent::modes::interactive::components::support::{
    plain_markdown_theme, theme_text_style, MessageBgBox,
};
use crate::coding_agent::modes::interactive::theme::Theme;
use crate::tui::component::Component;
use crate::tui::components::layout_widgets::Spacer;
use crate::tui::components::markdown::{DefaultTextStyle, Markdown, MarkdownTheme};
use crate::tui::components::text::Text;

/// Upstream `TextContent` slice of the custom message content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentBlock {
    pub block_type: String,
    pub text: Option<String>,
}

/// Upstream `CustomMessage.content` (string or content blocks).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CustomMessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

/// Upstream `CustomMessage` payload used by the component.
#[derive(Clone, Debug)]
pub struct CustomMessagePayload {
    pub custom_type: String,
    pub content: CustomMessageContent,
}

/// Upstream renderer options (`{ expanded, outputPad }`).
#[derive(Clone, Copy, Debug)]
pub struct MessageRendererOptions {
    pub expanded: bool,
    pub output_pad: usize,
}

/// Upstream `MessageRenderer` (`core/extensions/types.ts`): returns a
/// component or nothing; a panicking renderer falls through to the default
/// rendering (upstream `catch {}`).
pub type MessageRenderer = Box<
    dyn FnMut(&CustomMessagePayload, MessageRendererOptions, &Theme) -> Option<Box<dyn Component>>,
>;

/// Upstream `CustomMessageComponent`.
pub struct CustomMessageComponent {
    children: Vec<Box<dyn Component>>,
    message: CustomMessagePayload,
    custom_renderer: Option<MessageRenderer>,
    markdown_theme: MarkdownTheme,
    expanded: bool,
    output_pad: usize,
    theme: Arc<Theme>,
}

impl CustomMessageComponent {
    /// Upstream constructor.
    pub fn new(
        theme: Arc<Theme>,
        message: CustomMessagePayload,
        custom_renderer: Option<MessageRenderer>,
        markdown_theme: Option<MarkdownTheme>,
        output_pad: usize,
    ) -> Self {
        let markdown_theme = markdown_theme.unwrap_or_else(plain_markdown_theme);
        let mut component = Self {
            children: vec![Box::new(Spacer::new(1))],
            message,
            custom_renderer,
            markdown_theme,
            expanded: false,
            output_pad,
            theme,
        };
        component.rebuild();
        component
    }

    /// Upstream `setExpanded`.
    pub fn set_expanded(&mut self, expanded: bool) {
        if self.expanded != expanded {
            self.expanded = expanded;
            self.rebuild();
        }
    }

    /// Upstream `setOutputPad`.
    pub fn set_output_pad(&mut self, output_pad: usize) {
        if self.output_pad != output_pad {
            self.output_pad = output_pad;
            self.rebuild();
        }
    }

    fn rebuild(&mut self) {
        // Remove previous content component (the leading spacer stays).
        self.children.truncate(1);

        // Try custom renderer first - it handles its own styling.
        if let Some(renderer) = &mut self.custom_renderer {
            let attempt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                renderer(
                    &self.message,
                    MessageRendererOptions {
                        expanded: self.expanded,
                        output_pad: self.output_pad,
                    },
                    &self.theme,
                )
            }));
            if let Ok(Some(component)) = attempt {
                self.children.push(component);
                return;
            }
        }

        // Default rendering uses our box (label + content).
        let mut box_ = MessageBgBox::new(
            1,
            1,
            Some({
                let bg_theme = Arc::clone(&self.theme);
                Arc::new(move |t: &str| bg_theme.bg("customMessageBg", t).expect("customMessageBg"))
            }),
        );
        let label = theme_fg(
            &self.theme,
            "customMessageLabel",
            &format!("\x1b[1m[{}]\x1b[22m", self.message.custom_type),
        );
        box_.add_child(Box::new(Text::with_options(&label, 0, 0, None)));
        box_.add_child(Box::new(Spacer::new(1)));

        let text = match &self.message.content {
            CustomMessageContent::Text(text) => text.clone(),
            CustomMessageContent::Blocks(blocks) => blocks
                .iter()
                .filter(|block| block.block_type == "text")
                .filter_map(|block| block.text.clone())
                .collect::<Vec<_>>()
                .join("\n"),
        };

        box_.add_child(Box::new(Markdown::new(
            text,
            0,
            0,
            self.markdown_theme.clone(),
            Some(DefaultTextStyle {
                color: Some(theme_text_style(&self.theme, "customMessageText")),
                ..DefaultTextStyle::default()
            }),
            None,
        )));
        self.children.push(Box::new(box_));
    }
}

impl Component for CustomMessageComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        let mut lines = Vec::new();
        for child in &mut self.children {
            lines.extend(child.render(width));
        }
        lines
    }

    fn invalidate(&mut self) {
        self.rebuild();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding_agent::modes::interactive::theme::{load_builtin_theme, ColorMode};

    fn dark() -> Arc<Theme> {
        Arc::new(load_builtin_theme("dark", Some(ColorMode::Truecolor)).expect("dark"))
    }

    fn payload(custom_type: &str, content: CustomMessageContent) -> CustomMessagePayload {
        CustomMessagePayload {
            custom_type: custom_type.to_string(),
            content,
        }
    }

    /// Oracle scenario `message_components` (`custom`, `customBlocks`,
    /// `customWithRenderer`, `rendererCalls`, `customRendererNull`,
    /// `customRendererThrows`).
    #[test]
    fn custom_message_matches_oracle() {
        let theme = dark();
        let mut custom = CustomMessageComponent::new(
            theme.clone(),
            payload("plan", CustomMessageContent::Text("steps".to_string())),
            None,
            None,
            1,
        );
        let rendered = custom.render(40).join("\n");
        assert!(rendered.contains("\x1b[1m[plan]\x1b[22m"));
        assert!(rendered.contains("steps"));

        let mut blocks = CustomMessageComponent::new(
            theme.clone(),
            payload(
                "note",
                CustomMessageContent::Blocks(vec![
                    ContentBlock {
                        block_type: "text".to_string(),
                        text: Some("one".to_string()),
                    },
                    ContentBlock {
                        block_type: "image".to_string(),
                        text: None,
                    },
                    ContentBlock {
                        block_type: "text".to_string(),
                        text: Some("two".to_string()),
                    },
                ]),
            ),
            None,
            None,
            1,
        );
        let blocks_rendered = blocks.render(40).join("\n");
        assert!(blocks_rendered.contains("\x1b[1m[note]\x1b[22m"));

        let calls = std::rc::Rc::new(std::cell::Cell::new(0usize));
        let calls_handle = std::rc::Rc::clone(&calls);
        let mut with_renderer = CustomMessageComponent::new(
            theme.clone(),
            payload("n", CustomMessageContent::Text("c".to_string())),
            Some(Box::new(move |_message, _options, _theme| {
                calls_handle.set(calls_handle.get() + 1);
                Some(Box::new(Text::with_options("custom rendered", 0, 0, None))
                    as Box<dyn Component>)
            })),
            None,
            1,
        );
        let rendered = with_renderer.render(40).join("\n");
        assert!(rendered.contains("custom rendered"));
        assert_eq!(calls.get(), 1);

        // null return → default rendering
        let mut null_renderer = CustomMessageComponent::new(
            theme.clone(),
            payload("n", CustomMessageContent::Text("c".to_string())),
            Some(Box::new(|_m, _o, _t| None)),
            None,
            1,
        );
        let rendered = null_renderer.render(40).join("\n");
        assert!(rendered.contains("\x1b[1m[n]\x1b[22m"));

        // panicking renderer → default rendering (upstream catch {})
        let mut throwing_renderer = CustomMessageComponent::new(
            theme,
            payload("n", CustomMessageContent::Text("c".to_string())),
            Some(Box::new(|_m, _o, _t| panic!("renderer exploded"))),
            None,
            1,
        );
        let rendered = throwing_renderer.render(40).join("\n");
        assert!(rendered.contains("\x1b[1m[n]\x1b[22m"));
    }
}
