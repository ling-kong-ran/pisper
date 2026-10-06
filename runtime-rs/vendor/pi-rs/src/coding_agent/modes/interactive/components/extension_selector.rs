//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Port of `modes/interactive/components/extension-selector.ts` (112 lines,
//! sha256 `6df190e74f64dfd6edfec9f10c31afa302bd8341cba7484c4143e1501729edc0`):
//! generic string-option selector for extensions with keyboard navigation and
//! an optional countdown (same explicit-tick seam as
//! [`super::extension_input`]).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use crate::coding_agent::modes::interactive::components::countdown_timer::{
    CountdownCallbacks, CountdownTimer,
};
use crate::coding_agent::modes::interactive::components::model_selector::{
    key_hint, keybindings_match, raw_key_hint, theme_fg,
};
use crate::coding_agent::modes::interactive::components::support::Border;
use crate::coding_agent::modes::interactive::theme::Theme;
use crate::tui::component::Component;
use crate::tui::component_mouse::ComponentHandle;
use crate::tui::components::container::Container;
use crate::tui::components::layout_widgets::Spacer;
use crate::tui::components::text::Text;
use crate::tui::keybindings::with_keybindings;

/// Upstream `ExtensionSelectorOptions` (the toggle hook + timer surface +
/// the delta `description`).
pub struct ExtensionSelectorCallbacks {
    pub on_select: Box<dyn FnMut(&str)>,
    pub on_cancel: Box<dyn FnMut()>,
    pub on_toggle_tools_expanded: Option<Box<dyn FnMut()>>,
    /// Descriptive line under the title (upstream `opts.description`).
    pub description: Option<String>,
}

/// Upstream `ExtensionSelectorComponent`.
pub struct ExtensionSelectorComponent {
    children: Vec<ComponentHandle>,
    options: Vec<String>,
    selected_index: usize,
    list_container: Container,
    title_text: String,
    /// Upstream `opts.description` (styled at rebuild time).
    description_text: Option<String>,
    // Upstream resets the header title from this after countdown ticks; the
    // port re-derives it via the pending_title seam — wired into the
    // interactive shell in r19+.
    #[allow(dead_code)]
    base_title: String,
    countdown: Option<CountdownTimer>,
    pending_title: Rc<RefCell<Option<String>>>,
    pending_expire: Rc<RefCell<bool>>,
    callbacks: ExtensionSelectorCallbacks,
    theme: Arc<Theme>,
}

impl ExtensionSelectorComponent {
    /// Upstream constructor.
    pub fn new(
        theme: Arc<Theme>,
        title: &str,
        options: Vec<String>,
        callbacks: ExtensionSelectorCallbacks,
        timeout_ms: Option<u64>,
    ) -> Self {
        let pending_title = Rc::new(RefCell::new(None));
        let pending_expire = Rc::new(RefCell::new(false));
        let countdown = timeout_ms.filter(|ms| *ms > 0).map(|timeout_ms| {
            CountdownTimer::new(
                timeout_ms,
                CountdownCallbacks {
                    on_tick: {
                        let pending_title = Rc::clone(&pending_title);
                        let base_title = title.to_string();
                        let tick_theme = Arc::clone(&theme);
                        Box::new(move |seconds| {
                            *pending_title.borrow_mut() = Some(theme_fg(
                                &tick_theme,
                                "accent",
                                &tick_theme.bold(&format!("{base_title} ({seconds}s)")),
                            ));
                        })
                    },
                    on_expire: {
                        let pending_expire = Rc::clone(&pending_expire);
                        Box::new(move || *pending_expire.borrow_mut() = true)
                    },
                },
            )
        });
        let mut component = Self {
            children: Vec::new(),
            options,
            selected_index: 0,
            list_container: Container::default(),
            title_text: theme_fg(&theme, "accent", &theme.bold(title)),
            description_text: callbacks
                .description
                .as_deref()
                .map(|description| theme_fg(&theme, "text", description)),
            base_title: title.to_string(),
            countdown,
            pending_title,
            pending_expire,
            callbacks,
            theme,
        };
        component.rebuild();
        component
    }

    // Upstream builds the child list incrementally; keep the push-per-child
    // shape.
    #[allow(clippy::vec_init_then_push)]
    fn rebuild(&mut self) {
        let mut children: Vec<ComponentHandle> = Vec::new();
        children.push(ComponentHandle::new(Border::new(None)));
        children.push(ComponentHandle::new(Spacer::new(1)));
        children.push(ComponentHandle::new(Text::with_options(
            &self.title_text,
            1,
            0,
            None,
        )));
        // Upstream `if (opts?.description) { spacer; text(theme.fg("text", opts.description)); }`
        if let Some(description_text) = &self.description_text {
            children.push(ComponentHandle::new(Spacer::new(1)));
            children.push(ComponentHandle::new(Text::with_options(
                description_text,
                1,
                0,
                None,
            )));
        }
        children.push(ComponentHandle::new(Spacer::new(1)));
        children.push(ComponentHandle::new(SlotAdapter));
        children.push(ComponentHandle::new(Spacer::new(1)));
        children.push(ComponentHandle::new(Text::with_options(
            &(raw_key_hint(&self.theme, "↑↓", "navigate")
                + "  "
                + &key_hint(&self.theme, "tui.select.confirm", "select")
                + "  "
                + &key_hint(&self.theme, "tui.select.cancel", "cancel")),
            1,
            0,
            None,
        )));
        children.push(ComponentHandle::new(Spacer::new(1)));
        children.push(ComponentHandle::new(Border::new(None)));
        self.children = children;
        self.update_list();
    }

    fn update_list(&mut self) {
        self.list_container = Container::default();
        for (i, option) in self.options.iter().enumerate() {
            let is_selected = i == self.selected_index;
            let text = if is_selected {
                theme_fg(&self.theme, "accent", "→ ") + &theme_fg(&self.theme, "accent", option)
            } else {
                format!("  {}", theme_fg(&self.theme, "text", option))
            };
            self.list_container
                .add_child(ComponentHandle::new(Text::with_options(&text, 1, 0, None)));
        }
    }

    /// The selected row (the radius login shimmer needs it).
    pub fn selected_index(&self) -> usize {
        self.selected_index
    }

    /// Drive one countdown second (title update + expiry cancel).
    pub fn tick_countdown(&mut self) {
        if let Some(countdown) = self.countdown.as_mut() {
            countdown.tick();
            if let Some(title) = self.pending_title.borrow_mut().take() {
                self.title_text = title;
            }
            if !countdown.is_active() {
                self.countdown = None;
                if *self.pending_expire.borrow_mut() {
                    (self.callbacks.on_cancel)();
                }
            }
        }
    }

    /// Upstream `handleInput`.
    pub fn handle_input(&mut self, key_data: &str) {
        if keybindings_match(key_data, "app.tools.expand") {
            if let Some(toggle) = &mut self.callbacks.on_toggle_tools_expanded {
                toggle();
            }
        } else if with_keybindings(|kb| kb.matches(key_data, "tui.select.up")) || key_data == "k" {
            self.selected_index = self.selected_index.saturating_sub(1);
            self.update_list();
        } else if with_keybindings(|kb| kb.matches(key_data, "tui.select.down")) || key_data == "j"
        {
            self.selected_index =
                (self.selected_index + 1).min(self.options.len().saturating_sub(1));
            self.update_list();
        } else if with_keybindings(|kb| kb.matches(key_data, "tui.select.confirm"))
            || key_data == "\n"
        {
            if let Some(selected) = self.options.get(self.selected_index) {
                let selected = selected.clone();
                (self.callbacks.on_select)(&selected);
            }
        } else if with_keybindings(|kb| kb.matches(key_data, "tui.select.cancel")) {
            (self.callbacks.on_cancel)();
        }
    }

    /// Upstream `dispose`.
    pub fn dispose(&mut self) {
        self.countdown = None;
    }

    pub fn title_text(&self) -> &str {
        &self.title_text
    }
}

struct SlotAdapter;
impl Component for SlotAdapter {
    fn render(&mut self, _width: usize) -> Vec<String> {
        Vec::new()
    }
}

impl Component for ExtensionSelectorComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        let mut lines = Vec::new();
        for child in &mut self.children {
            lines.extend(child.render(width));
        }
        lines.extend(self.list_container.render(width));
        lines
    }

    fn handle_input(&mut self, data: &str) {
        self.handle_input(data);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    fn dark() -> Arc<Theme> {
        Arc::new(
            crate::coding_agent::modes::interactive::theme::load_builtin_theme(
                "dark",
                Some(crate::coding_agent::modes::interactive::theme::ColorMode::Truecolor),
            )
            .expect("dark"),
        )
    }

    /// Oracle scenario `extension_components` (selector half): navigation,
    /// confirm/cancel, the expand toggle and the countdown title.
    #[test]
    fn extension_selector_matches_oracle() {
        let submitted: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let cancelled: Rc<RefCell<Vec<&'static str>>> = Rc::new(RefCell::new(Vec::new()));
        let toggles = Rc::new(Cell::new(0usize));
        let submitted_handle = Rc::clone(&submitted);
        let cancelled_handle = Rc::clone(&cancelled);
        let toggles_handle = Rc::clone(&toggles);
        let mut selector = ExtensionSelectorComponent::new(
            dark(),
            "Choose",
            vec!["alpha".to_string(), "beta".to_string()],
            ExtensionSelectorCallbacks {
                on_select: Box::new(move |v| submitted_handle.borrow_mut().push(v.to_string())),
                on_cancel: Box::new(move || cancelled_handle.borrow_mut().push("selector")),
                on_toggle_tools_expanded: Some(Box::new(move || {
                    toggles_handle.set(toggles_handle.get() + 1)
                })),
                description: None,
            },
            Some(1500),
        );
        selector.handle_input("j");
        selector.handle_input("\x1b[B");
        selector.handle_input("k");
        selector.handle_input("\x1b[A");
        // index back at 0 after j/down/k/up → still "alpha"
        selector.handle_input("\x0f"); // ctrl+o → app.tools.expand
        assert_eq!(toggles.get(), 1);
        selector.handle_input("\r");
        assert_eq!(submitted.borrow().as_slice(), &["alpha".to_string()]);
        selector.handle_input("\x1b");
        assert_eq!(cancelled.borrow().as_slice(), &["selector"]);
        // countdown title (bold) after one tick
        let title_before = selector.title_text().to_string();
        selector.tick_countdown();
        assert_ne!(
            selector.title_text(),
            title_before,
            "countdown title shows (1s)"
        );
        selector.dispose();
    }
}
