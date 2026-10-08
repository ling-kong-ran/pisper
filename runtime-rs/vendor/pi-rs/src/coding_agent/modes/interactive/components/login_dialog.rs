//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Port of `modes/interactive/components/login-dialog.ts` (233 lines, sha256
//! `37608848ca7b72707437aed9627a612eadf570c57a54b1c86972c3532918298c`) —
//! the login dialog that replaces the editor during OAuth flows.
//!
//! Upstream's promise plumbing (`showManualInput`/`showPrompt` create a
//! promise resolved by the input's `onSubmit` and rejected by `cancel`)
//! becomes [`Self::take_pending_value`] — `None` while no request is open,
//! `Some(Ok(value))` after submit, `Some(Err("Login cancelled"))` after
//! cancel. The content sequences and message bytes are identical.
//! `openBrowser` is an injected hook (host seam); the title/content rows are
//! mounted through [`Self::content_children`].

use std::rc::Rc;
use std::sync::Arc;

use crate::coding_agent::modes::interactive::components::auth_url::AuthUrlComponent;
use crate::coding_agent::modes::interactive::components::model_selector::{key_hint, theme_fg};
use crate::coding_agent::modes::interactive::components::support::Border;
use crate::coding_agent::modes::interactive::theme::Theme;
use crate::tui::component::Component;
use crate::tui::component_mouse::ComponentHandle;
use crate::tui::components::input::{Input, InputOptions};
use crate::tui::components::layout_widgets::Spacer;
use crate::tui::components::text::Text;
use crate::tui::keybindings::with_keybindings;

/// Upstream `AuthInfoLink` slice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthInfoLink {
    pub label: Option<String>,
    pub url: String,
}

/// Upstream `OAuthDeviceCodeInfo` slice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OAuthDeviceCodeInfo {
    pub verification_uri: String,
    pub user_code: String,
}

/// Upstream `onComplete(success, message?)` callback.
pub type LoginCompleteCallback = Box<dyn FnMut(bool, Option<&str>)>;

/// Upstream `LoginDialogComponent`.
pub struct LoginDialogComponent {
    content_children: Vec<ComponentHandle>,
    /// The shown sign-in URL, which `app.message.copy` copies (v1.0.0).
    auth_url: Option<AuthUrlComponent>,
    /// Render handle the auth-URL component shares (upstream the `TUI`).
    auth_url_render: std::rc::Rc<dyn Fn()>,
    /// Shared input handle so submit can swap it for the submitted text row.
    input: ComponentHandle,
    input_shared: std::rc::Rc<std::cell::RefCell<Input>>,
    aborted: bool,
    request_open: bool,
    resolved_value: Option<Result<String, String>>,
    on_complete: LoginCompleteCallback,
    /// Shared so the auth-URL component can request renders too.
    request_render: std::rc::Rc<std::cell::RefCell<Box<dyn FnMut()>>>,
    open_browser: Option<Arc<dyn Fn(&str) + Send + Sync>>,
    theme: Arc<Theme>,
}

impl LoginDialogComponent {
    /// Upstream constructor.
    pub fn new(
        theme: Arc<Theme>,
        provider_id: &str,
        on_complete: LoginCompleteCallback,
        provider_name_override: Option<&str>,
        title_override: Option<&str>,
        request_render: Box<dyn FnMut()>,
        open_browser: Option<Arc<dyn Fn(&str) + Send + Sync>>,
    ) -> Self {
        let provider_name = provider_name_override.unwrap_or(provider_id);
        let title = title_override
            .map(str::to_string)
            .unwrap_or_else(|| format!("Login to {provider_name}"));

        let (input_handle, input_shared) =
            ComponentHandle::with_shared(Input::new(InputOptions::default()));
        let request_render: std::rc::Rc<std::cell::RefCell<Box<dyn FnMut()>>> =
            std::rc::Rc::new(std::cell::RefCell::new(request_render));
        let auth_url_render: std::rc::Rc<dyn Fn()> = {
            let request_render = std::rc::Rc::clone(&request_render);
            Rc::new(move || (request_render.borrow_mut())())
        };
        Self {
            content_children: Vec::new(),
            auth_url: None,
            auth_url_render,
            input: input_handle,
            input_shared,
            aborted: false,
            request_open: false,
            resolved_value: None,
            on_complete,
            request_render,
            open_browser,
            theme,
        }
        .with_frame_title(&title)
    }

    fn with_frame_title(mut self, title: &str) -> Self {
        // The frame mounts: top border / title / dynamic content area / input
        // slot / bottom border. The port keeps the title + input as content
        // children between the borders.
        let mut framed: Vec<ComponentHandle> = Vec::new();
        framed.push(ComponentHandle::new(Border::new(None)));
        framed.push(ComponentHandle::new(Text::with_options(
            &theme_fg(&self.theme, "accent", &self.theme.bold(title)),
            1,
            0,
            None,
        )));
        framed.append(&mut self.content_children);
        framed.push(ComponentHandle::new(self.input.clone()));
        framed.push(ComponentHandle::new(Border::new(None)));
        self.content_children = framed;
        self
    }

    /// Upstream `signal.aborted`.
    pub fn is_aborted(&self) -> bool {
        self.aborted
    }

    /// The input's registered submit hook (upstream `input.onSubmit`).
    pub fn submit_input(&mut self) {
        if self.request_open {
            let value = self.input_shared.borrow().value().to_string();
            self.replace_input_with_submitted_text(&value);
            self.request_open = false;
            self.resolved_value = Some(Ok(value));
        }
    }

    /// Upstream `replaceInputWithSubmittedText`.
    fn replace_input_with_submitted_text(&mut self, value: &str) {
        let input_id = self.input.id();
        for child in &mut self.content_children {
            if child.id() == input_id {
                *child =
                    ComponentHandle::new(Text::with_options(&format!("> {value}"), 0, 0, None));
                break;
            }
        }
    }

    /// Upstream `cancel`.
    pub fn cancel(&mut self) {
        self.aborted = true;
        if self.request_open {
            self.request_open = false;
            self.resolved_value = Some(Err("Login cancelled".to_string()));
        }
        (self.on_complete)(false, Some("Login cancelled"));
    }

    /// The open request's outcome (upstream promise resolution).
    pub fn take_pending_value(&mut self) -> Option<Result<String, String>> {
        self.resolved_value.take()
    }

    /// Upstream `showAuth`.
    pub fn show_auth(&mut self, url: &str, instructions: Option<&str>) {
        self.content_children.clear();
        self.content_children
            .push(ComponentHandle::new(Spacer::new(1)));
        self.auth_url = Some(AuthUrlComponent::new(
            Arc::clone(&self.theme),
            url,
            Rc::clone(&self.auth_url_render),
        ));
        if let Some(instructions) = instructions {
            self.content_children
                .push(ComponentHandle::new(Spacer::new(1)));
            self.content_children
                .push(ComponentHandle::new(Text::with_options(
                    &theme_fg(&self.theme, "warning", instructions),
                    1,
                    0,
                    None,
                )));
        }
        if let Some(open_browser) = &self.open_browser {
            open_browser(url);
        }
        (self.request_render.borrow_mut())();
    }

    /// Upstream `showDeviceCode`.
    pub fn show_device_code(&mut self, info: &OAuthDeviceCodeInfo) {
        self.auth_url = None;
        self.content_children.clear();
        self.content_children
            .push(ComponentHandle::new(Spacer::new(1)));
        let uri = &info.verification_uri;
        let linked_url = format!("\x1b]8;;{uri}\x07{uri}\x1b]8;;\x07");
        self.content_children
            .push(ComponentHandle::new(Text::with_options(
                &theme_fg(&self.theme, "accent", &linked_url),
                1,
                0,
                None,
            )));
        let click_hint = if cfg!(target_os = "macos") {
            "Cmd+click to open"
        } else {
            "Ctrl+click to open"
        };
        let hyperlink = format!("\x1b]8;;{uri}\x07{click_hint}\x1b]8;;\x07");
        self.content_children
            .push(ComponentHandle::new(Text::with_options(
                &theme_fg(&self.theme, "dim", &hyperlink),
                1,
                0,
                None,
            )));
        self.content_children
            .push(ComponentHandle::new(Spacer::new(1)));
        self.content_children
            .push(ComponentHandle::new(Text::with_options(
                &theme_fg(
                    &self.theme,
                    "warning",
                    &format!("Enter code: {}", info.user_code),
                ),
                1,
                0,
                None,
            )));
        (self.request_render.borrow_mut())();
    }

    /// Upstream `showManualInput`.
    pub fn show_manual_input(&mut self, prompt: &str) {
        self.input_shared.borrow_mut().set_value("");
        self.content_children
            .push(ComponentHandle::new(Spacer::new(1)));
        self.content_children
            .push(ComponentHandle::new(Text::with_options(
                &theme_fg(&self.theme, "dim", prompt),
                1,
                0,
                None,
            )));
        self.content_children.push(self.input.clone());
        self.content_children
            .push(ComponentHandle::new(Text::with_options(
                &format!(
                    "({})",
                    key_hint(&self.theme, "tui.select.cancel", "to cancel")
                ),
                1,
                0,
                None,
            )));
        (self.request_render.borrow_mut())();
        self.request_open = true;
    }

    /// Upstream `showPrompt` (appends; preserves the URL from showAuth).
    pub fn show_prompt(&mut self, message: &str, placeholder: Option<&str>) {
        self.content_children
            .push(ComponentHandle::new(Spacer::new(1)));
        self.content_children
            .push(ComponentHandle::new(Text::with_options(
                &theme_fg(&self.theme, "text", message),
                1,
                0,
                None,
            )));
        if let Some(placeholder) = placeholder {
            self.content_children
                .push(ComponentHandle::new(Text::with_options(
                    &theme_fg(&self.theme, "dim", &format!("e.g., {placeholder}")),
                    1,
                    0,
                    None,
                )));
        }
        self.content_children.push(self.input.clone());
        self.content_children
            .push(ComponentHandle::new(Text::with_options(
                &format!(
                    "({} {})",
                    key_hint(&self.theme, "tui.select.cancel", "to cancel,"),
                    key_hint(&self.theme, "tui.select.confirm", "to submit")
                ),
                1,
                0,
                None,
            )));
        self.input_shared.borrow_mut().set_value("");
        (self.request_render.borrow_mut())();
        self.request_open = true;
    }

    /// Upstream `showDetails`.
    pub fn show_details(&mut self, lines: &[&str]) {
        self.auth_url = None;
        self.content_children.clear();
        self.content_children
            .push(ComponentHandle::new(Spacer::new(1)));
        for line in lines {
            self.content_children
                .push(ComponentHandle::new(Text::with_options(line, 1, 0, None)));
        }
        (self.request_render.borrow_mut())();
    }

    /// Upstream `showInfo`.
    pub fn show_info(&mut self, message: &str, links: &[AuthInfoLink], show_close_hint: bool) {
        self.content_children
            .push(ComponentHandle::new(Spacer::new(1)));
        self.content_children
            .push(ComponentHandle::new(Text::with_options(
                &theme_fg(&self.theme, "text", message),
                1,
                0,
                None,
            )));
        for link in links {
            let text = match &link.label {
                Some(label) => format!("{label}: {}", link.url),
                None => link.url.clone(),
            };
            let url = &link.url;
            let hyperlink = format!("\x1b]8;;{url}\x07{text}\x1b]8;;\x07");
            self.content_children
                .push(ComponentHandle::new(Text::with_options(
                    &theme_fg(&self.theme, "accent", &hyperlink),
                    1,
                    0,
                    None,
                )));
        }
        if show_close_hint {
            self.content_children
                .push(ComponentHandle::new(Spacer::new(1)));
            self.content_children
                .push(ComponentHandle::new(Text::with_options(
                    &format!(
                        "({})",
                        key_hint(&self.theme, "tui.select.cancel", "to close")
                    ),
                    1,
                    0,
                    None,
                )));
        }
        (self.request_render.borrow_mut())();
    }

    /// Upstream `showWaiting`.
    pub fn show_waiting(&mut self, message: &str) {
        self.content_children
            .push(ComponentHandle::new(Spacer::new(1)));
        self.content_children
            .push(ComponentHandle::new(Text::with_options(
                &theme_fg(&self.theme, "dim", message),
                1,
                0,
                None,
            )));
        self.content_children
            .push(ComponentHandle::new(Text::with_options(
                &format!(
                    "({})",
                    key_hint(&self.theme, "tui.select.cancel", "to cancel")
                ),
                1,
                0,
                None,
            )));
        (self.request_render.borrow_mut())();
    }

    /// Upstream `showProgress`.
    pub fn show_progress(&mut self, message: &str) {
        self.content_children
            .push(ComponentHandle::new(Text::with_options(
                &theme_fg(&self.theme, "dim", message),
                1,
                0,
                None,
            )));
        (self.request_render.borrow_mut())();
    }

    /// Upstream `handleInput`.
    pub fn handle_input(&mut self, key_data: &str) {
        if with_keybindings(|kb| kb.matches(key_data, "tui.select.cancel")) {
            self.cancel();
            return;
        }
        if self.auth_url.is_some()
            && with_keybindings(|kb| kb.matches(key_data, "app.message.copy"))
        {
            if let Some(auth_url) = self.auth_url.as_mut() {
                // Upstream `void this.authUrl.copy()`; the clipboard write is
                // awaited inline like the shell's other copy paths.
                futures::executor::block_on(auth_url.copy());
            }
            return;
        }
        // Upstream: Enter fires the input's onSubmit; the input widget itself
        // handles that (vendored slice), so the port exposes [`Self::submit_input`].
        self.input_shared.borrow_mut().handle_input(key_data);
    }

    /// The dynamic content rows (title/borders excluded).
    pub fn content_children(&mut self, width: usize) -> Vec<String> {
        let mut lines = Vec::new();
        for child in &mut self.content_children {
            lines.extend(child.render(width));
        }
        if let Some(auth_url) = self.auth_url.as_mut() {
            lines.extend(auth_url.render(width));
        }
        lines
    }
}

impl Component for LoginDialogComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        let border = Border::new(None).0.render(width).join("");
        let mut lines = vec![border.clone()];
        lines.extend(self.content_children(width));
        lines.push(border);
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
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

    /// Oracle: tests/fixtures/interactive_r19_oracle scenario `login_dialog` —
    /// content sequences, promise resolution/cancellation, completion.
    #[test]
    fn login_dialog_matches_oracle() {
        let theme = dark();
        let completions: Rc<RefCell<Vec<(bool, Option<String>)>>> =
            Rc::new(RefCell::new(Vec::new()));
        let completions_handle = Rc::clone(&completions);
        let renders = Rc::new(Cell::new(0usize));
        let renders_handle = Rc::clone(&renders);
        let mut dialog = LoginDialogComponent::new(
            theme,
            "openai",
            Box::new(move |success, message| {
                completions_handle
                    .borrow_mut()
                    .push((success, message.map(str::to_string)));
            }),
            None,
            None,
            Box::new(move || renders_handle.set(renders_handle.get() + 1)),
            None,
        );

        dialog.show_auth("https://auth.example.com", Some("Paste the code below"));
        // showAuth clears the container; snapshot its rows before the next show.
        // v1.0.0 the auth URL renders through the `AuthUrlComponent`, whose
        // hyperlink helper terminates OSC 8 with ST instead of BEL.
        let auth_content = dialog.content_children(60).join("\n");
        assert!(
            auth_content.contains(
                "\x1b]8;;https://auth.example.com\x1b\\https://auth.example.com\x1b]8;;\x1b\\"
            ),
            "OSC8 hyperlink for the auth URL"
        );
        assert!(auth_content.contains("Ctrl+click to open"));
        assert!(auth_content.contains("to copy"));
        assert!(auth_content.contains("Paste the code below"));

        dialog.show_device_code(&OAuthDeviceCodeInfo {
            verification_uri: "https://device.example.com".to_string(),
            user_code: "ABCD-1234".to_string(),
        });
        let device_content = dialog.content_children(60).join("\n");
        assert!(device_content.contains("\x1b]8;;https://device.example.com\x07"));
        assert!(device_content.contains("Enter code: ABCD-1234"));

        dialog.show_details(&["line one", "line two"]);
        dialog.show_info(
            "info message",
            &[
                AuthInfoLink {
                    label: Some("docs".to_string()),
                    url: "https://docs.example.com".to_string(),
                },
                AuthInfoLink {
                    label: None,
                    url: "https://bare.example.com".to_string(),
                },
            ],
            true,
        );
        dialog.show_waiting("waiting...");
        dialog.show_progress("progressing");
        // final accumulated rows (showInfo/showWaiting/showProgress append);
        // oracle contentAfterShows child bytes survive trim_end of the padding
        let content = dialog
            .content_children(60)
            .iter()
            .map(|l| l.trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(content.contains("line one"));
        assert!(
            content.contains(
                "\x1b[38;2;167;152;215m\x1b]8;;https://docs.example.com\x07docs: https://docs.example.com\x1b]8;;\x07\x1b[39m"
            ),
            "accent OSC8 info link"
        );
        assert!(
            content.contains(
                "\x1b[38;2;167;152;215m\x1b]8;;https://bare.example.com\x07https://bare.example.com\x1b]8;;\x07\x1b[39m"
            ),
            "bare OSC8 info link"
        );
        assert!(
            content.contains("(\x1b[38;2;126;136;142mescape/ctrl+c\x1b[39m\x1b[38;2;157;165;169m to close\x1b[39m)"),
            "close hint"
        );
        assert!(content.contains("\x1b[38;2;126;136;142mwaiting...\x1b[39m"));
        assert!(
            content.contains("(\x1b[38;2;126;136;142mescape/ctrl+c\x1b[39m\x1b[38;2;157;165;169m to cancel\x1b[39m)"),
            "cancel hint"
        );
        assert!(content.contains("\x1b[38;2;126;136;142mprogressing\x1b[39m"));

        // manual input resolves with the submitted value
        dialog.show_manual_input("enter code");
        assert!(!dialog.is_aborted());
        dialog.input_shared.borrow_mut().set_value("the-code");
        dialog.submit_input();
        assert_eq!(
            dialog.take_pending_value(),
            Some(Ok("the-code".to_string()))
        );
        let content = dialog.content_children(60).join("\n");
        assert!(
            content.contains("> the-code"),
            "input replaced with submitted text"
        );

        // prompt resolves via Enter
        dialog.show_prompt("Who are you?", Some("your name"));
        dialog.input_shared.borrow_mut().set_value("my name");
        dialog.submit_input();
        assert_eq!(dialog.take_pending_value(), Some(Ok("my name".to_string())));

        // cancel path (upstream rejects with "Login cancelled" + onComplete)
        dialog.show_prompt("again", None);
        dialog.handle_input("\x1b");
        assert_eq!(
            dialog.take_pending_value(),
            Some(Err("Login cancelled".to_string()))
        );
        assert!(dialog.is_aborted());
        assert_eq!(
            completions.borrow().as_slice(),
            &[(false, Some("Login cancelled".to_string()))]
        );
        assert!(renders.get() > 0);
    }
}
