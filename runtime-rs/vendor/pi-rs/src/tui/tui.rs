//! Port of upstream `packages/tui/src/tui.ts` (the TUI core event loop:
//! `TuiBase`, `Container`, `compositeTuiLine`, overlay/focus machinery,
//! render scheduling, terminal queries).
//!
//! Earlier slices already extracted parts of this file into dedicated modules;
//! this module is the cohesive, upstream-authoritative port and reuses them
//! where their behavior is byte-equivalent:
//! - `component.rs`: `Component` trait, mouse-event vocabulary,
//!   `Focusable`, [`CURSOR_MARKER`] (tui.ts lines 21-168).
//! - `mouse_dispatch.rs`: [`retarget_mouse_event`] and the `MouseAction`
//!   protocol; `overlay.rs`: [`OverlayAnchor`], [`OverlayBounds`],
//!   `compositeTuiLine`.
//!
//! Disclosed substitutions for review:
//! - JS object identity becomes [`ComponentHandle`] (Rc-backed, id-compared).
//!   Upstream closure-based `OverlayHandle` becomes an id plus methods on
//!   [`Tui`] (`overlay_*`), which is the same state machine under Rust's
//!   borrow rules.
//! - `TuiBase` is abstract over `doRender`; the concrete renderers
//!   (`tui-main-screen.ts` / `tui-alt-screen.ts`) are separate slices. The
//!   abstract method is the [`TuiRenderer`] trait installed at construction;
//!   its hooks carry the upstream `resetRenderState`/`beforeTerminalStart`/
//!   `afterTerminalStart`/`beforeTerminalStop`/`afterTerminalStop` overrides.
//! - Node's `process.nextTick`/`setTimeout`/`performance.now` become an
//!   explicit scheduler on [`Tui`]: a FIFO next-tick queue drained by
//!   [`Tui::run_next_ticks`], a timer list fired by [`Tui::fire_due_timer`],
//!   and an injectable millisecond clock. Draining next-ticks before firing
//!   timers reproduces Node's microtask-before-macrotask ordering; timers
//!   fire in (deadline, id) order like Node.
//! - The terminal input/resize callbacks (`terminal.start`) cannot capture
//!   the `Tui` itself, so they enqueue into a shared event queue drained by
//!   [`Tui::poll_terminal_events`] — the Node stream-callback boundary made
//!   explicit. Event order is preserved.
//! - `queryTerminalColors` returns a Promise upstream; the port takes a
//!   one-shot `on_resolve` callback (resolved exactly once, with the same
//!   timeout semantics) plus an optional `on_late_reply` receiver.
//! - `setCellDimensions` is a terminal-image.ts global; the store belongs to
//!   that slice, so [`Tui`] exposes a sink closure receiving the parsed
//!   `CSI 6 ; height ; width t` values.
//! - Upstream checks `focusedComponent.handleInput` method presence; the Rust
//!   trait method always exists (no-op default), so a focused component
//!   without an input handler still triggers the (no-change) immediate
//!   render. Focused test/production components all define handlers.
//! - `OverlayOptions` numeric fields are JS numbers; the port computes layout
//!   in `f64` like upstream and truncates toward zero only at index/slice
//!   boundaries (upstream array indexing performs the same coercion).
//! - `VIEWPORT_TUI` (Symbol.for brand) becomes a bool brand plus the
//!   [`ViewportTui`] extension trait for the future viewport wrapper.
//! - The raw-mode/resize/native-helper OS shell of `ProcessTerminal` is the
//!   `terminal.rs` seam (M5); this module drives the `Terminal` trait only.

use std::cell::RefCell;
use std::collections::{BTreeSet, VecDeque};
use std::rc::Rc;
use std::sync::{Arc, Mutex, OnceLock};

use regex::Regex;

use crate::tui::component::{Component, TuiMouseEvent, TuiMouseEventResult, CURSOR_MARKER};
use crate::tui::component_mouse::{ComponentHandle, MouseAction};
use crate::tui::component_overlay::contains_component;
use crate::tui::keys::{is_key_release, matches_key};
use crate::tui::overlay::{composite_tui_line, OverlayAnchor, OverlayBounds};
use crate::tui::screen::InputListenerResult;
use crate::tui::terminal::Terminal;
use crate::tui::terminal_colors::{
    parse_osc_color_response, parse_terminal_color_scheme_report, OscColorTarget, RgbColor,
    TerminalColorScheme, TerminalColors,
};
use crate::tui::terminal_image::{get_capabilities, is_image_line};
use crate::tui::utils::{normalize_terminal_output, slice_by_column, visible_width};

pub use crate::tui::mouse_dispatch::retarget_mouse_event;

/// Upstream `SEGMENT_RESET`.
pub const SEGMENT_RESET: &str = "\x1b[0m\x1b]8;;\x07";

/// Upstream `TuiBase.MIN_RENDER_INTERVAL_MS`.
pub const MIN_RENDER_INTERVAL_MS: f64 = 16.0;

/// Upstream `VIEWPORT_TUI` (`Symbol.for("@earendil-works/pi-tui/viewport")`).
pub const VIEWPORT_TUI: &str = "@earendil-works/pi-tui/viewport";

/// Upstream `TuiMode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TuiMode {
    Regular,
    Fullscreen,
}

/// Upstream `TuiStopOptions`.
#[derive(Clone, Copy, Debug, Default)]
pub struct TuiStopOptions {
    /// Leave renderer output in place for another TUI taking over.
    pub preserve_screen: bool,
}

/// Upstream `TuiInputListenerResult` (reuse of the screen.rs projection).
pub type TuiInputListenerResult = InputListenerResult;

/// Upstream `TuiInputListener`.
pub type TuiInputListener = Box<dyn FnMut(&str) -> Option<TuiInputListenerResult> + Send>;

/// Upstream `OverlayMargin`: JS numbers may be negative (clamped to 0 later).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OverlayMarginValue {
    pub top: Option<f64>,
    pub right: Option<f64>,
    pub bottom: Option<f64>,
    pub left: Option<f64>,
}

/// Upstream `margin: OverlayMargin | number`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OverlayMarginSpec {
    Number(f64),
    Edges(OverlayMarginValue),
}

/// Upstream `SizeValue` (`number | \`${number}%\``). The percent string is
/// kept verbatim so the upstream format check (`^(\d+(?:\.\d+)?)%$`) decides
/// validity, with the center fallback for invalid formats.
#[derive(Clone, Debug, PartialEq)]
pub enum SizeValue {
    Number(f64),
    Percent(String),
}

/// Upstream `OverlayOptions`.
#[derive(Default)]
pub struct OverlayOptions {
    /// Width in columns, or percentage of terminal width.
    pub width: Option<SizeValue>,
    /// Minimum width in columns.
    pub min_width: Option<f64>,
    /// Maximum height in rows, or percentage of terminal height.
    pub max_height: Option<SizeValue>,
    /// Anchor point for positioning (default: center).
    pub anchor: Option<OverlayAnchor>,
    /// Horizontal offset from anchor (positive = right).
    pub offset_x: Option<f64>,
    /// Vertical offset from anchor (positive = down).
    pub offset_y: Option<f64>,
    /// Row position: absolute number or percentage.
    pub row: Option<SizeValue>,
    /// Column position: absolute number or percentage.
    pub col: Option<SizeValue>,
    /// Margin from terminal edges.
    pub margin: Option<OverlayMarginSpec>,
    /// Only render when this returns true (called each cycle with dimensions).
    pub visible: Option<Box<dyn FnMut(usize, usize) -> bool + 'static>>,
    /// Don't capture keyboard focus when shown.
    pub non_capturing: bool,
}

/// Upstream `OverlayUnfocusOptions`.
#[derive(Clone, Debug, Default)]
pub struct OverlayUnfocusOptions {
    /// Explicit target to focus after releasing the overlay.
    pub target: Option<ComponentHandle>,
}

/// Upstream closure `OverlayHandle` becomes an id; the state machine lives in
/// the `overlay_*` methods on [`Tui`]. Clone/equality address the same entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OverlayHandle {
    entry_id: u64,
}

/// Upstream `OverlayStackEntry`.
struct OverlayStackEntry {
    id: u64,
    component: ComponentHandle,
    options: Option<OverlayOptions>,
    pre_focus: Option<ComponentHandle>,
    hidden: bool,
    focus_order: u64,
    bounds: Option<OverlayBounds>,
}

/// Upstream `RenderedOverlayLayout`: last rendered terminal-relative rectangle.
#[derive(Clone, Debug)]
pub struct RenderedOverlayLayout {
    pub entry_id: u64,
    pub component: ComponentHandle,
    pub row: i64,
    pub col: i64,
    pub width: usize,
    pub height: usize,
}

/// Upstream `OverlayBlockedFocusResume`.
#[derive(Clone, Debug)]
enum OverlayFocusResume {
    RestoreOverlay,
    FocusTarget { target: Option<ComponentHandle> },
}

/// Upstream `OverlayFocusRestoreState` (`inactive`/`eligible`/`blocked`).
#[derive(Clone, Debug, Default)]
enum OverlayFocusRestore {
    #[default]
    Inactive,
    Eligible {
        overlay_id: u64,
    },
    Blocked {
        overlay_id: u64,
        blocked_by: ComponentHandle,
        resume: OverlayFocusResume,
    },
}

/// Upstream `OverlayFocusRestorePolicy`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OverlayFocusRestorePolicy {
    Clear,
    Preserve,
}

/// Result of [`Tui::resolve_overlay_layout`] (JS numbers preserved).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OverlayLayoutResult {
    pub width: f64,
    pub row: f64,
    pub col: f64,
    pub max_height: Option<f64>,
}

/// Result of dispatching an event to a concrete component with the retained
/// target and coordinate transform (upstream `TuiMouseDispatchResult`).
#[derive(Clone, Debug)]
pub struct TuiMouseDispatchResult {
    pub result: TuiMouseEventResult,
    pub target: TuiMouseDispatchTarget,
    /// Keyboard focus target, which may be a delegating parent container.
    pub focus_target: Option<ComponentHandle>,
}

/// Upstream `TuiMouseDispatchTarget`.
#[derive(Clone, Debug)]
pub struct TuiMouseDispatchTarget {
    pub component: ComponentHandle,
    pub origin_x: i64,
    pub origin_y: i64,
    pub width: usize,
    pub height: usize,
}

/// Result of [`Tui::dispatch_mouse_to_overlay`].
#[derive(Clone, Debug)]
pub struct OverlayMouseDispatch {
    pub hit: bool,
    pub result: Option<TuiMouseDispatchResult>,
}

/// Upstream `dispatchMouseEvent`: dispatch an event to a component and retain
/// the exact target and coordinate transform. Nested/forwarded actions are
/// resolved recursively; container focus delegation substitutes the container
/// handle (upstream `focusTarget: this`).
pub fn dispatch_mouse_event(
    handle: &ComponentHandle,
    event: &TuiMouseEvent,
) -> Option<TuiMouseDispatchResult> {
    let action = handle.with_mut(|c| c.mouse_action(event))?;
    match action {
        MouseAction::Dispatched(result) => {
            // The component forwarded the event to a child it hosts. Like a
            // delegating container, it routes keys to that child itself, so it
            // keeps keyboard focus. Focusing the child directly would leave
            // focus on a detached component once the host removes it, e.g. a
            // closed settings submenu.
            if result.result.focus && handle.with_mut(|c| c.delegates_mouse_focus()) {
                Some(TuiMouseDispatchResult {
                    result: result.result,
                    focus_target: Some(handle.clone()),
                    target: TuiMouseDispatchTarget {
                        component: result.target.component.clone(),
                        origin_x: result.target.origin_x,
                        origin_y: result.target.origin_y,
                        width: result.target.width,
                        height: result.target.height,
                    },
                })
            } else {
                Some(TuiMouseDispatchResult {
                    result: result.result,
                    focus_target: result.focus_target,
                    target: TuiMouseDispatchTarget {
                        component: result.target.component.clone(),
                        origin_x: result.target.origin_x,
                        origin_y: result.target.origin_y,
                        width: result.target.width,
                        height: result.target.height,
                    },
                })
            }
        }
        MouseAction::Direct(result) => {
            if !result.handled && !result.capture && !result.focus {
                return None;
            }
            Some(build_dispatch_result(handle.clone(), result, event))
        }
        MouseAction::Forward {
            child,
            event: forwarded,
            delegate_focus,
            fallback_to_self,
        } => {
            // Upstream Container.handleMouse: dispatch to the child; when the
            // child requests focus and the container owns a keyboard handler,
            // the container becomes the focus target.
            match dispatch_mouse_event(&child, &forwarded) {
                Some(mut inner) => {
                    if inner.result.focus && delegate_focus {
                        inner.focus_target = Some(handle.clone());
                    }
                    Some(inner)
                }
                None if fallback_to_self => {
                    // MouseRegion-style fallback: the forwarder's own handler
                    // runs only when the child declined.
                    let result = handle.with_mut(|c| c.handle_mouse(event))?;
                    if !result.handled && !result.capture && !result.focus {
                        return None;
                    }
                    Some(build_dispatch_result(handle.clone(), result, event))
                }
                None => None,
            }
        }
    }
}

fn build_dispatch_result(
    component: ComponentHandle,
    result: TuiMouseEventResult,
    event: &TuiMouseEvent,
) -> TuiMouseDispatchResult {
    TuiMouseDispatchResult {
        focus_target: result.focus.then(|| component.clone()),
        result: TuiMouseEventResult {
            handled: true,
            ..result
        },
        target: TuiMouseDispatchTarget {
            component,
            origin_x: event.screen_x - event.x,
            origin_y: event.screen_y - event.y,
            width: event.width,
            height: event.height,
        },
    }
}

/// Upstream `isFocusable` (via the [`Component::is_focusable`] trait check).
pub fn is_focusable(component: &ComponentHandle) -> bool {
    component.with_mut(|c| c.is_focusable())
}

fn same_component(a: &ComponentHandle, b: &ComponentHandle) -> bool {
    a.id() == b.id()
}

fn same_component_opt(a: &Option<ComponentHandle>, b: &ComponentHandle) -> bool {
    a.as_ref().is_some_and(|handle| same_component(handle, b))
}

/// Upstream `parseSizeValue`.
fn parse_size_value(value: Option<&SizeValue>, reference_size: f64) -> Option<f64> {
    let value = value?;
    match value {
        SizeValue::Number(number) => Some(*number),
        SizeValue::Percent(text) => percent_regex().captures(text).map(|captures| {
            (reference_size * captures[1].parse::<f64>().unwrap_or(f64::NAN) / 100.0).floor()
        }),
    }
}

fn percent_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^(\d+(?:\.\d+)?)%$").unwrap())
}

fn cell_size_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\x1b\[6;(\d+);(\d+)t$").unwrap())
}

/// The abstract `doRender` plus the protected subclass hooks
/// (`resetRenderState`, `beforeTerminalStart`, `afterTerminalStart`,
/// `beforeTerminalStop`, `afterTerminalStop`). Concrete renderers
/// (tui-main-screen / tui-alt-screen) implement this trait.
pub trait TuiRenderer {
    /// Upstream abstract `doRender`.
    fn do_render(&mut self, tui: &mut Tui);

    fn reset_render_state(&mut self, _tui: &mut Tui) {}
    fn before_terminal_start(&mut self, _tui: &mut Tui) {}
    fn after_terminal_start(&mut self, _tui: &mut Tui) {}
    fn before_terminal_stop(&mut self, _tui: &mut Tui, _options: &TuiStopOptions) {}
    fn after_terminal_stop(&mut self, _tui: &mut Tui, _options: &TuiStopOptions) {}
}

/// Upstream `ViewportTUI` extension surface (brand + `setLayoutRoot`).
pub trait ViewportTui {
    fn set_layout_root(&mut self, component: Option<ComponentHandle>);
}

/// Upstream `isViewportTUI`.
pub fn is_viewport_tui(tui: &Tui) -> bool {
    tui.viewport_brand
}

enum TerminalEvent {
    Input(String),
    Resize,
}

enum NextTickWork {
    ScheduleRender,
    ImmediateRender,
}

enum TimerWork {
    RenderFrame,
    TerminalColorTimeout(u64),
}

struct TimerEntry {
    id: u64,
    deadline_ms: f64,
    work: TimerWork,
}

/// Upstream `PendingTerminalColorQuery`.
struct TerminalColorQuery {
    seq: u64,
    foreground: Option<RgbColor>,
    background: Option<RgbColor>,
    palette: Vec<Option<RgbColor>>,
    /// Targets that already replied, so duplicates do not count twice.
    replied: BTreeSet<String>,
    /// Receives the result: the resolve callback until the timeout, then
    /// `on_late_reply`. Unset once the query completed (on the DA1 reply or
    /// once every color replied); later replies are ignored.
    deliver: Option<Box<dyn FnOnce(TerminalColors)>>,
    /// Receives the replies if the query completes after the timeout.
    on_late_reply: Option<Box<dyn FnOnce(TerminalColors)>>,
    timer_id: Option<u64>,
}

impl TerminalColorQuery {
    /// Vacant placeholder for taking the front query out of the deque.
    fn vacant() -> Self {
        Self {
            seq: 0,
            foreground: None,
            background: None,
            palette: Vec::new(),
            replied: BTreeSet::new(),
            deliver: None,
            on_late_reply: None,
            timer_id: None,
        }
    }
}

/// Upstream `TERMINAL_PALETTE_SIZE`.
const TERMINAL_PALETTE_SIZE: usize = 16;
/// OSC 10 and 11 plus OSC 4 for every palette color.
const TERMINAL_COLOR_REPLY_COUNT: usize = 2 + TERMINAL_PALETTE_SIZE;
/// Default colors, palette colors 0-15, and a trailing primary device
/// attributes (DA1) request. Every terminal answers DA1 and terminals answer
/// in order, so the DA1 reply marks the end of the color replies, including
/// for terminals that ignore the color queries.
const TERMINAL_COLOR_QUERY: &str = "\x1b]10;?\x07\x1b]11;?\x07\x1b]4;0;?\x07\x1b]4;1;?\x07\x1b]4;2;?\x07\x1b]4;3;?\x07\x1b]4;4;?\x07\x1b]4;5;?\x07\x1b]4;6;?\x07\x1b]4;7;?\x07\x1b]4;8;?\x07\x1b]4;9;?\x07\x1b]4;10;?\x07\x1b]4;11;?\x07\x1b]4;12;?\x07\x1b]4;13;?\x07\x1b]4;14;?\x07\x1b]4;15;?\x07\x1b[c";
/// Upstream `DEVICE_ATTRIBUTES_RESPONSE_PATTERN`.
fn is_device_attributes_response(data: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\x1b\[\?[\d;]*c$").unwrap())
        .is_match(data)
}

/// Test hook: the exact `TERMINAL_COLOR_QUERY` bytes written per query.
#[cfg(test)]
pub(crate) const TERMINAL_COLOR_QUERY_FOR_TEST: &str = TERMINAL_COLOR_QUERY;

type SchemeListener = Box<dyn FnMut(&TerminalColorScheme)>;

/// Upstream `TuiBase` / `TUI`: the main terminal-UI event loop.
pub struct Tui {
    mode: TuiMode,
    terminal: Rc<RefCell<dyn Terminal>>,
    renderer: Option<Box<dyn TuiRenderer>>,
    // Container state (TuiBase extends Container).
    children: Vec<ComponentHandle>,
    mouse_layout: Option<(usize, Vec<(ComponentHandle, usize)>)>,
    // Focus.
    focused_component: Option<ComponentHandle>,
    // Input.
    input_listeners: Vec<(u64, TuiInputListener)>,
    next_listener_id: u64,
    /// Global callback for the debug key (Shift+Ctrl+D).
    pub on_debug: Option<Box<dyn FnMut() + Send>>,
    // Render scheduling.
    render_requested: bool,
    immediate_render_scheduled: bool,
    render_timer: Option<u64>,
    last_render_at: f64,
    show_hardware_cursor: bool,
    clear_on_shrink: bool,
    full_redraw_count: usize,
    stopped: bool,
    // Terminal queries.
    /// Color queries waiting for their DA1 reply, oldest first. Terminals
    /// answer in order, so color replies belong to the oldest one. Queries
    /// stay here after a timeout to collect late replies.
    pending_terminal_color_queries: VecDeque<TerminalColorQuery>,
    next_query_seq: u64,
    terminal_color_scheme_listeners: Vec<(u64, SchemeListener)>,
    next_scheme_listener_id: u64,
    scheme_notifications_enabled: bool,
    cell_dimensions_sink: Box<dyn FnMut(u32, u32) + 'static>,
    /// Upstream `logDirectory`: debug/crash log directory.
    pub log_directory: Option<String>,
    // Overlay stack.
    focus_order_counter: u64,
    overlay_stack: Vec<OverlayStackEntry>,
    next_overlay_id: u64,
    rendered_overlay_layouts: Vec<RenderedOverlayLayout>,
    overlay_focus_restore: OverlayFocusRestore,
    // Viewport brand (upstream Symbol.for brand).
    viewport_brand: bool,
    // Scheduler seam.
    clock: Box<dyn Fn() -> f64 + 'static>,
    next_ticks: VecDeque<NextTickWork>,
    timers: Vec<TimerEntry>,
    next_timer_id: u64,
    terminal_events: Arc<Mutex<VecDeque<TerminalEvent>>>,
}

impl Tui {
    /// Upstream `TuiBase` constructor (`showHardwareCursor`/`logDirectory`
    /// optional). The concrete renderer is required — upstream subclasses
    /// provide `doRender`.
    pub fn new(
        terminal: Rc<RefCell<dyn Terminal>>,
        renderer: Box<dyn TuiRenderer>,
        mode: TuiMode,
        show_hardware_cursor: Option<bool>,
        log_directory: Option<String>,
    ) -> Self {
        Self {
            mode,
            terminal,
            renderer: Some(renderer),
            children: Vec::new(),
            mouse_layout: None,
            focused_component: None,
            input_listeners: Vec::new(),
            next_listener_id: 0,
            on_debug: None,
            render_requested: false,
            immediate_render_scheduled: false,
            render_timer: None,
            last_render_at: 0.0,
            show_hardware_cursor: show_hardware_cursor.unwrap_or(false),
            clear_on_shrink: false,
            full_redraw_count: 0,
            stopped: false,
            pending_terminal_color_queries: VecDeque::new(),
            next_query_seq: 0,
            terminal_color_scheme_listeners: Vec::new(),
            next_scheme_listener_id: 0,
            scheme_notifications_enabled: false,
            cell_dimensions_sink: Box::new(|_, _| {}),
            log_directory,
            focus_order_counter: 0,
            overlay_stack: Vec::new(),
            next_overlay_id: 0,
            rendered_overlay_layouts: Vec::new(),
            overlay_focus_restore: OverlayFocusRestore::Inactive,
            viewport_brand: false,
            clock: Box::new(default_clock()),
            next_ticks: VecDeque::new(),
            timers: Vec::new(),
            next_timer_id: 0,
            terminal_events: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    pub fn mode(&self) -> TuiMode {
        self.mode
    }

    pub fn terminal(&self) -> Rc<RefCell<dyn Terminal>> {
        self.terminal.clone()
    }

    /// Replace the wall clock (tests inject a virtual clock).
    pub fn set_clock(&mut self, clock: Box<dyn Fn() -> f64 + 'static>) {
        self.clock = clock;
    }

    /// Install the `setCellDimensions` sink (terminal-image global seam).
    pub fn set_cell_dimensions_sink(&mut self, sink: Box<dyn FnMut(u32, u32) + 'static>) {
        self.cell_dimensions_sink = sink;
    }

    /// Set the `Symbol.for("@earendil-works/pi-tui/viewport")` brand.
    pub fn set_viewport_brand(&mut self, branded: bool) {
        self.viewport_brand = branded;
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped
    }

    pub fn full_redraws(&self) -> usize {
        self.full_redraw_count
    }

    /// Upstream `this.fullRedrawCount += 1` from inside `doRender`.
    pub fn add_full_redraw(&mut self) {
        self.full_redraw_count += 1;
    }

    pub fn get_show_hardware_cursor(&self) -> bool {
        self.show_hardware_cursor
    }

    pub fn set_show_hardware_cursor(&mut self, enabled: bool) {
        if self.show_hardware_cursor == enabled {
            return;
        }
        self.show_hardware_cursor = enabled;
        if !enabled {
            self.hide_terminal_cursor();
        }
        self.request_render(false);
    }

    pub fn get_clear_on_shrink(&self) -> bool {
        self.clear_on_shrink
    }

    /// Whether a full re-render triggers when content shrinks.
    pub fn set_clear_on_shrink(&mut self, enabled: bool) {
        self.clear_on_shrink = enabled;
    }

    pub fn get_focused_component(&self) -> Option<&ComponentHandle> {
        self.focused_component.as_ref()
    }

    // ------------------------------------------------------------ Container

    /// Upstream `Container.addChild` / `TUI.addChild`.
    pub fn add_child(&mut self, component: ComponentHandle) {
        self.children.push(component);
    }

    /// Upstream `Container.removeChild` — identity removal, no focus effects.
    pub fn remove_child(&mut self, component: &ComponentHandle) {
        if let Some(index) = self
            .children
            .iter()
            .position(|c| same_component(c, component))
        {
            self.children.remove(index);
        }
    }

    /// Upstream `Container.clear`.
    pub fn clear(&mut self) {
        self.children.clear();
    }

    pub fn children(&self) -> &[ComponentHandle] {
        &self.children
    }

    /// Upstream `Container.render` (+ the `mouseLayout` cache).
    pub fn render(&mut self, width: usize) -> Vec<String> {
        let mut lines = Vec::new();
        let mut mouse_children = Vec::with_capacity(self.children.len());
        for child in &self.children {
            let child_lines = child.with_mut(|c| c.render(width));
            mouse_children.push((child.clone(), child_lines.len()));
            lines.extend(child_lines);
        }
        self.mouse_layout = Some((width, mouse_children));
        lines
    }

    /// Shared `Container.handleMouse` routing (upstream lines 344-364). The
    /// child hit is expressed as a `Forward` action; `dispatch_mouse_event`
    /// resolves it (including the `focusTarget: this` delegation).
    fn container_mouse_action(&mut self, event: &TuiMouseEvent) -> Option<MouseAction> {
        if event.y < 0 || event.y >= event.height as i64 {
            return None;
        }
        let mouse_children: Vec<(ComponentHandle, usize)> = match &self.mouse_layout {
            Some((cached_width, children)) if *cached_width == event.width => children.clone(),
            _ => self
                .children
                .iter()
                .map(|component| {
                    let height = component.with_mut(|c| c.render(event.width).len());
                    (component.clone(), height)
                })
                .collect(),
        };
        let mut child_y: i64 = 0;
        for (child, child_height) in mouse_children {
            if event.y >= child_y && event.y < child_y + child_height as i64 {
                return Some(MouseAction::Forward {
                    child,
                    event: TuiMouseEvent {
                        y: event.y - child_y,
                        height: child_height,
                        ..event.clone()
                    },
                    delegate_focus: false,
                    fallback_to_self: false,
                });
            }
            child_y += child_height as i64;
        }
        None
    }

    /// Upstream `TuiBase.invalidate` (roots plus overlays).
    pub fn invalidate(&mut self) {
        for root in &self.children {
            root.with_mut(|c| c.invalidate());
        }
        for overlay in &self.overlay_stack {
            overlay.component.with_mut(|c| c.invalidate());
        }
    }

    // ---------------------------------------------------------------- focus

    /// Upstream `setFocus`.
    pub fn set_focus(&mut self, component: Option<ComponentHandle>) {
        self.set_focus_internal(component, OverlayFocusRestorePolicy::Clear);
    }

    fn set_focus_internal(
        &mut self,
        component: Option<ComponentHandle>,
        overlay_focus_restore: OverlayFocusRestorePolicy,
    ) {
        let previous_focus = self.focused_component.clone();
        let mut next_focus = component;
        let previous_focused_overlay = previous_focus
            .as_ref()
            .and_then(|previous| self.find_visible_overlay_by_component(previous));
        let next_focus_is_overlay = next_focus.as_ref().is_some_and(|next| {
            self.overlay_stack
                .iter()
                .any(|entry| same_component(&entry.component, next))
        });
        let restore_state = self.get_visible_overlay_focus_restore();
        if next_focus.is_some() && !next_focus_is_overlay {
            let blocked_by_previous = matches!(
                &restore_state,
                OverlayFocusRestore::Blocked { blocked_by, .. }
                    if same_component_opt(&previous_focus, blocked_by)
            );
            if blocked_by_previous {
                let OverlayFocusRestore::Blocked {
                    overlay_id,
                    blocked_by,
                    resume,
                } = restore_state.clone()
                else {
                    unreachable!("blocked_by_previous implies Blocked state");
                };
                if matches!(resume, OverlayFocusResume::FocusTarget { .. })
                    || !self.is_component_mounted(&blocked_by)
                {
                    next_focus =
                        self.resolve_blocked_overlay_focus_restore(overlay_id, &blocked_by, resume);
                } else {
                    self.overlay_focus_restore = OverlayFocusRestore::Blocked {
                        overlay_id,
                        blocked_by: next_focus.clone().expect("checked non-none above"),
                        resume,
                    };
                }
            } else if let (Some(entry_index), Some(state_overlay_id)) =
                (previous_focused_overlay, overlay_id_of(&restore_state))
            {
                // restoreState.status !== "inactive" &&
                // restoreState.overlay === previousFocusedOverlay
                let overlay_id_matches = self.overlay_stack[entry_index].id == state_overlay_id;
                let is_ancestor = overlay_id_matches
                    && self.is_overlay_focus_ancestor(
                        entry_index,
                        next_focus.as_ref().expect("checked non-none above"),
                    );
                if overlay_id_matches && !is_ancestor {
                    self.overlay_focus_restore = OverlayFocusRestore::Blocked {
                        overlay_id: state_overlay_id,
                        blocked_by: next_focus.clone().expect("checked non-none above"),
                        resume: OverlayFocusResume::RestoreOverlay,
                    };
                }
            }
        } else if next_focus.is_none() {
            let blocked_by_previous = matches!(
                &restore_state,
                OverlayFocusRestore::Blocked { blocked_by, .. }
                    if same_component_opt(&previous_focus, blocked_by)
            );
            if blocked_by_previous {
                let OverlayFocusRestore::Blocked {
                    overlay_id,
                    blocked_by,
                    resume,
                } = restore_state.clone()
                else {
                    unreachable!("blocked_by_previous implies Blocked state");
                };
                next_focus =
                    self.resolve_blocked_overlay_focus_restore(overlay_id, &blocked_by, resume);
            } else if overlay_focus_restore == OverlayFocusRestorePolicy::Clear {
                self.clear_overlay_focus_restore();
            }
        }

        if let Some(previous) = &self.focused_component {
            if previous.with_mut(|c| c.is_focusable()) {
                previous.with_mut(|c| c.set_focused(false));
            }
        }

        self.focused_component = next_focus.clone();

        if let Some(next) = &self.focused_component {
            if next.with_mut(|c| c.is_focusable()) {
                next.with_mut(|c| c.set_focused(true));
            }
        }

        if let Some(next) = &next_focus {
            let focused_overlay = self.find_visible_overlay_by_component(next);
            if let Some(entry_index) = focused_overlay {
                let overlay_id = self.overlay_stack[entry_index].id;
                self.overlay_focus_restore = OverlayFocusRestore::Eligible { overlay_id };
            }
        }
    }

    fn clear_overlay_focus_restore(&mut self) {
        self.overlay_focus_restore = OverlayFocusRestore::Inactive;
    }

    fn clear_overlay_focus_restore_for(&mut self, entry_id: u64) {
        if overlay_id_of(&self.overlay_focus_restore) == Some(entry_id) {
            self.clear_overlay_focus_restore();
        }
    }

    fn resolve_blocked_overlay_focus_restore(
        &mut self,
        overlay_id: u64,
        _blocked_by: &ComponentHandle,
        resume: OverlayFocusResume,
    ) -> Option<ComponentHandle> {
        match resume {
            OverlayFocusResume::RestoreOverlay => self.overlay_component(overlay_id),
            OverlayFocusResume::FocusTarget { target } => {
                self.clear_overlay_focus_restore();
                target
            }
        }
    }

    fn get_visible_overlay_focus_restore(&mut self) -> OverlayFocusRestore {
        let restore_state = self.overlay_focus_restore.clone();
        let Some(overlay_id) = overlay_id_of(&restore_state) else {
            return restore_state;
        };
        let (columns, rows) = self.terminal_dimensions();
        let present = self
            .overlay_stack
            .iter()
            .any(|entry| entry.id == overlay_id);
        let visible = self
            .overlay_stack
            .iter_mut()
            .any(|entry| entry.id == overlay_id && entry_is_visible(entry, columns, rows));
        if !present || !visible {
            return OverlayFocusRestore::Inactive;
        }
        restore_state
    }

    /// Upstream `isOverlayFocusAncestor`: walk the preFocus chain from the
    /// overlay entry, with a visited set over components.
    fn is_overlay_focus_ancestor(&self, entry_index: usize, component: &ComponentHandle) -> bool {
        let mut visited: Vec<ComponentHandle> = Vec::new();
        let mut current = self.overlay_stack[entry_index].pre_focus.clone();
        while let Some(handle) = current {
            if visited.iter().any(|seen| same_component(seen, &handle)) {
                break;
            }
            visited.push(handle.clone());
            if same_component(&handle, component) {
                return true;
            }
            current = self
                .overlay_stack
                .iter()
                .find(|entry| same_component(&entry.component, &handle))
                .and_then(|entry| entry.pre_focus.clone());
        }
        false
    }

    fn is_component_mounted(&self, component: &ComponentHandle) -> bool {
        self.children
            .iter()
            .any(|root| contains_component(root, component))
    }

    fn overlay_component(&self, entry_id: u64) -> Option<ComponentHandle> {
        self.overlay_stack
            .iter()
            .find(|entry| entry.id == entry_id)
            .map(|entry| entry.component.clone())
    }

    // -------------------------------------------------------------- overlays

    /// Upstream `showOverlay`. Returns a handle whose operations are the
    /// `overlay_*` methods on [`Tui`].
    pub fn show_overlay(
        &mut self,
        component: ComponentHandle,
        options: Option<OverlayOptions>,
    ) -> OverlayHandle {
        self.next_overlay_id += 1;
        let entry_id = self.next_overlay_id;
        self.focus_order_counter += 1;
        let non_capturing = options
            .as_ref()
            .is_some_and(|options| options.non_capturing);
        let entry = OverlayStackEntry {
            id: entry_id,
            component: component.clone(),
            options,
            pre_focus: self.focused_component.clone(),
            hidden: false,
            focus_order: self.focus_order_counter,
            bounds: None,
        };
        self.overlay_stack.push(entry);
        // Only focus if the overlay is actually visible.
        let (columns, rows) = self.terminal_dimensions();
        if !non_capturing
            && self
                .overlay_stack
                .iter_mut()
                .find(|entry| entry.id == entry_id)
                .is_some_and(|entry| entry_is_visible(entry, columns, rows))
        {
            self.set_focus(Some(component.clone()));
        }
        self.hide_terminal_cursor();
        self.request_render(false);

        OverlayHandle { entry_id }
    }

    /// Upstream `OverlayHandle.hide`.
    pub fn overlay_hide(&mut self, handle: &OverlayHandle) {
        let Some(index) = self
            .overlay_stack
            .iter()
            .position(|entry| entry.id == handle.entry_id)
        else {
            return;
        };
        self.clear_overlay_focus_restore_for(handle.entry_id);
        let entry = self.overlay_stack.remove(index);
        self.retarget_overlay_pre_focus(&entry);
        // Restore focus if this overlay had focus.
        if same_component_opt(&self.focused_component, &entry.component) {
            let top_visible = self.topmost_visible_overlay();
            let target = top_visible
                .map(|top| self.overlay_stack[top].component.clone())
                .or(entry.pre_focus.clone());
            self.set_focus(target);
        }
        if self.overlay_stack.is_empty() {
            self.hide_terminal_cursor();
        }
        self.request_render(false);
    }

    /// Upstream `OverlayHandle.setHidden`.
    pub fn overlay_set_hidden(&mut self, handle: &OverlayHandle, hidden: bool) {
        let Some(index) = self
            .overlay_stack
            .iter()
            .position(|entry| entry.id == handle.entry_id)
        else {
            return;
        };
        if self.overlay_stack[index].hidden == hidden {
            return;
        }
        self.overlay_stack[index].hidden = hidden;
        let entry_id = handle.entry_id;
        let component = self.overlay_stack[index].component.clone();
        let non_capturing = self.overlay_stack[index]
            .options
            .as_ref()
            .is_some_and(|options| options.non_capturing);
        if hidden {
            self.clear_overlay_focus_restore_for(entry_id);
            // If this overlay had focus, move focus to the next visible one.
            if same_component_opt(&self.focused_component, &component) {
                let pre_focus = self.overlay_stack[index].pre_focus.clone();
                let top_visible = self.topmost_visible_overlay();
                let target = top_visible
                    .map(|top| self.overlay_stack[top].component.clone())
                    .or(pre_focus);
                self.set_focus(target);
            }
        } else {
            // Restore focus to this overlay when showing (if actually visible).
            let (columns, rows) = self.terminal_dimensions();
            if !non_capturing
                && self
                    .overlay_stack
                    .iter_mut()
                    .find(|entry| entry.id == entry_id)
                    .is_some_and(|entry| entry_is_visible(entry, columns, rows))
            {
                self.focus_order_counter += 1;
                self.overlay_stack[index].focus_order = self.focus_order_counter;
                self.set_focus(Some(component.clone()));
            }
        }
        self.request_render(false);
    }

    /// Upstream `OverlayHandle.isHidden`.
    pub fn overlay_is_hidden(&self, handle: &OverlayHandle) -> bool {
        self.overlay_stack
            .iter()
            .find(|entry| entry.id == handle.entry_id)
            .is_some_and(|entry| entry.hidden)
    }

    /// Upstream `OverlayHandle.focus`.
    pub fn overlay_focus(&mut self, handle: &OverlayHandle) {
        let Some(index) = self
            .overlay_stack
            .iter()
            .position(|entry| entry.id == handle.entry_id)
        else {
            return;
        };
        let (columns, rows) = self.terminal_dimensions();
        let visible = self
            .overlay_stack
            .iter_mut()
            .find(|entry| entry.id == handle.entry_id)
            .is_some_and(|entry| entry_is_visible(entry, columns, rows));
        if !visible {
            return;
        }
        self.focus_order_counter += 1;
        self.overlay_stack[index].focus_order = self.focus_order_counter;
        let component = self.overlay_stack[index].component.clone();
        self.set_focus(Some(component));
        self.request_render(false);
    }

    /// Upstream `OverlayHandle.unfocus`.
    pub fn overlay_unfocus(
        &mut self,
        handle: &OverlayHandle,
        options: Option<OverlayUnfocusOptions>,
    ) {
        let Some(index) = self
            .overlay_stack
            .iter()
            .position(|entry| entry.id == handle.entry_id)
        else {
            return;
        };
        let component = self.overlay_stack[index].component.clone();
        let is_focused = same_component_opt(&self.focused_component, &component);
        let restore_state = self.overlay_focus_restore.clone();
        let has_pending_restore = overlay_id_of(&restore_state) == Some(handle.entry_id);
        if !is_focused && !has_pending_restore {
            return;
        }
        if let OverlayFocusRestore::Blocked {
            overlay_id,
            blocked_by,
            ..
        } = &restore_state
        {
            if *overlay_id == handle.entry_id
                && same_component_opt(&self.focused_component, blocked_by)
            {
                match options {
                    Some(unfocus_options) => {
                        self.overlay_focus_restore = OverlayFocusRestore::Blocked {
                            overlay_id: handle.entry_id,
                            blocked_by: blocked_by.clone(),
                            resume: OverlayFocusResume::FocusTarget {
                                target: unfocus_options.target,
                            },
                        };
                    }
                    None => self.clear_overlay_focus_restore(),
                }
                self.request_render(false);
                return;
            }
        }
        self.clear_overlay_focus_restore_for(handle.entry_id);
        if is_focused || options.is_some() {
            let top_visible = self.topmost_visible_overlay();
            let fallback_target = top_visible
                .and_then(|top| {
                    let top_entry = &self.overlay_stack[top];
                    (top_entry.id != handle.entry_id).then(|| top_entry.component.clone())
                })
                .or_else(|| self.overlay_stack[index].pre_focus.clone());
            self.set_focus(match options {
                Some(unfocus_options) => unfocus_options.target,
                None => fallback_target,
            });
        }
        self.request_render(false);
    }

    /// Upstream `OverlayHandle.isFocused`.
    pub fn overlay_is_focused(&self, handle: &OverlayHandle) -> bool {
        self.overlay_stack
            .iter()
            .find(|entry| entry.id == handle.entry_id)
            .is_some_and(|entry| same_component_opt(&self.focused_component, &entry.component))
    }

    /// Upstream `OverlayHandle.getBounds`.
    pub fn overlay_bounds(&self, handle: &OverlayHandle) -> Option<OverlayBounds> {
        self.overlay_stack
            .iter()
            .find(|entry| entry.id == handle.entry_id)?
            .bounds
    }

    /// Upstream `hideOverlay`.
    pub fn hide_overlay(&mut self) {
        let Some(entry) = self.overlay_stack.pop() else {
            return;
        };
        self.clear_overlay_focus_restore_for(entry.id);
        self.retarget_overlay_pre_focus(&entry);
        if same_component_opt(&self.focused_component, &entry.component) {
            // Find the topmost visible overlay, or fall back to preFocus.
            let top_visible = self.topmost_visible_overlay();
            let target = top_visible
                .map(|top| self.overlay_stack[top].component.clone())
                .or(entry.pre_focus.clone());
            self.set_focus(target);
        }
        if self.overlay_stack.is_empty() {
            self.hide_terminal_cursor();
        }
        self.request_render(false);
    }

    /// Upstream `hideTerminalCursor`: hide the cursor while running. After
    /// stop(), the shell owns the cursor and it must stay visible.
    fn hide_terminal_cursor(&mut self) {
        if !self.stopped {
            self.terminal.borrow_mut().hide_cursor();
        }
    }

    /// Upstream `hasOverlay`.
    pub fn has_overlay(&mut self) -> bool {
        (0..self.overlay_stack.len()).any(|index| self.overlay_visible_at(index))
    }

    /// Upstream `hasOverlayEntries` getter.
    pub fn has_overlay_entries(&self) -> bool {
        !self.overlay_stack.is_empty()
    }

    /// Upstream `isOverlayFocused`.
    pub fn is_overlay_focused(&mut self) -> bool {
        let focused = self.focused_component.clone();
        focused.is_some_and(|focused| self.find_visible_overlay_by_component(&focused).is_some())
    }

    /// Upstream `resolveMouseFocusTarget`: keep overlay containers as keyboard
    /// focus owners when a nested control is clicked.
    pub fn resolve_mouse_focus_target(&mut self, component: &ComponentHandle) -> ComponentHandle {
        for index in (0..self.overlay_stack.len()).rev() {
            let overlay_component = self.overlay_stack[index].component.clone();
            if self.overlay_visible_for_component(&overlay_component)
                && contains_component(&overlay_component, component)
            {
                return overlay_component;
            }
        }
        component.clone()
    }

    /// Upstream `dispatchMouseToOverlay`: dispatch to the visually topmost
    /// overlay under the pointer.
    pub fn dispatch_mouse_to_overlay(&mut self, event: &TuiMouseEvent) -> OverlayMouseDispatch {
        for layout in self.rendered_overlay_layouts.iter().rev() {
            if event.screen_x < layout.col
                || event.screen_x >= layout.col + layout.width as i64
                || event.screen_y < layout.row
                || event.screen_y >= layout.row + layout.height as i64
            {
                continue;
            }
            let component = layout.component.clone();
            let local = TuiMouseEvent {
                x: event.screen_x - layout.col,
                y: event.screen_y - layout.row,
                width: layout.width,
                height: layout.height,
                ..event.clone()
            };
            let result = dispatch_mouse_event(&component, &local);
            return match result {
                Some(mut result) => {
                    if result.result.focus {
                        result.focus_target = Some(component.clone());
                    }
                    OverlayMouseDispatch {
                        hit: true,
                        result: Some(result),
                    }
                }
                None => OverlayMouseDispatch {
                    hit: true,
                    result: None,
                },
            };
        }
        OverlayMouseDispatch {
            hit: false,
            result: None,
        }
    }

    fn retarget_overlay_pre_focus(&mut self, removed: &OverlayStackEntry) {
        for overlay in &mut self.overlay_stack {
            if same_component_opt(&overlay.pre_focus, &removed.component) {
                overlay.pre_focus = removed.pre_focus.clone();
            }
        }
    }

    fn overlay_visible_at(&mut self, index: usize) -> bool {
        let (columns, rows) = self.terminal_dimensions();
        entry_is_visible(&mut self.overlay_stack[index], columns, rows)
    }

    fn overlay_visible_for_component(&mut self, component: &ComponentHandle) -> bool {
        let (columns, rows) = self.terminal_dimensions();
        self.overlay_stack
            .iter_mut()
            .find(|entry| same_component(&entry.component, component))
            .is_some_and(|entry| entry_is_visible(entry, columns, rows))
    }

    fn find_visible_overlay_by_component(&mut self, component: &ComponentHandle) -> Option<usize> {
        let (columns, rows) = self.terminal_dimensions();
        self.overlay_stack.iter_mut().position(|entry| {
            same_component(&entry.component, component) && entry_is_visible(entry, columns, rows)
        })
    }

    fn topmost_visible_overlay(&mut self) -> Option<usize> {
        let mut topmost: Option<usize> = None;
        for index in 0..self.overlay_stack.len() {
            let capturing = !self.overlay_stack[index]
                .options
                .as_ref()
                .is_some_and(|options| options.non_capturing);
            if !capturing {
                continue;
            }
            if !self.overlay_visible_at(index) {
                continue;
            }
            let better = match topmost {
                None => true,
                Some(current) => {
                    self.overlay_stack[index].focus_order > self.overlay_stack[current].focus_order
                }
            };
            if better {
                topmost = Some(index);
            }
        }
        topmost
    }

    // ------------------------------------------------------------ terminal i/o

    /// Upstream `start`.
    pub fn start(&mut self) {
        self.stopped = false;
        self.with_renderer(|renderer, tui| renderer.before_terminal_start(tui));
        let on_input = {
            let queue = self.terminal_events.clone();
            Box::new(move |data: String| {
                queue
                    .lock()
                    .expect("tui terminal event queue")
                    .push_back(TerminalEvent::Input(data));
            }) as Box<dyn FnMut(String) + Send>
        };
        let on_resize = {
            let queue = self.terminal_events.clone();
            Box::new(move || {
                queue
                    .lock()
                    .expect("tui terminal event queue")
                    .push_back(TerminalEvent::Resize);
            }) as Box<dyn FnMut() + Send>
        };
        self.terminal.borrow_mut().start(on_input, on_resize);
        self.with_renderer(|renderer, tui| renderer.after_terminal_start(tui));
        self.terminal.borrow_mut().hide_cursor();
        if self.scheme_notifications_enabled {
            self.terminal.borrow_mut().write("\x1b[?2031h");
        }
        self.query_cell_size();
        self.request_render(false);
    }

    /// Drain the terminal event queue (input data / resize) — the Node
    /// stream-callback boundary made explicit; each event is delivered in
    /// arrival order exactly as the upstream callbacks would.
    pub fn poll_terminal_events(&mut self) {
        loop {
            let event = self
                .terminal_events
                .lock()
                .expect("tui terminal event queue")
                .pop_front();
            match event {
                Some(TerminalEvent::Input(data)) => self.handle_terminal_input(&data),
                Some(TerminalEvent::Resize) => self.request_render(false),
                None => break,
            }
        }
    }

    /// Upstream `addInputListener`; returns an unsubscribe id.
    pub fn add_input_listener(
        &mut self,
        listener: impl FnMut(&str) -> Option<TuiInputListenerResult> + Send + 'static,
    ) -> u64 {
        let id = self.next_listener_id;
        self.next_listener_id += 1;
        self.input_listeners.push((id, Box::new(listener)));
        id
    }

    /// Upstream `addInputListener`'s returned unsubscribe closure.
    pub fn remove_input_listener(&mut self, id: u64) {
        self.input_listeners
            .retain(|(listener_id, _)| *listener_id != id);
    }

    /// Upstream `onTerminalColorSchemeChange`; returns an unsubscribe id.
    pub fn on_terminal_color_scheme_change(
        &mut self,
        listener: impl FnMut(&TerminalColorScheme) + 'static,
    ) -> u64 {
        let id = self.next_scheme_listener_id;
        self.next_scheme_listener_id += 1;
        self.terminal_color_scheme_listeners
            .push((id, Box::new(listener)));
        id
    }

    /// Unsubscribe from [`Tui::on_terminal_color_scheme_change`].
    pub fn remove_terminal_color_scheme_listener(&mut self, id: u64) {
        self.terminal_color_scheme_listeners
            .retain(|(listener_id, _)| *listener_id != id);
    }

    /// Upstream `setTerminalColorSchemeNotifications`.
    pub fn set_terminal_color_scheme_notifications(&mut self, enabled: bool) {
        if self.scheme_notifications_enabled == enabled {
            return;
        }
        self.scheme_notifications_enabled = enabled;
        if !self.stopped {
            self.terminal.borrow_mut().write(if enabled {
                "\x1b[?2031h"
            } else {
                "\x1b[?2031l"
            });
        }
    }

    fn query_cell_size(&mut self) {
        // Only query if the terminal supports images (cell size is only used
        // for image rendering).
        if get_capabilities().images.is_none() {
            return;
        }
        // Query terminal for cell size in pixels: CSI 16 t.
        self.terminal.borrow_mut().write("\x1b[16t");
    }

    /// Upstream `stop`.
    pub fn stop(&mut self, options: TuiStopOptions) {
        self.stopped = true;
        self.cancel_render_timer();
        if self.scheme_notifications_enabled {
            self.terminal.borrow_mut().write("\x1b[?2031l");
        }
        self.with_renderer(|renderer, tui| renderer.before_terminal_stop(tui, &options));
        self.terminal.borrow_mut().show_cursor();
        self.terminal.borrow_mut().stop();
        self.with_renderer(|renderer, tui| renderer.after_terminal_stop(tui, &options));
    }

    // ------------------------------------------------------- render scheduling

    /// Upstream `renderNow`.
    pub fn render_now(&mut self, force: bool) {
        let mut renderer = self.take_renderer();
        if force {
            renderer.reset_render_state(self);
        }
        self.render_requested = false;
        self.cancel_render_timer();
        self.last_render_at = (self.clock)();
        renderer.do_render(self);
        self.renderer = Some(renderer);
    }

    /// Upstream `requestRender`.
    pub fn request_render(&mut self, force: bool) {
        if force {
            self.with_renderer(|renderer, tui| renderer.reset_render_state(tui));
            self.request_immediate_render();
            return;
        }
        if self.render_requested {
            return;
        }
        self.render_requested = true;
        self.next_ticks.push_back(NextTickWork::ScheduleRender);
    }

    fn request_immediate_render(&mut self) {
        self.cancel_render_timer();
        self.render_requested = true;
        if self.immediate_render_scheduled {
            return;
        }
        self.immediate_render_scheduled = true;
        self.next_ticks.push_back(NextTickWork::ImmediateRender);
    }

    fn cancel_render_timer(&mut self) {
        let Some(timer_id) = self.render_timer.take() else {
            return;
        };
        self.timers.retain(|timer| timer.id != timer_id);
    }

    fn schedule_render(&mut self) {
        if self.stopped || self.render_timer.is_some() || !self.render_requested {
            return;
        }
        let elapsed = (self.clock)() - self.last_render_at;
        let delay = (MIN_RENDER_INTERVAL_MS - elapsed).max(0.0);
        self.render_timer = Some(self.set_timer(delay, TimerWork::RenderFrame));
    }

    fn do_render(&mut self) {
        let mut renderer = self.take_renderer();
        renderer.do_render(self);
        self.renderer = Some(renderer);
    }

    fn take_renderer(&mut self) -> Box<dyn TuiRenderer> {
        self.renderer
            .take()
            .expect("tui renderer installed at construction")
    }

    fn with_renderer(&mut self, call: impl FnOnce(&mut Box<dyn TuiRenderer>, &mut Tui)) {
        let mut renderer = self.take_renderer();
        call(&mut renderer, self);
        self.renderer = Some(renderer);
    }

    // -------------------------------------------------------- scheduler seam

    /// Whether any `process.nextTick` work is queued.
    pub fn has_next_ticks(&self) -> bool {
        !self.next_ticks.is_empty()
    }

    /// Drain all queued next-tick work, including work queued while draining
    /// (Node's microtask semantics).
    pub fn run_next_ticks(&mut self) {
        while let Some(work) = self.next_ticks.pop_front() {
            match work {
                NextTickWork::ScheduleRender => self.schedule_render(),
                NextTickWork::ImmediateRender => {
                    self.immediate_render_scheduled = false;
                    if self.stopped || !self.render_requested {
                        continue;
                    }
                    // A previously queued schedule_render() can create a timer
                    // before this callback runs. User input must preempt that
                    // throttled frame.
                    self.cancel_render_timer();
                    self.render_requested = false;
                    self.last_render_at = (self.clock)();
                    self.do_render();
                }
            }
        }
    }

    /// Earliest pending timer deadline, if any (for host event loops).
    pub fn next_timer_deadline(&self) -> Option<f64> {
        self.timers
            .iter()
            .map(|timer| timer.deadline_ms)
            .reduce(f64::min)
    }

    /// Fire the earliest due timer (deadline <= now), if any. Node runs each
    /// timer callback to completion before the next; drain next-ticks between
    /// calls to mirror microtask ordering.
    pub fn fire_due_timer(&mut self) -> bool {
        let now = (self.clock)();
        let mut chosen: Option<usize> = None;
        for (index, timer) in self.timers.iter().enumerate() {
            if timer.deadline_ms > now {
                continue;
            }
            let better = match chosen {
                None => true,
                Some(current) => {
                    let current_timer = &self.timers[current];
                    (timer.deadline_ms, timer.id) < (current_timer.deadline_ms, current_timer.id)
                }
            };
            if better {
                chosen = Some(index);
            }
        }
        let Some(index) = chosen else {
            return false;
        };
        let entry = self.timers.remove(index);
        match entry.work {
            TimerWork::RenderFrame => {
                self.render_timer = None;
                if self.stopped || !self.render_requested {
                    return true;
                }
                self.render_requested = false;
                self.last_render_at = (self.clock)();
                self.do_render();
                if self.render_requested {
                    self.schedule_render();
                }
            }
            TimerWork::TerminalColorTimeout(seq) => {
                if let Some(query) = self
                    .pending_terminal_color_queries
                    .iter_mut()
                    .find(|query| query.seq == seq)
                {
                    // Resolve with the replies so far, and keep collecting
                    // late replies for `onLateReply`.
                    query.timer_id = None;
                    if let Some(deliver) = query.deliver.take() {
                        deliver(Self::terminal_color_query_result(query));
                    }
                    query.deliver = query.on_late_reply.take();
                }
            }
        }
        true
    }

    fn set_timer(&mut self, delay_ms: f64, work: TimerWork) -> u64 {
        let id = self.next_timer_id;
        self.next_timer_id += 1;
        let deadline_ms = (self.clock)() + delay_ms;
        self.timers.push(TimerEntry {
            id,
            deadline_ms,
            work,
        });
        id
    }

    fn cancel_timer(&mut self, timer_id: u64) {
        self.timers.retain(|timer| timer.id != timer_id);
    }

    // -------------------------------------------------------- terminal input

    /// Upstream `handleTerminalInput`.
    pub fn handle_terminal_input(&mut self, data: &str) {
        if self.consume_terminal_color_response(data) {
            return;
        }
        if self.consume_terminal_color_scheme_report(data) {
            return;
        }

        if !self.input_listeners.is_empty() {
            let mut current = data.to_string();
            let mut index = 0;
            while index < self.input_listeners.len() {
                let result = {
                    let (_, listener) = &mut self.input_listeners[index];
                    listener(&current)
                };
                index += 1;
                if let Some(result) = result {
                    if result.consume {
                        return;
                    }
                    if let Some(replacement) = result.data {
                        current = replacement;
                    }
                }
            }
            if current.is_empty() {
                return;
            }
            self.handle_terminal_input_after_listeners(&current);
            return;
        }
        self.handle_terminal_input_after_listeners(data);
    }

    fn handle_terminal_input_after_listeners(&mut self, data: &str) {
        // Consume terminal cell size responses without blocking unrelated input.
        if self.consume_cell_size_response(data) {
            return;
        }

        // Global debug key handler (Shift+Ctrl+D).
        if matches_key(data, "shift+ctrl+d") && self.on_debug.is_some() {
            if let Some(debug) = self.on_debug.as_mut() {
                debug();
            }
            return;
        }

        // If the focused component is an overlay, verify it's still visible
        // (visibility can change due to terminal resize or the visible callback).
        let focused_entry = self.focused_component.clone().and_then(|focused| {
            self.overlay_stack
                .iter()
                .position(|entry| same_component(&entry.component, &focused))
        });
        if let Some(entry_index) = focused_entry {
            if !self.overlay_visible_at(entry_index) {
                // Focused overlay is no longer visible; redirect to the
                // topmost visible overlay.
                if let Some(top) = self.topmost_visible_overlay() {
                    let component = self.overlay_stack[top].component.clone();
                    self.set_focus(Some(component));
                } else {
                    let pre_focus = self.overlay_stack[entry_index].pre_focus.clone();
                    self.set_focus_internal(pre_focus, OverlayFocusRestorePolicy::Preserve);
                }
            }
        }

        let focus_is_overlay = self.focused_component.as_ref().is_some_and(|focused| {
            self.overlay_stack
                .iter()
                .any(|entry| same_component(&entry.component, focused))
        });
        if !focus_is_overlay {
            let restore_state = self.get_visible_overlay_focus_restore();
            match restore_state {
                OverlayFocusRestore::Eligible { overlay_id } => {
                    let component = self.overlay_component(overlay_id);
                    self.set_focus(component);
                }
                OverlayFocusRestore::Blocked {
                    overlay_id,
                    blocked_by,
                    resume,
                } if !same_component_opt(&self.focused_component, &blocked_by) => match resume {
                    OverlayFocusResume::RestoreOverlay => {
                        let component = self.overlay_component(overlay_id);
                        self.set_focus(component);
                    }
                    OverlayFocusResume::FocusTarget { target } => {
                        self.clear_overlay_focus_restore();
                        self.set_focus(target);
                    }
                },
                _ => {}
            }
        }

        // Pass input to the focused component (including Ctrl+C): the focused
        // component decides how to handle it.
        if let Some(focused) = self.focused_component.clone() {
            let wants_release = focused.with_mut(|c| c.wants_key_release());
            if is_key_release(data) && !wants_release {
                return;
            }
            focused.with_mut(|c| c.handle_input(data));
            // Keyboard input is latency-sensitive. Avoid the throttled timer
            // path, where even setTimeout(0) can take a full 16 ms tick on
            // Windows.
            self.request_immediate_render();
        }
    }

    fn consume_terminal_color_response(&mut self, data: &str) -> bool {
        if self.pending_terminal_color_queries.is_empty() {
            return false;
        }
        if is_device_attributes_response(data) {
            // Pop before completing: `deliver` may re-enter the TUI.
            let completed = self
                .pending_terminal_color_queries
                .pop_front()
                .expect("checked non-empty");
            self.complete_terminal_color_query(completed);
            return true;
        }

        let Some((target, rgb)) = parse_osc_color_response(data) else {
            return false;
        };
        let completed = {
            let query = self
                .pending_terminal_color_queries
                .front_mut()
                .expect("checked non-empty");
            let key = target.reply_key();
            if query.deliver.is_none() || query.replied.contains(&key) {
                None
            } else {
                query.replied.insert(key);
                match target {
                    OscColorTarget::Foreground => query.foreground = rgb,
                    OscColorTarget::Background => query.background = rgb,
                    OscColorTarget::Index(index) => {
                        if (index as usize) < TERMINAL_PALETTE_SIZE {
                            query.palette[index as usize] = rgb;
                        }
                    }
                }
                if query.replied.len() == TERMINAL_COLOR_REPLY_COUNT {
                    Some(std::mem::replace(query, TerminalColorQuery::vacant()))
                } else {
                    None
                }
            }
        };
        // Upstream completes the query WITHOUT shifting it: the completed
        // entry stays at the front and consumes the trailing DA1 reply (which
        // then shifts it), so that DA1 is not forwarded as input.
        if let Some(completed) = completed {
            self.complete_terminal_color_query(completed);
        }
        true
    }

    /// Upstream `terminalColorQueryResult`: the palette is only set when all
    /// 16 colors arrived.
    fn terminal_color_query_result(query: &TerminalColorQuery) -> TerminalColors {
        let palette = if query.palette.iter().all(|color| color.is_some()) {
            Some(
                query
                    .palette
                    .iter()
                    .map(|color| color.expect("checked all"))
                    .collect(),
            )
        } else {
            None
        };
        TerminalColors {
            foreground: query.foreground,
            background: query.background,
            palette,
        }
    }

    fn complete_terminal_color_query(&mut self, mut query: TerminalColorQuery) {
        let deliver = query.deliver.take();
        if let Some(timer_id) = query.timer_id.take() {
            self.cancel_timer(timer_id);
        }
        if let Some(deliver) = deliver {
            deliver(Self::terminal_color_query_result(&query));
        }
    }

    fn consume_terminal_color_scheme_report(&mut self, data: &str) -> bool {
        let Some(scheme) = parse_terminal_color_scheme_report(data) else {
            return false;
        };
        let listener_ids: Vec<u64> = self
            .terminal_color_scheme_listeners
            .iter()
            .map(|(id, _)| *id)
            .collect();
        for id in listener_ids {
            let Some((_, listener)) = self
                .terminal_color_scheme_listeners
                .iter_mut()
                .find(|(listener_id, _)| *listener_id == id)
            else {
                continue; // removed during this dispatch
            };
            listener(&scheme);
        }
        true
    }

    fn consume_cell_size_response(&mut self, data: &str) -> bool {
        // Response format: ESC [ 6 ; height ; width t
        let Some(captures) = cell_size_regex().captures(data) else {
            return false;
        };
        let height_px: i64 = captures[1].parse().unwrap_or(0);
        let width_px: i64 = captures[2].parse().unwrap_or(0);
        if height_px <= 0 || width_px <= 0 {
            return true;
        }
        (self.cell_dimensions_sink)(width_px as u32, height_px as u32);
        // Invalidate all components so images re-render with correct
        // dimensions.
        self.invalidate();
        self.request_render(false);
        true
    }

    // ------------------------------------------------------ terminal queries

    /// Upstream `queryTerminalColors`: query the terminal's theme colors —
    /// the default foreground (OSC 10), the default background (OSC 11), and
    /// ANSI colors 0-15 (OSC 4), followed by a DA1 request that marks the end
    /// of the replies. Resolves when the DA1 reply or all color replies
    /// arrive, or when the timeout expires. Colors the terminal did not report
    /// are `None`; the palette is only set when all 16 arrived. (Upstream
    /// Promises become a one-shot `on_resolve` callback; a query that timed
    /// out keeps collecting late replies for `on_late_reply`.)
    pub fn query_terminal_colors(
        &mut self,
        timeout_ms: f64,
        on_resolve: impl FnOnce(TerminalColors) + 'static,
        on_late_reply: Option<Box<dyn FnOnce(TerminalColors)>>,
    ) {
        self.next_query_seq += 1;
        let seq = self.next_query_seq;
        let timer_id = self.set_timer(timeout_ms, TimerWork::TerminalColorTimeout(seq));
        self.pending_terminal_color_queries
            .push_back(TerminalColorQuery {
                seq,
                foreground: None,
                background: None,
                palette: vec![None; TERMINAL_PALETTE_SIZE],
                replied: BTreeSet::new(),
                deliver: Some(Box::new(on_resolve)),
                on_late_reply,
                timer_id: Some(timer_id),
            });
        self.terminal.borrow_mut().write(TERMINAL_COLOR_QUERY);
    }

    // ------------------------------------------------------- overlay layout

    /// Upstream `resolveOverlayLayout` (JS-number semantics).
    pub fn resolve_overlay_layout(
        &self,
        options: Option<&OverlayOptions>,
        overlay_height: f64,
        term_width: f64,
        term_height: f64,
    ) -> OverlayLayoutResult {
        const DEFAULT_OPTIONS: OverlayOptions = OverlayOptions {
            width: None,
            min_width: None,
            max_height: None,
            anchor: None,
            offset_x: None,
            offset_y: None,
            row: None,
            col: None,
            margin: None,
            visible: None,
            non_capturing: false,
        };
        let opt = options.unwrap_or(&DEFAULT_OPTIONS);

        // Parse margin (clamp to non-negative).
        let margin = match opt.margin {
            Some(OverlayMarginSpec::Number(value)) => OverlayMarginValue {
                top: Some(value),
                right: Some(value),
                bottom: Some(value),
                left: Some(value),
            },
            Some(OverlayMarginSpec::Edges(edges)) => edges,
            None => OverlayMarginValue::default(),
        };
        let margin_top = margin.top.unwrap_or(0.0).max(0.0);
        let margin_right = margin.right.unwrap_or(0.0).max(0.0);
        let margin_bottom = margin.bottom.unwrap_or(0.0).max(0.0);
        let margin_left = margin.left.unwrap_or(0.0).max(0.0);

        // Available space after margins.
        let avail_width = (term_width - margin_left - margin_right).max(1.0);
        let avail_height = (term_height - margin_top - margin_bottom).max(1.0);

        // Resolve width.
        let mut width = parse_size_value(opt.width.as_ref(), term_width)
            .unwrap_or_else(|| 80.0f64.min(avail_width));
        if let Some(min_width) = opt.min_width {
            width = width.max(min_width);
        }
        width = width.max(1.0).min(avail_width);

        // Resolve maxHeight.
        let mut max_height = parse_size_value(opt.max_height.as_ref(), term_height);
        if let Some(value) = max_height {
            max_height = Some(value.max(1.0).min(avail_height));
        }

        // Effective overlay height (may be clamped by maxHeight).
        let effective_height = match max_height {
            Some(max) => overlay_height.min(max),
            None => overlay_height,
        };

        // Resolve position.
        let row = match &opt.row {
            Some(SizeValue::Percent(text)) => match percent_regex().captures(text) {
                Some(captures) => {
                    // 0% = top, 100% = bottom (overlay stays in bounds).
                    let max_row = (avail_height - effective_height).max(0.0);
                    let percent = captures[1].parse::<f64>().unwrap_or(0.0) / 100.0;
                    margin_top + (max_row * percent).floor()
                }
                None => self.resolve_anchor_row(
                    opt.anchor.unwrap_or(OverlayAnchor::Center),
                    effective_height,
                    avail_height,
                    margin_top,
                ),
            },
            Some(SizeValue::Number(value)) => *value,
            None => self.resolve_anchor_row(
                opt.anchor.unwrap_or(OverlayAnchor::Center),
                effective_height,
                avail_height,
                margin_top,
            ),
        };

        let col = match &opt.col {
            Some(SizeValue::Percent(text)) => match percent_regex().captures(text) {
                Some(captures) => {
                    let max_col = (avail_width - width).max(0.0);
                    let percent = captures[1].parse::<f64>().unwrap_or(0.0) / 100.0;
                    margin_left + (max_col * percent).floor()
                }
                None => self.resolve_anchor_col(
                    opt.anchor.unwrap_or(OverlayAnchor::Center),
                    width,
                    avail_width,
                    margin_left,
                ),
            },
            Some(SizeValue::Number(value)) => *value,
            None => self.resolve_anchor_col(
                opt.anchor.unwrap_or(OverlayAnchor::Center),
                width,
                avail_width,
                margin_left,
            ),
        };

        // Apply offsets.
        let row = row + opt.offset_y.unwrap_or(0.0);
        let col = col + opt.offset_x.unwrap_or(0.0);

        // Clamp to terminal bounds (respecting margins).
        let row = margin_top.max(row.min(term_height - margin_bottom - effective_height));
        let col = margin_left.max(col.min(term_width - margin_right - width));

        OverlayLayoutResult {
            width,
            row,
            col,
            max_height,
        }
    }

    fn resolve_anchor_row(
        &self,
        anchor: OverlayAnchor,
        height: f64,
        avail_height: f64,
        margin_top: f64,
    ) -> f64 {
        match anchor {
            OverlayAnchor::TopLeft | OverlayAnchor::TopCenter | OverlayAnchor::TopRight => {
                margin_top
            }
            OverlayAnchor::BottomLeft
            | OverlayAnchor::BottomCenter
            | OverlayAnchor::BottomRight => margin_top + avail_height - height,
            OverlayAnchor::LeftCenter | OverlayAnchor::Center | OverlayAnchor::RightCenter => {
                margin_top + ((avail_height - height) / 2.0).floor()
            }
        }
    }

    fn resolve_anchor_col(
        &self,
        anchor: OverlayAnchor,
        width: f64,
        avail_width: f64,
        margin_left: f64,
    ) -> f64 {
        match anchor {
            OverlayAnchor::TopLeft | OverlayAnchor::LeftCenter | OverlayAnchor::BottomLeft => {
                margin_left
            }
            OverlayAnchor::TopRight | OverlayAnchor::RightCenter | OverlayAnchor::BottomRight => {
                margin_left + avail_width - width
            }
            OverlayAnchor::TopCenter | OverlayAnchor::Center | OverlayAnchor::BottomCenter => {
                margin_left + ((avail_width - width) / 2.0).floor()
            }
        }
    }

    /// Upstream `compositeOverlays`: composite all visible overlays into the
    /// content lines (sorted by focusOrder, higher = on top).
    pub fn composite_overlays(
        &mut self,
        lines: Vec<String>,
        term_width: usize,
        term_height: usize,
    ) -> Vec<String> {
        if self.overlay_stack.is_empty() {
            self.rendered_overlay_layouts.clear();
            return lines;
        }
        let mut result = lines;

        for entry in &mut self.overlay_stack {
            entry.bounds = None;
        }

        struct RenderedOverlay {
            entry_index: usize,
            overlay_lines: Vec<String>,
            row: usize,
            col: usize,
            width: usize,
        }

        // Pre-render all visible overlays and calculate positions.
        let mut rendered: Vec<RenderedOverlay> = Vec::new();
        let mut min_lines_needed = result.len();

        let mut visible: Vec<usize> = Vec::new();
        for index in 0..self.overlay_stack.len() {
            if self.overlay_visible_at(index) {
                visible.push(index);
            }
        }
        visible.sort_by_key(|&index| self.overlay_stack[index].focus_order);

        let term_width_f = term_width as f64;
        let term_height_f = term_height as f64;
        for &index in &visible {
            // Layout with height=0 first: width and maxHeight don't depend on
            // the overlay height.
            let entry_options_ptr = &self.overlay_stack[index];
            let layout = self.resolve_overlay_layout(
                entry_options_ptr.options.as_ref(),
                0.0,
                term_width_f,
                term_height_f,
            );
            let width = layout.width as usize;

            // Render the component at the calculated width.
            let mut overlay_lines = self.overlay_stack[index]
                .component
                .with_mut(|c| c.render(width));

            // Apply maxHeight if specified.
            if let Some(max_height) = layout.max_height {
                let max_height = max_height as usize;
                if overlay_lines.len() > max_height {
                    overlay_lines.truncate(max_height);
                }
            }

            // Final row/col with the actual overlay height.
            let final_layout = self.resolve_overlay_layout(
                self.overlay_stack[index].options.as_ref(),
                overlay_lines.len() as f64,
                term_width_f,
                term_height_f,
            );
            let row = final_layout.row as usize;
            let col = final_layout.col as usize;
            self.overlay_stack[index].bounds = Some(OverlayBounds {
                row,
                col,
                width,
                height: overlay_lines.len(),
            });

            min_lines_needed = min_lines_needed.max(row + overlay_lines.len());
            rendered.push(RenderedOverlay {
                entry_index: index,
                overlay_lines,
                row,
                col,
                width,
            });
        }
        self.rendered_overlay_layouts = rendered
            .iter()
            .map(|rendered| RenderedOverlayLayout {
                entry_id: self.overlay_stack[rendered.entry_index].id,
                component: self.overlay_stack[rendered.entry_index].component.clone(),
                row: rendered.row as i64,
                col: rendered.col as i64,
                width: rendered.width,
                height: rendered.overlay_lines.len(),
            })
            .collect();

        // Pad to at least terminal height so overlays have screen-relative
        // positions (see upstream comment about maxLinesRendered inflation).
        let working_height = result.len().max(term_height).max(min_lines_needed);
        while result.len() < working_height {
            result.push(String::new());
        }

        let viewport_start = working_height.saturating_sub(term_height);

        // Composite each overlay.
        for rendered_overlay in &rendered {
            for (i, overlay_line) in rendered_overlay.overlay_lines.iter().enumerate() {
                let idx = viewport_start + rendered_overlay.row + i;
                if idx < result.len() {
                    // Defensive: truncate the overlay line to the declared
                    // width before compositing.
                    let width = rendered_overlay.width;
                    let truncated_overlay_line = if visible_width(overlay_line) > width {
                        slice_by_column(overlay_line, 0, width, true)
                    } else {
                        overlay_line.clone()
                    };
                    result[idx] = composite_tui_line(
                        &result[idx],
                        &truncated_overlay_line,
                        rendered_overlay.col,
                        width,
                        term_width,
                    );
                }
            }
        }

        result
    }

    /// Upstream `applyLineResets`.
    pub fn apply_line_resets(&self, mut lines: Vec<String>) -> Vec<String> {
        for line in lines.iter_mut() {
            if !is_image_line(line) {
                *line = format!("{}{}", normalize_terminal_output(line), SEGMENT_RESET);
            }
        }
        lines
    }

    /// Upstream `extractCursorPosition`: find and strip `CURSOR_MARKER` from
    /// the rendered lines, scanning the bottom `height` lines (visible
    /// viewport). Returns the (row, col) of the marker.
    pub fn extract_cursor_position(
        &self,
        lines: &mut [String],
        height: usize,
    ) -> Option<(usize, usize)> {
        let viewport_top = lines.len().saturating_sub(height);
        for row in (viewport_top..lines.len()).rev() {
            let line = &lines[row];
            if let Some(marker_index) = line.find(CURSOR_MARKER) {
                // Visual column: width of the text before the marker.
                let col = visible_width(&line[..marker_index]);
                // Strip the marker from the line.
                lines[row] = format!(
                    "{}{}",
                    &line[..marker_index],
                    &line[marker_index + CURSOR_MARKER.len()..]
                );
                return Some((row, col));
            }
        }
        None
    }

    pub fn rendered_overlay_layouts(&self) -> &[RenderedOverlayLayout] {
        &self.rendered_overlay_layouts
    }

    fn terminal_dimensions(&self) -> (usize, usize) {
        let terminal = self.terminal.borrow();
        (terminal.columns(), terminal.rows())
    }
}

fn entry_is_visible(entry: &mut OverlayStackEntry, columns: usize, rows: usize) -> bool {
    if entry.hidden {
        return false;
    }
    match entry
        .options
        .as_mut()
        .and_then(|options| options.visible.as_mut())
    {
        Some(visible) => visible(columns, rows),
        None => true,
    }
}

fn overlay_id_of(state: &OverlayFocusRestore) -> Option<u64> {
    match state {
        OverlayFocusRestore::Inactive => None,
        OverlayFocusRestore::Eligible { overlay_id } => Some(*overlay_id),
        OverlayFocusRestore::Blocked { overlay_id, .. } => Some(*overlay_id),
    }
}

fn default_clock() -> impl Fn() -> f64 {
    let start = std::time::Instant::now();
    move || start.elapsed().as_secs_f64() * 1000.0
}

impl Component for Tui {
    fn render(&mut self, width: usize) -> Vec<String> {
        Tui::render(self, width)
    }

    fn handle_mouse(&mut self, _event: &TuiMouseEvent) -> Option<TuiMouseEventResult> {
        // The flag-only projection cannot express the container's dispatched
        // target; routing uses `mouse_action`/`dispatch_mouse_event`.
        None
    }

    fn mouse_action(&mut self, event: &TuiMouseEvent) -> Option<MouseAction> {
        // TuiBase inherits Container.handleMouse; it defines no handleInput,
        // so focus delegation is false (upstream `(this as Component).
        // handleInput` is undefined).
        self.container_mouse_action(event)
    }

    fn is_container_component(&self) -> bool {
        true
    }

    fn invalidate(&mut self) {
        Tui::invalidate(self);
    }
}

/// Upstream `Container`: a component that contains other components.
pub struct Container {
    pub children: Vec<ComponentHandle>,
    mouse_layout: Option<(usize, Vec<(ComponentHandle, usize)>)>,
    /// JS checks method presence (`this.handleInput`) at dispatch time; the
    /// Rust port models the optional handler with this flag.
    pub has_input_handler: bool,
}

impl Default for Container {
    fn default() -> Self {
        Self::new()
    }
}

impl Container {
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
            mouse_layout: None,
            has_input_handler: false,
        }
    }

    pub fn add_child(&mut self, component: ComponentHandle) {
        self.children.push(component);
    }

    pub fn remove_child(&mut self, component: &ComponentHandle) {
        if let Some(index) = self
            .children
            .iter()
            .position(|c| same_component(c, component))
        {
            self.children.remove(index);
        }
    }

    pub fn clear(&mut self) {
        self.children.clear();
    }

    pub fn invalidate_children(&mut self) {
        for child in &self.children {
            child.with_mut(|c| c.invalidate());
        }
    }

    fn mouse_action(&mut self, event: &TuiMouseEvent) -> Option<MouseAction> {
        if event.y < 0 || event.y >= event.height as i64 {
            return None;
        }
        let mouse_children: Vec<(ComponentHandle, usize)> = match &self.mouse_layout {
            Some((cached_width, children)) if *cached_width == event.width => children.clone(),
            _ => self
                .children
                .iter()
                .map(|component| {
                    let height = component.with_mut(|c| c.render(event.width).len());
                    (component.clone(), height)
                })
                .collect(),
        };
        let mut child_y: i64 = 0;
        for (child, child_height) in mouse_children {
            if event.y >= child_y && event.y < child_y + child_height as i64 {
                return Some(MouseAction::Forward {
                    child,
                    event: TuiMouseEvent {
                        y: event.y - child_y,
                        height: child_height,
                        ..event.clone()
                    },
                    delegate_focus: self.has_input_handler,
                    fallback_to_self: false,
                });
            }
            child_y += child_height as i64;
        }
        None
    }
}

impl Component for Container {
    fn render(&mut self, width: usize) -> Vec<String> {
        let mut lines = Vec::new();
        let mut mouse_children = Vec::with_capacity(self.children.len());
        for child in &self.children {
            let child_lines = child.with_mut(|c| c.render(width));
            mouse_children.push((child.clone(), child_lines.len()));
            lines.extend(child_lines);
        }
        self.mouse_layout = Some((width, mouse_children));
        lines
    }

    fn handle_mouse(&mut self, _event: &TuiMouseEvent) -> Option<TuiMouseEventResult> {
        None
    }

    fn mouse_action(&mut self, event: &TuiMouseEvent) -> Option<MouseAction> {
        Container::mouse_action(self, event)
    }

    fn is_container_component(&self) -> bool {
        true
    }

    fn delegates_mouse_focus(&self) -> bool {
        self.has_input_handler
    }

    /// Structural children for `contains_component` (upstream
    /// `Container.children` descent in `isComponentMounted` and
    /// `resolveMouseFocusTarget`).
    fn mouse_child(&mut self, index: usize) -> Option<ComponentHandle> {
        self.children.get(index).cloned()
    }

    fn invalidate(&mut self) {
        Container::invalidate_children(self);
    }
}

#[cfg(test)]
#[path = "tui_tests.rs"]
mod tui_tests;
