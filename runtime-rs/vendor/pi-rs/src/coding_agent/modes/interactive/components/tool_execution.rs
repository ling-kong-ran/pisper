//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Port of `modes/interactive/components/tool-execution.ts` (421 lines, sha256
//! `f88c4333efc7be6b4a4d758fe694601116b2f4ae82ac6b307dbbedf6e5c3dc71`):
//! renders tool executions through extension-provided call/result renderers
//! with generic fallbacks, expand/collapse, image blocks and partial states.
//!
//! Disclosed seams (S19.7 in `components/mod.rs`):
//! - `convertToPng`/`ensurePngTranscoder` (`utils/image-convert.ts`) map to
//!   [`crate::coding_agent::utils::image_process`]'s synchronous transcoder;
//!   the per-component conversion cache upstream (`convertedImages`) is
//!   needless here because the child payloads are rebuilt deterministically
//!   (upstream's `imageSources` reuse only preserves converted data inside
//!   the mounted Image widget). Upstream re-renders once the transcoder
//!   registers; the native one registers synchronously, so the converted
//!   image renders in the same pass.
//! - `getRenderedTextOutput` (`core/tools/render-utils.ts`) is re-stated in
//!   [`get_text_output`] (its other exports belong to other slices); terminal
//!   capabilities come from the vendored `tui::terminal_image` (test hook
//!   `set_capabilities`).
//! - The extension `Image` widget is carried as an [`ImageSpec`] payload (the
//!   interactive shell mounts the vendored tui image component; the oracle
//!   pins the data/mime/width triples).

use std::sync::Arc;

use crate::coding_agent::modes::interactive::components::model_selector::theme_fg;
use crate::coding_agent::modes::interactive::components::support::MessageBgBox;
use crate::coding_agent::modes::interactive::theme::Theme;
use crate::coding_agent::utils::ansi::strip_ansi;
use crate::tui::component::{
    Component, TuiMouseButton, TuiMouseEvent, TuiMouseEventResult, TuiMouseEventType,
};
use crate::tui::components::text::Text;
use crate::tui::terminal_image::{get_capabilities, image_fallback};

const FALLBACK_PREVIEW_LINES: usize = 10;

/// Upstream `COLLAPSED_ARGS_CHARS` (core/tools/render-utils.ts).
const COLLAPSED_ARGS_CHARS: usize = 100;

/// Upstream `ToolExecutionOptions`.
#[derive(Clone, Copy, Debug, Default)]
pub struct ToolExecutionOptions {
    pub show_images: Option<bool>,
    pub image_width_cells: Option<usize>,
}

/// Upstream `ToolRenderContext`.
pub struct ToolRenderContext<'a> {
    pub args: &'a serde_json::Value,
    pub tool_call_id: &'a str,
    pub state: &'a serde_json::Value,
    pub cwd: &'a str,
    pub execution_started: bool,
    pub args_complete: bool,
    pub is_partial: bool,
    pub expanded: bool,
    pub show_images: bool,
    pub is_error: bool,
}

/// Upstream `ToolRenderResultOptions`.
#[derive(Clone, Copy, Debug)]
pub struct ToolRenderResultOptions {
    pub expanded: bool,
    pub is_partial: bool,
}

/// Upstream `renderShell` values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderShell {
    Default,
    SelfRender,
}

/// One result content block.
#[derive(Clone, Debug, Default)]
pub struct ToolResultBlock {
    pub block_type: String,
    pub text: Option<String>,
    pub data: Option<String>,
    pub mime_type: Option<String>,
}

/// Upstream result shape (`content` / `details` / `isError`).
#[derive(Clone, Debug, Default)]
pub struct ToolResultPayload {
    pub content: Vec<ToolResultBlock>,
    pub details: Option<serde_json::Value>,
    pub is_error: bool,
}

type RenderCallFn =
    Box<dyn FnMut(&serde_json::Value, &Theme, &ToolRenderContext) -> Box<dyn Component> + Send>;
type RenderResultFn = Box<
    dyn FnMut(
            &ToolResultPayload,
            ToolRenderResultOptions,
            &Theme,
            &ToolRenderContext,
        ) -> Box<dyn Component>
        + Send,
>;

/// Upstream `ToolRenderers`: what this component needs from a tool — how to
/// draw it.
#[derive(Default)]
pub struct ToolRenderers {
    pub render_shell: Option<RenderShell>,
    pub render_call: Option<RenderCallFn>,
    pub render_result: Option<RenderResultFn>,
}

/// The image child payload the shell mounts (upstream `new Image(...)`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageSpec {
    pub data: String,
    pub mime_type: String,
    pub max_width_cells: usize,
    pub fallback_color: &'static str,
}

/// Upstream `ToolExecutionComponent`.
pub struct ToolExecutionComponent {
    content_box: MessageBgBox,
    content_text: Text,
    renderer_state: serde_json::Value,
    image_components: Vec<ImageSpec>,
    tool_name: String,
    tool_call_id: String,
    args: serde_json::Value,
    expanded: bool,
    show_images: bool,
    image_width_cells: usize,
    is_partial: bool,
    tool_definition: Option<ToolRenderers>,
    cwd: String,
    execution_started: bool,
    args_complete: bool,
    result: Option<ToolResultPayload>,
    hide_component: bool,
    theme: Arc<Theme>,
}

impl ToolExecutionComponent {
    /// Upstream constructor.
    pub fn new(
        theme: Arc<Theme>,
        tool_name: &str,
        tool_call_id: &str,
        args: serde_json::Value,
        options: ToolExecutionOptions,
        tool_definition: Option<ToolRenderers>,
        cwd: &str,
    ) -> Self {
        let mut component = Self {
            content_box: MessageBgBox::new(
                1,
                1,
                Some({
                    let bg_theme = Arc::clone(&theme);
                    Arc::new(move |t: &str| bg_theme.bg("toolPendingBg", t).expect("toolPendingBg"))
                }),
            ),
            content_text: Text::with_options("", 1, 1, None),
            renderer_state: serde_json::json!({}),
            image_components: Vec::new(),
            tool_name: tool_name.to_string(),
            tool_call_id: tool_call_id.to_string(),
            args,
            expanded: false,
            show_images: options.show_images.unwrap_or(true),
            image_width_cells: options.image_width_cells.unwrap_or(60),
            is_partial: true,
            tool_definition,
            cwd: cwd.to_string(),
            execution_started: false,
            args_complete: false,
            result: None,
            hide_component: false,
            theme,
        };
        component.update_display();
        component
    }

    fn has_renderer_definition(&self) -> bool {
        self.tool_definition.is_some()
    }

    fn render_shell(&self) -> RenderShell {
        self.tool_definition
            .as_ref()
            .and_then(|d| d.render_shell)
            .unwrap_or(RenderShell::Default)
    }

    fn context(&self, _last_component: Option<()>) -> ToolRenderContext<'_> {
        ToolRenderContext {
            args: &self.args,
            tool_call_id: &self.tool_call_id,
            state: &self.renderer_state,
            cwd: &self.cwd,
            execution_started: self.execution_started,
            args_complete: self.args_complete,
            is_partial: self.is_partial,
            expanded: self.expanded,
            show_images: self.show_images,
            is_error: self.result.as_ref().is_some_and(|r| r.is_error),
        }
    }

    fn call_fallback(&self) -> Text {
        Text::with_options(
            &format_tool_call_with_args(&self.theme, &self.tool_name, &self.args, self.expanded),
            0,
            0,
            None,
        )
    }

    fn result_fallback(&self) -> Option<Text> {
        let output = self.get_text_output();
        if output.is_empty() {
            return None;
        }
        let lines: Vec<&str> = output.split('\n').collect();
        let display_lines = if self.expanded {
            &lines[..]
        } else {
            &lines[..lines.len().min(FALLBACK_PREVIEW_LINES)]
        };
        let remaining = lines.len() - display_lines.len();
        let mut text = display_lines
            .iter()
            .map(|line| theme_fg(&self.theme, "toolOutput", line))
            .collect::<Vec<_>>()
            .join("\n");
        if remaining > 0 {
            text += &format!(
                "{} {}{}",
                theme_fg(
                    &self.theme,
                    "muted",
                    &format!("\n... ({remaining} more lines,")
                ),
                crate::coding_agent::modes::interactive::components::model_selector::key_hint(
                    &self.theme,
                    "app.tools.expand",
                    "to expand",
                ),
                theme_fg(&self.theme, "muted", ")")
            );
        }
        Some(Text::with_options(&text, 0, 0, None))
    }

    /// Upstream `updateArgs`.
    pub fn update_args(&mut self, args: serde_json::Value) {
        self.args = args;
        self.update_display();
    }

    /// Upstream `markExecutionStarted`.
    pub fn mark_execution_started(&mut self) {
        self.execution_started = true;
        self.update_display();
    }

    /// Upstream `setArgsComplete`.
    pub fn set_args_complete(&mut self) {
        self.args_complete = true;
        self.update_display();
    }

    /// Upstream `updateResult`.
    pub fn update_result(&mut self, result: ToolResultPayload, is_partial: bool) {
        self.result = Some(result);
        self.is_partial = is_partial;
        self.update_display();
    }

    /// Upstream `setExpanded`.
    pub fn set_expanded(&mut self, expanded: bool) {
        self.expanded = expanded;
        self.update_display();
    }

    /// Upstream `setShowImages`.
    pub fn set_show_images(&mut self, show: bool) {
        self.show_images = show;
        self.update_display();
    }

    /// Upstream `setImageWidthCells`.
    pub fn set_image_width_cells(&mut self, width: usize) {
        self.image_width_cells = (width).max(1);
        self.update_display();
    }

    /// Upstream `updateDisplay` (rebuilt children; deterministic payloads).
    fn update_display(&mut self) -> Vec<ChildPayload> {
        let is_partial = self.is_partial;
        let is_error = self.result.as_ref().is_some_and(|r| r.is_error);
        let bg_color: &'static str = if is_partial {
            "toolPendingBg"
        } else if is_error {
            "toolErrorBg"
        } else {
            "toolSuccessBg"
        };
        let bg_theme = Arc::clone(&self.theme);
        let bg_fn: Arc<dyn Fn(&str) -> String + Send + Sync> =
            Arc::new(move |t: &str| bg_theme.bg(bg_color, t).expect("tool bg"));

        #[allow(unused_assignments)] // seeded `false` only to satisfy the reader at the tail guard
        let mut has_content = false;
        self.hide_component = false;
        let mut children: Vec<ChildPayload> = Vec::new();

        if self.has_renderer_definition() {
            let use_self = self.render_shell() == RenderShell::SelfRender;
            if !use_self {
                self.content_box.set_bg_fn(Some(Arc::clone(&bg_fn)));
            }

            // Call renderer (the renderer closure is taken out of the option
            // for the call and restored afterwards — Rust borrowck vs the
            // upstream `this` aliasing).
            let mut call_renderer = self
                .tool_definition
                .as_mut()
                .and_then(|d| d.render_call.take());
            match call_renderer.as_mut() {
                None => {
                    let fallback = self.call_fallback();
                    has_content = true;
                    children.push(ChildPayload::BoxChild(Box::new(fallback)));
                }
                Some(render_call) => {
                    let context = self.context(None);
                    let args = self.args.clone();
                    let attempt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        render_call(&args, &self.theme, &context)
                    }));
                    match attempt {
                        Ok(component) => {
                            has_content = true;
                            children.push(ChildPayload::BoxChild(component));
                        }
                        Err(_) => {
                            let fallback = self.call_fallback();
                            has_content = true;
                            children.push(ChildPayload::BoxChild(Box::new(fallback)));
                        }
                    }
                }
            }
            if let Some(tool_definition) = self.tool_definition.as_mut() {
                tool_definition.render_call = call_renderer.take();
            }

            // Result renderer
            if self.result.is_some() {
                let mut result_renderer = self
                    .tool_definition
                    .as_mut()
                    .and_then(|d| d.render_result.take());
                match result_renderer.as_mut() {
                    None => {
                        if let Some(fallback) = self.result_fallback() {
                            has_content = true;
                            children.push(ChildPayload::BoxChild(Box::new(fallback)));
                        }
                    }
                    Some(render_result) => {
                        let context = self.context(None);
                        let payload = self.result.clone().expect("result present");
                        let options = ToolRenderResultOptions {
                            expanded: self.expanded,
                            is_partial: self.is_partial,
                        };
                        let attempt =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                render_result(&payload, options, &self.theme, &context)
                            }));
                        match attempt {
                            Ok(component) => {
                                has_content = true;
                                children.push(ChildPayload::BoxChild(component));
                            }
                            Err(_) => {
                                if let Some(fallback) = self.result_fallback() {
                                    has_content = true;
                                    children.push(ChildPayload::BoxChild(Box::new(fallback)));
                                }
                            }
                        }
                    }
                }
                if let Some(tool_definition) = self.tool_definition.as_mut() {
                    tool_definition.render_result = result_renderer.take();
                }
            }
            let _ = use_self;
        } else {
            self.content_text.set_custom_bg_fn(Some(Arc::clone(&bg_fn)));
            let text = self.format_tool_execution();
            self.content_text.set_text(&text);
            has_content = true;
            children.push(ChildPayload::ContentText);
        }

        // Image blocks
        self.image_components.clear();
        if let Some(result) = &self.result {
            let image_blocks: Vec<&ToolResultBlock> = result
                .content
                .iter()
                .filter(|block| block.block_type == "image")
                .collect();
            let caps = get_capabilities();
            // upstream: `caps.images && this.showImages && img.data && img.mimeType`
            let images_enabled = caps.images.is_some() && self.show_images;
            for img in image_blocks {
                if let (true, Some(data), Some(mime_type)) =
                    (images_enabled, &img.data, &img.mime_type)
                {
                    let mut image_data = data.clone();
                    let mut image_mime_type = mime_type.clone();
                    if caps.images == Some("kitty") && mime_type != "image/png" {
                        // v1.0.0: on kitty, a registered PNG transcoder
                        // converts non-PNG images; before it loads they are
                        // skipped and its registration schedules a re-render.
                        // The native transcoder registers synchronously, so
                        // the conversion runs in the same pass.
                        crate::coding_agent::utils::image_process::ensure_png_transcoder(|| {
                            // Upstream `onRegistered` invalidates and requests
                            // a render; the synchronous port renders the
                            // converted image in this pass instead.
                        });
                        let converted =
                            crate::coding_agent::utils::image_process::registered_png_transcoder()
                                .and_then(|transcoder| transcoder(data));
                        let Some(png) = converted else {
                            continue;
                        };
                        image_data = png;
                        image_mime_type = "image/png".to_string();
                    }
                    children.push(ChildPayload::Image(ImageSpec {
                        data: image_data,
                        mime_type: image_mime_type,
                        max_width_cells: self.image_width_cells,
                        fallback_color: "toolOutput",
                    }));
                }
            }
        }

        if self.has_renderer_definition() && !has_content && self.image_components.is_empty() {
            self.hide_component = true;
        }
        children
    }

    /// The deterministic child payload for the shell/tests.
    pub fn content_payload(&mut self) -> Vec<ChildPayload> {
        self.update_display()
    }

    pub fn is_hidden(&self) -> bool {
        self.hide_component
    }

    pub fn image_width_cells(&self) -> usize {
        self.image_width_cells
    }

    /// Upstream `getTextOutput` → render-utils `getTextOutput` (S19.7).
    fn get_text_output(&self) -> String {
        get_text_output(self.result.as_ref(), self.show_images)
    }

    /// Upstream `formatToolExecution` (generic fallback body).
    fn format_tool_execution(&self) -> String {
        let mut text = theme_fg(&self.theme, "toolTitle", &self.theme.bold(&self.tool_name));
        let content = serde_json::to_string_pretty(&self.args).unwrap_or_default();
        if !self.args.is_null() && !content.is_empty() {
            text += &format!("\n\n{content}");
        }
        let output = self.get_text_output();
        if !output.is_empty() {
            text += &format!("\n{output}");
        }
        text
    }
}

/// JS string `length`/`slice` operate on UTF-16 code units; the collapsed
/// preview cut must count the same units for non-ASCII arguments.
fn utf16_len(text: &str) -> usize {
    text.chars().map(|c| c.len_utf16()).sum()
}

/// JS `String.prototype.slice(0, n)` over UTF-16 code units. A cut that
/// splits a surrogate pair leaves the high half in the JS string (upstream),
/// which renders as the replacement character; the port emits U+FFFD for it
/// (Rust strings cannot hold a lone surrogate) — the oracle canonicalizes the
/// captured bytes the same way.
fn utf16_slice(text: &str, end: usize) -> String {
    const REPLACEMENT: char = '\u{FFFD}';
    let mut units = 0usize;
    let mut out = String::new();
    for c in text.chars() {
        let len = c.len_utf16();
        if units + len > end {
            if len == 2 && units + 1 == end {
                out.push(REPLACEMENT);
            }
            break;
        }
        units += len;
        out.push(c);
    }
    out
}

/// Upstream `replaceTabs` (core/tools/render-utils.ts): tabs become 3 spaces.
fn replace_tabs(text: &str) -> String {
    text.replace('\t', "   ")
}

/// Upstream `formatToolCallWithArgs` (core/tools/render-utils.ts): the generic
/// tool call header — the title followed by the arguments. Collapsed, they
/// are `key=value` pairs on the title line, cut to [`COLLAPSED_ARGS_CHARS`].
/// Expanded, each is a `key: value` line below the title, with strings shown
/// raw and continuation lines indented.
pub fn format_tool_call_with_args(
    theme: &Theme,
    title: &str,
    args: &serde_json::Value,
    expanded: bool,
) -> String {
    let header = theme_fg(theme, "toolTitle", &theme.bold(title));
    if args.is_null() {
        return header;
    }
    // `typeof args === "object" && !Array.isArray(args) ? Object.entries(args)
    // : [["args", args]]`.
    let entries: Vec<(String, &serde_json::Value)> = match args {
        serde_json::Value::Object(map) => map.iter().map(|(k, v)| (k.clone(), v)).collect(),
        other => vec![("args".to_string(), other)],
    };
    if entries.is_empty() {
        return header;
    }
    if expanded {
        let lines = entries
            .iter()
            .map(|(key, value)| {
                let text = match value {
                    serde_json::Value::String(text) => text.clone(),
                    other => {
                        serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string())
                    }
                };
                let text = replace_tabs(&text).replace('\r', "");
                format!(
                    "  {key}: {}",
                    text.split('\n').collect::<Vec<_>>().join("\n    ")
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        format!("{header}\n{}", theme_fg(theme, "muted", &lines))
    } else {
        let pairs = entries
            .iter()
            .map(|(key, value)| {
                let text = serde_json::to_string(value).unwrap_or_else(|_| value.to_string());
                format!("{key}={text}")
            })
            .collect::<Vec<_>>()
            .join(" ");
        let preview = if utf16_len(&pairs) > COLLAPSED_ARGS_CHARS {
            format!("{}...", utf16_slice(&pairs, COLLAPSED_ARGS_CHARS - 3))
        } else {
            pairs
        };
        format!("{header} {}", theme_fg(theme, "muted", &preview))
    }
}

/// Upstream render-utils `getTextOutput` (subset; S19.7).
pub fn get_text_output(result: Option<&ToolResultPayload>, show_images: bool) -> String {
    let Some(result) = result else {
        return String::new();
    };
    let text_blocks: Vec<&ToolResultBlock> = result
        .content
        .iter()
        .filter(|block| block.block_type == "text")
        .collect();
    let image_blocks: Vec<&ToolResultBlock> = result
        .content
        .iter()
        .filter(|block| block.block_type == "image")
        .collect();

    let mut output = text_blocks
        .iter()
        .map(|block| {
            sanitize_binary_output(
                &strip_ansi(block.text.as_deref().unwrap_or("")).replace('\r', ""),
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let caps = get_capabilities();
    if !image_blocks.is_empty() && (caps.images.is_none() || !show_images) {
        let image_indicators = image_blocks
            .iter()
            .map(|img| {
                image_fallback(
                    img.mime_type.as_deref().unwrap_or("image/unknown"),
                    None,
                    None,
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        output = if output.is_empty() {
            image_indicators
        } else {
            format!("{output}\n{image_indicators}")
        };
    }
    output
}

/// `sanitizeBinaryOutput` (utils/shell.ts): drop control/format characters
/// that break width measurement (kept verbatim; tab/newline/CR survive).
fn sanitize_binary_output(value: &str) -> String {
    value
        .chars()
        .filter(|c| {
            let code = *c as u32;
            if code == 0x09 || code == 0x0a || code == 0x0d {
                return true;
            }
            if code <= 0x1f {
                return false;
            }
            !(0xfff9..=0xfffb).contains(&code)
        })
        .collect()
}

/// The rebuilt child payloads (box children, content text, image specs).
pub enum ChildPayload {
    BoxChild(Box<dyn Component>),
    ContentText,
    Image(ImageSpec),
}

impl Component for ToolExecutionComponent {
    /// Upstream `render` (self-shell flattening + hide).
    fn render(&mut self, width: usize) -> Vec<String> {
        if self.hide_component {
            return Vec::new();
        }
        let use_self =
            self.has_renderer_definition() && self.render_shell() == RenderShell::SelfRender;
        if use_self {
            let mut lines: Vec<String> = Vec::new();
            let content_lines = self.update_display();
            let mut saw_content = false;
            for child in content_lines {
                match child {
                    ChildPayload::BoxChild(component) => {
                        let mut component = component;
                        let rendered = component.render(width);
                        if !rendered.is_empty() {
                            saw_content = true;
                            lines.push(String::new());
                            lines.extend(rendered);
                        }
                    }
                    ChildPayload::Image(image) => {
                        saw_content = true;
                        lines.push(String::new());
                        lines.push(format!(
                            "[image {} ({} cells)]",
                            image.mime_type, image.max_width_cells
                        ));
                    }
                    ChildPayload::ContentText => {}
                }
            }
            if !saw_content && self.image_components.is_empty() {
                return Vec::new();
            }
            return lines;
        }

        // Default shell: Spacer(1) + contentBox (or the content-text region).
        let mut lines = vec![String::new()];
        let payload = self.update_display();
        let mut box_ = std::mem::replace(&mut self.content_box, MessageBgBox::new(1, 1, None));
        for child in payload {
            match child {
                ChildPayload::BoxChild(component) => box_.add_child(component),
                ChildPayload::ContentText => {}
                ChildPayload::Image(image) => {
                    lines.push(String::new());
                    lines.push(format!(
                        "[image {} ({} cells)]",
                        image.mime_type, image.max_width_cells
                    ));
                }
            }
        }
        if !self.has_renderer_definition() {
            let mut text =
                std::mem::replace(&mut self.content_text, Text::with_options("", 1, 1, None));
            lines.extend(text.render(width));
        } else {
            lines.extend(box_.render(width));
        }
        lines
    }

    fn invalidate(&mut self) {
        self.update_display();
    }

    fn handle_mouse(&mut self, event: &TuiMouseEvent) -> Option<TuiMouseEventResult> {
        if event.event_type == TuiMouseEventType::Click
            && event.button == TuiMouseButton::Left
            && self.result.is_some()
        {
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
    use crate::tui::terminal_image::{set_capabilities, TerminalCapabilities};

    fn dark() -> Arc<Theme> {
        Arc::new(load_builtin_theme("dark", Some(ColorMode::Truecolor)).expect("dark"))
    }

    /// The capability registry is process-global; these tests run in parallel,
    /// so every caps-touching test holds this lock for its whole body (drop the
    /// guard before re-acquiring with a different mode mid-test).
    static CAPS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn caps_off() -> std::sync::MutexGuard<'static, ()> {
        let guard = CAPS_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        set_capabilities(TerminalCapabilities {
            hyperlinks: false,
            images: None,
            true_color: true,
        });
        guard
    }

    fn caps_kitty() -> std::sync::MutexGuard<'static, ()> {
        let guard = CAPS_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        set_capabilities(TerminalCapabilities {
            hyperlinks: false,
            images: Some("kitty"),
            true_color: true,
        });
        guard
    }

    fn caps_sixel() -> std::sync::MutexGuard<'static, ()> {
        let guard = CAPS_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        set_capabilities(TerminalCapabilities {
            hyperlinks: false,
            images: Some("sixel"),
            true_color: true,
        });
        guard
    }

    /// Rendered rows with ANSI stripped, the Box's single leading padding
    /// space removed and trailing padding trimmed — deeper indentation (e.g.
    /// the JSON arg block) is preserved, matching the oracle child-text rows.
    fn plain_rows(rendered: &[String]) -> String {
        rendered
            .iter()
            .map(|l| {
                let plain = crate::coding_agent::utils::ansi::strip_ansi(l);
                let stripped = plain.strip_prefix(' ').unwrap_or(&plain);
                stripped.trim_end().to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Oracle scenario `tool_execution`: generic-fallback content, expansion,
    /// image fallbacks, kitty gating, self-shell render, hide behavior.
    #[test]
    fn tool_execution_matches_oracle() {
        let _caps = caps_off();
        let theme = dark();
        let mut plain = ToolExecutionComponent::new(
            theme.clone(),
            "read",
            "call-1",
            serde_json::json!({"path": "a.txt", "n": 2}),
            ToolExecutionOptions::default(),
            None,
            "C:\\w",
        );
        let content = plain.content_payload();
        assert!(matches!(content.last(), Some(ChildPayload::ContentText)));
        let rendered = plain.render(60);
        assert!(rendered
            .join("\n")
            .contains("\x1b[38;2;222;224;225m\x1b[1mread\x1b[22m\x1b[39m"));
        // oracle plainContent rows: ANSI-stripped, padding-trimmed render rows
        assert!(plain_rows(&rendered).contains("{\n  \"path\": \"a.txt\",\n  \"n\": 2\n}"));

        plain.mark_execution_started();
        plain.set_args_complete();
        plain.update_result(
            ToolResultPayload {
                content: vec![
                    ToolResultBlock {
                        block_type: "text".to_string(),
                        text: Some("file contents".to_string()),
                        ..Default::default()
                    },
                    ToolResultBlock {
                        block_type: "image".to_string(),
                        data: Some("imgdata".to_string()),
                        mime_type: Some("image/png".to_string()),
                        ..Default::default()
                    },
                ],
                details: None,
                is_error: false,
            },
            false,
        );
        let rendered = plain.render(60).join("\n");
        assert!(rendered.contains("file contents"));
        assert!(
            rendered.contains("[Image: [image/png]]"),
            "images hidden: fallback marker"
        );
        plain.set_expanded(true);
        assert!(plain.render(60).join("\n").contains("file contents"));
    }

    #[test]
    fn tool_execution_renderers_and_images() {
        let _caps = caps_kitty();
        let theme = dark();
        let mut with_renderers = ToolExecutionComponent::new(
            theme.clone(),
            "bash",
            "call-2",
            serde_json::json!({"cmd": "ls"}),
            ToolExecutionOptions::default(),
            Some(ToolRenderers {
                render_shell: None,
                render_call: Some(Box::new(|args, _theme, ctx| {
                    Box::new(Text::with_options(
                        &format!("CALL({args} {} {})", ctx.is_partial, ctx.expanded),
                        0,
                        0,
                        None,
                    )) as Box<dyn Component>
                })),
                render_result: Some(Box::new(|result, options, _theme, _ctx| {
                    if options.is_partial {
                        Box::new(Text::with_options(
                            &format!("PARTIAL({:?})", result.details),
                            0,
                            0,
                            None,
                        )) as Box<dyn Component>
                    } else {
                        Box::new(Text::with_options(
                            &format!(
                                "RESULT({}) {}",
                                result
                                    .content
                                    .iter()
                                    .map(|b| b.text.as_deref().unwrap_or("[img]"))
                                    .collect::<Vec<_>>()
                                    .join(","),
                                options.expanded
                            ),
                            0,
                            0,
                            None,
                        )) as Box<dyn Component>
                    }
                })),
            }),
            "C:\\w",
        );
        with_renderers.update_result(
            ToolResultPayload {
                content: vec![ToolResultBlock {
                    block_type: "text".to_string(),
                    text: Some("out".to_string()),
                    ..Default::default()
                }],
                details: Some(serde_json::json!({"n": 1})),
                is_error: false,
            },
            true,
        );
        let rendered = with_renderers.render(60).join("\n");
        assert!(
            rendered.contains("CALL({\"cmd\":\"ls\"} true false)"),
            "{rendered}"
        );
        assert!(rendered.contains("PARTIAL(Some(Object"));

        with_renderers.update_result(
            ToolResultPayload {
                content: vec![ToolResultBlock {
                    block_type: "text".to_string(),
                    text: Some("out".to_string()),
                    ..Default::default()
                }],
                details: None,
                is_error: false,
            },
            false,
        );
        let rendered = with_renderers.render(60).join("\n");
        assert!(rendered.contains("RESULT(out) false"));

        // kitty: png mounts; jpeg has no transcodable bytes ("jpegdata" does
        // not decode), so it is skipped exactly like a failed conversion.
        let mut kitty = ToolExecutionComponent::new(
            theme.clone(),
            "img",
            "c4",
            serde_json::json!({}),
            ToolExecutionOptions::default(),
            None,
            "C:\\w",
        );
        kitty.update_result(
            ToolResultPayload {
                content: vec![
                    ToolResultBlock {
                        block_type: "image".to_string(),
                        data: Some("pngdata".to_string()),
                        mime_type: Some("image/png".to_string()),
                        ..Default::default()
                    },
                    ToolResultBlock {
                        block_type: "image".to_string(),
                        data: Some("jpegdata".to_string()),
                        mime_type: Some("image/jpeg".to_string()),
                        ..Default::default()
                    },
                ],
                details: None,
                is_error: false,
            },
            false,
        );
        let payload = kitty.content_payload();
        let images: Vec<&ImageSpec> = payload
            .iter()
            .filter_map(|p| match p {
                ChildPayload::Image(image) => Some(image),
                _ => None,
            })
            .collect();
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].mime_type, "image/png");
        assert_eq!(images[0].data, "pngdata");
        assert_eq!(images[0].max_width_cells, 60);

        drop(_caps);
        let _caps_sixel = caps_sixel();
        let mut no_kitty = ToolExecutionComponent::new(
            theme.clone(),
            "img",
            "c5",
            serde_json::json!({}),
            ToolExecutionOptions::default(),
            None,
            "C:\\w",
        );
        no_kitty.update_result(
            ToolResultPayload {
                content: vec![ToolResultBlock {
                    block_type: "image".to_string(),
                    data: Some("jpegdata".to_string()),
                    mime_type: Some("image/jpeg".to_string()),
                    ..Default::default()
                }],
                details: None,
                is_error: false,
            },
            false,
        );
        let payload = no_kitty.content_payload();
        let images: Vec<&ImageSpec> = payload
            .iter()
            .filter_map(|p| match p {
                ChildPayload::Image(image) => Some(image),
                _ => None,
            })
            .collect();
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].mime_type, "image/jpeg");
        no_kitty.set_show_images(false);
        assert!(no_kitty
            .content_payload()
            .iter()
            .all(|p| !matches!(p, ChildPayload::Image(_))));
        no_kitty.set_show_images(true);
        no_kitty.set_image_width_cells(40);
        assert_eq!(no_kitty.image_width_cells(), 40);
    }

    /// Oracle `selfRender` / `hideRender`: the self shell flattens its own
    /// framing; empty content + renderers hides the component.
    #[test]
    fn self_shell_and_hidden_components() {
        let _caps = caps_off();
        let theme = dark();
        let mut self_shell = ToolExecutionComponent::new(
            theme.clone(),
            "self",
            "c6",
            serde_json::json!({}),
            ToolExecutionOptions::default(),
            Some(ToolRenderers {
                render_shell: Some(RenderShell::SelfRender),
                render_call: Some(Box::new(|_args, _theme, _ctx| {
                    Box::new(Text::with_options("SELF CALL", 0, 0, None)) as Box<dyn Component>
                })),
                render_result: None,
            }),
            "C:\\w",
        );
        let rendered = self_shell.render(30);
        assert_eq!(rendered.len(), 2);
        assert_eq!(rendered[0], "");
        assert!(rendered[1].starts_with("SELF CALL"));

        let mut empty_hide = ToolExecutionComponent::new(
            theme,
            "ghost",
            "c7",
            serde_json::json!({}),
            ToolExecutionOptions::default(),
            Some(ToolRenderers {
                render_shell: None,
                render_call: Some(Box::new(|_args, _theme, _ctx| {
                    Box::new(Text::with_options("", 0, 0, None)) as Box<dyn Component>
                })),
                render_result: None,
            }),
            "C:\\w",
        );
        empty_hide.update_result(
            ToolResultPayload {
                content: Vec::new(),
                details: None,
                is_error: false,
            },
            false,
        );
        // hasContent stays true (call renderer produced a component), so the
        // component renders only the leading spacer row.
        let rendered = empty_hide.render(30);
        assert_eq!(rendered, vec![String::new()]);
    }

    #[test]
    fn panicking_renderers_fall_back() {
        let _caps = caps_off();
        let theme = dark();
        let mut throwing = ToolExecutionComponent::new(
            theme,
            "t",
            "c3",
            serde_json::json!({}),
            ToolExecutionOptions::default(),
            Some(ToolRenderers {
                render_shell: None,
                render_call: Some(Box::new(|_a, _t, _c| panic!("call boom"))),
                render_result: Some(Box::new(|_r, _o, _t, _c| panic!("result boom"))),
            }),
            "C:\\w",
        );
        throwing.update_result(
            ToolResultPayload {
                content: vec![
                    ToolResultBlock {
                        block_type: "text".to_string(),
                        text: Some("fallback output".to_string()),
                        ..Default::default()
                    },
                    ToolResultBlock {
                        block_type: "text".to_string(),
                        text: Some(
                            (0..11)
                                .map(|i| format!("line{}", i + 2))
                                .collect::<Vec<_>>()
                                .join("\n"),
                        ),
                        ..Default::default()
                    },
                ],
                details: None,
                is_error: true,
            },
            false,
        );
        let rendered = throwing.render(60).join("\n");
        assert!(
            rendered.contains("\x1b[38;2;222;224;225m\x1b[1mt\x1b[22m\x1b[39m"),
            "call fallback"
        );
        assert!(rendered.contains("fallback output"), "result fallback");
        // 12 output lines exceed FALLBACK_PREVIEW_LINES (10) → the expand hint
        // (byte-faithful: the muted ")" follows keyHint's reset)
        assert!(
            rendered.contains("to expand\x1b[39m\x1b[38;2;157;165;169m)"),
            "preview hint"
        );
        assert!(rendered.contains("\x1b[48;2;91;40;42m"), "toolErrorBg");
    }

    /// render-utils getTextOutput probe (oracle `textOutputProbe`).
    #[test]
    fn text_output_joins_and_falls_back() {
        let _caps = caps_off();
        let output = get_text_output(
            Some(&ToolResultPayload {
                content: vec![
                    ToolResultBlock {
                        block_type: "text".to_string(),
                        text: Some("one".to_string()),
                        ..Default::default()
                    },
                    ToolResultBlock {
                        block_type: "text".to_string(),
                        text: Some("two".to_string()),
                        ..Default::default()
                    },
                    ToolResultBlock {
                        block_type: "image".to_string(),
                        data: Some("d".to_string()),
                        mime_type: Some("image/jpeg".to_string()),
                        ..Default::default()
                    },
                ],
                details: None,
                is_error: false,
            }),
            true,
        );
        assert_eq!(output, "one\ntwo\n[Image: [image/jpeg]]");
    }
}
