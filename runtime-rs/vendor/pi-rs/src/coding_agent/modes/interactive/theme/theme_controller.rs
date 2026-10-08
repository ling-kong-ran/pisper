//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Port of upstream `coding-agent/src/modes/interactive/theme/theme-controller.ts`
//! (253 lines at v0.99.1 HEAD `2bbfcca43`, sha256
//! `b1ac907ac1ed5e1fbb754d93b42795605ca093cecdcfa3c661b3ed66f48a0d7f`) —
//! applies the theme setting and keeps it in sync with the terminal.
//!
//! Seams (see `../mod.rs`):
//! - **D1** — the fs watcher of upstream `initTheme`/`setTheme` is
//!   presentation and stays unported; `enableWatcher` arguments are dropped.
//! - Upstream holds `ui: TUI`; the port talks to a [`ThemeControllerUi`] trait
//!   mirroring the used surface (the ported
//!   [`Tui`](crate::tui::tui::Tui) implements the same methods).
//! - Upstream's closures capture `this`; Rust callbacks are `'static`, so
//!   reported colors and scheme reports are delivered into per-controller
//!   inboxes ([`InteractiveThemeController::deliver_pending_terminal_colors`] /
//!   [`InteractiveThemeController::deliver_pending_scheme_reports`]) that the
//!   owning shell drains into the controller from event-loop code.
//! - Upstream `requestTerminalColors` returns a Promise and
//!   `waitForTerminalColors()` awaits it; the port shares the settle state as
//!   a [`TerminalColorQuery`] flag that flips when the settle-time colors are
//!   delivered (late replies deliver through the same inbox afterwards).
//! - Upstream wraps `ui.queryTerminalColors` in try/catch (a failed start
//!   resolves `Promise.resolve({})` and applies no colors); the trait cannot
//!   throw, so implementations model a failed start by resolving with empty
//!   colors.

use std::sync::{Arc, Mutex};

use crate::coding_agent::modes::interactive::system_theme::SYSTEM_THEME_NAME;
use crate::tui::terminal_colors::{RgbColor, TerminalColorScheme, TerminalColors};

use super::{
    get_terminal_theme, init_theme, mark_terminal_colors_pending, parse_auto_theme_setting,
    resolve_theme_setting, set_terminal_color_scheme, set_terminal_colors, set_theme,
    set_theme_instance, TerminalTheme, Theme, ThemeResult,
};

/// How long the system theme stays grayscale before falling back to palette
/// indices. Terminals answer the trailing DA1 request right after the color
/// replies, so this only matters for terminals that answer neither. Replies
/// arriving later still apply.
pub const TERMINAL_QUERY_TIMEOUT_MS: f64 = 100.0;

/// A delivered set of reported terminal colors.
pub type TerminalColorsCallback = Box<dyn FnOnce(TerminalColors) + Send>;

/// A terminal light/dark report (mode 2031) listener.
pub type SchemeChangeListener = Box<dyn FnMut(&TerminalColorScheme) + Send>;

/// The TUI surface the controller uses (upstream holds `ui: TUI`).
pub trait ThemeControllerUi {
    /// Upstream `ui.queryTerminalColors({ timeoutMs, onLateReply })`: the
    /// settle callback fires when the query completes or times out; a reply
    /// arriving after the timeout fires the late-reply callback.
    fn query_terminal_colors(
        &mut self,
        timeout_ms: f64,
        on_resolve: TerminalColorsCallback,
        on_late_reply: Option<TerminalColorsCallback>,
    );
    /// Upstream `ui.setTerminalColorSchemeNotifications`.
    fn set_terminal_color_scheme_notifications(&mut self, enabled: bool);
    /// Upstream `ui.onTerminalColorSchemeChange`; returns an unsubscribe id.
    fn on_terminal_color_scheme_change(&mut self, listener: SchemeChangeListener) -> u64;
    /// Unsubscribe from [`ThemeControllerUi::on_terminal_color_scheme_change`].
    fn remove_terminal_color_scheme_listener(&mut self, id: u64);
    /// Upstream `ui.invalidate`.
    fn invalidate(&mut self);
    /// Upstream `ui.requestRender()` (no force argument).
    fn request_render(&mut self);
}

/// Upstream `requestTerminalColors`: query the terminal's colors and pass them
/// to `apply` when the query completes or times out, and again if the terminal
/// answers after the timeout. A failed query applies no colors (the `catch`
/// seam resolves empty). Settles after the first apply.
pub fn request_terminal_colors(
    ui: &mut dyn ThemeControllerUi,
    apply: Arc<Mutex<dyn FnMut(TerminalColors) + Send>>,
) {
    let late = Arc::clone(&apply);
    ui.query_terminal_colors(
        TERMINAL_QUERY_TIMEOUT_MS,
        Box::new(move |colors: TerminalColors| {
            (apply.lock().expect("terminal colors apply"))(colors);
        }),
        Some(Box::new(move |colors: TerminalColors| {
            (late.lock().expect("terminal colors apply"))(colors);
        })),
    );
}

/// Upstream `sameRgb`.
fn same_rgb(a: Option<RgbColor>, b: Option<RgbColor>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a == b,
        (None, None) => true,
        _ => false,
    }
}

/// Upstream `sameTerminalColors`.
fn same_terminal_colors(a: &TerminalColors, b: &TerminalColors) -> bool {
    if !same_rgb(a.foreground, b.foreground) || !same_rgb(a.background, b.background) {
        return false;
    }
    match (&a.palette, &b.palette) {
        // `a.palette === b.palette` (reference identity) collapses to
        // element equality on the shared values.
        (Some(a), Some(b)) => {
            a.len() == b.len()
                && a.iter()
                    .zip(b.iter())
                    .all(|(first, second)| same_rgb(Some(*first), Some(*second)))
        }
        (None, None) => true,
        _ => false,
    }
}

/// Constructor options (upstream `options`).
pub struct InteractiveThemeControllerOptions {
    /// Upstream `getSettingsManager: () => SettingsManager` reduced to the
    /// only setting the controller reads (`getThemeSetting()`).
    pub get_theme_setting: Box<dyn Fn() -> Option<String> + Send + Sync>,
    pub show_error: Box<dyn Fn(&str) + Send + Sync>,
    pub on_changed: Box<dyn FnMut() + Send>,
    pub initial_theme_setting: Option<String>,
}

/// Settles when the latest color query completed or timed out, and its colors
/// applied (upstream `terminalColorQuery: Promise<void>`, which starts
/// resolved).
pub type TerminalColorQuery = Arc<Mutex<bool>>;

/// Upstream `InteractiveThemeController`. The theme applies immediately, and
/// the terminal's colors update it when they arrive; the system theme renders
/// in grayscale until then. Callers that bake theme colors into content can
/// wait for the colors with
/// [`InteractiveThemeController::wait_for_terminal_colors`].
pub struct InteractiveThemeController {
    get_theme_setting: Box<dyn Fn() -> Option<String> + Send + Sync>,
    show_error: Box<dyn Fn(&str) + Send + Sync>,
    on_changed: Box<dyn FnMut() + Send>,
    current_theme_setting: Option<String>,
    // Last reported colors; a query that times out keeps them instead of
    // erasing them.
    terminal_colors: Option<TerminalColors>,
    active_theme_name: Option<String>,
    auto_sync_enabled: bool,
    terminal_color_scheme_unsubscribe: Option<u64>,
    // Settles when the latest color query completed or timed out, and its
    // colors applied.
    terminal_color_query: TerminalColorQuery,
    // Delivery inboxes for the `'static` tui callbacks (upstream captures
    // `this` in the closures instead).
    reported_colors_inbox: Arc<Mutex<Vec<TerminalColors>>>,
    scheme_reports_inbox: Arc<Mutex<Vec<TerminalTheme>>>,
}

impl InteractiveThemeController {
    pub fn new(ui: &mut dyn ThemeControllerUi, options: InteractiveThemeControllerOptions) -> Self {
        let InteractiveThemeControllerOptions {
            get_theme_setting,
            show_error,
            on_changed,
            initial_theme_setting,
        } = options;
        let mut controller = Self {
            get_theme_setting,
            show_error,
            on_changed,
            current_theme_setting: initial_theme_setting,
            terminal_colors: None,
            active_theme_name: None,
            auto_sync_enabled: false,
            terminal_color_scheme_unsubscribe: None,
            terminal_color_query: Arc::new(Mutex::new(true)),
            reported_colors_inbox: Arc::new(Mutex::new(Vec::new())),
            scheme_reports_inbox: Arc::new(Mutex::new(Vec::new())),
        };
        controller.active_theme_name = Some(controller.resolve_theme_name());
        // The system theme starts in grayscale; color follows once the
        // terminal reports its colors.
        mark_terminal_colors_pending();
        init_theme(controller.active_theme_name.as_deref());
        controller.bind_terminal_color_scheme_listener(ui);
        controller
    }

    pub fn rebind_tui(&mut self, ui: &mut dyn ThemeControllerUi) {
        if let Some(id) = self.terminal_color_scheme_unsubscribe.take() {
            ui.remove_terminal_color_scheme_listener(id);
        }
        self.bind_terminal_color_scheme_listener(ui);
        ui.set_terminal_color_scheme_notifications(self.auto_sync_enabled);
    }

    /// Apply the theme setting now and query the terminal's colors, which
    /// update the theme when they arrive. Theme pairs and the system theme
    /// follow terminal appearance changes.
    pub fn apply_from_settings(&mut self, ui: &mut dyn ThemeControllerUi) {
        let theme_setting = self.get_theme_setting_value();
        let theme_name = self.resolve_theme_name();
        let auto = parse_auto_theme_setting(theme_setting.as_deref()).is_some()
            || theme_name == SYSTEM_THEME_NAME;
        self.set_auto_sync(ui, auto);
        self.apply_theme_name(ui, &theme_name, theme_setting.is_some());
        self.query_terminal_colors(ui);
    }

    /// Wait until the latest color query completed or timed out. Content that
    /// bakes theme colors into strings, such as the startup header, should be
    /// built after this. Terminals answer the DA1 request right after the
    /// color replies, so this only takes the full timeout when a terminal
    /// answers nothing.
    pub fn wait_for_terminal_colors(&self) -> TerminalColorQuery {
        Arc::clone(&self.terminal_color_query)
    }

    pub fn get_theme_selection(&self) -> Option<String> {
        self.current_theme_setting
            .clone()
            .or_else(|| (self.get_theme_setting)())
            .or_else(|| self.active_theme_name.clone())
    }

    pub fn set_theme_name(
        &mut self,
        ui: &mut dyn ThemeControllerUi,
        theme_name: &str,
    ) -> ThemeResult {
        self.set_auto_sync(ui, theme_name == SYSTEM_THEME_NAME);
        let result = self.apply_theme_name(ui, theme_name, false);
        if result.success {
            self.current_theme_setting = Some(theme_name.to_string());
        }
        result
    }

    pub fn set_theme_setting(&mut self, ui: &mut dyn ThemeControllerUi, theme_setting: &str) {
        self.current_theme_setting = Some(theme_setting.to_string());
        self.apply_from_settings(ui);
    }

    pub fn set_theme_instance(&mut self, ui: &mut dyn ThemeControllerUi, theme_instance: Theme) {
        self.set_auto_sync(ui, false);
        set_theme_instance(theme_instance);
        self.active_theme_name = Some("<in-memory>".to_string());
        self.notify_changed(ui);
    }

    pub fn preview(&mut self, ui: &mut dyn ThemeControllerUi, theme_setting_or_name: &str) {
        let Some(theme_name) =
            resolve_theme_setting(Some(theme_setting_or_name), get_terminal_theme())
                .or_else(|| self.active_theme_name.clone())
        else {
            return;
        };
        if set_theme(&theme_name).success {
            ui.invalidate();
            ui.request_render();
        }
    }

    pub fn disable_auto_sync(&mut self, ui: &mut dyn ThemeControllerUi) {
        self.set_auto_sync(ui, false);
    }

    pub fn dispose(&mut self, ui: &mut dyn ThemeControllerUi) {
        self.set_auto_sync(ui, false);
        if let Some(id) = self.terminal_color_scheme_unsubscribe.take() {
            ui.remove_terminal_color_scheme_listener(id);
        }
    }

    pub fn get_terminal_theme(&self) -> TerminalTheme {
        get_terminal_theme()
    }

    /// Deliver pending reported colors (settle + late replies, in arrival
    /// order) to [`InteractiveThemeController::apply_terminal_colors`], and
    /// pending scheme reports to the handler. The `'static` tui callbacks
    /// push here; the owning shell drains.
    pub fn deliver_pending(&mut self, ui: &mut dyn ThemeControllerUi) {
        let reported =
            std::mem::take(&mut *self.reported_colors_inbox.lock().expect("reported inbox"));
        for colors in reported {
            self.apply_terminal_colors(ui, colors);
        }
        let schemes = std::mem::take(&mut *self.scheme_reports_inbox.lock().expect("scheme inbox"));
        for scheme in schemes {
            self.apply_terminal_color_scheme_change(ui, scheme);
        }
    }

    /// Record reported colors: themes use the default colors for tokens set to
    /// "", the system theme is generated from all of them, and light/dark
    /// detection uses them. Re-renders only when they changed.
    pub fn apply_terminal_colors(
        &mut self,
        ui: &mut dyn ThemeControllerUi,
        reported: TerminalColors,
    ) {
        let previous = self.terminal_colors.clone();
        let next = TerminalColors {
            foreground: reported
                .foreground
                .or_else(|| previous.as_ref().and_then(|colors| colors.foreground)),
            background: reported
                .background
                .or_else(|| previous.as_ref().and_then(|colors| colors.background)),
            palette: reported
                .palette
                .or_else(|| previous.as_ref().and_then(|colors| colors.palette.clone())),
        };
        // Re-rendering rebuilds every component, so skip it when nothing
        // changed (including timeouts).
        if let Some(previous) = &previous {
            if same_terminal_colors(previous, &next) {
                return;
            }
        }
        self.terminal_colors = Some(next.clone());
        set_terminal_colors(next);
        self.reapply_for_terminal(ui);
        ui.invalidate();
        ui.request_render();
    }

    /// The terminal reported a light/dark switch. Its colors changed too, so
    /// query them again: they decide the appearance. The reported scheme only
    /// matters for terminals that do not report their background.
    pub fn apply_terminal_color_scheme_change(
        &mut self,
        ui: &mut dyn ThemeControllerUi,
        terminal_theme: TerminalTheme,
    ) {
        if !self.auto_sync_enabled {
            return;
        }
        let previous = get_terminal_theme();
        set_terminal_color_scheme(Some(terminal_theme));
        if get_terminal_theme() != previous {
            self.reapply_for_terminal(ui);
        }
        self.query_terminal_colors(ui);
    }

    fn get_theme_setting_value(&self) -> Option<String> {
        self.current_theme_setting
            .clone()
            .or_else(|| (self.get_theme_setting)())
    }

    /// The theme for the current setting and terminal appearance. Without a
    /// setting, pi uses the system theme.
    fn resolve_theme_name(&self) -> String {
        resolve_theme_setting(
            self.get_theme_setting_value().as_deref(),
            get_terminal_theme(),
        )
        .unwrap_or_else(|| SYSTEM_THEME_NAME.to_string())
    }

    fn apply_theme_name(
        &mut self,
        ui: &mut dyn ThemeControllerUi,
        theme_name: &str,
        show_error: bool,
    ) -> ThemeResult {
        let result = set_theme(theme_name);
        self.active_theme_name = Some(if result.success {
            theme_name.to_string()
        } else {
            SYSTEM_THEME_NAME.to_string()
        });
        self.notify_changed(ui);
        if !result.success && show_error {
            (self.show_error)(&format!(
                "Failed to load theme \"{theme_name}\": {}\nFell back to the system theme.",
                result.error.as_deref().unwrap_or_default()
            ));
        }
        result
    }

    /// Query the terminal's colors without waiting for them;
    /// [`InteractiveThemeController::wait_for_terminal_colors`] waits for this
    /// query. Each query installs a fresh pending flag (upstream replaces
    /// `terminalColorQuery` with the new promise).
    fn query_terminal_colors(&mut self, ui: &mut dyn ThemeControllerUi) {
        let inbox = Arc::clone(&self.reported_colors_inbox);
        let settle = Arc::new(Mutex::new(false));
        self.terminal_color_query = Arc::clone(&settle);
        let apply = Arc::new(Mutex::new(move |colors: TerminalColors| {
            inbox.lock().expect("reported inbox").push(colors);
            *settle.lock().expect("terminal color query") = true;
        })) as Arc<Mutex<dyn FnMut(TerminalColors) + Send>>;
        request_terminal_colors(ui, apply);
    }

    /// Re-apply the setting after the terminal's colors or appearance changed:
    /// regenerate the system theme, or switch the theme of a pair. Themes set
    /// through extensions or previews are left alone.
    fn reapply_for_terminal(&mut self, ui: &mut dyn ThemeControllerUi) {
        if self.active_theme_name.as_deref() == Some("<in-memory>") {
            return;
        }
        let theme_name = self.resolve_theme_name();
        if theme_name == SYSTEM_THEME_NAME
            || Some(theme_name.as_str()) != self.active_theme_name.as_deref()
        {
            self.apply_theme_name(ui, &theme_name, false);
        }
    }

    fn set_auto_sync(&mut self, ui: &mut dyn ThemeControllerUi, enabled: bool) {
        if self.auto_sync_enabled == enabled {
            return;
        }
        self.auto_sync_enabled = enabled;
        ui.set_terminal_color_scheme_notifications(enabled);
    }

    fn bind_terminal_color_scheme_listener(&mut self, ui: &mut dyn ThemeControllerUi) {
        let inbox: Arc<Mutex<Vec<TerminalTheme>>> = Arc::clone(&self.scheme_reports_inbox);
        self.terminal_color_scheme_unsubscribe = Some(ui.on_terminal_color_scheme_change(
            Box::new(move |scheme: &TerminalColorScheme| {
                inbox
                    .lock()
                    .expect("scheme inbox")
                    .push(TerminalTheme::from_scheme(*scheme));
            }),
        ));
    }

    fn notify_changed(&mut self, ui: &mut dyn ThemeControllerUi) {
        ui.invalidate();
        (self.on_changed)();
    }
}
