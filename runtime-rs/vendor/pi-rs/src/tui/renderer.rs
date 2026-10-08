//! Port of the differential rendering core from upstream
//! `packages/tui/src/tui-main-screen.ts` (`doRender`) plus the input
//! dispatch/focus plumbing of `TuiBase` (`tui.ts`).
//!
//! The upstream renderer talks to a real terminal through `terminal.write`.
//! The port models the same decisions as an explicit *write plan* consumed by
//! a `write` sink, so the differential behavior is testable without a tty:
//!
//! - first render emits every line without clearing;
//! - width/height changes and clear-on-shrink trigger a full synchronized
//!   clear + re-render;
//! - otherwise only the first..last changed line range is repainted, and pure
//!   appends are emitted as `\r\n` + line writes after the previous content.
//!
//! Disclosed substitutions: Kitty image placeholder reservations, hardware
//! cursor positioning and the debug-redraw log are renderer/OS integration
//! concerns that join the ProcessTerminal slice; the plan reports line counts
//! and the synchronized-output markers so tests can pin them.

/// Upstream `MIN_RENDER_INTERVAL_MS` (16ms throttle between frames).
pub const MIN_RENDER_INTERVAL_MS: u64 = 16;

/// Synchronized-output begin marker (upstream `\x1b[?2026h`).
pub const SYNC_BEGIN: &str = "\x1b[?2026h";
/// Synchronized-output end marker (upstream `\x1b[?2026l`).
pub const SYNC_END: &str = "\x1b[?2026l";

/// One rendered frame decision (upstream `doRender` branching).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameKind {
    /// Clean screen: write all lines without clearing.
    FirstRender,
    /// Full clear + re-render (size change / clear-on-shrink).
    FullRender { clear_screen: bool },
    /// Repaint only the changed line range.
    Partial {
        first_changed: usize,
        last_changed: usize,
    },
    /// Nothing to paint.
    NoChange,
}

/// The write operations for one frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriteOp {
    BeginSync,
    EndSync,
    ClearScreen,
    WriteLine(String),
    /// Move the cursor down (positive) or up (negative) before the next line.
    MoveBy(i64),
    CarriageReturn,
}

/// State carried between frames (upstream `previousLines`/`previousWidth`/...).
#[derive(Clone, Debug, Default)]
pub struct RenderState {
    pub previous_lines: Vec<String>,
    pub previous_width: usize,
    pub previous_height: usize,
    pub max_lines_rendered: usize,
    pub full_redraws: usize,
}

impl RenderState {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Configuration for the differential renderer.
#[derive(Clone, Copy, Debug, Default)]
pub struct RenderOptions {
    /// Upstream `setClearOnShrink`.
    pub clear_on_shrink: bool,
}

/// The computed frame: kind plus the exact write operations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub kind: FrameKind,
    pub ops: Vec<WriteOp>,
}

/// Compute the frame for `new_lines` given the previous state and terminal
/// dimensions (upstream `doRender` decision tree + changed-range repaint).
pub fn compute_frame(
    state: &mut RenderState,
    options: &RenderOptions,
    new_lines: Vec<String>,
    width: usize,
    height: usize,
) -> Frame {
    let width_changed = state.previous_width != 0 && state.previous_width != width;
    let height_changed = state.previous_height != 0 && state.previous_height != height;

    // First render - output everything without clearing.
    if state.previous_lines.is_empty() && !width_changed && !height_changed {
        return frame_full(
            state,
            new_lines,
            width,
            height,
            FrameKind::FirstRender,
            false,
        );
    }

    // Width changes always need a full re-render because wrapping changes.
    if width_changed {
        return frame_full(
            state,
            new_lines,
            width,
            height,
            FrameKind::FullRender { clear_screen: true },
            true,
        );
    }

    // Height changes normally need a full re-render.
    if height_changed {
        return frame_full(
            state,
            new_lines,
            width,
            height,
            FrameKind::FullRender { clear_screen: true },
            true,
        );
    }

    // Content shrunk below the working area.
    if options.clear_on_shrink && new_lines.len() < state.max_lines_rendered {
        return frame_full(
            state,
            new_lines,
            width,
            height,
            FrameKind::FullRender { clear_screen: true },
            true,
        );
    }

    // Find first and last changed lines.
    let mut first_changed: i64 = -1;
    let mut last_changed: i64 = -1;
    let max_lines = new_lines.len().max(state.previous_lines.len());
    for i in 0..max_lines {
        let old_line = state
            .previous_lines
            .get(i)
            .map(String::as_str)
            .unwrap_or("");
        let new_line = new_lines.get(i).map(String::as_str).unwrap_or("");
        if old_line != new_line {
            if first_changed == -1 {
                first_changed = i as i64;
            }
            last_changed = i as i64;
        }
    }

    let appended = new_lines.len() > state.previous_lines.len();
    if appended {
        if first_changed == -1 {
            first_changed = state.previous_lines.len() as i64;
        }
        last_changed = new_lines.len() as i64 - 1;
    }

    // No changes.
    if first_changed == -1 {
        return Frame {
            kind: FrameKind::NoChange,
            ops: Vec::new(),
        };
    }

    // All changes are in deleted lines: nothing to paint, just synchronize.
    if first_changed as usize >= new_lines.len() {
        return Frame {
            kind: FrameKind::NoChange,
            ops: vec![WriteOp::BeginSync, WriteOp::EndSync],
        };
    }

    let first = first_changed as usize;
    let last = last_changed as usize;
    let mut ops = vec![WriteOp::BeginSync];
    for (i, line) in new_lines
        .iter()
        .enumerate()
        .skip(first)
        .take(last + 1 - first)
    {
        if i > first {
            ops.push(WriteOp::MoveBy(1));
        }
        ops.push(WriteOp::CarriageReturn);
        ops.push(WriteOp::WriteLine(line.clone()));
    }
    ops.push(WriteOp::EndSync);

    state.previous_lines = new_lines.clone();
    state.max_lines_rendered = state.max_lines_rendered.max(new_lines.len());

    Frame {
        kind: FrameKind::Partial {
            first_changed: first,
            last_changed: last,
        },
        ops,
    }
}

fn frame_full(
    state: &mut RenderState,
    new_lines: Vec<String>,
    width: usize,
    height: usize,
    kind: FrameKind,
    clear_screen: bool,
) -> Frame {
    state.full_redraws += 1;
    let mut ops = vec![WriteOp::BeginSync];
    if clear_screen {
        ops.push(WriteOp::ClearScreen);
    }
    for (i, line) in new_lines.iter().enumerate() {
        if i > 0 {
            ops.push(WriteOp::MoveBy(1));
        }
        ops.push(WriteOp::WriteLine(line.clone()));
    }
    ops.push(WriteOp::EndSync);
    state.previous_lines = new_lines;
    state.max_lines_rendered = state
        .max_lines_rendered
        .max(new_lines_len(&state.previous_lines));
    state.previous_width = width;
    state.previous_height = height;
    Frame { kind, ops }
}

fn new_lines_len(lines: &[String]) -> usize {
    lines.len()
}

/// Reset tracking (upstream `resetRenderState`).
pub fn reset_render_state(state: &mut RenderState) {
    state.previous_lines.clear();
    state.previous_width = 0;
    state.previous_height = 0;
    state.max_lines_rendered = 0;
}
