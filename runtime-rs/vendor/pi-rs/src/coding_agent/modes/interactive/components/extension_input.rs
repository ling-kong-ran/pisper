//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Port of `modes/interactive/components/extension-input.ts` (87 lines, sha256
//! `47a0608c60e35bd1936f9d33be24986fa6d5698b141251670840c6e2407ad790`):
//! simple text input for extensions, with an optional countdown that renames
//! the title and cancels on expiry (upstream `CountdownTimer`).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use crate::coding_agent::modes::interactive::components::countdown_timer::{
    CountdownCallbacks, CountdownTimer,
};
use crate::coding_agent::modes::interactive::components::model_selector::{key_hint, theme_fg};
use crate::coding_agent::modes::interactive::components::support::Border;
use crate::coding_agent::modes::interactive::theme::Theme;
use crate::tui::component::Component;
use crate::tui::component_mouse::ComponentHandle;
use crate::tui::components::input::{Input, InputOptions};
use crate::tui::components::layout_widgets::Spacer;
use crate::tui::components::text::Text;
use crate::tui::keybindings::with_keybindings;

/// Upstream `ExtensionInputOptions` (the option surface the port threads
/// explicitly).
#[derive(Default)]
pub struct ExtensionInputOptions {
    /// Initial editor content (upstream `initialValue`).
    pub initial_value: Option<String>,
    /// Descriptive line under the title (upstream `description`).
    pub description: Option<String>,
}

/// Upstream `ExtensionInputComponent`.
pub struct ExtensionInputComponent {
    children: Vec<ComponentHandle>,
    input: Input,
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
    on_submit: Box<dyn FnMut(&str)>,
    on_cancel: Box<dyn FnMut()>,
    theme: Arc<Theme>,
}

impl ExtensionInputComponent {
    /// Upstream constructor (`opts` threaded as an explicit struct).
    pub fn new(
        theme: Arc<Theme>,
        title: &str,
        placeholder: Option<&str>,
        on_submit: Box<dyn FnMut(&str)>,
        on_cancel: Box<dyn FnMut()>,
        timeout_ms: Option<u64>,
    ) -> Self {
        Self::with_options(
            theme,
            title,
            placeholder,
            on_submit,
            on_cancel,
            timeout_ms,
            ExtensionInputOptions::default(),
        )
    }

    /// Upstream constructor with `opts` (`initialValue`, `description`).
    #[allow(clippy::too_many_arguments)]
    pub fn with_options(
        theme: Arc<Theme>,
        title: &str,
        placeholder: Option<&str>,
        on_submit: Box<dyn FnMut(&str)>,
        on_cancel: Box<dyn FnMut()>,
        timeout_ms: Option<u64>,
        options: ExtensionInputOptions,
    ) -> Self {
        let _ = placeholder;
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
                                &format!("{base_title} ({seconds}s)"),
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
        let mut input = Input::new(InputOptions::default());
        // Upstream `if (opts?.initialValue) this.input.setValue(opts.initialValue);`
        if let Some(initial_value) = options.initial_value.as_deref() {
            if !initial_value.is_empty() {
                input.set_value(initial_value);
            }
        }
        let mut component = Self {
            children: Vec::new(),
            input,
            title_text: theme_fg(&theme, "accent", title),
            description_text: options
                .description
                .map(|description| theme_fg(&theme, "text", &description)),
            base_title: title.to_string(),
            countdown,
            pending_title,
            pending_expire,
            on_submit,
            on_cancel,
            theme,
        };
        component.rebuild();
        component
    }

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
            &format!(
                "{}  {}",
                key_hint(&self.theme, "tui.select.confirm", "submit"),
                key_hint(&self.theme, "tui.select.cancel", "cancel")
            ),
            1,
            0,
            None,
        )));
        children.push(ComponentHandle::new(Spacer::new(1)));
        children.push(ComponentHandle::new(Border::new(None)));
        self.children = children;
    }

    /// Drive one countdown second; the pending title (upstream
    /// `titleText.setText(...)`) and the expiry cancel apply within the same
    /// call.
    pub fn tick_countdown(&mut self) {
        if let Some(countdown) = self.countdown.as_mut() {
            countdown.tick();
            if let Some(title) = self.pending_title.borrow_mut().take() {
                self.title_text = title;
            }
            if !countdown.is_active() {
                self.countdown = None;
                if *self.pending_expire.borrow_mut() {
                    (self.on_cancel)();
                }
            }
        }
    }

    /// Upstream `handleInput`.
    pub fn handle_input(&mut self, key_data: &str) {
        if with_keybindings(|kb| kb.matches(key_data, "tui.select.confirm")) || key_data == "\n" {
            let value = self.input.value().to_string();
            (self.on_submit)(&value);
        } else if with_keybindings(|kb| kb.matches(key_data, "tui.select.cancel")) {
            (self.on_cancel)();
        } else {
            self.input.handle_input(key_data);
        }
    }

    /// Upstream `dispose`.
    pub fn dispose(&mut self) {
        self.countdown = None;
    }

    pub fn title_text(&self) -> &str {
        &self.title_text
    }

    pub fn value(&self) -> &str {
        self.input.value()
    }

    pub fn dialog_lines(&mut self, width: usize) -> Vec<String> {
        let mut lines = Vec::new();
        for child in &mut self.children {
            lines.extend(child.render(width));
        }
        lines
    }
}

struct SlotAdapter;
impl Component for SlotAdapter {
    fn render(&mut self, _width: usize) -> Vec<String> {
        Vec::new()
    }
}

impl Component for ExtensionInputComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.dialog_lines(width)
    }

    fn handle_input(&mut self, data: &str) {
        self.handle_input(data);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
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

    /// Oracle scenario `extension_components` (input half): countdown title
    /// updates, submit/cancel key routing.
    #[test]
    fn extension_input_matches_oracle() {
        let submitted: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let cancelled: Rc<RefCell<Vec<&'static str>>> = Rc::new(RefCell::new(Vec::new()));
        let submitted_handle = Rc::clone(&submitted);
        let cancelled_handle = Rc::clone(&cancelled);
        let mut input = ExtensionInputComponent::new(
            dark(),
            "Pick one",
            None,
            Box::new(move |v| submitted_handle.borrow_mut().push(v.to_string())),
            Box::new(move || cancelled_handle.borrow_mut().push("input")),
            Some(2500),
        );
        input.tick_countdown();
        assert_eq!(
            input.title_text(),
            "\x1b[38;2;167;152;215mPick one (2s)\x1b[39m"
        );
        input.handle_input("h");
        input.handle_input("i");
        input.handle_input("\r");
        assert_eq!(submitted.borrow().as_slice(), &["hi".to_string()]);
        input.tick_countdown();
        assert_eq!(
            input.title_text(),
            "\x1b[38;2;167;152;215mPick one (1s)\x1b[39m"
        );
        input.tick_countdown();
        assert_eq!(
            cancelled.borrow().as_slice(),
            &["input"],
            "countdown expired"
        );
        input.dispose();
    }
}
