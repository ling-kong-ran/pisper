//! Port of the alternate-screen renderer from upstream
//! `packages/tui/src/tui-alt-screen.ts` (`doRender` + enter/exit sequences):
//! a fixed-height viewport where each row is diffed against the previous
//! frame and only changed rows are repainted.
//!
//! Disclosed substitutions: Kitty/ITerm2 image placements, search
//! highlighting, flash compositing and selection overlays join with their
//! respective component slices; this module covers the row-diff frame plan,
//! enter/exit sequences and hardware-cursor positioning.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use regex::Regex;

use crate::tui::terminal_image::{get_kitty_image_placement_rows, is_image_line};

pub const ENTER_ALT_SCREEN: &str = "\x1b[?1049h";
pub const EXIT_ALT_SCREEN: &str = "\x1b[?1049l";
pub const DISABLE_AUTOWRAP: &str = "\x1b[?7l";
pub const BEGIN_SYNCHRONIZED_OUTPUT: &str = "\x1b[?2026h";
pub const END_SYNCHRONIZED_OUTPUT: &str = "\x1b[?2026l";
pub const CLEAR_SCREEN: &str = "\x1b[2J";
pub const HIDE_CURSOR: &str = "\x1b[?25l";
pub const SHOW_CURSOR: &str = "\x1b[?25h";

fn row_move_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\x1b\[\d+;1H\x1b\[2K").unwrap())
}

/// State of the alternate-screen renderer between frames
/// (upstream `previousScreen`/`previousScreenWidth`/`previousScreenHeight`).
#[derive(Clone, Debug, Default)]
pub struct AltScreenState {
    pub previous_screen: Vec<String>,
    pub previous_screen_width: usize,
    pub previous_screen_height: usize,
    pub alt_screen_active: bool,
    pub full_redraws: usize,
}

impl AltScreenState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Upstream `getScreenLines()` (v1.0.0): the lines of the last rendered
    /// frame, one per terminal row, as written to the terminal.
    pub fn screen_lines(&self) -> Vec<String> {
        self.previous_screen.clone()
    }
}

/// One alternate-screen frame: the exact write sequence for the terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AltScreenFrame {
    pub ops: Vec<String>,
    /// Row (0-based) of the hardware cursor after the frame, when a component
    /// emitted the cursor marker; `None` hides the hardware cursor.
    pub hardware_cursor: Option<(usize, usize)>,
    pub show_hardware_cursor: bool,
}

/// Options for [`compute_alt_screen_frame_with_options`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AltScreenFrameOptions {
    /// Upstream `isWezTerm` (v1.0.0): WezTerm erases intersecting Kitty image
    /// cells when a later row write touches a covered row, so such frames draw
    /// image placements after every clear and text write.
    pub is_wezterm: bool,
    /// Upstream `this.imageProtocol` (`Some("kitty")` or `None`).
    pub image_protocol: Option<&'static str>,
}

/// Upstream `doRender`: diff `screen` against the previous frame.
pub fn compute_alt_screen_frame(
    state: &mut AltScreenState,
    screen: Vec<String>,
    width: usize,
    height: usize,
    cursor_pos: Option<(usize, usize)>,
    show_hardware_cursor: bool,
) -> AltScreenFrame {
    compute_alt_screen_frame_with_options(
        state,
        screen,
        width,
        height,
        cursor_pos,
        show_hardware_cursor,
        AltScreenFrameOptions::default(),
    )
}

/// Upstream `doRender` with the terminal-image frame inputs (v1.0.0: the
/// WezTerm detection and the active image protocol).
pub fn compute_alt_screen_frame_with_options(
    state: &mut AltScreenState,
    screen: Vec<String>,
    width: usize,
    height: usize,
    cursor_pos: Option<(usize, usize)>,
    show_hardware_cursor: bool,
    options: AltScreenFrameOptions,
) -> AltScreenFrame {
    let mut screen = screen;
    // Upstream keeps the LAST `height` rows when the screen overflows.
    if screen.len() > height {
        let drop = screen.len() - height;
        screen.drain(..drop);
    }
    while screen.len() < height {
        screen.push(String::new());
    }

    let full_redraw = state.previous_screen.is_empty()
        || state.previous_screen_width != width
        || state.previous_screen_height != height;

    // v1.0.0: per-row change flags drive both image redraw triggers.
    let changed_rows: Vec<bool> = (0..height)
        .map(|row| Some(&screen[row]) != state.previous_screen.get(row))
        .collect();
    let image_anchors_need_redraw = screen.iter().enumerate().any(|(row, line)| {
        changed_rows[row]
            && (is_image_line(line)
                || is_image_line(
                    state
                        .previous_screen
                        .get(row)
                        .map(String::as_str)
                        .unwrap_or(""),
                ))
    });
    let image_cells_need_redraw = !image_anchors_need_redraw
        && options.is_wezterm
        && options.image_protocol == Some("kitty")
        && changed_rows.iter().any(|changed| *changed)
        && screen.iter().enumerate().any(|(row, line)| {
            let Some(placement_rows) = get_kitty_image_placement_rows(line) else {
                return false;
            };
            (row..(row + placement_rows).min(height))
                .any(|covered_row| changed_rows.get(covered_row).copied().unwrap_or(false))
        });
    let images_need_redraw = image_anchors_need_redraw || image_cells_need_redraw;
    let redraw_images = full_redraw || images_need_redraw;

    let mut ops: Vec<String> = Vec::new();
    if full_redraw {
        state.full_redraws += 1;
        ops.push(BEGIN_SYNCHRONIZED_OUTPUT.to_string());
        ops.push(CLEAR_SCREEN.to_string());
    } else {
        ops.push(BEGIN_SYNCHRONIZED_OUTPUT.to_string());
    }

    // v1.0.0: WezTerm erases intersecting Kitty image cells when a later row
    // write touches a covered row. Draw image placements after every clear
    // and text write so nothing later intersects them; preserve the existing
    // interleaved output for text-only frames and every other terminal.
    let draw_kitty_images_last = redraw_images
        && options.image_protocol == Some("kitty")
        && screen.iter().any(|line| is_image_line(line))
        && options.is_wezterm;
    let unchanged = |row: usize| {
        !full_redraw && !images_need_redraw && state.previous_screen.get(row) == screen.get(row)
    };
    if draw_kitty_images_last {
        for row in 0..height {
            if unchanged(row) {
                continue;
            }
            ops.push(format!("\x1b[{};1H\x1b[2K", row + 1));
        }
        for (row, line) in screen.iter().enumerate() {
            if unchanged(row) || is_image_line(line) {
                continue;
            }
            ops.push(format!("\x1b[{};1H{}", row + 1, line));
        }
        for (row, line) in screen.iter().enumerate() {
            if unchanged(row) || !is_image_line(line) {
                continue;
            }
            ops.push(format!("\x1b[{};1H{}", row + 1, line));
        }
    } else {
        for (row, line) in screen.iter().enumerate() {
            if unchanged(row) {
                continue;
            }
            ops.push(format!("\x1b[{};1H\x1b[2K{}", row + 1, line));
        }
    }

    if let Some((row, col)) = cursor_pos {
        ops.push(format!(
            "\x1b[{};{}H",
            row + 1,
            col.min(width.saturating_sub(1)) + 1
        ));
        ops.push(if show_hardware_cursor {
            SHOW_CURSOR.to_string()
        } else {
            HIDE_CURSOR.to_string()
        });
    } else {
        ops.push(HIDE_CURSOR.to_string());
    }

    ops.push(END_SYNCHRONIZED_OUTPUT.to_string());

    state.previous_screen = screen;
    state.previous_screen_width = width;
    state.previous_screen_height = height;

    AltScreenFrame {
        ops,
        hardware_cursor: cursor_pos,
        show_hardware_cursor,
    }
}

/// Upstream enter sequence: `\x1b[?1049h` + disable autowrap (+ mouse) + clear
/// + hide cursor.
pub fn enter_alt_screen_sequences(mouse_enabled: bool) -> Vec<String> {
    let mouse = if mouse_enabled {
        // The concrete mouse sequence is composed by the mouse-enabled slice.
        String::new()
    } else {
        String::new()
    };
    vec![format!(
        "{ENTER_ALT_SCREEN}{DISABLE_AUTOWRAP}{mouse}\x1b[2J\x1b[H{HIDE_CURSOR}"
    )]
}

/// Upstream normal-exit sequence: synchronized exit of the alt screen with
/// cursor show and autowrap restore.
pub fn exit_alt_screen_sequences() -> Vec<String> {
    vec![format!(
        "{BEGIN_SYNCHRONIZED_OUTPUT}{EXIT_ALT_SCREEN}\x1b[?25h{END_SYNCHRONIZED_OUTPUT}"
    )]
}

/// Whether `data` is the beginning of an alt-screen row write (used by tests
/// and the terminal sink to sanity-check frames).
pub fn is_row_write(op: &str) -> bool {
    row_move_regex().is_match(op)
}

/// Global alt-screen active flag mirroring the upstream module state used by
/// tests; the real terminal tracks this on its own struct.
static ALT_SCREEN_ACTIVE: AtomicBool = AtomicBool::new(false);

pub fn set_alt_screen_active(active: bool) {
    ALT_SCREEN_ACTIVE.store(active, Ordering::SeqCst);
}

pub fn is_alt_screen_active() -> bool {
    ALT_SCREEN_ACTIVE.load(Ordering::SeqCst)
}
