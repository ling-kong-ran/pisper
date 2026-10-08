//! Tests for the `tui-main-screen.ts` port: the full upstream scenario set
//! (tui-render.test.ts, tui-shrink.test.ts) plus byte-exact oracle
//! comparisons against the real upstream `TuiMainScreen` run under node
//! (`tests/fixtures/main_screen_oracle/`). Every write array is a FakeTerminal write
//! boundary sequence (one entry per `terminal.write` call), captured by the
//! actual `TuiMainScreen` — including the Kitty-image branches, the bounded
//! 1 MiB chunking, the crash dump, the redraw debug log and the Termux
//! height-change branch.
//!
//! The renderer is shared via `Rc<RefCell<TuiMainScreen>>` (upstream the
//! `Tui` object *is* the `TuiMainScreen` subclass), which also exposes
//! `captureRenderState`/`restoreRenderState` for the state-seam tests.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::json;

use super::*;
use crate::tui::component::Component;
use crate::tui::terminal::Terminal;
use crate::tui::terminal_image::{
    reset_capabilities_cache, set_capabilities, TerminalCapabilities,
};
use crate::tui::tui::{Tui, TuiMode, TuiRenderer, TuiStopOptions};

#[path = "main_screen_oracle_consts.rs"]
mod oracle_consts;

// ------------------------------------------------------------ infrastructure

type SharedTerminal = Rc<RefCell<TestTerminal>>;
type Clock = Rc<Cell<f64>>;

/// Memory pseudo terminal mirroring the oracle's FakeTerminal: writes are
/// recorded with boundaries; hide/show cursor emit the same ANSI sequences
/// the oracle terminal writes.
#[derive(Default)]
struct TestTerminal {
    writes: Vec<String>,
    columns: usize,
    rows: usize,
    input_handler: Option<Box<dyn FnMut(String) + Send>>,
    resize_handler: Option<Box<dyn FnMut() + Send>>,
    start_calls: usize,
    stop_calls: usize,
}

impl TestTerminal {
    fn new(columns: usize, rows: usize) -> Self {
        Self {
            writes: Vec::new(),
            columns,
            rows,
            ..Default::default()
        }
    }

    fn send_input(&mut self, data: &str) {
        if let Some(handler) = &mut self.input_handler {
            handler(data.to_string());
        }
    }

    fn resize(&mut self, columns: usize, rows: usize) {
        self.columns = columns;
        self.rows = rows;
        if let Some(handler) = &mut self.resize_handler {
            handler();
        }
    }
}

impl Terminal for TestTerminal {
    fn start(
        &mut self,
        on_input: Box<dyn FnMut(String) + Send>,
        on_resize: Box<dyn FnMut() + Send>,
    ) {
        self.input_handler = Some(on_input);
        self.resize_handler = Some(on_resize);
        self.start_calls += 1;
    }

    fn stop(&mut self) {
        self.input_handler = None;
        self.resize_handler = None;
        self.stop_calls += 1;
    }

    fn write(&mut self, data: &str) {
        self.writes.push(data.to_string());
    }

    fn columns(&self) -> usize {
        self.columns
    }

    fn rows(&self) -> usize {
        self.rows
    }

    fn kitty_protocol_active(&self) -> bool {
        false
    }

    fn move_by(&mut self, _lines: i64) {}

    fn hide_cursor(&mut self) {
        self.writes.push("\x1b[?25l".to_string());
    }

    fn show_cursor(&mut self) {
        self.writes.push("\x1b[?25h".to_string());
    }

    fn clear_line(&mut self) {}
    fn clear_from_cursor(&mut self) {}
    fn clear_screen(&mut self) {}
    fn set_title(&mut self, _title: &str) {}
    fn set_progress(&mut self, _active: bool) {}
}

/// Delegating renderer: keeps the [`TuiMainScreen`] reachable for the
/// `captureRenderState`/`restoreRenderState` seam (upstream one object).
struct SharedScreen(Rc<RefCell<TuiMainScreen>>);

impl TuiRenderer for SharedScreen {
    fn do_render(&mut self, tui: &mut Tui) {
        self.0.borrow_mut().do_render(tui);
    }

    fn reset_render_state(&mut self, tui: &mut Tui) {
        self.0.borrow_mut().reset_render_state(tui);
    }

    fn before_terminal_stop(&mut self, tui: &mut Tui, options: &TuiStopOptions) {
        self.0.borrow_mut().before_terminal_stop(tui, options);
    }
}

struct Harness {
    terminal: SharedTerminal,
    tui: Tui,
    clock: Clock,
    screen: Rc<RefCell<TuiMainScreen>>,
}

fn make_harness(columns: usize, rows: usize) -> Harness {
    make_harness_with(columns, rows, None, None)
}

fn make_harness_with(
    columns: usize,
    rows: usize,
    show_hardware_cursor: Option<bool>,
    log_directory: Option<String>,
) -> Harness {
    let terminal = Rc::new(RefCell::new(TestTerminal::new(columns, rows)));
    let screen = Rc::new(RefCell::new(TuiMainScreen::new()));
    let clock: Clock = Rc::new(Cell::new(0.0));
    let clock_clone = clock.clone();
    let mut tui = Tui::new(
        terminal.clone(),
        Box::new(SharedScreen(screen.clone())),
        TuiMode::Regular,
        show_hardware_cursor,
        log_directory,
    );
    tui.set_clock(Box::new(move || clock_clone.get()));
    Harness {
        terminal,
        tui,
        clock,
        screen,
    }
}

/// Drain terminal events, next-ticks, then fire due timers within the next
/// `advance_ms` of virtual time (mirrors `waitForRender`'s
/// nextTick + setTimeout(20) upstream).
fn pump(harness: &mut Harness, advance_ms: f64) {
    let target = harness.clock.get() + advance_ms;
    loop {
        harness.tui.poll_terminal_events();
        harness.tui.run_next_ticks();
        match harness.tui.next_timer_deadline() {
            Some(deadline) if deadline <= target => {
                harness.clock.set(deadline);
                harness.tui.fire_due_timer();
            }
            _ => break,
        }
    }
    harness.clock.set(target);
    harness.tui.poll_terminal_events();
    harness.tui.run_next_ticks();
}

fn settle(harness: &mut Harness) {
    pump(harness, 25.0);
}

fn send_input(harness: &mut Harness, data: &str) {
    harness.terminal.borrow_mut().send_input(data);
    harness.tui.poll_terminal_events();
}

fn clear_writes(harness: &mut Harness) {
    harness.terminal.borrow_mut().writes.clear();
}

fn writes(harness: &Harness) -> Vec<String> {
    harness.terminal.borrow().writes.clone()
}

fn joined_writes(harness: &Harness) -> String {
    harness.terminal.borrow().writes.join("")
}

fn assert_writes(actual: &[String], expected: &[&str]) {
    let actual: Vec<&str> = actual.iter().map(String::as_str).collect();
    assert_eq!(actual, expected, "terminal write sequence mismatch");
}

fn assert_oracle_writes(harness: &Harness, expected: &[&str]) {
    assert_writes(&writes(harness), expected);
}

fn assert_oracle_json(actual: serde_json::Value, expected: &str) {
    let expected: serde_json::Value = serde_json::from_str(expected).expect("valid oracle JSON");
    assert_eq!(actual, expected, "oracle JSON mismatch");
}

/// The capability cache, `process.env` and `/tmp/tui` are process-global;
/// serialize the tests in this module so byte-level write assertions cannot
/// be polluted by concurrent scenarios.
fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_temp_dir(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "pi-tui-ms-{label}-{}-{}",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::SeqCst)
    ))
}

/// Mask ISO timestamps the same way the oracle generator did.
fn mask_timestamps(text: &str) -> String {
    regex::Regex::new(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z")
        .expect("timestamp regex")
        .replace_all(text, "<TS>")
        .into_owned()
}

fn sha256_hex_utf16le(text: &str) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    for unit in text.encode_utf16() {
        hasher.update(unit.to_le_bytes());
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

// ---------------------------------------------------------------- components

/// Upstream `TestComponent`/`Lines`; optionally focusable with an
/// `InputComponent`-style input handler.
struct Lines {
    lines: Vec<String>,
    render_count: usize,
    focused: bool,
    set_lines_on_input: bool,
}

impl Lines {
    fn new(lines: &[&str]) -> Self {
        Self {
            lines: lines.iter().map(|line| (*line).to_string()).collect(),
            render_count: 0,
            focused: false,
            set_lines_on_input: false,
        }
    }

    fn input_component(lines: &[&str]) -> Self {
        Self {
            set_lines_on_input: true,
            ..Self::new(lines)
        }
    }

    fn set_lines(&mut self, lines: Vec<String>) {
        self.lines = lines;
    }
}

impl Component for Lines {
    fn render(&mut self, _width: usize) -> Vec<String> {
        self.render_count += 1;
        self.lines.clone()
    }

    fn handle_input(&mut self, data: &str) {
        if self.set_lines_on_input {
            self.lines = vec![data.to_string()];
        }
    }

    fn is_focusable(&self) -> bool {
        true
    }

    fn focused(&self) -> bool {
        self.focused
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }
}

type SharedLines = Rc<RefCell<Lines>>;

fn lines_handle(lines: &[&str]) -> (crate::tui::component_mouse::ComponentHandle, SharedLines) {
    let (handle, shared) =
        crate::tui::component_mouse::ComponentHandle::with_shared(Lines::new(lines));
    (handle, shared)
}

/// `&["a", "b"]` as owned lines (keeps the literal arrays clippy-clean).
fn strings(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|line| line.to_string()).collect()
}

// ------------------------------------------------------------------- tests
//
// The differential-path scenarios below replay the oracle scenario block
// step-for-step (the upstream "TUI differential rendering" its: non-adjacent
// changes, appends, deleted tails, cursor tracking after shrink, first/last
// line changes, content -> empty -> content), asserting byte-exact writes at
// each clearWrites boundary.

#[test]
fn first_render_stop_modes_and_no_change_oracle() {
    let _lock = test_lock();
    reset_capabilities_cache();
    let mut harness = make_harness(40, 10);
    let (component, _shared) = lines_handle(&["Line 0", "Line 1", "Line 2"]);
    harness.tui.add_child(component);
    harness.tui.start();
    settle(&mut harness);
    assert_oracle_writes(&harness, oracle_consts::FIRST_RENDER_WRITES);
    assert_eq!(
        harness.tui.full_redraws(),
        oracle_consts::FIRST_RENDER_FULL_REDRAWS
    );
    clear_writes(&mut harness);
    harness.tui.stop(TuiStopOptions::default());
    settle(&mut harness);
    assert_oracle_writes(&harness, oracle_consts::STOP_DEFAULT_WRITES);
    // Render after stop is a no-op.
    clear_writes(&mut harness);
    harness.tui.request_render(false);
    settle(&mut harness);
    assert_oracle_writes(&harness, oracle_consts::RENDER_AFTER_STOP_WRITES);
}

#[test]
fn differential_render_paths_oracle() {
    let _lock = test_lock();
    reset_capabilities_cache();
    let mut harness = make_harness(40, 10);
    let (component, shared) = lines_handle(&["Line 0", "Line 1", "Line 2", "Line 3", "Line 4"]);
    harness.tui.add_child(component);
    harness.tui.start();
    settle(&mut harness);
    clear_writes(&mut harness);

    // Multiple non-adjacent lines change.
    shared.borrow_mut().set_lines(vec![
        "Line 0".into(),
        "CHANGED 1".into(),
        "Line 2".into(),
        "CHANGED 3".into(),
        "Line 4".into(),
    ]);
    harness.tui.request_render(false);
    settle(&mut harness);
    assert_oracle_writes(&harness, oracle_consts::NONADJACENT_CHANGE_WRITES);
    clear_writes(&mut harness);

    // Append two lines (appendStart path).
    shared.borrow_mut().set_lines(vec![
        "Line 0".into(),
        "CHANGED 1".into(),
        "Line 2".into(),
        "CHANGED 3".into(),
        "Line 4".into(),
        "Line 5".into(),
        "Line 6".into(),
    ]);
    harness.tui.request_render(false);
    settle(&mut harness);
    assert_oracle_writes(&harness, oracle_consts::APPEND_LINES_WRITES);
    clear_writes(&mut harness);

    // Delete two tail lines (deleted-lines path, unchanged head).
    shared.borrow_mut().set_lines(
        ["Line 0", "CHANGED 1", "Line 2", "CHANGED 3", "Line 4"]
            .iter()
            .map(|line| line.to_string())
            .collect(),
    );
    harness.tui.request_render(false);
    settle(&mut harness);
    assert_oracle_writes(&harness, oracle_consts::DELETE_TAIL_LINES_WRITES);
    clear_writes(&mut harness);

    // Cursor tracking after shrink: 5 -> 3 lines with line 1 changed.
    shared.borrow_mut().set_lines(
        ["Line 0", "CHANGED", "Line 2"]
            .iter()
            .map(|line| line.to_string())
            .collect(),
    );
    harness.tui.request_render(false);
    settle(&mut harness);
    assert_oracle_writes(&harness, oracle_consts::SHRINK_THEN_CHANGE_WRITES);
    clear_writes(&mut harness);

    // First-line-only change.
    shared.borrow_mut().set_lines(
        ["CHANGED", "Line 1", "Line 2"]
            .iter()
            .map(|line| line.to_string())
            .collect(),
    );
    harness.tui.request_render(false);
    settle(&mut harness);
    assert_oracle_writes(&harness, oracle_consts::FIRST_LINE_CHANGE_WRITES);
    clear_writes(&mut harness);

    shared.borrow_mut().set_lines(vec![
        "CHANGED".to_string(),
        "Line 1".to_string(),
        "Line 2".to_string(),
        "Line 3".to_string(),
    ]);
    harness.tui.request_render(false);
    settle(&mut harness);
    assert_oracle_writes(&harness, oracle_consts::APPEND_AFTER_CHANGE_WRITES);
    clear_writes(&mut harness);

    // Last-line-only change.
    shared.borrow_mut().set_lines(vec![
        "CHANGED".to_string(),
        "Line 1".to_string(),
        "Line 2".to_string(),
        "FINAL".to_string(),
    ]);
    harness.tui.request_render(false);
    settle(&mut harness);
    assert_oracle_writes(&harness, oracle_consts::LAST_LINE_CHANGE_WRITES);
    clear_writes(&mut harness);

    // Content -> empty -> content.
    shared.borrow_mut().set_lines(Vec::new());
    harness.tui.request_render(false);
    settle(&mut harness);
    assert_oracle_writes(&harness, oracle_consts::SHRINK_TO_EMPTY_WRITES);
    clear_writes(&mut harness);
    shared.borrow_mut().set_lines(
        ["New Line 0", "New Line 1"]
            .iter()
            .map(|line| line.to_string())
            .collect(),
    );
    harness.tui.request_render(false);
    settle(&mut harness);
    assert_oracle_writes(&harness, oracle_consts::REGROW_AFTER_EMPTY_WRITES);
}

#[test]
fn stop_preserves_screen_when_requested() {
    let _lock = test_lock();
    reset_capabilities_cache();
    let mut harness = make_harness(40, 10);
    let (component, _shared) = lines_handle(&["Line 0", "Line 1", "Line 2"]);
    harness.tui.add_child(component);
    harness.tui.start();
    settle(&mut harness);
    clear_writes(&mut harness);
    harness.tui.stop(TuiStopOptions {
        preserve_screen: true,
    });
    settle(&mut harness);
    assert_oracle_writes(&harness, oracle_consts::STOP_PRESERVE_SCREEN_WRITES);
}

#[test]
fn spinner_frames_stay_byte_identical() {
    let _lock = test_lock();
    reset_capabilities_cache();
    let mut harness = make_harness(40, 10);
    let (component, shared) = lines_handle(&[]);
    harness.tui.add_child(component);
    harness.tui.start();
    settle(&mut harness);
    clear_writes(&mut harness);
    for (frame, expected) in [
        ("|", oracle_consts::SPINNER_FRAME_0_WRITES),
        ("/", oracle_consts::SPINNER_FRAME_1_WRITES),
        ("-", oracle_consts::SPINNER_FRAME_2_WRITES),
        ("\\", oracle_consts::SPINNER_FRAME_3_WRITES),
    ] {
        shared.borrow_mut().set_lines(vec![
            "Header".into(),
            format!("Working {frame}"),
            "Footer".into(),
        ]);
        harness.tui.request_render(false);
        settle(&mut harness);
        assert_oracle_writes(&harness, expected);
        clear_writes(&mut harness);
    }
}

#[test]
fn no_change_render_only_moves_cursor() {
    let _lock = test_lock();
    reset_capabilities_cache();
    let mut harness = make_harness(40, 10);
    let (component, _shared) = lines_handle(&["Line 0", "Line 1", "Line 2"]);
    harness.tui.add_child(component);
    harness.tui.start();
    settle(&mut harness);
    clear_writes(&mut harness);
    harness.tui.request_render(false);
    settle(&mut harness);
    assert_oracle_writes(&harness, oracle_consts::NO_CHANGE_WRITES);
}

// tui-render.test.ts "TUI content shrinkage" + tui-shrink.test.ts.

#[test]
fn clear_on_shrink_full_redraws_and_resets() {
    let _lock = test_lock();
    reset_capabilities_cache();
    let mut harness = make_harness(40, 10);
    harness.tui.set_clear_on_shrink(true);
    let (component, shared) = lines_handle(&[]);
    harness.tui.add_child(component);
    shared.borrow_mut().set_lines(strings(&[
        "Line 0", "Line 1", "Line 2", "Line 3", "Line 4", "Line 5",
    ]));
    harness.tui.start();
    settle(&mut harness);
    let redraws_before = harness.tui.full_redraws();
    clear_writes(&mut harness);
    shared
        .borrow_mut()
        .set_lines(strings(&["Line 0", "Line 1"]));
    harness.tui.request_render(false);
    settle(&mut harness);
    assert_oracle_writes(&harness, oracle_consts::CLEAR_ON_SHRINK_WRITES);
    assert_eq!(
        harness.tui.full_redraws() - redraws_before,
        oracle_consts::CLEAR_ON_SHRINK_DELTA
    );
    clear_writes(&mut harness);
    shared.borrow_mut().set_lines(vec!["Only line".to_string()]);
    harness.tui.request_render(false);
    settle(&mut harness);
    assert_oracle_writes(&harness, oracle_consts::CLEAR_ON_SHRINK_SINGLE_WRITES);
    clear_writes(&mut harness);
    shared.borrow_mut().set_lines(Vec::new());
    harness.tui.request_render(false);
    settle(&mut harness);
    assert_oracle_writes(&harness, oracle_consts::CLEAR_ON_SHRINK_EMPTY_WRITES);
}

#[test]
fn clear_children_shrinks_to_zero_without_clear_on_shrink() {
    let _lock = test_lock();
    reset_capabilities_cache();
    let mut harness = make_harness(40, 10);
    let (component, _shared) = lines_handle(&["first", "second", "third"]);
    harness.tui.add_child(component);
    harness.tui.start();
    settle(&mut harness);
    clear_writes(&mut harness);
    harness.tui.clear();
    harness.tui.request_render(false);
    settle(&mut harness);
    assert_oracle_writes(&harness, oracle_consts::CLEAR_CHILDREN_WRITES);
}

// tui-render.test.ts "full re-renders when deleted lines move the viewport
// upward" + "appends after a shrink without another full redraw".

#[test]
fn deleted_lines_moving_viewport_up_force_full_redraw() {
    let _lock = test_lock();
    reset_capabilities_cache();
    let mut harness = make_harness(20, 5);
    let (component, shared) = lines_handle(&[]);
    harness.tui.add_child(component);
    shared
        .borrow_mut()
        .set_lines((0..12).map(|i| format!("Line {i}")).collect::<Vec<_>>());
    harness.tui.start();
    settle(&mut harness);
    let redraws_before = harness.tui.full_redraws();
    clear_writes(&mut harness);
    shared
        .borrow_mut()
        .set_lines((0..7).map(|i| format!("Line {i}")).collect());
    harness.tui.request_render(false);
    settle(&mut harness);
    assert_oracle_writes(&harness, oracle_consts::DELETED_VIEWPORT_UP_WRITES);
    assert_eq!(
        harness.tui.full_redraws() - redraws_before,
        oracle_consts::DELETED_VIEWPORT_UP_DELTA
    );
    clear_writes(&mut harness);
    let redraws_after_shrink = harness.tui.full_redraws();
    shared
        .borrow_mut()
        .set_lines(strings(&["Line 0", "Line 1", "Line 2"]));
    harness.tui.request_render(false);
    settle(&mut harness);
    assert_oracle_writes(&harness, oracle_consts::APPEND_AFTER_VIEWPORT_RESET_WRITES);
    assert_eq!(
        harness.tui.full_redraws() - redraws_after_shrink,
        oracle_consts::APPEND_AFTER_VIEWPORT_RESET_DELTA
    );
}

// tui-render.test.ts "clears stale content when maxLinesRendered was
// inflated by a transient component".

#[test]
fn transient_component_inflation_clears_stale_content() {
    let _lock = test_lock();
    reset_capabilities_cache();
    let mut harness = make_harness(40, 10);
    let (chat, chat_shared) = lines_handle(&[]);
    let (editor, editor_shared) = lines_handle(&[]);
    harness.tui.add_child(chat);
    harness.tui.add_child(editor);
    let long_chat: Vec<String> = (0..15).map(|i| format!("Chat {i}")).collect();
    let short_chat: Vec<String> = (0..12).map(|i| format!("Chat {i}")).collect();
    let editor_lines: Vec<String> = ["Editor 0", "Editor 1", "Editor 2"]
        .iter()
        .map(|l| l.to_string())
        .collect();
    let selector_lines: Vec<String> = (0..8).map(|i| format!("Selector {i}")).collect();
    chat_shared.borrow_mut().set_lines(long_chat);
    editor_shared.borrow_mut().set_lines(editor_lines.clone());
    harness.tui.start();
    settle(&mut harness);
    editor_shared.borrow_mut().set_lines(selector_lines);
    harness.tui.request_render(false);
    settle(&mut harness);
    editor_shared.borrow_mut().set_lines(editor_lines);
    harness.tui.request_render(false);
    settle(&mut harness);
    let redraws_before_switch = harness.tui.full_redraws();
    clear_writes(&mut harness);
    chat_shared.borrow_mut().set_lines(short_chat);
    harness.tui.request_render(false);
    settle(&mut harness);
    assert_oracle_writes(&harness, oracle_consts::TRANSIENT_INFLATION_WRITES);
    assert_eq!(
        harness.tui.full_redraws() - redraws_before_switch,
        oracle_consts::TRANSIENT_INFLATION_DELTA
    );
}

// tui-render.test.ts "TUI resize handling".

#[test]
fn width_and_height_changes_force_full_redraws() {
    let _lock = test_lock();
    reset_capabilities_cache();
    std::env::remove_var("TERMUX_VERSION");
    let mut harness = make_harness(40, 10);
    let (component, _shared) = lines_handle(&["Line 0", "Line 1", "Line 2"]);
    harness.tui.add_child(component);
    harness.tui.start();
    settle(&mut harness);
    let redraws_before = harness.tui.full_redraws();
    clear_writes(&mut harness);
    harness.terminal.borrow_mut().resize(60, 10);
    settle(&mut harness);
    assert_oracle_writes(&harness, oracle_consts::RESIZE_WIDTH_WRITES);
    assert_eq!(
        harness.tui.full_redraws() - redraws_before,
        oracle_consts::RESIZE_WIDTH_DELTA
    );
    clear_writes(&mut harness);
    harness.terminal.borrow_mut().resize(60, 15);
    settle(&mut harness);
    assert_oracle_writes(&harness, oracle_consts::RESIZE_HEIGHT_WRITES);
    assert_eq!(
        harness.tui.full_redraws() - redraws_before - oracle_consts::RESIZE_WIDTH_DELTA,
        oracle_consts::RESIZE_HEIGHT_DELTA
    );
}

#[test]
fn termux_height_changes_stay_differential() {
    let _lock = test_lock();
    reset_capabilities_cache();
    std::env::set_var("TERMUX_VERSION", "1");
    let mut harness = make_harness(40, 10);
    let (component, shared) = lines_handle(&[]);
    harness.tui.add_child(component);
    shared
        .borrow_mut()
        .set_lines((0..20).map(|i| format!("Line {i}")).collect());
    harness.tui.start();
    settle(&mut harness);
    clear_writes(&mut harness);
    let redraws_before = harness.tui.full_redraws();
    for (height, expected) in [
        (15, oracle_consts::TERMUX_RESIZE_FRAME_0_WRITES),
        (8, oracle_consts::TERMUX_RESIZE_FRAME_1_WRITES),
        (14, oracle_consts::TERMUX_RESIZE_FRAME_2_WRITES),
        (11, oracle_consts::TERMUX_RESIZE_FRAME_3_WRITES),
    ] {
        harness.terminal.borrow_mut().resize(40, height);
        settle(&mut harness);
        assert_oracle_writes(&harness, expected);
        let joined = joined_writes(&harness);
        assert!(
            !joined.contains("\x1b[2J"),
            "height change must not clear the screen"
        );
        assert!(
            !joined.contains("\x1b[3J"),
            "height change must not clear scrollback"
        );
        clear_writes(&mut harness);
    }
    assert_eq!(
        harness.tui.full_redraws() - redraws_before,
        oracle_consts::TERMUX_RESIZE_DELTA
    );
    std::env::remove_var("TERMUX_VERSION");
}

// tui-render.test.ts cursor marker positioning (hardware cursor seam).

#[test]
fn cursor_marker_positions_hardware_cursor() {
    let _lock = test_lock();
    reset_capabilities_cache();
    let mut harness = make_harness(40, 10);
    let (component, shared) = lines_handle(&[]);
    harness.tui.add_child(component);
    shared.borrow_mut().set_lines(vec![
        format!("alpha{}", crate::tui::component::CURSOR_MARKER),
        "beta".to_string(),
    ]);
    harness.tui.start();
    settle(&mut harness);
    clear_writes(&mut harness);
    shared.borrow_mut().set_lines(vec![
        "alpha".to_string(),
        format!("beta{}", crate::tui::component::CURSOR_MARKER),
    ]);
    harness.tui.request_render(false);
    settle(&mut harness);
    assert_oracle_writes(&harness, oracle_consts::CURSOR_MARKER_MOVED_WRITES);

    // With showHardwareCursor the cursor stays visible.
    let mut visible = make_harness_with(40, 10, Some(true), None);
    let (component, shared) = lines_handle(&[]);
    visible.tui.add_child(component);
    shared.borrow_mut().set_lines(vec![
        format!("alpha{}", crate::tui::component::CURSOR_MARKER),
        "beta".to_string(),
    ]);
    visible.tui.start();
    settle(&mut visible);
    clear_writes(&mut visible);
    shared.borrow_mut().set_lines(vec![
        "alpha".to_string(),
        format!("beta{}", crate::tui::component::CURSOR_MARKER),
    ]);
    visible.tui.request_render(false);
    settle(&mut visible);
    assert_oracle_writes(&visible, oracle_consts::CURSOR_MARKER_VISIBLE_WRITES);
}

// tui-render.test.ts "resets styles after each rendered line".

#[test]
fn styles_reset_after_each_rendered_line() {
    let _lock = test_lock();
    reset_capabilities_cache();
    let mut harness = make_harness(20, 6);
    let (component, _shared) = lines_handle(&["\x1b[3mItalic", "Plain"]);
    harness.tui.add_child(component);
    harness.tui.start();
    settle(&mut harness);
    assert_oracle_writes(&harness, oracle_consts::STYLES_RESET_WRITES);
    // The reset suffix (SEGMENT_RESET) closes the italic before "Plain".
    let joined = joined_writes(&harness);
    assert!(joined.contains("\x1b[3mItalic\x1b[0m\x1b]8;;\x07\r\nPlain"));
}

// tui-render.test.ts "TUI render scheduling".

#[test]
fn keyboard_input_preempts_throttled_frame() {
    let _lock = test_lock();
    reset_capabilities_cache();
    let mut harness = make_harness(40, 10);
    let (component, shared) =
        crate::tui::component_mouse::ComponentHandle::with_shared(Lines::input_component(&[
            "initial",
        ]));
    harness.tui.add_child(component.clone());
    harness.tui.set_focus(Some(component));
    harness.tui.start();
    harness.tui.render_now(false);
    let render_count_before_input = shared.borrow().render_count;
    shared.borrow_mut().set_lines(vec!["pending".to_string()]);
    harness.tui.request_render(false);
    send_input(&mut harness, "first");
    send_input(&mut harness, "second");
    send_input(&mut harness, "typed");
    harness.tui.run_next_ticks();
    assert_oracle_json(
        json!({
            "renderCountDelta": shared.borrow().render_count - render_count_before_input,
            "lines": shared.borrow().lines,
        }),
        oracle_consts::KEYBOARD_PREEMPT,
    );
    harness.tui.stop(TuiStopOptions::default());
}

// tui-render.test.ts "TUI bounded render output".

fn owned_lines_handle(
    lines: Vec<String>,
) -> (crate::tui::component_mouse::ComponentHandle, SharedLines) {
    let (handle, shared) =
        crate::tui::component_mouse::ComponentHandle::with_shared(Lines::new(&[]));
    shared.borrow_mut().set_lines(lines);
    (handle, shared)
}

#[test]
fn bounded_full_render_splits_without_changing_output() {
    let _lock = test_lock();
    reset_capabilities_cache();
    let kitty_huge = format!("\x1b_Ga=T,f=100;{}\x1b\\", "A".repeat(1_200_000));
    let mut harness = make_harness(80, 24);
    let (component, _shared) = owned_lines_handle(vec![kitty_huge.clone(), kitty_huge.clone()]);
    harness.tui.add_child(component);
    harness.tui.render_now(false);

    let expected: serde_json::Value =
        serde_json::from_str(oracle_consts::BOUNDED_FULL).expect("valid oracle JSON");
    let joined = joined_writes(&harness);
    assert_eq!(
        serde_json::to_value(harness.terminal.borrow().writes.len()).unwrap(),
        expected["writeCount"],
        "write boundary count mismatch"
    );
    let lengths: Vec<usize> = harness
        .terminal
        .borrow()
        .writes
        .iter()
        .map(String::len)
        .collect();
    assert_eq!(
        serde_json::to_value(lengths).unwrap(),
        expected["writeLengths"],
        "write boundary lengths mismatch"
    );
    // JS string .length counts UTF-16 units.
    let units: Vec<u16> = joined.encode_utf16().collect();
    assert_eq!(
        serde_json::to_value(units.len()).unwrap(),
        expected["joinedLength"]
    );
    assert_eq!(sha256_hex_utf16le(&joined), expected["joinedSha256"]);
    let head = String::from_utf16_lossy(&units[..80]);
    let tail = String::from_utf16_lossy(&units[units.len() - 80..]);
    assert_eq!(head, expected["head"]);
    assert_eq!(tail, expected["tail"]);
    assert_eq!(
        serde_json::to_value(harness.tui.full_redraws()).unwrap(),
        expected["fullRedraws"],
        "first render counts as one full redraw"
    );
}

#[test]
fn bounded_differential_render_stays_on_diff_path() {
    let _lock = test_lock();
    reset_capabilities_cache();
    let kitty_huge = format!("\x1b_Ga=T,f=100;{}\x1b\\", "A".repeat(1_200_000));
    let mut harness = make_harness(80, 24);
    let (component, shared) = owned_lines_handle(vec!["before".to_string()]);
    harness.tui.add_child(component);
    harness.tui.render_now(false);
    clear_writes(&mut harness);
    shared.borrow_mut().set_lines(vec![
        "before".to_string(),
        kitty_huge.clone(),
        kitty_huge.clone(),
    ]);
    harness.tui.render_now(false);

    let expected: serde_json::Value =
        serde_json::from_str(oracle_consts::BOUNDED_DIFF).expect("valid oracle JSON");
    let joined = joined_writes(&harness);
    assert_eq!(
        serde_json::to_value(harness.terminal.borrow().writes.len()).unwrap(),
        expected["writeCount"],
        "write boundary count mismatch"
    );
    let lengths: Vec<usize> = harness
        .terminal
        .borrow()
        .writes
        .iter()
        .map(String::len)
        .collect();
    assert_eq!(
        serde_json::to_value(lengths).unwrap(),
        expected["writeLengths"],
        "write boundary lengths mismatch"
    );
    let units: Vec<u16> = joined.encode_utf16().collect();
    assert_eq!(
        serde_json::to_value(units.len()).unwrap(),
        expected["joinedLength"]
    );
    assert_eq!(sha256_hex_utf16le(&joined), expected["joinedSha256"]);
    let head = String::from_utf16_lossy(&units[..80]);
    let tail = String::from_utf16_lossy(&units[units.len() - 80..]);
    assert_eq!(head, expected["head"]);
    assert_eq!(tail, expected["tail"]);
    assert_eq!(
        joined.starts_with("\x1b[?2026h"),
        expected["startsWithSync"]
    );
    assert_eq!(joined.ends_with("\x1b[?2026l"), expected["endsWithSync"]);
    assert_eq!(joined.contains("\x1b[2J"), expected["hasFullClear"]);
}

// tui-render.test.ts "TUI Kitty image cleanup" — byte-exact via oracle.

fn kitty_harness(columns: usize, rows: usize) -> Harness {
    reset_capabilities_cache();
    set_capabilities(TerminalCapabilities {
        hyperlinks: true,
        images: Some("kitty"),
        true_color: true,
    });
    make_harness(columns, rows)
}

fn kitty_teardown(harness: &mut Harness) {
    harness.tui.stop(TuiStopOptions::default());
    reset_capabilities_cache();
}

/// The placement lines the oracle used: one Kitty sequence plus the reserved
/// placeholder rows the image occupies.
fn image_lines(sequence: &str, rows: usize) -> Vec<String> {
    let mut lines = vec![sequence.to_string()];
    lines.resize(rows, String::new());
    lines
}

#[test]
fn kitty_reserved_rows_cleared_before_placement() {
    let _lock = test_lock();
    let kitty2 = "\x1b_Ga=T,f=100,q=2,C=1,c=2,r=2,i=42;AAAA\x1b\\";
    let mut harness = kitty_harness(40, 10);
    let (component, shared) = lines_handle(&["before"]);
    harness.tui.add_child(component);
    harness.tui.start();
    settle(&mut harness);
    clear_writes(&mut harness);
    let mut lines = vec!["before".to_string()];
    lines.extend(image_lines(kitty2, 2));
    lines.push("after".to_string());
    shared.borrow_mut().set_lines(lines);
    harness.tui.request_render(false);
    settle(&mut harness);
    assert_oracle_writes(&harness, oracle_consts::KITTY_RESERVED_ROWS_DIFF_WRITES);
    kitty_teardown(&mut harness);
}

#[test]
fn kitty_preclear_that_would_scroll_falls_back_to_full_redraw() {
    let _lock = test_lock();
    let kitty2 = "\x1b_Ga=T,f=100,q=2,C=1,c=2,r=2,i=42;AAAA\x1b\\";
    let mut harness = kitty_harness(40, 2);
    let (component, shared) = lines_handle(&["before"]);
    harness.tui.add_child(component);
    harness.tui.start();
    settle(&mut harness);
    let redraws_before = harness.tui.full_redraws();
    clear_writes(&mut harness);
    let mut lines = vec!["before".to_string()];
    lines.extend(image_lines(kitty2, 2));
    lines.push("after".to_string());
    shared.borrow_mut().set_lines(lines);
    harness.tui.request_render(false);
    settle(&mut harness);
    assert_oracle_writes(&harness, oracle_consts::KITTY_PRECLEAR_SCROLL_WRITES);
    assert_eq!(
        harness.tui.full_redraws() - redraws_before,
        oracle_consts::KITTY_PRECLEAR_SCROLL_DELTA
    );
    let joined = joined_writes(&harness);
    assert!(
        joined.contains("\x1b[2J"),
        "fallback should clear and fully redraw"
    );
    kitty_teardown(&mut harness);
}

#[test]
fn kitty_full_redraw_reserves_visible_image_rows() {
    let _lock = test_lock();
    let kitty3 = "\x1b_Ga=T,f=100,q=2,C=1,c=3,r=3,i=55;AAAA\x1b\\";
    let mut harness = kitty_harness(40, 5);
    let (component, shared) = lines_handle(&["l0", "l1", "l2", "l3", "l4"]);
    harness.tui.add_child(component);
    harness.tui.start();
    settle(&mut harness);
    let redraws_before = harness.tui.full_redraws();
    clear_writes(&mut harness);
    let mut lines: Vec<String> = ["l0", "l1", "l2", "l3", "l4"]
        .iter()
        .map(|l| l.to_string())
        .collect();
    lines.extend(image_lines(kitty3, 3));
    lines.push("after".to_string());
    shared.borrow_mut().set_lines(lines);
    harness.tui.request_render(false);
    settle(&mut harness);
    assert_oracle_writes(&harness, oracle_consts::KITTY_FULLREDRAW_RESERVED_WRITES);
    assert_eq!(
        harness.tui.full_redraws() - redraws_before,
        oracle_consts::KITTY_FULLREDRAW_RESERVED_DELTA
    );
    let joined = joined_writes(&harness);
    assert!(
        joined.contains(&format!("\r\n\r\n\x1b[2A{kitty3}\x1b[2B")),
        "full redraw should reserve visible image rows before drawing the placement"
    );
    assert!(
        !joined.contains(&format!("{kitty3}\r\n\x1b[0m")),
        "full redraw must not write reserved padding rows after drawing the placement"
    );
    kitty_teardown(&mut harness);
}

#[test]
fn kitty_taller_than_viewport_keeps_first_row_placement() {
    let _lock = test_lock();
    let kitty6 = "\x1b_Ga=T,f=100,q=2,C=1,c=6,r=6,i=66;AAAA\x1b\\";
    let mut harness = kitty_harness(40, 5);
    let (component, shared) = lines_handle(&["before"]);
    harness.tui.add_child(component);
    harness.tui.start();
    settle(&mut harness);
    clear_writes(&mut harness);
    let mut lines = vec!["before".to_string()];
    lines.extend(image_lines(kitty6, 6));
    lines.push("after".to_string());
    shared.borrow_mut().set_lines(lines);
    harness.tui.request_render(true);
    settle(&mut harness);
    assert_oracle_writes(&harness, oracle_consts::KITTY_TALLER_THAN_VIEWPORT_WRITES);
    let joined = joined_writes(&harness);
    assert!(joined.contains(kitty6), "image placement should be drawn");
    let has_cursor_up = joined.contains(&format!("\x1b[7A{kitty6}"));
    assert_eq!(
        serde_json::to_value(has_cursor_up).unwrap(),
        serde_json::from_str::<serde_json::Value>(oracle_consts::KITTY_TALLER_HAS_CURSOR_UP_PREFIX)
            .unwrap()["value"],
        "taller-than-viewport images must keep the first-row placement path"
    );
    kitty_teardown(&mut harness);
}

#[test]
fn kitty_deletes_changed_ids_before_drawing_moved_placements() {
    let _lock = test_lock();
    reset_capabilities_cache();
    let old_image = "\x1b_Ga=T,f=100,q=2,C=1,c=2,r=2,i=42;AAAA\x1b\\";
    let new_image = "\x1b_Ga=T,f=100,q=2,C=1,c=2,r=1,i=42;BBBB\x1b\\";
    let delete_42 = "\x1b_Ga=d,d=I,i=42,q=2\x1b\\";
    let mut harness = make_harness(40, 10);
    let (component, shared) = lines_handle(&["top", old_image]);
    harness.tui.add_child(component);
    harness.tui.start();
    settle(&mut harness);
    clear_writes(&mut harness);
    shared
        .borrow_mut()
        .set_lines(vec![new_image.to_string(), String::new()]);
    harness.tui.request_render(false);
    settle(&mut harness);
    let joined = joined_writes(&harness);
    assert_eq!(joined, oracle_consts::KITTY_DELETE_CHANGED_JOINED);
    let delete_index = joined.find(delete_42);
    let draw_index = joined.find(new_image);
    assert_oracle_json(
        json!({
            "deleteIndex": delete_index.map(|i| i as i64).unwrap_or(-1),
            "drawIndex": draw_index.map(|i| i as i64).unwrap_or(-1),
            "deleteBeforeDraw": matches!(delete_index, Some(d) if draw_index.is_some_and(|n| d < n)),
        }),
        oracle_consts::KITTY_DELETE_CHANGED_META,
    );
    harness.tui.stop(TuiStopOptions::default());
}

#[test]
fn kitty_reserved_row_change_redraws_image_line() {
    let _lock = test_lock();
    reset_capabilities_cache();
    let image = "\x1b_Ga=T,f=100,q=2,C=1,c=2,r=2,i=88;AAAA\x1b\\";
    let delete_88 = "\x1b_Ga=d,d=I,i=88,q=2\x1b\\";
    let mut harness = make_harness(40, 10);
    let (component, shared) = lines_handle(&["", image]);
    harness.tui.add_child(component);
    harness.tui.start();
    settle(&mut harness);
    clear_writes(&mut harness);
    shared
        .borrow_mut()
        .set_lines(vec!["covered".to_string(), image.to_string()]);
    harness.tui.request_render(false);
    settle(&mut harness);
    let joined = joined_writes(&harness);
    assert_eq!(joined, oracle_consts::KITTY_RESERVED_ROW_CHANGE_JOINED);
    let delete_index = joined.find(delete_88);
    let draw_index = joined.find(image);
    assert_oracle_json(
        json!({
            "deleteIndex": delete_index.map(|i| i as i64).unwrap_or(-1),
            "drawIndex": draw_index.map(|i| i as i64).unwrap_or(-1),
            "deleteBeforeDraw": matches!(delete_index, Some(d) if draw_index.is_some_and(|n| d < n)),
            "hasFullClear": joined.contains("\x1b[2J"),
        }),
        oracle_consts::KITTY_RESERVED_ROW_CHANGE_META,
    );
    harness.tui.stop(TuiStopOptions::default());
}

#[test]
fn kitty_full_redraws_delete_previously_rendered_ids() {
    let _lock = test_lock();
    reset_capabilities_cache();
    let image = "\x1b_Ga=T,f=100,q=2,C=1,c=2,r=2,i=77;AAAA\x1b\\";
    let delete_77 = "\x1b_Ga=d,d=I,i=77,q=2\x1b\\";
    let mut harness = make_harness(40, 10);
    let (component, shared) = lines_handle(&[image]);
    harness.tui.add_child(component);
    harness.tui.start();
    settle(&mut harness);
    clear_writes(&mut harness);
    shared
        .borrow_mut()
        .set_lines(vec!["plain text".to_string()]);
    harness.tui.request_render(true);
    settle(&mut harness);
    let joined = joined_writes(&harness);
    assert_eq!(
        joined,
        oracle_consts::KITTY_FULLREDRAW_DELETES_PREVIOUS_JOINED
    );
    let delete_index = joined.find(delete_77);
    let clear_index = joined.find("\x1b[2J");
    assert_oracle_json(
        json!({
            "deleteIndex": delete_index.map(|i| i as i64).unwrap_or(-1),
            "clearIndex": clear_index.map(|i| i as i64).unwrap_or(-1),
            "deleteBeforeClear": matches!(delete_index, Some(d) if clear_index.is_some_and(|n| d < n)),
        }),
        oracle_consts::KITTY_FULLREDRAW_DELETES_PREVIOUS_META,
    );
    harness.tui.stop(TuiStopOptions::default());
}

// tui-render.test.ts "TUI crash dump" (both the configured-directory and the
// OS-temp-directory fallback paths).

fn assert_crash_behavior(harness: &mut Harness, expected_message: &str, expected_text: &str) {
    let (component, shared) = lines_handle(&["ok"]);
    harness.tui.add_child(component);
    harness.tui.start();
    settle(harness);
    clear_writes(harness);

    shared
        .borrow_mut()
        .set_lines(vec!["ok".to_string(), "x".repeat(60)]);
    let panic_payload = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        harness.tui.render_now(false);
    }))
    .expect_err("width overflow must panic");
    let message = panic_payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| panic_payload.downcast_ref::<&str>().map(|s| s.to_string()))
        .expect("string panic payload");
    assert_eq!(
        normalize_path(&message),
        normalize_path(expected_message),
        "panic message must match the oracle (masked paths)"
    );

    let crash_text = std::fs::read_to_string(expected_crash_path(harness)).expect("crash log");
    assert_eq!(
        normalize_path(&mask_timestamps(&crash_text)),
        normalize_path(expected_text),
        "crash dump content must match the oracle (masked timestamps/paths)"
    );
    // The inline stop transition ran before the panic (upstream `this.stop()`).
    assert_oracle_writes(harness, oracle_consts::CRASH_DUMP_LOGDIR_STOP_WRITES);
}

fn normalize_path(text: &str) -> String {
    text.replace('\\', "/")
}

fn expected_crash_path(harness: &Harness) -> std::path::PathBuf {
    let base = harness
        .tui
        .log_directory
        .clone()
        .unwrap_or_else(|| std::env::temp_dir().to_string_lossy().into_owned());
    std::path::Path::new(&base).join("pi-tui-crash.log")
}

#[test]
fn crash_dump_written_to_configured_log_directory() {
    let _lock = test_lock();
    reset_capabilities_cache();
    let log_dir = unique_temp_dir("crash-logdir");
    std::fs::create_dir_all(&log_dir).expect("create log dir");
    let mut harness = make_harness_with(40, 10, None, Some(log_dir.to_string_lossy().into_owned()));
    // The oracle masked the machine-specific log directory as <LOG_DIR>.
    let message =
        oracle_consts::CRASH_DUMP_LOGDIR_MESSAGE.replace("<LOG_DIR>", &log_dir.to_string_lossy());
    let text =
        oracle_consts::CRASH_DUMP_LOGDIR_TEXT.replace("<LOG_DIR>", &log_dir.to_string_lossy());
    assert_crash_behavior(&mut harness, &message, &text);
    std::fs::remove_dir_all(&log_dir).ok();
}

#[test]
fn crash_dump_falls_back_to_os_temp_directory() {
    let _lock = test_lock();
    reset_capabilities_cache();
    let crash_dir = unique_temp_dir("crash-tmpdir");
    std::fs::create_dir_all(&crash_dir).expect("create crash dir");
    // Mirror the upstream withEnv: override every temp-dir variable.
    let previous = ["TMPDIR", "TEMP", "TMP"].map(|name| (name, std::env::var(name).ok()));
    for (name, _) in previous.iter() {
        std::env::set_var(name, crash_dir.to_str().expect("utf8 temp dir"));
    }
    let mut harness = make_harness(40, 10);
    let message = oracle_consts::CRASH_DUMP_TMPDIR_MESSAGE
        .replace("<CRASH_DIR>", &crash_dir.to_string_lossy());
    let text =
        oracle_consts::CRASH_DUMP_TMPDIR_TEXT.replace("<CRASH_DIR>", &crash_dir.to_string_lossy());
    assert_crash_behavior(&mut harness, &message, &text);
    for (name, value) in previous.iter() {
        match value {
            Some(value) => std::env::set_var(name, value),
            None => std::env::remove_var(name),
        }
    }
    std::fs::remove_dir_all(&crash_dir).ok();
}

// tui-render.test.ts "TUI debug logging" + the PI_TUI_DEBUG dump branch.

#[test]
fn redraw_logs_written_to_log_directory() {
    let _lock = test_lock();
    reset_capabilities_cache();
    std::env::set_var("PI_TUI_DEBUG_REDRAW", "1");
    let log_dir = unique_temp_dir("redraw-log");
    std::fs::create_dir_all(&log_dir).expect("create log dir");
    let mut harness = make_harness_with(40, 10, None, Some(log_dir.to_string_lossy().into_owned()));
    let (component, shared) = lines_handle(&["Line 0", "Line 1", "Line 2"]);
    harness.tui.add_child(component);
    harness.tui.start();
    settle(&mut harness);
    harness.terminal.borrow_mut().resize(60, 10);
    settle(&mut harness);
    harness.tui.set_clear_on_shrink(true);
    shared.borrow_mut().set_lines(vec!["Line 0".to_string()]);
    harness.tui.request_render(false);
    settle(&mut harness);
    let log_text =
        std::fs::read_to_string(log_dir.join("pi-tui-debug.log")).expect("debug log exists");
    assert_eq!(
        mask_timestamps(&log_text),
        oracle_consts::DEBUG_REDRAW_LOG_TEXT,
        "redraw log must match the oracle (masked timestamps)"
    );
    std::env::remove_var("PI_TUI_DEBUG_REDRAW");
    std::fs::remove_dir_all(&log_dir).ok();
}

#[test]
fn pi_tui_debug_writes_render_dump() {
    let _lock = test_lock();
    reset_capabilities_cache();
    let debug_dir = std::path::Path::new("/tmp/tui");
    std::fs::create_dir_all(debug_dir).expect("create /tmp/tui");
    let before: Vec<std::path::PathBuf> = std::fs::read_dir(debug_dir)
        .expect("list /tmp/tui")
        .map(|entry| entry.expect("dir entry").path())
        .collect();
    std::env::set_var("PI_TUI_DEBUG", "1");
    let mut harness = make_harness(40, 10);
    let (component, shared) = lines_handle(&["Header", "Working |", "Footer"]);
    harness.tui.add_child(component);
    harness.tui.start();
    settle(&mut harness);
    clear_writes(&mut harness);
    shared.borrow_mut().set_lines(vec![
        "Header".to_string(),
        "Working /".to_string(),
        "Footer".to_string(),
    ]);
    harness.tui.request_render(false);
    settle(&mut harness);
    std::env::remove_var("PI_TUI_DEBUG");

    let mut fresh = std::fs::read_dir(debug_dir)
        .expect("list /tmp/tui")
        .map(|entry| entry.expect("dir entry").path())
        .filter(|path| !before.contains(path))
        .collect::<Vec<_>>();
    fresh.sort();
    assert_eq!(fresh.len(), 1, "exactly one fresh debug dump expected");
    let content = std::fs::read_to_string(&fresh[0]).expect("debug dump readable");
    assert_eq!(
        content,
        oracle_consts::PI_TUI_DEBUG_DUMP_CONTENT,
        "PI_TUI_DEBUG dump must match the oracle byte-for-byte"
    );
    std::fs::remove_file(&fresh[0]).ok();
}

// captureRenderState / restoreRenderState (public renderer state seam).

#[test]
fn restore_render_state_replays_from_restored_lines() {
    let _lock = test_lock();
    reset_capabilities_cache();
    let mut harness = make_harness(40, 10);
    let (component, shared) = lines_handle(&["alpha", "beta"]);
    harness.tui.add_child(component);
    harness.tui.start();
    settle(&mut harness);
    let state = harness.screen.borrow().capture_render_state();
    shared
        .borrow_mut()
        .set_lines(vec!["alpha".to_string(), "gamma".to_string()]);
    harness.tui.request_render(false);
    settle(&mut harness);
    clear_writes(&mut harness);
    harness.screen.borrow_mut().restore_render_state(state);
    shared
        .borrow_mut()
        .set_lines(vec!["alpha".to_string(), "beta".to_string()]);
    harness.tui.request_render(false);
    settle(&mut harness);
    // Restored state matches the current content: nothing to redraw.
    assert_oracle_writes(&harness, oracle_consts::RESTORE_RENDER_STATE_WRITES);
}

#[test]
fn restore_render_state_drops_image_lines_and_ids() {
    let _lock = test_lock();
    reset_capabilities_cache();
    let kitty2 = "\x1b_Ga=T,f=100,q=2,C=1,c=2,r=2,i=42;AAAA\x1b\\";
    let mut harness = make_harness(40, 10);
    let (component, shared) = lines_handle(&[]);
    harness.tui.add_child(component);
    shared
        .borrow_mut()
        .set_lines(vec!["keep".to_string(), kitty2.to_string(), String::new()]);
    harness.tui.start();
    settle(&mut harness);
    let state = harness.screen.borrow().capture_render_state();
    shared.borrow_mut().set_lines(vec![
        "keep".to_string(),
        "changed".to_string(),
        String::new(),
    ]);
    harness.tui.request_render(false);
    settle(&mut harness);
    clear_writes(&mut harness);
    harness.screen.borrow_mut().restore_render_state(state);
    // Rendering the same content again rewrites the (restored-to-empty) image
    // line: the image ids were reset with the state.
    shared.borrow_mut().set_lines(vec![
        "keep".to_string(),
        "changed".to_string(),
        String::new(),
    ]);
    harness.tui.request_render(false);
    settle(&mut harness);
    assert_oracle_writes(&harness, oracle_consts::RESTORE_RENDER_STATE_IMAGES_WRITES);
}

#[test]
fn render_state_shape_matches_oracle() {
    let _lock = test_lock();
    reset_capabilities_cache();
    let mut harness = make_harness(40, 10);
    let (component, _shared) = lines_handle(&["a", "b"]);
    harness.tui.add_child(component);
    harness.tui.start();
    settle(&mut harness);
    let state = harness.screen.borrow().capture_render_state();
    assert_oracle_json(
        json!({
            "previousWidth": state.previous_width,
            "previousHeight": state.previous_height,
            "cursorRow": state.cursor_row,
            "hardwareCursorRow": state.hardware_cursor_row,
            "maxLinesRendered": state.max_lines_rendered,
            "previousViewportTop": state.previous_viewport_top,
            "previousLinesLength": state.previous_lines.len(),
            "mode": "regular",
        }),
        oracle_consts::RENDER_STATE_SHAPE,
    );
    assert_eq!(TuiMainScreen::MODE, TuiMode::Regular);
}

// ------------------------------------------------- direct seam unit tests

#[test]
fn parse_kitty_image_header_extracts_ids_rows_and_skips_invalid() {
    // Decimal ids, row counts, missing '=', non-numeric and out-of-range
    // values are exercised through the public behavior (oracle scenarios);
    // here: the parser corners, including JS Number literal forms.
    assert_eq!(
        super::parse_kitty_image_header("\x1b_Ga=T,f=100,q=2,C=1,c=2,r=2,i=42;AAAA\x1b\\")
            .map(|header| (header.ids, header.rows)),
        Some((vec![42], 2))
    );
    assert_eq!(
        super::parse_kitty_image_header("\x1b_Gi=0x10;\x1b\\").map(|header| header.ids),
        Some(vec![16]),
        "Number() accepts hex literals"
    );
    assert_eq!(
        super::parse_kitty_image_header("\x1b_Gi=4294967296;\x1b\\").map(|header| header.ids),
        Some(vec![]),
        "ids above 0xffffffff are rejected"
    );
    assert_eq!(
        super::parse_kitty_image_header("\x1b_Gi=-5,r=0,i=nope;\x1b\\")
            .map(|header| (header.ids, header.rows)),
        Some((vec![], 1)),
        "negative/non-positive/non-numeric values are skipped"
    );
    assert_eq!(
        super::parse_kitty_image_header("\x1b_Ga=T,f=100;payload\x1b\\").map(|header| header.ids),
        Some(vec![]),
        "params terminated at the first ';'"
    );
    assert_eq!(super::parse_kitty_image_header("plain line"), None);
    assert_eq!(
        super::parse_kitty_image_header("\x1b_Ga=T,i=5=6,junk,r=3;\x1b\\")
            .map(|header| (header.ids, header.rows)),
        Some((vec![5], 3)),
        "split('=', 2) truncates after the first '=' and missing '=' params are skipped"
    );
}

#[test]
fn js_number_matches_the_literal_forms_number_accepts() {
    assert_eq!(super::js_number(""), 0.0);
    assert_eq!(super::js_number(" 42 "), 42.0);
    assert_eq!(super::js_number("0x10"), 16.0);
    assert_eq!(super::js_number("0X10"), 16.0);
    assert!(
        super::js_number("+0x10").is_nan(),
        "signs invalidate radix literals"
    );
    assert!(super::js_number("-0x10").is_nan());
    assert_eq!(super::js_number("0b101"), 5.0);
    assert_eq!(super::js_number("0o17"), 15.0);
    assert!(super::js_number("Infinity").is_infinite());
    assert!(super::js_number("-Infinity").is_infinite());
    assert!(super::js_number("nope").is_nan());
    assert_eq!(super::js_number("2.5"), 2.5);
}

#[test]
fn bounded_writer_never_splits_surrogate_pairs() {
    let _lock = test_lock();
    reset_capabilities_cache();
    let mut harness = make_harness(40, 10);
    let mut writer = super::BoundedTerminalWriter::default();
    // With a 2-unit limit, the pair boundary lands exactly on the chunk edge
    // (upstream would loop forever from an empty buffer at limit 1; limit 2
    // exercises the split guard).
    writer.append_with_limit("a\u{1F600}b\u{1F601}", 2, &mut harness.tui);
    writer.flush(&mut harness.tui);
    let joined = joined_writes(&harness);
    assert_eq!(
        joined, "a\u{1F600}b\u{1F601}",
        "content is preserved across chunks"
    );
    // Each flushed chunk is valid UTF-16 (no lone surrogates).
    for write in harness.terminal.borrow().writes.iter() {
        assert_eq!(
            write.as_str(),
            String::from_utf16_lossy(&write.encode_utf16().collect::<Vec<_>>())
        );
    }
}

#[test]
fn bounded_writer_flushes_at_exact_unit_boundaries() {
    let _lock = test_lock();
    reset_capabilities_cache();
    let mut harness = make_harness(40, 10);
    let mut writer = super::BoundedTerminalWriter::default();
    writer.append_with_limit("abcdef", 4, &mut harness.tui);
    assert_eq!(writer.len(), 6, "length counts written and buffered units");
    // "abcdef" hits the 4-unit limit mid-append: "abcd" flushed, "ef" pending.
    assert_eq!(
        harness.terminal.borrow().writes.clone(),
        vec!["abcd".to_string()],
        "flush fires at the exact unit limit"
    );
    writer.append_with_limit("gh", 4, &mut harness.tui);
    // "efgh" fills the pending chunk to 4 units -> flushed.
    assert_eq!(
        harness.terminal.borrow().writes.clone(),
        vec!["abcd".to_string(), "efgh".to_string()]
    );
    writer.flush(&mut harness.tui);
    assert_eq!(
        harness.terminal.borrow().writes.len(),
        2,
        "final flush writes nothing when empty"
    );
}
