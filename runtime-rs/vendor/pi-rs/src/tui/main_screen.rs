//! Port of upstream `packages/tui/src/tui-main-screen.ts` (655 lines): the
//! `TuiMainScreen` renderer — differential main-screen rendering with
//! scrollback, line composition and hardware-cursor positioning.
//!
//! This is the concrete [`TuiRenderer`] (upstream abstract `doRender` plus the
//! `resetRenderState`/`beforeTerminalStop` overrides) installed into
//! [`Tui`](crate::tui::tui::Tui); all composition helpers (`render`,
//! `compositeOverlays`, `extractCursorPosition`, `applyLineResets`) live on
//! `Tui` per tui.ts and are reused from here.
//!
//! Disclosed substitutions for review:
//! - JS string indexing is UTF-16 code-unit based, and
//!   `MAX_RENDER_WRITE_CHARS` bounds *code units*; [`BoundedTerminalWriter`]
//!   therefore buffers/splits on UTF-16 units (the write-boundary seam stays
//!   byte-identical, including the surrogate-pair split guard).
//! - JS `Set<number>` iteration order is observable through
//!   `deleteKittyImages`' concatenation, so membership uses the
//!   insertion-ordered [`OrderedIdSet`] instead of `HashSet`.
//! - `param.split("=", 2)` and `Number(...)` parsing in
//!   `parseKittyImageHeader` are reproduced (including hex/octal/binary
//!   literals accepted by `Number`), see [`js_number`].
//! - `process.env` reads (`TERMUX_VERSION`, `PI_TUI_DEBUG_REDRAW`,
//!   `PI_TUI_DEBUG`) happen per `doRender` exactly like upstream.
//! - The width-overflow crash path calls `this.stop()` upstream, which
//!   re-enters the subclass hooks; the Rust scheduler holds the renderer out
//!   of the `Tui` for the whole `doRender` call, so `Tui::stop` cannot be
//!   re-entered there. The same teardown transition is applied inline
//!   (renderer `beforeTerminalStop` hook + terminal showCursor/stop) and the
//!   error surfaces as a panic carrying the upstream message. Only
//!   `Tui::stop`'s `stopped` flag / timer cancel are skipped; the render
//!   already failed either way. The scheme-notification teardown write cannot
//!   be observed from the renderer seam (private Tui state) and is skipped.
//! - `Math.random().toString(36).slice(2)` in the `PI_TUI_DEBUG` dump filename
//!   becomes a random base36 `u64` (same role: unique debug filename).

use std::collections::HashSet;

use crate::tui::terminal_image::is_image_line;
use crate::tui::tui::{Tui, TuiMode, TuiRenderer, TuiStopOptions};
use crate::tui::utils::visible_width;

/// Upstream `KITTY_SEQUENCE_PREFIX`.
const KITTY_SEQUENCE_PREFIX: &str = "\x1b_G";

/// Upstream `MAX_RENDER_WRITE_CHARS`: 1 MiB of UTF-16 code units.
const MAX_RENDER_WRITE_CHARS: usize = 1024 * 1024;

/// Upstream `deleteKittyImage` (terminal-image.ts): delete a Kitty graphics
/// image by id, freeing the image data.
fn delete_kitty_image(image_id: u64) -> String {
    format!("\x1b_Ga=d,d=I,i={image_id},q=2\x1b\\")
}

/// `Number(value)` for the string forms the Kitty header parser can observe.
/// Handles JS-only literal forms (hex/octal/binary, `Infinity`, empty string)
/// that Rust's `f64::from_str` rejects or accepts differently.
fn js_number(text: &str) -> f64 {
    // StringToNumber first strips leading/trailing JS whitespace.
    let trimmed = text.trim_matches(|c: char| c.is_whitespace());
    if trimmed.is_empty() {
        return 0.0;
    }
    // A sign is only valid in front of a decimal literal or `Infinity`
    // (`Number("+0x10")` is NaN, `Number("0x10")` is 16).
    let (sign, rest, signed) = match trimmed.as_bytes()[0] {
        b'+' => (1.0, &trimmed[1..], true),
        b'-' => (-1.0, &trimmed[1..], true),
        _ => (1.0, trimmed, false),
    };
    if signed || rest.is_empty() {
        if rest.eq_ignore_ascii_case("infinity") {
            return f64::INFINITY * sign;
        }
        return rest.parse::<f64>().unwrap_or(f64::NAN) * sign;
    }
    let lower = rest.to_ascii_lowercase();
    if let Some(hex) = lower.strip_prefix("0x") {
        return u64::from_str_radix(hex, 16)
            .map(|v| v as f64)
            .unwrap_or(f64::NAN);
    }
    if let Some(oct) = lower.strip_prefix("0o") {
        return u64::from_str_radix(oct, 8)
            .map(|v| v as f64)
            .unwrap_or(f64::NAN);
    }
    if let Some(bin) = lower.strip_prefix("0b") {
        return u64::from_str_radix(bin, 2)
            .map(|v| v as f64)
            .unwrap_or(f64::NAN);
    }
    rest.parse::<f64>().unwrap_or(f64::NAN)
}

/// JS `Number.isInteger`.
fn js_is_integer(value: f64) -> bool {
    value.is_finite() && value.fract() == 0.0
}

/// Insertion-ordered membership set (JS `Set<number>`): iteration order is
/// observable through the concatenated delete sequences.
#[derive(Default)]
struct OrderedIdSet {
    items: Vec<u64>,
    seen: HashSet<u64>,
}

impl OrderedIdSet {
    fn insert(&mut self, id: u64) {
        if self.seen.insert(id) {
            self.items.push(id);
        }
    }

    fn iter(&self) -> impl Iterator<Item = u64> + '_ {
        self.items.iter().copied()
    }
}

/// Upstream `KittyImageHeader`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct KittyImageHeader {
    ids: Vec<u64>,
    rows: u64,
}

/// Upstream `parseKittyImageHeader`.
fn parse_kitty_image_header(line: &str) -> Option<KittyImageHeader> {
    let sequence_start = line.find(KITTY_SEQUENCE_PREFIX)?;
    let params_start = sequence_start + KITTY_SEQUENCE_PREFIX.len();
    let params_end = params_start + line[params_start..].find(';')?;
    let mut header = KittyImageHeader {
        ids: Vec::new(),
        rows: 1,
    };
    for param in line[params_start..params_end].split(',') {
        // JS `param.split("=", 2)`: the first '=' separates; any further '='
        // truncates the remainder of the value. A param without '=' is
        // skipped, not fatal.
        let mut parts = param.split('=');
        let key = parts.next().unwrap_or("");
        let Some(value) = parts.next() else {
            continue;
        };
        let number = js_number(value);
        if !js_is_integer(number) || number <= 0.0 || number > 4294967295.0 {
            continue;
        }
        match key {
            "i" => header.ids.push(number as u64),
            "r" => header.rows = number as u64,
            _ => {}
        }
    }
    Some(header)
}

/// Upstream `extractKittyImageIds`.
fn extract_kitty_image_ids(line: &str) -> Vec<u64> {
    parse_kitty_image_header(line)
        .map(|header| header.ids)
        .unwrap_or_default()
}

/// Upstream `extractKittyImageRows`.
fn extract_kitty_image_rows(line: &str) -> u64 {
    parse_kitty_image_header(line)
        .map(|header| header.rows)
        .unwrap_or(1)
}

/// Upstream `isTermuxSession`.
fn is_termux_session() -> bool {
    std::env::var("TERMUX_VERSION").is_ok_and(|value| !value.is_empty())
}

/// `new Date().toISOString()` (UTC, millisecond precision) without a date
/// dependency.
fn iso_timestamp() -> String {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let total_secs = elapsed.as_secs();
    let (year, month, day) = civil_from_days((total_secs / 86400) as i64);
    let secs_of_day = total_secs % 86400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60,
        elapsed.subsec_millis(),
    )
}

/// Howard Hinnant's `civil_from_days`: days since 1970-01-01 to (y, m, d).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Streams terminal output in 1 MiB chunks so a full render never forms one
/// string large enough to exceed V8's limit.
///
/// `append()` fills the current chunk and flushes it when full. Oversized
/// input is split at chunk boundaries, preserving surrogate pairs so each
/// write remains valid UTF-16. Callers append synchronized-output begin/end
/// sequences themselves; the final `flush()` writes any remainder, including
/// the end sequence.
#[derive(Default)]
struct BoundedTerminalWriter {
    /// Pending chunk as UTF-16 code units (JS string indexing semantics).
    buffer: Vec<u16>,
    /// Code units already written (for the debug `length` getter).
    written_chars: usize,
}

impl BoundedTerminalWriter {
    /// Upstream `length` getter: written units plus pending units.
    fn len(&self) -> usize {
        self.written_chars + self.buffer.len()
    }

    /// Append terminal data, flushing full chunks as needed. Callers must
    /// call `flush()` after the final append. Oversized values are split
    /// without splitting surrogate pairs.
    fn append(&mut self, value: &str, tui: &mut Tui) {
        self.append_with_limit(value, MAX_RENDER_WRITE_CHARS, tui);
    }

    /// The chunking loop parameterized over the chunk limit (tests exercise
    /// the surrogate-pair guard with a small limit; with the production limit
    /// this is exactly upstream's `append`).
    fn append_with_limit(&mut self, value: &str, limit: usize, tui: &mut Tui) {
        let units: Vec<u16> = value.encode_utf16().collect();
        let mut offset = 0usize;
        while offset < units.len() {
            let capacity = limit - self.buffer.len();
            if capacity == 0 {
                self.flush(tui);
                continue;
            }

            let mut end = units.len().min(offset + capacity);
            if end < units.len()
                && (0xd800..=0xdbff).contains(&units[end - 1])
                && (0xdc00..=0xdfff).contains(&units[end])
            {
                end -= 1;
            }
            if end == offset {
                self.flush(tui);
                continue;
            }

            self.buffer.extend_from_slice(&units[offset..end]);
            offset = end;
            if self.buffer.len() == limit {
                self.flush(tui);
            }
        }
    }

    /// Write the current chunk, if any, and retain only its character count
    /// for debug output.
    fn flush(&mut self, tui: &mut Tui) {
        if self.buffer.is_empty() {
            return;
        }
        let chunk = String::from_utf16(&self.buffer).expect("chunks split on scalar boundaries");
        tui.terminal().borrow_mut().write(&chunk);
        self.written_chars += self.buffer.len();
        self.buffer.clear();
    }
}

/// Upstream `TuiMainScreenRenderState`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TuiMainScreenRenderState {
    pub previous_lines: Vec<String>,
    pub previous_width: i64,
    pub previous_height: i64,
    pub cursor_row: i64,
    pub hardware_cursor_row: i64,
    pub max_lines_rendered: usize,
    pub previous_viewport_top: i64,
}

/// Upstream `TuiMainScreen`: TUI implementation that renders into the
/// terminal's main screen and scrollback.
#[derive(Default)]
pub struct TuiMainScreen {
    previous_lines: Vec<String>,
    previous_kitty_image_ids: OrderedIdSet,
    previous_width: i64,
    previous_height: i64,
    cursor_row: i64,
    hardware_cursor_row: i64,
    max_lines_rendered: usize,
    previous_viewport_top: i64,
}

impl TuiMainScreen {
    /// Upstream `readonly mode = "regular"`.
    pub const MODE: TuiMode = TuiMode::Regular;

    /// Upstream constructor (all render state starts at its zero value).
    pub fn new() -> Self {
        Self::default()
    }

    /// Upstream `captureRenderState`.
    pub fn capture_render_state(&self) -> TuiMainScreenRenderState {
        TuiMainScreenRenderState {
            previous_lines: self.previous_lines.clone(),
            previous_width: self.previous_width,
            previous_height: self.previous_height,
            cursor_row: self.cursor_row,
            hardware_cursor_row: self.hardware_cursor_row,
            max_lines_rendered: self.max_lines_rendered,
            previous_viewport_top: self.previous_viewport_top,
        }
    }

    /// Upstream `restoreRenderState`.
    pub fn restore_render_state(&mut self, state: TuiMainScreenRenderState) {
        self.previous_lines = state
            .previous_lines
            .into_iter()
            .map(|line| {
                if is_image_line(&line) {
                    String::new()
                } else {
                    line
                }
            })
            .collect();
        self.previous_kitty_image_ids = OrderedIdSet::default();
        self.previous_width = state.previous_width;
        self.previous_height = state.previous_height;
        self.cursor_row = state.cursor_row;
        self.hardware_cursor_row = state.hardware_cursor_row;
        self.max_lines_rendered = state.max_lines_rendered;
        self.previous_viewport_top = state.previous_viewport_top;
    }

    /// Upstream `collectKittyImageIds`.
    fn collect_kitty_image_ids(&self, lines: &[String]) -> OrderedIdSet {
        let mut ids = OrderedIdSet::default();
        for line in lines {
            for id in extract_kitty_image_ids(line) {
                ids.insert(id);
            }
        }
        ids
    }

    /// Upstream `deleteKittyImages`.
    fn delete_kitty_images(&self, ids: &OrderedIdSet) -> String {
        let mut buffer = String::new();
        for id in ids.iter() {
            buffer.push_str(&delete_kitty_image(id));
        }
        buffer
    }

    /// Upstream `getKittyImageReservedRows`.
    fn get_kitty_image_reserved_rows(&self, lines: &[String], index: i64, max_index: i64) -> i64 {
        let rows =
            extract_kitty_image_rows(lines.get(index as usize).map(String::as_str).unwrap_or(""))
                as i64;
        if rows <= 1 {
            return 1;
        }

        let max_rows = rows
            .min(max_index - index + 1)
            .min(lines.len() as i64 - index);
        let mut reserved_rows: i64 = 1;
        while reserved_rows < max_rows {
            let line = lines
                .get((index + reserved_rows) as usize)
                .map(String::as_str)
                .unwrap_or("");
            if is_image_line(line) || visible_width(line) > 0 {
                break;
            }
            reserved_rows += 1;
        }
        reserved_rows
    }

    /// Upstream `expandChangedRangeForKittyImages`.
    fn expand_changed_range_for_kitty_images(
        &self,
        first_changed: i64,
        last_changed: i64,
        new_lines: &[String],
    ) -> (i64, i64) {
        let mut expanded_first_changed = first_changed;
        let mut expanded_last_changed = last_changed;
        let mut expand_for_lines = |lines: &[String]| {
            for i in 0..lines.len() as i64 {
                if extract_kitty_image_ids(&lines[i as usize]).is_empty() {
                    continue;
                }
                let block_end =
                    i + self.get_kitty_image_reserved_rows(lines, i, lines.len() as i64 - 1) - 1;
                if i >= expanded_first_changed
                    || (i <= expanded_last_changed && block_end >= expanded_first_changed)
                {
                    expanded_first_changed = expanded_first_changed.min(i);
                    expanded_last_changed = expanded_last_changed.max(block_end);
                }
            }
        };

        expand_for_lines(&self.previous_lines);
        expand_for_lines(new_lines);
        (expanded_first_changed, expanded_last_changed)
    }

    /// Upstream `deleteChangedKittyImages`.
    fn delete_changed_kitty_images(&self, first_changed: i64, last_changed: i64) -> String {
        if first_changed < 0 || last_changed < first_changed {
            return String::new();
        }

        let mut ids = OrderedIdSet::default();
        let max_line = last_changed.min(self.previous_lines.len() as i64 - 1);
        for i in first_changed..=max_line {
            for id in extract_kitty_image_ids(
                self.previous_lines
                    .get(i as usize)
                    .map(String::as_str)
                    .unwrap_or(""),
            ) {
                ids.insert(id);
            }
        }

        self.delete_kitty_images(&ids)
    }

    /// Upstream `doRender`. One closure-free expansion of the upstream method:
    /// the inline `fullRender` closure becomes [`TuiMainScreen::full_render`],
    /// `computeLineDiff` is inlined at its two use sites, and `logRedraw`
    /// becomes [`log_redraw`] with the same values passed explicitly.
    fn do_render_impl(&mut self, tui: &mut Tui) {
        if tui.is_stopped() {
            return;
        }
        let (width, height) = {
            let terminal = tui.terminal();
            let terminal = terminal.borrow();
            (terminal.columns() as i64, terminal.rows() as i64)
        };
        let width_changed = self.previous_width != 0 && self.previous_width != width;
        let height_changed = self.previous_height != 0 && self.previous_height != height;
        let previous_buffer_length = if self.previous_height > 0 {
            self.previous_viewport_top + self.previous_height
        } else {
            height
        };
        let mut prev_viewport_top = if height_changed {
            (previous_buffer_length - height).max(0)
        } else {
            self.previous_viewport_top
        };
        let mut viewport_top = prev_viewport_top;
        let mut hardware_cursor_row = self.hardware_cursor_row;

        // Render all components to get new lines.
        let mut new_lines = tui.render(width as usize);

        // Composite overlays into the rendered lines (before differential
        // compare).
        if tui.has_overlay_entries() {
            new_lines = tui.composite_overlays(new_lines, width as usize, height as usize);
        }

        // Extract cursor position before applying line resets (marker must be
        // found first).
        let cursor_pos = tui.extract_cursor_position(&mut new_lines, height as usize);

        let new_lines = tui.apply_line_resets(new_lines);

        let redraw_log_directory = if std::env::var("PI_TUI_DEBUG_REDRAW").as_deref() == Ok("1") {
            tui.log_directory.clone()
        } else {
            None
        };

        // First render - just output everything without clearing (assumes
        // clean screen).
        if self.previous_lines.is_empty() && !width_changed && !height_changed {
            log_redraw(
                &redraw_log_directory,
                "first render",
                self,
                &new_lines,
                height,
            );
            self.full_render(tui, &new_lines, width, height, false, cursor_pos);
            return;
        }

        // Width changes always need a full re-render because wrapping changes.
        if width_changed {
            log_redraw(
                &redraw_log_directory,
                &format!(
                    "terminal width changed ({} -> {width})",
                    self.previous_width
                ),
                self,
                &new_lines,
                height,
            );
            self.full_render(tui, &new_lines, width, height, true, cursor_pos);
            return;
        }

        // Height changes normally need a full re-render to keep the visible
        // viewport aligned, but Termux changes height when the software
        // keyboard shows or hides. In that environment, a full redraw causes
        // the entire history to replay on every toggle.
        if height_changed && !is_termux_session() {
            log_redraw(
                &redraw_log_directory,
                &format!(
                    "terminal height changed ({} -> {height})",
                    self.previous_height
                ),
                self,
                &new_lines,
                height,
            );
            self.full_render(tui, &new_lines, width, height, true, cursor_pos);
            return;
        }

        // Content shrunk below the working area and no overlays - re-render to
        // clear empty rows (overlays need the padding, so only do this when no
        // overlays are active). Configurable via setClearOnShrink().
        if tui.get_clear_on_shrink()
            && new_lines.len() < self.max_lines_rendered
            && !tui.has_overlay_entries()
        {
            log_redraw(
                &redraw_log_directory,
                &format!(
                    "clearOnShrink (maxLinesRendered={})",
                    self.max_lines_rendered
                ),
                self,
                &new_lines,
                height,
            );
            self.full_render(tui, &new_lines, width, height, true, cursor_pos);
            return;
        }

        // Find first and last changed lines.
        let mut first_changed: i64 = -1;
        let mut last_changed: i64 = -1;
        let max_lines = new_lines.len().max(self.previous_lines.len());
        for i in 0..max_lines {
            let old_line = self.previous_lines.get(i).map(String::as_str).unwrap_or("");
            let new_line = new_lines.get(i).map(String::as_str).unwrap_or("");
            if old_line != new_line {
                if first_changed == -1 {
                    first_changed = i as i64;
                }
                last_changed = i as i64;
            }
        }
        let appended_lines = new_lines.len() > self.previous_lines.len();
        if appended_lines {
            if first_changed == -1 {
                first_changed = self.previous_lines.len() as i64;
            }
            last_changed = new_lines.len() as i64 - 1;
        }
        if first_changed != -1 {
            let (expanded_first, expanded_last) =
                self.expand_changed_range_for_kitty_images(first_changed, last_changed, &new_lines);
            first_changed = expanded_first;
            last_changed = expanded_last;
        }
        let append_start = appended_lines
            && first_changed == self.previous_lines.len() as i64
            && first_changed > 0;

        // No changes - but still need to update hardware cursor position if it
        // moved.
        if first_changed == -1 {
            self.position_hardware_cursor(tui, cursor_pos, new_lines.len() as i64);
            self.previous_viewport_top = prev_viewport_top;
            self.previous_height = height;
            return;
        }

        // All changes are in deleted lines (nothing to render, just clear).
        if first_changed >= new_lines.len() as i64 {
            if self.previous_lines.len() > new_lines.len() {
                let mut output = BoundedTerminalWriter::default();
                output.append("\x1b[?2026h", tui);
                output.append(
                    &self.delete_changed_kitty_images(first_changed, last_changed),
                    tui,
                );
                // Move to end of new content (clamp to 0 for empty content).
                let target_row = (new_lines.len() as i64 - 1).max(0);
                if target_row < prev_viewport_top {
                    log_redraw(
                        &redraw_log_directory,
                        &format!(
                            "deleted lines moved viewport up ({target_row} < {prev_viewport_top})"
                        ),
                        self,
                        &new_lines,
                        height,
                    );
                    self.full_render(tui, &new_lines, width, height, true, cursor_pos);
                    return;
                }
                let line_diff =
                    (target_row - viewport_top) - (hardware_cursor_row - prev_viewport_top);
                if line_diff > 0 {
                    output.append(&format!("\x1b[{line_diff}B"), tui);
                } else if line_diff < 0 {
                    output.append(&format!("\x1b[{}A", -line_diff), tui);
                }
                output.append("\r", tui);
                // Clear extra lines without scrolling.
                let extra_lines = self.previous_lines.len() as i64 - new_lines.len() as i64;
                if extra_lines > height {
                    log_redraw(
                        &redraw_log_directory,
                        &format!("extraLines > height ({extra_lines} > {height})"),
                        self,
                        &new_lines,
                        height,
                    );
                    self.full_render(tui, &new_lines, width, height, true, cursor_pos);
                    return;
                }
                let clear_start_offset = i64::from(!new_lines.is_empty());
                if extra_lines > 0 && clear_start_offset > 0 {
                    output.append(&format!("\x1b[{clear_start_offset}B"), tui);
                }
                for i in 0..extra_lines {
                    output.append("\r\x1b[2K", tui);
                    if i < extra_lines - 1 {
                        output.append("\x1b[1B", tui);
                    }
                }
                let move_back = (extra_lines - 1 + clear_start_offset).max(0);
                if move_back > 0 {
                    output.append(&format!("\x1b[{move_back}A"), tui);
                }
                output.append("\x1b[?2026l", tui);
                output.flush(tui);
                self.cursor_row = target_row;
                self.hardware_cursor_row = target_row;
            }
            self.position_hardware_cursor(tui, cursor_pos, new_lines.len() as i64);
            self.previous_lines = new_lines;
            self.previous_kitty_image_ids = self.collect_kitty_image_ids(&self.previous_lines);
            self.previous_width = width;
            self.previous_height = height;
            self.previous_viewport_top = prev_viewport_top;
            return;
        }

        // Differential rendering can only touch what was actually visible. If
        // the first changed line is above the previous viewport, we need a
        // full redraw.
        if first_changed < prev_viewport_top {
            log_redraw(
                &redraw_log_directory,
                &format!("firstChanged < viewportTop ({first_changed} < {prev_viewport_top})"),
                self,
                &new_lines,
                height,
            );
            self.full_render(tui, &new_lines, width, height, true, cursor_pos);
            return;
        }

        // Render from first changed line to end. Keep updates wrapped in
        // synchronized output while writing bounded chunks.
        let mut output = BoundedTerminalWriter::default();
        output.append("\x1b[?2026h", tui); // Begin synchronized output
        output.append(
            &self.delete_changed_kitty_images(first_changed, last_changed),
            tui,
        );
        let prev_viewport_bottom = prev_viewport_top + height - 1;
        let move_target_row = if append_start {
            first_changed - 1
        } else {
            first_changed
        };
        if move_target_row > prev_viewport_bottom {
            let current_screen_row =
                (hardware_cursor_row - prev_viewport_top).clamp(0, (height - 1).max(0));
            let move_to_bottom = height - 1 - current_screen_row;
            if move_to_bottom > 0 {
                output.append(&format!("\x1b[{move_to_bottom}B"), tui);
            }
            let scroll = move_target_row - prev_viewport_bottom;
            output.append(&"\r\n".repeat(scroll as usize), tui);
            prev_viewport_top += scroll;
            viewport_top += scroll;
            hardware_cursor_row = move_target_row;
        }

        // Move cursor to first changed line (use hardwareCursorRow for actual
        // position).
        let line_diff =
            (move_target_row - viewport_top) - (hardware_cursor_row - prev_viewport_top);
        if line_diff > 0 {
            output.append(&format!("\x1b[{line_diff}B"), tui); // Move down
        } else if line_diff < 0 {
            output.append(&format!("\x1b[{}A", -line_diff), tui); // Move up
        }

        output.append(if append_start { "\r\n" } else { "\r" }, tui); // Move to column 0

        // Only render changed lines (firstChanged to lastChanged), not all
        // lines to end. This reduces flicker when only a single line changes
        // (e.g., spinner animation).
        let render_end = last_changed.min(new_lines.len() as i64 - 1);
        let mut i = first_changed;
        while i <= render_end {
            if i > first_changed {
                output.append("\r\n", tui);
            }
            let line = &new_lines[i as usize];
            let is_image = is_image_line(line);
            let image_reserved_rows = if is_image {
                self.get_kitty_image_reserved_rows(&new_lines, i, render_end)
            } else {
                1
            };
            if image_reserved_rows > 1 {
                let image_start_screen_row = i - viewport_top;
                if image_start_screen_row < 0
                    || image_start_screen_row + image_reserved_rows > height
                {
                    log_redraw(
                        &redraw_log_directory,
                        &format!(
                            "kitty image pre-clear would scroll ({image_start_screen_row} + \
                             {image_reserved_rows} > {height})"
                        ),
                        self,
                        &new_lines,
                        height,
                    );
                    self.full_render(tui, &new_lines, width, height, true, cursor_pos);
                    return;
                }

                output.append("\x1b[2K", tui);
                for _ in 1..image_reserved_rows {
                    output.append("\r\n\x1b[2K", tui);
                }
                output.append(&format!("\x1b[{}A", image_reserved_rows - 1), tui);
                output.append(line, tui);
                output.append(&format!("\x1b[{}B", image_reserved_rows - 1), tui);
                i += image_reserved_rows;
                continue;
            }

            output.append("\x1b[2K", tui); // Clear current line
            if !is_image && visible_width(line) > width as usize {
                // Log all lines to crash file for debugging.
                let base_directory = tui
                    .log_directory
                    .clone()
                    .unwrap_or_else(|| std::env::temp_dir().to_string_lossy().into_owned());
                let crash_log_path = std::path::Path::new(&base_directory).join("pi-tui-crash.log");
                let mut crash_lines = vec![
                    format!("Crash at {}", iso_timestamp()),
                    format!("Terminal width: {width}"),
                    format!("Line {i} visible width: {}", visible_width(line)),
                    String::new(),
                    "=== All rendered lines ===".to_string(),
                ];
                for (idx, rendered) in new_lines.iter().enumerate() {
                    crash_lines.push(format!(
                        "[{idx}] (w={}) {rendered}",
                        visible_width(rendered)
                    ));
                }
                crash_lines.push(String::new());
                let crash_data = crash_lines.join("\n");
                let _ = std::fs::create_dir_all(
                    crash_log_path.parent().unwrap_or(std::path::Path::new(".")),
                );
                let _ = std::fs::write(&crash_log_path, crash_data);

                // Clean up terminal state before throwing.
                self.stop_from_do_render(tui);

                let error_message = [
                    format!(
                        "Rendered line {i} exceeds terminal width ({} > {width}).",
                        visible_width(line)
                    ),
                    String::new(),
                    "This is likely caused by a custom TUI component not truncating its output."
                        .to_string(),
                    "Use visibleWidth() to measure and truncateToWidth() to truncate lines."
                        .to_string(),
                    String::new(),
                    format!("Debug log written to: {}", crash_log_path.display()),
                ]
                .join("\n");
                panic!("{error_message}");
            }
            output.append(line, tui);
            i += 1;
        }

        // Track where cursor ended up after rendering.
        let mut final_cursor_row = render_end;

        // If we had more lines before, clear them and move cursor back.
        if self.previous_lines.len() > new_lines.len() {
            // Move to end of new content first if we stopped before it.
            if render_end < new_lines.len() as i64 - 1 {
                let move_down = new_lines.len() as i64 - 1 - render_end;
                output.append(&format!("\x1b[{move_down}B"), tui);
                final_cursor_row = new_lines.len() as i64 - 1;
            }
            let extra_lines = self.previous_lines.len() - new_lines.len();
            for _ in new_lines.len()..self.previous_lines.len() {
                output.append("\r\n\x1b[2K", tui);
            }
            // Move cursor back to end of new content.
            output.append(&format!("\x1b[{extra_lines}A"), tui);
        }

        output.append("\x1b[?2026l", tui); // End synchronized output

        if std::env::var("PI_TUI_DEBUG").as_deref() == Ok("1") {
            write_render_debug_dump(
                first_changed,
                viewport_top,
                self.cursor_row,
                height,
                line_diff,
                hardware_cursor_row,
                render_end,
                final_cursor_row,
                cursor_pos,
                &new_lines,
                &self.previous_lines,
                output.len(),
            );
        }

        output.flush(tui);

        // Track cursor position for next render. cursorRow tracks end of
        // content (for viewport calculation); hardwareCursorRow tracks actual
        // terminal cursor position (for movement).
        self.cursor_row = (new_lines.len() as i64 - 1).max(0);
        self.hardware_cursor_row = final_cursor_row;
        // Track terminal's working area (grows but doesn't shrink unless
        // cleared).
        self.max_lines_rendered = self.max_lines_rendered.max(new_lines.len());
        self.previous_viewport_top = prev_viewport_top.max(final_cursor_row - height + 1);

        // Position hardware cursor for IME.
        self.position_hardware_cursor(tui, cursor_pos, new_lines.len() as i64);

        self.previous_lines = new_lines;
        self.previous_kitty_image_ids = self.collect_kitty_image_ids(&self.previous_lines);
        self.previous_width = width;
        self.previous_height = height;
    }

    /// The upstream inline `fullRender` closure: clear scrollback and viewport
    /// and render all new lines.
    fn full_render(
        &mut self,
        tui: &mut Tui,
        new_lines: &[String],
        width: i64,
        height: i64,
        clear: bool,
        cursor_pos: Option<(usize, usize)>,
    ) {
        tui.add_full_redraw();
        let mut output = BoundedTerminalWriter::default();
        output.append("\x1b[?2026h", tui); // Begin synchronized output
        if clear {
            output.append(
                &self.delete_kitty_images(&self.previous_kitty_image_ids),
                tui,
            );
            output.append("\x1b[2J\x1b[H\x1b[3J", tui); // Clear screen, home, then clear scrollback
        }
        let mut i: i64 = 0;
        while i < new_lines.len() as i64 {
            if i > 0 {
                output.append("\r\n", tui);
            }
            let line = &new_lines[i as usize];
            let is_image = is_image_line(line);
            let image_reserved_rows = if is_image {
                self.get_kitty_image_reserved_rows(new_lines, i, new_lines.len() as i64 - 1)
            } else {
                1
            };
            if image_reserved_rows > 1 && image_reserved_rows <= height {
                for _ in 1..image_reserved_rows {
                    output.append("\r\n", tui);
                }
                output.append(&format!("\x1b[{}A", image_reserved_rows - 1), tui);
                output.append(line, tui);
                output.append(&format!("\x1b[{}B", image_reserved_rows - 1), tui);
                i += image_reserved_rows;
                continue;
            }
            output.append(line, tui);
            i += 1;
        }
        output.append("\x1b[?2026l", tui); // End synchronized output
        output.flush(tui);
        self.cursor_row = (new_lines.len() as i64 - 1).max(0);
        self.hardware_cursor_row = self.cursor_row;
        // Reset max lines when clearing, otherwise track growth.
        if clear {
            self.max_lines_rendered = new_lines.len();
        } else {
            self.max_lines_rendered = self.max_lines_rendered.max(new_lines.len());
        }
        let buffer_length = (height as usize).max(new_lines.len());
        self.previous_viewport_top = buffer_length.saturating_sub(height as usize) as i64;
        self.position_hardware_cursor(tui, cursor_pos, new_lines.len() as i64);
        self.previous_lines = new_lines.to_vec();
        self.previous_kitty_image_ids = self.collect_kitty_image_ids(new_lines);
        self.previous_width = width;
        self.previous_height = height;
    }

    /// Upstream `positionHardwareCursor`: position the hardware cursor for the
    /// IME candidate window.
    fn position_hardware_cursor(
        &mut self,
        tui: &mut Tui,
        cursor_pos: Option<(usize, usize)>,
        total_lines: i64,
    ) {
        let Some((row, col)) = cursor_pos else {
            tui.terminal().borrow_mut().hide_cursor();
            return;
        };
        if total_lines <= 0 {
            tui.terminal().borrow_mut().hide_cursor();
            return;
        }

        // Clamp cursor position to valid range.
        let target_row = (row as i64).clamp(0, total_lines - 1);
        let target_col = (col as i64).max(0);

        // Move cursor from current position to target.
        let row_delta = target_row - self.hardware_cursor_row;
        let mut buffer = String::new();
        if row_delta > 0 {
            buffer.push_str(&format!("\x1b[{row_delta}B")); // Move down
        } else if row_delta < 0 {
            buffer.push_str(&format!("\x1b[{}A", -row_delta)); // Move up
        }
        // Move to absolute column (1-indexed).
        buffer.push_str(&format!("\x1b[{}G", target_col + 1));

        if !buffer.is_empty() {
            tui.terminal().borrow_mut().write(&buffer);
        }

        self.hardware_cursor_row = target_row;
        if tui.get_show_hardware_cursor() {
            tui.terminal().borrow_mut().show_cursor();
        } else {
            tui.terminal().borrow_mut().hide_cursor();
        }
    }

    /// The crash path's `this.stop()` (`TuiStopOptions = {}`). See the module
    /// docs: the renderer is held out of the `Tui` while `doRender` runs, so
    /// the teardown transition is applied inline through the renderer hook and
    /// the terminal.
    fn stop_from_do_render(&mut self, tui: &mut Tui) {
        TuiRenderer::before_terminal_stop(self, tui, &TuiStopOptions::default());
        let terminal = tui.terminal();
        terminal.borrow_mut().show_cursor();
        terminal.borrow_mut().stop();
    }
}

/// Upstream `logRedraw`: append a fullRender reason to `pi-tui-debug.log` in
/// the configured directory.
fn log_redraw(
    redraw_log_directory: &Option<String>,
    reason: &str,
    state: &TuiMainScreen,
    new_lines: &[String],
    height: i64,
) {
    let Some(directory) = redraw_log_directory else {
        return;
    };
    let log_path = std::path::Path::new(directory).join("pi-tui-debug.log");
    let message = format!(
        "[{}] fullRender: {reason} (prev={}, new={}, height={height})\n",
        iso_timestamp(),
        state.previous_lines.len(),
        new_lines.len(),
    );
    let _ = std::fs::create_dir_all(log_path.parent().unwrap_or(std::path::Path::new(".")));
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .and_then(|mut file| std::io::Write::write_all(&mut file, message.as_bytes()));
}

/// The `PI_TUI_DEBUG=1` render dump (`/tmp/tui/render-<ts>-<rand>.log`),
/// byte-for-byte the upstream debugData fields.
#[allow(clippy::too_many_arguments)]
fn write_render_debug_dump(
    first_changed: i64,
    viewport_top: i64,
    cursor_row: i64,
    height: i64,
    line_diff: i64,
    hardware_cursor_row: i64,
    render_end: i64,
    final_cursor_row: i64,
    cursor_pos: Option<(usize, usize)>,
    new_lines: &[String],
    previous_lines: &[String],
    output_length: usize,
) {
    let debug_dir = std::path::Path::new("/tmp/tui");
    let _ = std::fs::create_dir_all(debug_dir);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(0);
    let random_suffix = base36(rand::random::<u64>());
    let debug_path = debug_dir.join(format!("render-{millis}-{random_suffix}.log"));
    let cursor_pos_json = match cursor_pos {
        Some((row, col)) => format!("{{\"row\":{row},\"col\":{col}}}"),
        None => "null".to_string(),
    };
    let debug_data = [
        format!("firstChanged: {first_changed}"),
        format!("viewportTop: {viewport_top}"),
        format!("cursorRow: {cursor_row}"),
        format!("height: {height}"),
        format!("lineDiff: {line_diff}"),
        format!("hardwareCursorRow: {hardware_cursor_row}"),
        format!("renderEnd: {render_end}"),
        format!("finalCursorRow: {final_cursor_row}"),
        format!("cursorPos: {cursor_pos_json}"),
        format!("newLines.length: {}", new_lines.len()),
        format!("previousLines.length: {}", previous_lines.len()),
        String::new(),
        "=== newLines ===".to_string(),
        serde_json::to_string_pretty(new_lines).unwrap_or_default(),
        String::new(),
        "=== previousLines ===".to_string(),
        serde_json::to_string_pretty(previous_lines).unwrap_or_default(),
        String::new(),
        "=== buffer ===".to_string(),
        format!("[{output_length} chars written in bounded chunks]"),
    ]
    .join("\n");
    let _ = std::fs::write(debug_path, debug_data);
}

/// `value.toString(36)` for u64 (lowercase, no prefix).
fn base36(mut value: u64) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if value == 0 {
        return "0".to_string();
    }
    let mut digits = Vec::new();
    while value > 0 {
        digits.push(DIGITS[(value % 36) as usize]);
        value /= 36;
    }
    digits.reverse();
    String::from_utf8(digits).expect("base36 digits are ASCII")
}

impl TuiRenderer for TuiMainScreen {
    fn do_render(&mut self, tui: &mut Tui) {
        self.do_render_impl(tui);
    }

    fn reset_render_state(&mut self, _tui: &mut Tui) {
        self.previous_lines = Vec::new();
        self.previous_width = -1;
        self.previous_height = -1;
        self.cursor_row = 0;
        self.hardware_cursor_row = 0;
        self.max_lines_rendered = 0;
        self.previous_viewport_top = 0;
    }

    fn before_terminal_stop(&mut self, tui: &mut Tui, options: &TuiStopOptions) {
        if options.preserve_screen || self.previous_lines.is_empty() {
            return;
        }
        let terminal = tui.terminal();
        terminal.borrow_mut().write(" ");
        let target_row = self.previous_lines.len() as i64;
        let line_diff = target_row - self.hardware_cursor_row;
        if line_diff > 0 {
            terminal.borrow_mut().write(&format!("\x1b[{line_diff}B"));
        } else if line_diff < 0 {
            terminal
                .borrow_mut()
                .write(&format!("\x1b[{}A", -line_diff));
        }
        terminal.borrow_mut().write("\r\n");
    }
}

#[cfg(test)]
#[path = "main_screen_tests.rs"]
mod main_screen_tests;
