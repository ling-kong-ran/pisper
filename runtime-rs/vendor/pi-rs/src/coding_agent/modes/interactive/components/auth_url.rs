//! Port of `modes/interactive/components/auth-url.ts` (v1.0.0): a sign-in URL
//! with a click hint and a copy hint. Hosts call [`AuthUrlComponent::copy`]
//! when `app.message.copy` is pressed, since a long URL wraps and often cannot
//! be selected or clicked as a whole (SSH, tmux).

use std::rc::Rc;
use std::sync::Arc;

use crate::coding_agent::modes::interactive::components::model_selector::{key_hint, theme_fg};
use crate::coding_agent::modes::interactive::theme::Theme;
use crate::coding_agent::utils::clipboard::copy_to_clipboard;
use crate::tui::component::Component;
use crate::tui::components::text::Text;
use crate::tui::terminal_image::hyperlink;

/// Upstream `AuthUrlComponent`. Like the other interactive components, it runs
/// on the UI thread (`Rc` request-render handle stands in for the upstream
/// `TUI`).
pub struct AuthUrlComponent {
    url_line: Text,
    hint: Text,
    url: String,
    theme: Arc<Theme>,
    request_render: Rc<dyn Fn()>,
}

impl AuthUrlComponent {
    pub fn new(theme: Arc<Theme>, url: &str, request_render: Rc<dyn Fn()>) -> Self {
        let mut component = AuthUrlComponent {
            url_line: Text::with_options(
                &theme_fg(&theme, "accent", &hyperlink(url, url)),
                1,
                0,
                None,
            ),
            hint: Text::with_options("", 1, 0, None),
            url: url.to_string(),
            theme,
            request_render,
        };
        component.set_hint(key_hint(&component.theme, "app.message.copy", "to copy"));
        component
    }

    /// The shown URL, which `app.message.copy` copies.
    pub fn url(&self) -> &str {
        &self.url
    }

    fn set_hint(&mut self, suffix: String) {
        let click_hint = if cfg!(target_os = "macos") {
            "Cmd+click to open"
        } else {
            "Ctrl+click to open"
        };
        let text = format!(
            "{} {} {}",
            theme_fg(&self.theme, "dim", &hyperlink(click_hint, &self.url)),
            theme_fg(&self.theme, "dim", "•"),
            suffix
        );
        self.hint.set_text(&text);
        (self.request_render)();
    }

    /// Upstream `copy()`.
    pub async fn copy(&mut self) {
        let result = copy_to_clipboard(&self.url).await;
        match result {
            Ok(()) => self.set_hint(theme_fg(&self.theme, "success", "Copied URL to clipboard")),
            Err(error) => self.set_hint(theme_fg(&self.theme, "error", &error.to_string())),
        }
    }
}

impl Component for AuthUrlComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        let mut lines = self.url_line.render(width);
        lines.extend(self.hint.render(width));
        lines
    }

    fn invalidate(&mut self) {
        self.url_line.invalidate();
        self.hint.invalidate();
    }
}
