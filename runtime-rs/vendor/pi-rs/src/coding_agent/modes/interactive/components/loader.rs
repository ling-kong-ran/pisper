//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Port of the upstream tui `Loader` + `CancellableLoader`
//! (`packages/tui/src/components/loader.ts`, `cancellable-loader.ts`) — the
//! spinner widget the interactive components compose.
//!
//! The vendored tui slice does not carry these widgets yet, and they are only
//! consumed by the interactive component files in this slice, so they land
//! here (documented seam S19.3 in `components/mod.rs`). The frame-interval
//! scheduling is a host seam: upstream drives `currentFrame` from
//! `setInterval`; the port exposes [`Loader::advance_frame`] and keeps the
//! render/indicator semantics byte-faithful.

use std::sync::Arc;

use crate::tui::component::Component;
use crate::tui::components::text::Text;
use crate::tui::keybindings::with_keybindings;

/// Upstream `DEFAULT_FRAMES`.
pub const DEFAULT_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
/// Upstream `DEFAULT_INTERVAL_MS`.
pub const DEFAULT_INTERVAL_MS: u64 = 80;

pub type LoaderStyle = Arc<dyn Fn(&str) -> String + Send + Sync>;

/// Upstream `LoaderIndicatorOptions`.
#[derive(Clone, Debug, Default)]
pub struct LoaderIndicatorOptions {
    /// Animation frames. Empty array hides the indicator.
    pub frames: Option<Vec<String>>,
    /// Frame interval in milliseconds.
    pub interval_ms: Option<u64>,
}

/// Upstream `Loader` (a `Text` subclass; composition here).
pub struct Loader {
    text: Text,
    frames: Vec<String>,
    interval_ms: u64,
    current_frame: usize,
    running: bool,
    render_indicator_verbatim: bool,
    spinner_color_fn: LoaderStyle,
    message_color_fn: LoaderStyle,
    message: String,
}

impl Loader {
    /// Upstream constructor (`text = ""`, padding 1, 0).
    pub fn new(
        spinner_color_fn: LoaderStyle,
        message_color_fn: LoaderStyle,
        message: impl Into<String>,
        indicator: Option<LoaderIndicatorOptions>,
    ) -> Self {
        let mut loader = Self {
            text: Text::with_options("", 1, 0, None),
            frames: DEFAULT_FRAMES.iter().map(|s| s.to_string()).collect(),
            interval_ms: DEFAULT_INTERVAL_MS,
            current_frame: 0,
            running: false,
            render_indicator_verbatim: false,
            spinner_color_fn,
            message_color_fn,
            message: message.into(),
        };
        loader.set_indicator(indicator);
        loader
    }

    /// Upstream `start`.
    pub fn start(&mut self) {
        self.update_display();
        self.restart_animation();
    }

    /// Upstream `stop`.
    pub fn stop(&mut self) {
        self.running = false;
    }

    /// Upstream `setMessage`.
    pub fn set_message(&mut self, message: impl Into<String>) {
        self.message = message.into();
        self.update_display();
    }

    /// Upstream `setIndicator`.
    pub fn set_indicator(&mut self, indicator: Option<LoaderIndicatorOptions>) {
        self.render_indicator_verbatim = indicator.is_some();
        self.frames = match indicator.as_ref().and_then(|i| i.frames.as_ref()) {
            Some(frames) => frames.clone(),
            None => DEFAULT_FRAMES.iter().map(|s| s.to_string()).collect(),
        };
        self.interval_ms = match indicator.as_ref().and_then(|i| i.interval_ms) {
            Some(ms) if ms > 0 => ms,
            _ => DEFAULT_INTERVAL_MS,
        };
        self.current_frame = 0;
        self.start();
    }

    fn restart_animation(&mut self) {
        self.stop();
        // upstream: setInterval only when frames.length > 1; the timer is the
        // host seam and is driven by `advance_frame` instead.
        self.running = self.frames.len() > 1;
    }

    /// One upstream interval tick (the timer seam).
    pub fn advance_frame(&mut self) {
        if self.running && !self.frames.is_empty() {
            self.current_frame = (self.current_frame + 1) % self.frames.len();
            self.update_display();
        }
    }

    /// Upstream `getRenderedIndicator` (protected).
    pub fn get_rendered_indicator(&self) -> String {
        let frame = self
            .frames
            .get(self.current_frame)
            .cloned()
            .unwrap_or_default();
        if self.render_indicator_verbatim {
            frame
        } else {
            (self.spinner_color_fn)(&frame)
        }
    }

    fn update_display(&mut self) {
        let rendered_frame = self.get_rendered_indicator();
        let indicator = if !rendered_frame.is_empty() {
            format!("{rendered_frame} ")
        } else {
            String::new()
        };
        let text = format!("{}{}", indicator, (self.message_color_fn)(&self.message));
        self.text.set_text(&text);
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    pub fn interval_ms(&self) -> u64 {
        self.interval_ms
    }
}

impl Component for Loader {
    fn render(&mut self, width: usize) -> Vec<String> {
        let mut lines = vec![String::new()];
        lines.extend(self.text.render(width));
        lines
    }

    fn invalidate(&mut self) {
        self.text.invalidate();
        self.update_display();
    }

    fn handle_input(&mut self, _data: &str) {}
}

/// Upstream `CancellableLoader`: a `Loader` with an abort flag + hook.
pub struct CancellableLoader {
    loader: Loader,
    aborted: bool,
    on_abort: Option<Box<dyn FnMut()>>,
}

impl CancellableLoader {
    /// Upstream constructor.
    pub fn new(
        spinner_color_fn: LoaderStyle,
        message_color_fn: LoaderStyle,
        message: impl Into<String>,
        indicator: Option<LoaderIndicatorOptions>,
    ) -> Self {
        Self {
            loader: Loader::new(spinner_color_fn, message_color_fn, message, indicator),
            aborted: false,
            on_abort: None,
        }
    }

    /// Upstream `onAbort` property.
    pub fn set_on_abort(&mut self, on_abort: Option<Box<dyn FnMut()>>) {
        self.on_abort = on_abort;
    }

    /// Upstream `aborted` getter.
    pub fn aborted(&self) -> bool {
        self.aborted
    }

    /// Upstream `handleInput`: Escape cancels.
    pub fn handle_input(&mut self, data: &str) {
        if with_keybindings(|kb| kb.matches(data, "tui.select.cancel")) {
            self.abort();
        }
    }

    /// Upstream abort side effect (AbortController + onAbort hook).
    pub fn abort(&mut self) {
        self.aborted = true;
        if let Some(mut on_abort) = self.on_abort.take() {
            on_abort();
        }
    }

    /// Upstream `dispose` (= `stop`).
    pub fn dispose(&mut self) {
        self.loader.stop();
    }

    pub fn loader(&mut self) -> &mut Loader {
        &mut self.loader
    }

    pub fn message(&self) -> &str {
        self.loader.message()
    }
}

impl Component for CancellableLoader {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.loader.render(width)
    }

    fn invalidate(&mut self) {
        self.loader.invalidate();
    }

    fn handle_input(&mut self, data: &str) {
        self.handle_input(data);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding_agent::modes::interactive::theme::{load_builtin_theme, ColorMode, Theme};
    use std::sync::Arc;

    fn dark() -> Arc<Theme> {
        Arc::new(load_builtin_theme("dark", Some(ColorMode::Truecolor)).expect("dark theme"))
    }

    fn spinner() -> LoaderStyle {
        let theme = dark();
        Arc::new(move |s: &str| theme.fg("accent", s).expect("accent"))
    }

    fn muted() -> LoaderStyle {
        let theme = dark();
        Arc::new(move |s: &str| theme.fg("muted", s).expect("muted"))
    }

    #[test]
    fn loader_renders_indicator_and_message() {
        let mut loader = Loader::new(spinner(), muted(), "Working...", None);
        let rendered = loader.get_rendered_indicator();
        assert_eq!(rendered, "\x1b[38;2;167;152;215m⠋\x1b[39m".to_string());
        loader.advance_frame();
        assert!(loader.get_rendered_indicator().contains("⠙"));
        // advance wraps after the last frame
        for _ in 0..(DEFAULT_FRAMES.len() - 1) {
            loader.advance_frame();
        }
        assert!(loader.get_rendered_indicator().contains("⠋"));
        assert_eq!(loader.interval_ms(), DEFAULT_INTERVAL_MS);
    }

    #[test]
    fn loader_verbatim_indicator_and_empty_frames() {
        let mut loader = Loader::new(
            spinner(),
            muted(),
            "Uploading",
            Some(LoaderIndicatorOptions {
                frames: Some(vec!["▪".to_string(), "▪▪".to_string()]),
                interval_ms: Some(120),
            }),
        );
        assert!(loader.is_running());
        assert_eq!(loader.get_rendered_indicator(), "▪");
        assert_eq!(loader.interval_ms(), 120);
        loader.advance_frame();
        assert_eq!(loader.get_rendered_indicator(), "▪▪");
        // empty frames hide the indicator and stop the animation
        loader.set_indicator(Some(LoaderIndicatorOptions {
            frames: Some(Vec::new()),
            interval_ms: None,
        }));
        assert!(!loader.is_running());
        loader.advance_frame();
        assert_eq!(loader.get_rendered_indicator(), "");
    }

    #[test]
    fn loader_render_has_leading_blank_line() {
        let mut loader = Loader::new(spinner(), muted(), "Loading...", None);
        let lines = loader.render(30);
        assert_eq!(lines[0], "");
        // " ⠋ <accent>⠋</accent> <muted>Loading...</muted>" padded to the visible
        // width (oracle workingRender row bytes)
        assert!(lines[1].starts_with(
            " \x1b[38;2;167;152;215m⠋\x1b[39m \x1b[38;2;157;165;169mLoading...\x1b[39m"
        ));
        assert_eq!(crate::tui::utils::visible_width(&lines[1]), 30);
    }

    #[test]
    fn cancellable_loader_aborts_on_escape() {
        let mut loader = CancellableLoader::new(spinner(), muted(), "Working...", None);
        let aborted_flag = std::rc::Rc::new(std::cell::Cell::new(false));
        let handle = std::rc::Rc::clone(&aborted_flag);
        loader.set_on_abort(Some(Box::new(move || handle.set(true))));
        loader.handle_input("x");
        assert!(!loader.aborted());
        loader.handle_input("\x1b");
        assert!(loader.aborted());
        assert!(aborted_flag.get());
        loader.dispose();
    }
}
