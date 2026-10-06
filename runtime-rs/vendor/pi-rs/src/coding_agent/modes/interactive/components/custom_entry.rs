//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Port of `modes/interactive/components/custom-entry.ts` (62 lines, sha256
//! `8796bd95c4b496aad74674bbfb0e286768731532ad5ab8f51e6471cfbfd9404e`) —
//! renders a custom session entry from extensions; the host owns transcript
//! spacing, so renderer output provides only its content.

use std::sync::Arc;

use crate::coding_agent::modes::interactive::components::model_selector::theme_fg;
use crate::coding_agent::modes::interactive::components::support::MessageBgBox;
use crate::coding_agent::modes::interactive::theme::Theme;
use crate::tui::component::Component;
use crate::tui::components::layout_widgets::Spacer;
use crate::tui::components::text::Text;

/// Upstream `CustomEntry` payload used by the component.
#[derive(Clone, Debug, Default)]
pub struct CustomEntryPayload {
    pub custom_type: String,
}

/// Upstream `EntryRenderer` (`core/extensions/types.ts`): returns a component
/// or nothing; a panicking renderer falls back to the error box (upstream
/// `catch`).
pub type EntryRenderer =
    Box<dyn FnMut(&CustomEntryPayload, EntryRendererOptions, &Theme) -> Option<Box<dyn Component>>>;

/// Upstream renderer options (`{ expanded }`).
#[derive(Clone, Copy, Debug)]
pub struct EntryRendererOptions {
    pub expanded: bool,
}

/// Upstream `CustomEntryComponent`.
pub struct CustomEntryComponent {
    children: Vec<Box<dyn Component>>,
    entry: CustomEntryPayload,
    renderer: EntryRenderer,
    has_content: bool,
    expanded: bool,
    theme: Arc<Theme>,
}

impl CustomEntryComponent {
    /// Upstream constructor.
    pub fn new(theme: Arc<Theme>, entry: CustomEntryPayload, renderer: EntryRenderer) -> Self {
        let mut component = Self {
            children: Vec::new(),
            entry,
            renderer,
            has_content: false,
            expanded: false,
            theme,
        };
        component.rebuild();
        component
    }

    /// Upstream `hasContent`.
    pub fn has_content(&self) -> bool {
        self.has_content
    }

    /// Upstream `setExpanded`.
    pub fn set_expanded(&mut self, expanded: bool) {
        if self.expanded != expanded {
            self.expanded = expanded;
            self.rebuild();
        }
    }

    fn rebuild(&mut self) {
        self.children.clear();
        self.has_content = false;

        let attempt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            (self.renderer)(
                &self.entry,
                EntryRendererOptions {
                    expanded: self.expanded,
                },
                &self.theme,
            )
        }));
        let component = match attempt {
            Ok(component) => component,
            Err(error) => {
                let message = error
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| error.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_else(|| "Box<dyn Any>".to_string());
                let mut box_ = MessageBgBox::new(
                    1,
                    1,
                    Some({
                        let bg_theme = Arc::clone(&self.theme);
                        Arc::new(move |t: &str| {
                            bg_theme.bg("customMessageBg", t).expect("customMessageBg")
                        })
                    }),
                );
                let text = theme_fg(
                    &self.theme,
                    "error",
                    &format!("[{}] renderer failed: {}", self.entry.custom_type, message),
                );
                box_.add_child(Box::new(Text::with_options(&text, 0, 0, None)));
                Some(Box::new(box_) as Box<dyn Component>)
            }
        };

        let Some(component) = component else {
            return;
        };
        self.has_content = true;
        self.children.push(Box::new(Spacer::new(1)));
        self.children.push(component);
    }
}

impl Component for CustomEntryComponent {
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

    fn entry(custom_type: &str) -> CustomEntryPayload {
        CustomEntryPayload {
            custom_type: custom_type.to_string(),
        }
    }

    /// Oracle scenario `message_components` (`entryOk`, `entryHasContent`,
    /// `entryEmpty`, `entryThrow`).
    #[test]
    fn custom_entry_matches_oracle() {
        let theme = dark();
        let mut ok = CustomEntryComponent::new(
            theme.clone(),
            entry("ext"),
            Box::new(|_entry, _options, _theme| {
                Some(Box::new(Text::with_options("entry body", 0, 0, None)) as Box<dyn Component>)
            }),
        );
        assert!(ok.has_content());
        let rendered = ok.render(40).join("\n");
        assert!(rendered.contains("entry body"));
        ok.set_expanded(true);
        assert!(ok.render(40).join("\n").contains("entry body"));

        let mut empty = CustomEntryComponent::new(
            theme.clone(),
            entry("ext"),
            Box::new(|_entry, _options, _theme| None),
        );
        assert!(!empty.has_content());
        assert!(empty.render(40).is_empty());

        let mut failing = CustomEntryComponent::new(
            theme,
            entry("bad"),
            Box::new(|_entry, _options, _theme| panic!("entry failed")),
        );
        assert!(failing.has_content(), "error box counts as content");
        let rendered = failing.render(40).join("\n");
        assert!(rendered.contains("[bad] renderer failed: entry failed"));
    }
}
