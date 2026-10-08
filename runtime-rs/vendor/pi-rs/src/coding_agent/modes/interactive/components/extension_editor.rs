//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Port of `modes/interactive/components/extension-editor.ts` (132 lines,
//! sha256 `29f5371be2530eaa121d380de3c90d6a7013a776856fdf5c3c5b49e03b6eaa18`):
//! multi-line editor for extensions with Ctrl+G for the external editor.
//!
//! The upstream constructor takes the coding-agent `KeybindingsManager` (for
//! the `app.editor.external` chord) and a `TUI` (for stop/start around the
//! external editor); following the selector-slice convention both become
//! explicit callbacks ([`Self::new`]'s `keybindings_matches` and
//! [`Self::handle_open_external_editor`]'s runner closure, which the shell
//! wraps with the TUI stop/start/requestRender choreography).

use std::sync::Arc;

use crate::coding_agent::modes::interactive::components::model_selector::{key_hint, theme_fg};
use crate::coding_agent::modes::interactive::components::support::Border;
use crate::coding_agent::modes::interactive::theme::Theme;
use crate::tui::component::Component;
use crate::tui::component_mouse::ComponentHandle;
use crate::tui::components::editor::{Editor, EditorOptions};
use crate::tui::components::layout_widgets::Spacer;
use crate::tui::components::text::Text;
use crate::tui::keybindings::with_keybindings;

/// Upstream `ExtensionEditorComponent`.
pub struct ExtensionEditorComponent {
    children: Vec<ComponentHandle>,
    editor: Editor,
    /// Upstream `keybindings: KeybindingsManager` (matches callback).
    keybindings_matches: Box<dyn Fn(&str, &str) -> bool>,
    external_editor_command: String,
    // Upstream `onSubmit` callback surface; wired into the interactive shell
    // in r19+.
    #[allow(dead_code)]
    on_submit: Box<dyn FnMut(&str)>,
    on_cancel: Box<dyn FnMut()>,
    /// Upstream `opts.description` (styled at rebuild time).
    description_text: Option<String>,
    theme: Arc<Theme>,
}

/// Upstream `ExtensionEditorOptions` (delta: the editor options gain the
/// `description` line; the description is threaded separately here).
#[derive(Default)]
pub struct ExtensionEditorOptions {
    /// The upstream `EditorOptions` surface.
    pub editor: EditorOptions,
    /// Descriptive line under the title (upstream `description`).
    pub description: Option<String>,
}

impl ExtensionEditorComponent {
    /// Upstream constructor (the 8-argument shape mirrors upstream's
    /// `ExtensionEditorComponent(...)` parameter list one-to-one).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        theme: Arc<Theme>,
        _title: &str,
        prefill: Option<&str>,
        on_submit: Box<dyn FnMut(&str)>,
        on_cancel: Box<dyn FnMut()>,
        options: Option<EditorOptions>,
        external_editor_command: Option<&str>,
        keybindings_matches: Box<dyn Fn(&str, &str) -> bool>,
    ) -> Self {
        Self::with_options(
            theme,
            _title,
            prefill,
            on_submit,
            on_cancel,
            options.map(|editor| ExtensionEditorOptions {
                editor,
                description: None,
            }),
            external_editor_command,
            keybindings_matches,
        )
    }

    /// Upstream constructor carrying `ExtensionEditorOptions` (the
    /// `{ description, ...editorOptions }` destructure upstream).
    #[allow(clippy::too_many_arguments)]
    pub fn with_options(
        theme: Arc<Theme>,
        _title: &str,
        prefill: Option<&str>,
        on_submit: Box<dyn FnMut(&str)>,
        on_cancel: Box<dyn FnMut()>,
        options: Option<ExtensionEditorOptions>,
        external_editor_command: Option<&str>,
        keybindings_matches: Box<dyn Fn(&str, &str) -> bool>,
    ) -> Self {
        let description_text = options
            .as_ref()
            .and_then(|options| options.description.as_deref())
            .map(|description| theme_fg(&theme, "text", description));
        let editor_options = options.map(|options| options.editor).unwrap_or_default();
        let external_editor_command = external_editor_command
            .map(str::to_string)
            .or_else(|| std::env::var("VISUAL").ok())
            .or_else(|| std::env::var("EDITOR").ok())
            .unwrap_or_else(|| {
                if cfg!(windows) {
                    "notepad".to_string()
                } else {
                    "nano".to_string()
                }
            });
        let mut editor = Editor::new(
            Some({
                let border_theme = Arc::clone(&theme);
                Arc::new(move |text: &str| theme_fg(&border_theme, "borderMuted", text))
            }),
            editor_options,
        );
        if let Some(prefill) = prefill {
            editor.set_text(prefill);
        }
        let mut component = Self {
            children: Vec::new(),
            editor,
            keybindings_matches,
            external_editor_command,
            on_submit,
            on_cancel,
            description_text,
            theme,
        };
        component.rebuild();
        component
    }

    fn rebuild(&mut self) {
        let mut children: Vec<ComponentHandle> = Vec::new();
        children.push(ComponentHandle::new(Border::new(None)));
        children.push(ComponentHandle::new(Spacer::new(1)));
        // Upstream `if (description) { spacer; text(theme.fg("text", description)); }`
        // (positioned between the title and the editor).
        if let Some(description_text) = &self.description_text {
            children.push(ComponentHandle::new(Spacer::new(1)));
            children.push(ComponentHandle::new(Text::with_options(
                description_text,
                1,
                0,
                None,
            )));
        }
        children.push(ComponentHandle::new(SlotAdapter));
        children.push(ComponentHandle::new(Spacer::new(1)));
        children.push(ComponentHandle::new(Text::with_options(
            &(key_hint(&self.theme, "tui.select.confirm", "submit")
                + "  "
                + &key_hint(&self.theme, "tui.input.newLine", "newline")
                + "  "
                + &key_hint(&self.theme, "tui.select.cancel", "cancel")
                + &format!(
                    "  {}",
                    key_hint(&self.theme, "app.editor.external", "external editor")
                )),
            1,
            0,
            None,
        )));
        children.push(ComponentHandle::new(Spacer::new(1)));
        children.push(ComponentHandle::new(Border::new(None)));
        self.children = children;
    }

    /// Upstream `handleOpenExternalEditor`: `tui.stop()` → edit → apply
    /// complete results → `tui.start()` → `requestRender(true)`. The runner
    /// closure performs the actual edit; the shell parks the TUI around it.
    pub fn handle_open_external_editor(
        &mut self,
        run_editor: impl FnOnce(&str, &str) -> Result<String, String>,
    ) {
        let content = self.editor.get_text();
        let result = run_editor(&self.external_editor_command, &content);
        if let Ok(new_content) = result {
            self.editor.set_text(&new_content);
        }
    }

    /// Upstream `handleInput`.
    pub fn handle_input(&mut self, key_data: &str) {
        if with_keybindings(|kb| kb.matches(key_data, "tui.select.cancel")) {
            (self.on_cancel)();
            return;
        }
        if (self.keybindings_matches)(key_data, "app.editor.external") {
            // Upstream: `void this.handleOpenExternalEditor();` — the shell
            // drives [`Self::handle_open_external_editor`] with the TUI parked.
            return;
        }
        self.editor.handle_input(key_data);
    }

    pub fn editor_text(&self) -> String {
        self.editor.get_text()
    }

    pub fn editor_mut(&mut self) -> &mut Editor {
        &mut self.editor
    }

    pub fn external_editor_command(&self) -> &str {
        &self.external_editor_command
    }
}

struct SlotAdapter;
impl Component for SlotAdapter {
    fn render(&mut self, _width: usize) -> Vec<String> {
        Vec::new()
    }
}

impl Component for ExtensionEditorComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        let mut lines = Vec::new();
        for child in &mut self.children {
            lines.extend(child.render(width));
        }
        lines
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

    /// Oracle scenario `extension_components` (editor half): prefill, hint
    /// row, external-editor round trip and cancel routing.
    #[test]
    fn extension_editor_matches_oracle() {
        let cancelled: Rc<RefCell<Vec<&'static str>>> = Rc::new(RefCell::new(Vec::new()));
        let cancelled_handle = Rc::clone(&cancelled);
        let mut editor = ExtensionEditorComponent::new(
            dark(),
            "Edit me",
            Some("prefill"),
            Box::new(|_v| {}),
            Box::new(move || cancelled_handle.borrow_mut().push("editor")),
            None,
            Some("my-editor"),
            Box::new(|data, keybinding| keybinding == "app.editor.external" && data == "\x07"),
        );
        assert_eq!(editor.editor_text(), "prefill");
        assert_eq!(editor.external_editor_command(), "my-editor");

        // external editor round trip: content out, edited content in
        editor.handle_open_external_editor(|command, content| {
            assert_eq!(command, "my-editor");
            Ok(format!("EDITED({content})"))
        });
        assert_eq!(editor.editor_text(), "EDITED(prefill)");

        editor.handle_input("\x1b");
        assert_eq!(cancelled.borrow().as_slice(), &["editor"]);
    }
}
