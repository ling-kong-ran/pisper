//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Port of `modes/interactive/components/assistant-message.ts` (202 lines,
//! sha256
//! `f8a20228291816aec253d3f115fec5ba765807275bc14077780225117e5912cb`) —
//! renders a complete assistant message: trimmed text runs as Markdown,
//! consecutive thinking blocks merge into one italic region with a
//! click-to-toggle visibility override, and stop reasons surface as error
//! lines. The render wraps the block in OSC-133 shell-integration zones
//! unless it contains tool calls.

use std::sync::Arc;

use crate::coding_agent::modes::interactive::components::markdown_transform::{
    create_markdown_transform, MarkdownTransformer,
};
use crate::coding_agent::modes::interactive::components::model_selector::theme_fg;
use crate::coding_agent::modes::interactive::components::support::{
    plain_markdown_theme, theme_text_style, OSC133_ZONE_END, OSC133_ZONE_FINAL, OSC133_ZONE_START,
};
use crate::coding_agent::modes::interactive::theme::Theme;
use crate::tui::component::{
    Component, TuiMouseButton, TuiMouseEvent, TuiMouseEventResult, TuiMouseEventType,
};
use crate::tui::components::container::Container;
use crate::tui::components::layout_widgets::Spacer;
use crate::tui::components::markdown::{DefaultTextStyle, Markdown, MarkdownTheme};
use crate::tui::components::text::Text;

/// Upstream `AssistantMessage` slice consumed by the component.
#[derive(Clone, Debug, Default)]
pub struct AssistantMessage {
    pub content: Vec<AssistantContentBlock>,
    pub stop_reason: Option<String>,
    pub error_message: Option<String>,
}

/// Upstream `AssistantMessage.content` entries (text / thinking / toolCall).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssistantContentBlock {
    Text { text: String },
    Thinking { thinking: String },
    ToolCall { id: String },
}

/// Upstream `AssistantMessageComponent`.
pub struct AssistantMessageComponent {
    content_container: Container,
    hide_thinking_block: bool,
    markdown_theme: MarkdownTheme,
    hidden_thinking_label: String,
    output_pad: usize,
    markdown_transformers: Arc<Vec<MarkdownTransformer>>,
    last_message: Option<AssistantMessage>,
    has_tool_calls: bool,
    is_streaming: bool,
    thinking_visibility_overrides: Vec<(usize, bool)>,
    /// Shared click flags per thinking run (see [`ThinkingRegion`]).
    thinking_toggle_flags: Vec<(usize, Arc<std::sync::atomic::AtomicBool>)>,
    theme: Arc<Theme>,
}

impl AssistantMessageComponent {
    /// Upstream constructor.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        theme: Arc<Theme>,
        message: Option<AssistantMessage>,
        hide_thinking_block: bool,
        markdown_theme: Option<MarkdownTheme>,
        hidden_thinking_label: impl Into<String>,
        output_pad: usize,
        markdown_transformers: Vec<MarkdownTransformer>,
    ) -> Self {
        let mut component = Self {
            content_container: Container::default(),
            hide_thinking_block,
            markdown_theme: markdown_theme.unwrap_or_else(plain_markdown_theme),
            hidden_thinking_label: hidden_thinking_label.into(),
            output_pad,
            markdown_transformers: Arc::new(markdown_transformers),
            last_message: None,
            has_tool_calls: false,
            is_streaming: false,
            thinking_visibility_overrides: Vec::new(),
            thinking_toggle_flags: Vec::new(),
            theme,
        };
        if let Some(message) = message {
            component.update_content(message, false);
        }
        component
    }

    /// Upstream `setHideThinkingBlock` (clears per-run overrides).
    pub fn set_hide_thinking_block(&mut self, hide: bool) {
        self.hide_thinking_block = hide;
        self.thinking_visibility_overrides.clear();
        if let Some(message) = self.last_message.clone() {
            self.update_content(message, self.is_streaming);
        }
    }

    /// Upstream `setHiddenThinkingLabel`.
    pub fn set_hidden_thinking_label(&mut self, label: impl Into<String>) {
        self.hidden_thinking_label = label.into();
        if let Some(message) = self.last_message.clone() {
            self.update_content(message, self.is_streaming);
        }
    }

    /// Upstream `setOutputPad`.
    pub fn set_output_pad(&mut self, padding: usize) {
        self.output_pad = padding;
        if let Some(message) = self.last_message.clone() {
            self.update_content(message, self.is_streaming);
        }
    }

    /// Upstream `updateContent`.
    pub fn update_content(&mut self, message: AssistantMessage, is_streaming: bool) {
        self.last_message = Some(message.clone());
        self.is_streaming = is_streaming;

        // Clear content container
        self.content_container = Container::default();

        let has_visible_content = message.content.iter().any(|c| match c {
            AssistantContentBlock::Text { text } => !text.trim().is_empty(),
            AssistantContentBlock::Thinking { thinking } => !thinking.trim().is_empty(),
            AssistantContentBlock::ToolCall { .. } => false,
        });

        if has_visible_content {
            self.content_container
                .add_child(crate::tui::component_mouse::ComponentHandle::new(
                    Spacer::new(1),
                ));
        }

        // Render content in order (thinking runs merge).
        let mut thinking_run_index = 0usize;
        let mut i = 0usize;
        while i < message.content.len() {
            match &message.content[i] {
                AssistantContentBlock::Text { text } if !text.trim().is_empty() => {
                    // paddingY=0 avoids extra spacing before tool executions
                    let transform = create_markdown_transform(
                        "assistant",
                        self.is_streaming,
                        Arc::clone(&self.markdown_transformers),
                    );
                    self.content_container.add_child(
                        crate::tui::component_mouse::ComponentHandle::new(Markdown::new(
                            text.trim().to_string(),
                            self.output_pad,
                            0,
                            self.markdown_theme.clone(),
                            None,
                            Some(crate::tui::components::markdown::MarkdownOptions {
                                preserve_ordered_list_markers: false,
                                preserve_backslash_escapes: false,
                                transform: Some(transform),
                                render_latex: true,
                            }),
                        )),
                    );
                    i += 1;
                }
                AssistantContentBlock::Thinking { .. } => {
                    let mut thinking_blocks: Vec<String> = Vec::new();
                    while i < message.content.len() {
                        match &message.content[i] {
                            AssistantContentBlock::Thinking { thinking } => {
                                let thinking = thinking.trim();
                                if !thinking.is_empty() {
                                    thinking_blocks.push(thinking.to_string());
                                }
                                i += 1;
                            }
                            _ => break,
                        }
                    }
                    // upstream `i--` re-positions the loop onto the last
                    // consumed block; the outer `i += 1` below nets to i
                    // pointing past it.
                    i -= 1;

                    if thinking_blocks.is_empty() {
                        i += 1;
                        continue;
                    }

                    // Add spacing only when another visible content block
                    // follows.
                    let has_visible_content_after =
                        message.content[i + 1..].iter().any(|c| match c {
                            AssistantContentBlock::Text { text } => !text.trim().is_empty(),
                            AssistantContentBlock::Thinking { thinking } => {
                                !thinking.trim().is_empty()
                            }
                            AssistantContentBlock::ToolCall { .. } => false,
                        });

                    let run_index = thinking_run_index;
                    thinking_run_index += 1;
                    let toggle_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
                    self.thinking_toggle_flags
                        .push((run_index, Arc::clone(&toggle_flag)));
                    let hidden = self
                        .thinking_visibility_overrides
                        .iter()
                        .find(|(index, _)| *index == run_index)
                        .map(|(_, hidden)| *hidden)
                        .unwrap_or(self.hide_thinking_block);
                    let thinking_component: Box<dyn Component> = if hidden {
                        Box::new(Text::with_options(
                            &self.theme.italic(&theme_fg(
                                &self.theme,
                                "thinkingText",
                                &self.hidden_thinking_label,
                            )),
                            self.output_pad,
                            0,
                            None,
                        ))
                    } else {
                        let transform = create_markdown_transform(
                            "assistant-thinking",
                            self.is_streaming,
                            Arc::clone(&self.markdown_transformers),
                        );
                        Box::new(Markdown::new(
                            thinking_blocks.join("\n\n"),
                            self.output_pad,
                            0,
                            self.markdown_theme.clone(),
                            Some(DefaultTextStyle {
                                color: Some(theme_text_style(&self.theme, "thinkingText")),
                                italic: true,
                                ..DefaultTextStyle::default()
                            }),
                            Some(crate::tui::components::markdown::MarkdownOptions {
                                preserve_ordered_list_markers: false,
                                preserve_backslash_escapes: false,
                                transform: Some(transform),
                                render_latex: true,
                            }),
                        ))
                    };
                    self.content_container.add_child(
                        crate::tui::component_mouse::ComponentHandle::new(ThinkingRegion::new(
                            toggle_flag,
                            thinking_component,
                        )),
                    );
                    if has_visible_content_after {
                        self.content_container.add_child(
                            crate::tui::component_mouse::ComponentHandle::new(Spacer::new(1)),
                        );
                    }
                    i += 1;
                }
                _ => {
                    i += 1;
                }
            }
        }

        // Check if incomplete/failed - show after partial content.
        let has_tool_calls = message
            .content
            .iter()
            .any(|c| matches!(c, AssistantContentBlock::ToolCall { .. }));
        self.has_tool_calls = has_tool_calls;
        if message.stop_reason.as_deref() == Some("length") {
            self.content_container
                .add_child(crate::tui::component_mouse::ComponentHandle::new(
                    Spacer::new(1),
                ));
            self.content_container
                .add_child(crate::tui::component_mouse::ComponentHandle::new(
                    Text::with_options(
                        &theme_fg(
                            &self.theme,
                            "error",
                            "Response was truncated before completion.",
                        ),
                        self.output_pad,
                        0,
                        None,
                    ),
                ));
        } else if !has_tool_calls {
            if message.stop_reason.as_deref() == Some("aborted") {
                // Binding mirrors the upstream camelCase `errorMessage` field.
                #[allow(non_snake_case)]
                let abort_message = match &message.error_message {
                    Some(errorMessage)
                        if errorMessage != "Request was aborted" && !errorMessage.is_empty() =>
                    {
                        errorMessage.clone()
                    }
                    _ => "Operation aborted".to_string(),
                };
                self.content_container.add_child(
                    crate::tui::component_mouse::ComponentHandle::new(Spacer::new(1)),
                );
                self.content_container.add_child(
                    crate::tui::component_mouse::ComponentHandle::new(Text::with_options(
                        &theme_fg(&self.theme, "error", &abort_message),
                        self.output_pad,
                        0,
                        None,
                    )),
                );
            } else if message.stop_reason.as_deref() == Some("error") {
                let error_msg = message
                    .error_message
                    .clone()
                    .unwrap_or_else(|| "Unknown error".to_string());
                self.content_container.add_child(
                    crate::tui::component_mouse::ComponentHandle::new(Spacer::new(1)),
                );
                self.content_container.add_child(
                    crate::tui::component_mouse::ComponentHandle::new(Text::with_options(
                        &theme_fg(&self.theme, "error", &format!("Error: {error_msg}")),
                        self.output_pad,
                        0,
                        None,
                    )),
                );
            }
        }
    }

    pub fn has_tool_calls(&self) -> bool {
        self.has_tool_calls
    }
}

/// Upstream wraps the thinking component in a `MouseRegion` whose click flips
/// `thinkingVisibilityOverrides[runIndex]` and re-runs `updateContent`. The
/// port records the click into a shared flag; the component applies it when
/// [`AssistantMessageComponent::handle_mouse`] observes the event (same
/// observable sequence — see S19.6 in `components/mod.rs`).
struct ThinkingRegion {
    child: Box<dyn Component>,
    toggle_flag: Arc<std::sync::atomic::AtomicBool>,
}

impl ThinkingRegion {
    fn new(toggle_flag: Arc<std::sync::atomic::AtomicBool>, child: Box<dyn Component>) -> Self {
        Self { child, toggle_flag }
    }
}

impl Component for ThinkingRegion {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.child.render(width)
    }
    fn invalidate(&mut self) {
        self.child.invalidate();
    }
    fn handle_mouse(&mut self, event: &TuiMouseEvent) -> Option<TuiMouseEventResult> {
        if event.event_type == TuiMouseEventType::Click && event.button == TuiMouseButton::Left {
            self.toggle_flag
                .store(true, std::sync::atomic::Ordering::SeqCst);
            return Some(TuiMouseEventResult {
                handled: true,
                ..TuiMouseEventResult::default()
            });
        }
        None
    }
}

impl Component for AssistantMessageComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        let mut lines = self.content_container.render(width);
        if self.has_tool_calls || lines.is_empty() {
            return lines;
        }
        lines[0] = format!("{OSC133_ZONE_START}{}", lines[0]);
        let last = lines.len() - 1;
        lines[last] = format!("{OSC133_ZONE_END}{OSC133_ZONE_FINAL}{}", lines[last]);
        lines
    }

    fn invalidate(&mut self) {
        self.content_container.invalidate();
        if let Some(message) = self.last_message.clone() {
            self.update_content(message, self.is_streaming);
        }
    }

    fn handle_mouse(&mut self, event: &TuiMouseEvent) -> Option<TuiMouseEventResult> {
        let result = self.content_container.handle_mouse(event);
        // Apply any thinking-visibility toggle recorded during delegation.
        // (Upstream's MouseRegion handler mutates the override map and calls
        // updateContent synchronously inside the same event dispatch.)
        let mut toggled = false;
        for (run_index, flag) in &self.thinking_toggle_flags {
            if flag.swap(false, std::sync::atomic::Ordering::SeqCst) {
                let current = self
                    .thinking_visibility_overrides
                    .iter()
                    .find(|(index, _)| index == run_index)
                    .map(|(_, hidden)| *hidden)
                    .unwrap_or(self.hide_thinking_block);
                match self
                    .thinking_visibility_overrides
                    .iter_mut()
                    .find(|(index, _)| index == run_index)
                {
                    Some(entry) => entry.1 = !current,
                    None => self
                        .thinking_visibility_overrides
                        .push((*run_index, !current)),
                }
                toggled = true;
            }
        }
        if toggled {
            if let Some(message) = self.last_message.clone() {
                self.update_content(message, self.is_streaming);
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding_agent::modes::interactive::theme::{load_builtin_theme, ColorMode};

    fn dark() -> Arc<Theme> {
        Arc::new(load_builtin_theme("dark", Some(ColorMode::Truecolor)).expect("dark"))
    }

    fn message(
        content: Vec<AssistantContentBlock>,
        stop_reason: Option<&str>,
        error: Option<&str>,
    ) -> AssistantMessage {
        AssistantMessage {
            content,
            stop_reason: stop_reason.map(str::to_string),
            error_message: error.map(str::to_string),
        }
    }

    fn describe(component: &mut AssistantMessageComponent) -> Vec<String> {
        // The container children are opaque handles; describe them by rendering
        // the container and joining (structure asserted via rendered bytes).
        // Wide enough that oracle-captured child lines ("Response was
        // truncated before completion." + padding = 43 columns) don't truncate.
        component.content_container.render(60)
    }

    /// Oracle scenario `assistant_message` (textOnly / thinkingThenText /
    /// hiddenThinking / lengthStop / aborted / abortedCustom / errored / blank
    /// / streaming / zoneProbe / zoneProbeTool / zoneProbeEmpty).
    #[test]
    fn assistant_message_matches_oracle() {
        let theme = dark();
        let mut text_only = AssistantMessageComponent::new(
            theme.clone(),
            Some(message(
                vec![AssistantContentBlock::Text {
                    text: "  hello world  ".to_string(),
                }],
                None,
                None,
            )),
            false,
            None,
            "Thinking...",
            1,
            Vec::new(),
        );
        assert!(describe(&mut text_only)
            .iter()
            .any(|l| l.contains("hello world")));

        let mut thinking_then_text = AssistantMessageComponent::new(
            theme.clone(),
            Some(message(
                vec![
                    AssistantContentBlock::Thinking {
                        thinking: " step one ".to_string(),
                    },
                    AssistantContentBlock::Thinking {
                        thinking: "step two".to_string(),
                    },
                    AssistantContentBlock::Text {
                        text: "answer".to_string(),
                    },
                ],
                None,
                None,
            )),
            false,
            None,
            "Thinking...",
            1,
            Vec::new(),
        );
        let rendered = describe(&mut thinking_then_text);
        assert_eq!(
            rendered.first().map(String::as_str),
            Some(""),
            "leading spacer"
        );

        let mut hidden = AssistantMessageComponent::new(
            theme.clone(),
            Some(message(
                vec![
                    AssistantContentBlock::Thinking {
                        thinking: "secret".to_string(),
                    },
                    AssistantContentBlock::Text {
                        text: "visible".to_string(),
                    },
                ],
                None,
                None,
            )),
            false,
            None,
            "Thinking...",
            1,
            Vec::new(),
        );
        hidden.set_hide_thinking_block(true);
        let rendered = describe(&mut hidden).join(
            "
",
        );
        assert!(rendered.contains("[3m[38;2;150;160;164mThinking...[39m[23m"));

        let mut tool_calls = AssistantMessageComponent::new(
            theme.clone(),
            Some(message(
                vec![
                    AssistantContentBlock::Text {
                        text: "working".to_string(),
                    },
                    AssistantContentBlock::ToolCall {
                        id: "t1".to_string(),
                    },
                ],
                None,
                None,
            )),
            false,
            None,
            "Thinking...",
            1,
            Vec::new(),
        );
        assert!(tool_calls.has_tool_calls());
        // render: hasToolCalls suppresses the OSC-133 zone wrapping
        let rendered = tool_calls.render(40);
        assert!(rendered.iter().all(|l| !l.starts_with("]133;A")));

        let mut len_stop = AssistantMessageComponent::new(
            theme.clone(),
            Some(message(
                vec![AssistantContentBlock::Text {
                    text: "partial".to_string(),
                }],
                Some("length"),
                None,
            )),
            false,
            None,
            "Thinking...",
            1,
            Vec::new(),
        );
        let rendered = describe(&mut len_stop).join(
            "
",
        );
        assert!(
            rendered.contains("[38;2;234;127;129mResponse was truncated before completion.[39m")
        );

        let mut aborted = AssistantMessageComponent::new(
            theme.clone(),
            Some(message(
                vec![AssistantContentBlock::Text {
                    text: "p".to_string(),
                }],
                Some("aborted"),
                None,
            )),
            false,
            None,
            "Thinking...",
            1,
            Vec::new(),
        );
        assert!(describe(&mut aborted)
            .join(
                "
"
            )
            .contains("Operation aborted"));

        let mut aborted_custom = AssistantMessageComponent::new(
            theme.clone(),
            Some(message(
                vec![AssistantContentBlock::Text {
                    text: "p".to_string(),
                }],
                Some("aborted"),
                Some("user hit the button"),
            )),
            false,
            None,
            "Thinking...",
            1,
            Vec::new(),
        );
        assert!(describe(&mut aborted_custom)
            .join(
                "
"
            )
            .contains("user hit the button"));

        let mut errored = AssistantMessageComponent::new(
            theme.clone(),
            Some(message(
                vec![AssistantContentBlock::Text {
                    text: "p".to_string(),
                }],
                Some("error"),
                Some("boom"),
            )),
            false,
            None,
            "Thinking...",
            1,
            Vec::new(),
        );
        assert!(describe(&mut errored)
            .join(
                "
"
            )
            .contains("Error: boom"));

        let mut blank = AssistantMessageComponent::new(
            theme.clone(),
            Some(message(
                vec![
                    AssistantContentBlock::Text {
                        text: "   ".to_string(),
                    },
                    AssistantContentBlock::Thinking {
                        thinking: "".to_string(),
                    },
                ],
                None,
                None,
            )),
            false,
            None,
            "Thinking...",
            1,
            Vec::new(),
        );
        assert!(
            describe(&mut blank).is_empty(),
            "no visible content → no spacer"
        );

        // zone probe: text-only render wraps zones; empty render is empty
        let mut probe = AssistantMessageComponent::new(
            theme.clone(),
            Some(message(
                vec![AssistantContentBlock::Text {
                    text: "probe".to_string(),
                }],
                None,
                None,
            )),
            false,
            None,
            "Thinking...",
            1,
            Vec::new(),
        );
        let rendered = probe.render(40);
        assert!(rendered[0].starts_with("]133;A"));
        assert!(rendered[rendered.len() - 1].starts_with("]133;B]133;C"));

        let empty =
            AssistantMessageComponent::new(theme, None, false, None, "Thinking...", 1, Vec::new());
        let mut empty = empty;
        assert!(empty.render(40).is_empty());
    }
}
