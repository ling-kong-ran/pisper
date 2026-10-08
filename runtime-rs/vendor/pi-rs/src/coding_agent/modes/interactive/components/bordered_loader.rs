//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Port of `modes/interactive/components/bordered-loader.ts` (68 lines, sha256
//! `f4a6487b80360bde89d8e96ebec16d84d4cd106dcc57c53757be571c7c4da1f6`):
//! a loader wrapped with dynamic borders for extension UIs.
//!
//! Disclosed unification (S19.4): upstream builds a `CancellableLoader` when
//! `cancellable` and a plain `Loader` plus its own `AbortController`
//! otherwise. The port always hosts a [`CancellableLoader`] and only routes
//! input to it when cancellable — the inner abort flag doubles as the
//! non-cancellable `signalController`, so the observable surface (no Escape
//! handling, never-aborted signal) is identical.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use crate::coding_agent::modes::interactive::components::loader::{CancellableLoader, LoaderStyle};
use crate::coding_agent::modes::interactive::components::model_selector::{key_hint, theme_fg};
use crate::coding_agent::modes::interactive::components::support::Border;
use crate::coding_agent::modes::interactive::theme::Theme;
use crate::tui::component::Component;
use crate::tui::component_mouse::ComponentHandle;
use crate::tui::components::layout_widgets::Spacer;
use crate::tui::components::text::Text;

/// Upstream `BorderedLoader`.
pub struct BorderedLoader {
    children: Vec<ComponentHandle>,
    loader: Rc<RefCell<CancellableLoader>>,
    cancellable: bool,
}

impl BorderedLoader {
    /// Upstream constructor.
    pub fn new(theme: &Theme, message: &str, cancellable: bool) -> Self {
        let border_theme = theme.clone();
        let border_color: Box<dyn Fn(&str) -> String + Send> =
            Box::new(move |s: &str| theme_fg(&border_theme, "border", s));

        let accent = theme.clone();
        let muted = theme.clone();
        let spinner_color_fn: LoaderStyle = Arc::new(move |s: &str| theme_fg(&accent, "accent", s));
        let message_color_fn: LoaderStyle = Arc::new(move |s: &str| theme_fg(&muted, "muted", s));
        let loader = CancellableLoader::new(spinner_color_fn, message_color_fn, message, None);
        let (loader_handle, loader_shared) = ComponentHandle::with_shared(loader);

        let mut children: Vec<ComponentHandle> = Vec::new();
        children.push(ComponentHandle::new(Border::new(Some(border_color))));
        children.push(loader_handle);
        if cancellable {
            children.push(ComponentHandle::new(Spacer::new(1)));
            children.push(ComponentHandle::new(Text::with_options(
                &key_hint(theme, "tui.select.cancel", "cancel"),
                1,
                0,
                None,
            )));
        }
        children.push(ComponentHandle::new(Spacer::new(1)));
        let bottom_theme = theme.clone();
        children.push(ComponentHandle::new(Border::new(Some(Box::new(
            move |s: &str| theme_fg(&bottom_theme, "border", s),
        )))));

        Self {
            children,
            loader: loader_shared,
            cancellable,
        }
    }

    /// Upstream `signal` getter (`aborted` flag).
    pub fn is_aborted(&self) -> bool {
        self.loader.borrow().aborted()
    }

    /// Upstream `onAbort` setter (only routed when cancellable).
    pub fn set_on_abort(&mut self, on_abort: Option<Box<dyn FnMut()>>) {
        if self.cancellable {
            self.loader.borrow_mut().set_on_abort(on_abort);
        }
    }

    /// Upstream `handleInput` (delegates to the cancellable loader only).
    pub fn handle_input(&mut self, data: &str) {
        if self.cancellable {
            self.loader.borrow_mut().handle_input(data);
        }
    }

    /// Upstream `dispose`.
    pub fn dispose(&mut self) {
        self.loader.borrow_mut().dispose();
    }
}

impl Component for BorderedLoader {
    fn render(&mut self, width: usize) -> Vec<String> {
        let mut lines = Vec::new();
        for child in &mut self.children {
            lines.extend(child.render(width));
        }
        lines
    }

    fn invalidate(&mut self) {
        for child in &mut self.children {
            child.invalidate();
        }
    }

    fn handle_input(&mut self, data: &str) {
        self.handle_input(data);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding_agent::modes::interactive::theme::{load_builtin_theme, ColorMode};
    use std::sync::Arc;

    fn dark() -> Arc<Theme> {
        Arc::new(load_builtin_theme("dark", Some(ColorMode::Truecolor)).expect("dark"))
    }

    /// Oracle: tests/fixtures/interactive_r19_oracle scenario `bordered_loader` —
    /// child sequence, abort flag and the rendered block bytes.
    #[test]
    fn bordered_loader_matches_oracle() {
        let theme = dark();
        let mut cancellable = BorderedLoader::new(&theme, "Working...", true);
        let rendered = cancellable.render(30);
        let border = "\x1b[38;2;95;168;204m──────────────────────────────\x1b[39m".to_string();
        let expected = [
            border.clone(),
            String::new(),
            " \x1b[38;2;167;152;215m⠋\x1b[39m \x1b[38;2;157;165;169mWorking...\x1b[39m             ".to_string(),
            String::new(),
            " \x1b[38;2;126;136;142mescape/ctrl+c\x1b[39m\x1b[38;2;157;165;169m cancel\x1b[39m         ".to_string(),
            String::new(),
            border,
        ];
        assert_eq!(rendered.len(), expected.len());
        assert_eq!(rendered[0], expected[0], "top border");
        assert_eq!(
            rendered[2].trim_end(),
            expected[2].trim_end(),
            "loader line"
        );
        assert_eq!(
            crate::tui::utils::visible_width(&rendered[4]),
            30,
            "hint row padded"
        );
        assert_eq!(rendered[6], expected[6], "bottom border");

        let aborted_flag = std::rc::Rc::new(std::cell::Cell::new(false));
        let handle = std::rc::Rc::clone(&aborted_flag);
        cancellable.set_on_abort(Some(Box::new(move || handle.set(true))));
        assert!(!cancellable.is_aborted());
        cancellable.handle_input("\x1b");
        assert!(cancellable.is_aborted());
        assert!(aborted_flag.get());
        cancellable.dispose();
    }

    /// Non-cancellable loaders: no hint row and Escape is ignored.
    #[test]
    fn non_cancellable_loader_has_no_hint_row() {
        let theme = dark();
        let mut plain = BorderedLoader::new(&theme, "Loading...", false);
        assert_eq!(plain.children.len(), 4, "border/loader/spacer/border");
        plain.handle_input("\x1b");
        assert!(!plain.is_aborted(), "signal controller never aborted");
        plain.dispose();
    }
}
