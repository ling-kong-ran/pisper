//! Owning fullscreen text-selection controller from `tui-alt-screen.ts`.
//!
//! Uses real layout/scroll identities and synchronous Gesture/Overlay/Focus
//! dispatch. It is not the OS host or a clipboard success implementation.
//! Intl word segmentation, event-loop intervals (unreferenced), URL launching
//! and initiating asynchronous clipboard delivery are required host services.
//! No default no-op implementations are provided. Stop the interval before
//! dropping a controller; callbacks must be queued to its single-threaded host,
//! never execute ComponentHandle operations from a ScrollView worker thread.
//! Coordinates are finite integer cells; arbitrary JS numbers, UTF-16 lone
//! surrogates and self-reentrant host callbacks are outside this layer.
use crate::tui::component::TuiMouseEventType;
use crate::tui::component_gesture::{ComponentGesture, ComponentGestureHost};
use crate::tui::components::scroll_view::ScrollHandle;
use crate::tui::layout::{get_scroll_view_box, get_scroll_views_at, LayoutFrame};
use crate::tui::mouse_dispatch::create_mouse_event;
use crate::tui::utils::{
    get_grapheme_cell_range, get_osc8_link_at_column, js_trim_end, slice_by_column,
    strip_terminal_sequences, visible_width,
};
use crate::tui::viewport_mouse::SgrMouseEvent;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectionPoint {
    pub row: usize,
    pub col: usize,
    pub scroll_view: Option<ScrollHandle>,
    pub boundary: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectionRange {
    pub start: SelectionPoint,
    pub end: SelectionPoint,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SelectionGranularity {
    #[default]
    Character,
    Word,
    Line,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectionClick {
    pub timestamp: i64,
    pub count: u8,
    pub row: usize,
    pub scroll_view: Option<ScrollHandle>,
    pub word_start: usize,
    pub word_end: usize,
}
/// External Intl-equivalent segmentation, NOT a Rust ICU implementation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectionWordSegment {
    pub text: String,
    pub is_word_like: bool,
}
#[derive(Clone, Debug, Default)]
pub struct SelectionState {
    pub anchor: Option<SelectionPoint>,
    pub focus: Option<SelectionPoint>,
    pub granularity: SelectionGranularity,
    pub initial_range: Option<SelectionRange>,
    pub last_click: Option<SelectionClick>,
    pub drag_pointer: Option<(i64, i64)>,
    pub auto_scroll_direction: i64,
    pub timer: Option<u64>,
    pub press_active: bool,
    pub pressed_url: Option<String>,
    pub dragged: bool,
}
/// All effects are immediate. `request_copy_text` initiates delivery; it does
/// not imply eventual clipboard success/flash. `open_url` failures are ignored
/// by upstream selection, but Rust panics are not swallowed as JS exceptions.
/// Interval tokens must identify this controller's queued ticks; cancel stale
/// queued deliveries in the host. No interval callback borrows this controller.
pub trait ComponentSelectionHost: ComponentGestureHost {
    fn selection_frame(&self) -> Option<&LayoutFrame>;
    fn selection_screen(&self) -> &[String];
    fn selection_has_overlay(&mut self) -> bool;
    fn selection_word_segments(&mut self, plain: &str) -> Vec<SelectionWordSegment>;
    fn start_selection_interval(&mut self, millis: u64) -> u64;
    fn cancel_selection_interval(&mut self, token: u64);
    fn has_url_opener(&self) -> bool;
    fn open_selection_url(&mut self, url: &str) -> Result<(), String>;
    fn request_copy_text(&mut self, text: String);
}
#[derive(Debug)]
pub struct ComponentSelection {
    state: SelectionState,
    copy_on_select: bool,
}
impl Default for ComponentSelection {
    fn default() -> Self {
        Self {
            state: SelectionState::default(),
            copy_on_select: true,
        }
    }
}
fn cell(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}
fn clamp_pointer(value: i64, size: usize) -> usize {
    value.min(cell(size).saturating_sub(1)).max(0) as usize
}
impl ComponentSelection {
    pub fn state(&self) -> &SelectionState {
        &self.state
    }
    pub fn copy_on_select(&self) -> bool {
        self.copy_on_select
    }
    pub fn set_copy_on_select(&mut self, enabled: bool) {
        self.copy_on_select = enabled;
    }

    pub fn scroll_selection_point(
        &self,
        host: &impl ComponentSelectionHost,
        scroll: &ScrollHandle,
        x: i64,
        y: i64,
    ) -> Option<SelectionPoint> {
        let b = get_scroll_view_box(host.selection_frame()?, scroll)?;
        if b.rect.height == 0 || b.clip.height == 0 {
            return None;
        }
        let (_, rows) = host.terminal_size();
        let top = 0.max(b.rect.y).max(b.clip.y);
        let bottom = cell(rows)
            .saturating_sub(1)
            .min(
                b.rect
                    .y
                    .saturating_add(cell(b.rect.height))
                    .saturating_sub(1),
            )
            .min(
                b.clip
                    .y
                    .saturating_add(cell(b.clip.height))
                    .saturating_sub(1),
            );
        if bottom < top {
            return None;
        }
        let pointer_row = y.min(bottom).max(top);
        let max_row = b
            .scroll_content_lines
            .as_ref()
            .map_or(1, |l| l.len())
            .saturating_sub(1);
        Some(SelectionPoint {
            row: cell(scroll.snapshot().scroll_top)
                .saturating_add(pointer_row)
                .saturating_sub(b.rect.y)
                .min(cell(max_row))
                .max(0) as usize,
            col: x
                .saturating_sub(b.rect.x)
                .min(cell(b.rect.width).saturating_sub(1))
                .max(0) as usize,
            scroll_view: Some(scroll.clone()),
            boundary: false,
        })
    }
    pub fn selection_point(
        &self,
        host: &impl ComponentSelectionHost,
        raw: SgrMouseEvent,
        scroll: Option<&ScrollHandle>,
    ) -> SelectionPoint {
        if let Some(point) = scroll.and_then(|s| self.scroll_selection_point(host, s, raw.x, raw.y))
        {
            return point;
        }
        let (columns, rows) = host.terminal_size();
        SelectionPoint {
            row: clamp_pointer(raw.y, rows),
            col: clamp_pointer(raw.x, columns),
            scroll_view: None,
            boundary: false,
        }
    }
    fn source_line(&self, host: &impl ComponentSelectionHost, point: &SelectionPoint) -> String {
        if let Some(lines) = point.scroll_view.as_ref().and_then(|s| {
            get_scroll_view_box(host.selection_frame()?, s)?
                .scroll_content_lines
                .as_ref()
        }) {
            return lines.get(point.row).unwrap_or("").to_owned();
        }
        host.selection_screen()
            .get(point.row)
            .cloned()
            .unwrap_or_default()
    }
    pub fn word_selection(
        &self,
        host: &mut impl ComponentSelectionHost,
        point: &SelectionPoint,
    ) -> Option<SelectionRange> {
        let line = strip_terminal_sequences(&self.source_line(host, point));
        let mut start = 0;
        let segments: Vec<_> = host
            .selection_word_segments(&line)
            .into_iter()
            .map(|s| {
                let end = start + visible_width(&s.text);
                let joiner = s.text == "/" || s.text == "-";
                let entry = (start, end, s.is_word_like || joiner, joiner);
                start = end;
                entry
            })
            .collect();
        let index = segments
            .iter()
            .position(|s| point.col >= s.0 && point.col < s.1)?;
        let joins =
            |a: usize, b: usize| segments[a].2 && segments[b].2 && (segments[a].3 || segments[b].3);
        let mut left = index;
        while left > 0 && joins(left - 1, left) {
            left -= 1;
        }
        let mut right = index;
        while right + 1 < segments.len() && joins(right, right + 1) {
            right += 1;
        }
        let mut first = point.clone();
        first.col = segments[left].0;
        let mut last = point.clone();
        last.col = segments[right].1;
        last.boundary = true;
        Some(SelectionRange {
            start: first,
            end: last,
        })
    }
    pub fn line_selection(
        &self,
        host: &impl ComponentSelectionHost,
        point: &SelectionPoint,
    ) -> SelectionRange {
        let mut first = point.clone();
        first.col = 0;
        let mut last = point.clone();
        last.col = visible_width(&self.source_line(host, point));
        last.boundary = true;
        SelectionRange {
            start: first,
            end: last,
        }
    }
    fn update_focus(&mut self, host: &mut impl ComponentSelectionHost, point: SelectionPoint) {
        let Some(initial) = self
            .state
            .initial_range
            .clone()
            .filter(|_| self.state.granularity != SelectionGranularity::Character)
        else {
            self.state.focus = Some(point);
            return;
        };
        let range = if self.state.granularity == SelectionGranularity::Word {
            self.word_selection(host, &point)
        } else {
            Some(self.line_selection(host, &point))
        };
        let Some(range) = range else {
            return;
        };
        if (range.start.row, range.start.col) < (initial.start.row, initial.start.col) {
            self.state.anchor = Some(initial.end);
            self.state.focus = Some(range.start);
        } else {
            self.state.anchor = Some(initial.start);
            self.state.focus = Some(range.end);
        }
    }
    fn click_count(
        &mut self,
        host: &mut impl ComponentSelectionHost,
        point: &SelectionPoint,
        word: Option<&SelectionRange>,
    ) -> u8 {
        let now = host.now_ms();
        let count = match (word, self.state.last_click.as_ref()) {
            (Some(word), Some(previous))
                if i128::from(now) - i128::from(previous.timestamp) <= 500
                    && previous.row == point.row
                    && previous.scroll_view == point.scroll_view
                    && previous.word_start == word.start.col
                    && previous.word_end == word.end.col =>
            {
                previous.count % 3 + 1
            }
            _ => 1,
        };
        self.state.last_click = word.map(|word| SelectionClick {
            timestamp: now,
            count,
            row: point.row,
            scroll_view: point.scroll_view.clone(),
            word_start: word.start.col,
            word_end: word.end.col,
        });
        count
    }
    pub fn stop_auto_scroll(&mut self, host: &mut impl ComponentSelectionHost) {
        if let Some(token) = self.state.timer.take() {
            host.cancel_selection_interval(token);
        }
        self.state.auto_scroll_direction = 0;
        self.state.drag_pointer = None;
    }
    fn update_auto_scroll(&mut self, host: &mut impl ComponentSelectionHost, raw: SgrMouseEvent) {
        let b = self
            .state
            .anchor
            .as_ref()
            .and_then(|p| p.scroll_view.as_ref())
            .and_then(|s| get_scroll_view_box(host.selection_frame()?, s));
        let Some(b) = b.filter(|b| b.rect.height > 0 && b.clip.height > 0) else {
            self.stop_auto_scroll(host);
            return;
        };
        let (_, rows) = host.terminal_size();
        let top = 0.max(b.rect.y).max(b.clip.y);
        let bottom = cell(rows)
            .saturating_sub(1)
            .min(
                b.rect
                    .y
                    .saturating_add(cell(b.rect.height))
                    .saturating_sub(1),
            )
            .min(
                b.clip
                    .y
                    .saturating_add(cell(b.clip.height))
                    .saturating_sub(1),
            );
        self.state.drag_pointer = Some((raw.x, raw.y));
        self.state.auto_scroll_direction = if raw.y <= top {
            -1
        } else if raw.y >= bottom {
            1
        } else {
            0
        };
        if self.state.auto_scroll_direction == 0 {
            self.stop_auto_scroll(host);
            return;
        }
        if self.state.timer.is_none() {
            self.state.timer = Some(host.start_selection_interval(50));
        }
    }
    /// Host event-loop tick. The timer never owns or borrows the controller.
    pub fn auto_scroll_tick(&mut self, host: &mut impl ComponentSelectionHost) {
        let scroll = self
            .state
            .anchor
            .as_ref()
            .and_then(|p| p.scroll_view.clone());
        let direction = self.state.auto_scroll_direction;
        let (Some(scroll), Some((x, y))) = (scroll, self.state.drag_pointer) else {
            self.stop_auto_scroll(host);
            return;
        };
        if direction == 0 || scroll.scroll_by(direction) == direction {
            self.stop_auto_scroll(host);
            return;
        }
        if let Some(point) = self.scroll_selection_point(host, &scroll, x, y) {
            self.update_focus(host, point);
        }
        host.request_render();
    }
    /// Upstream clearTextSelection does NOT clear selection click history.
    pub fn clear(&mut self, host: &mut impl ComponentSelectionHost) {
        self.stop_auto_scroll(host);
        let last_click = self.state.last_click.take();
        self.state = SelectionState {
            last_click,
            ..SelectionState::default()
        };
    }
    pub fn bounds(&self) -> Option<SelectionRange> {
        let anchor = self.state.anchor.as_ref()?;
        let focus = self.state.focus.as_ref()?;
        if anchor.scroll_view != focus.scroll_view
            || (anchor.row, anchor.col) == (focus.row, focus.col)
        {
            return None;
        }
        let (start, end) = if (anchor.row, anchor.col) < (focus.row, focus.col) {
            (anchor, focus)
        } else {
            (focus, anchor)
        };
        Some(SelectionRange {
            start: start.clone(),
            end: end.clone(),
        })
    }
    pub fn selection_columns(
        line: &str,
        row: usize,
        selection: &SelectionRange,
        min: usize,
        max: usize,
    ) -> (usize, usize) {
        let width = visible_width(line);
        let mut start = min;
        let mut end = width.min(max);
        if row == selection.start.row {
            start = get_grapheme_cell_range(line, selection.start.col)
                .map_or(selection.start.col.min(width), |r| r.0);
        }
        if row == selection.end.row {
            end = if selection.end.boundary {
                selection.end.col.min(width)
            } else {
                get_grapheme_cell_range(line, selection.end.col)
                    .map_or(selection.end.col.saturating_add(1).min(width), |r| r.1)
            };
        }
        (start.max(min), end.min(max))
    }
    pub fn active_text(&self, host: &impl ComponentSelectionHost) -> Option<String> {
        let selection = self.bounds()?;
        let scroll_lines = if let Some(scroll) = &selection.start.scroll_view {
            Some(
                get_scroll_view_box(host.selection_frame()?, scroll)?
                    .scroll_content_lines
                    .as_ref()?,
            )
        } else {
            None
        };
        let mut lines = Vec::new();
        for row in selection.start.row..=selection.end.row {
            let line = if let Some(lines) = scroll_lines {
                lines.get(row).unwrap_or("")
            } else {
                host.selection_screen()
                    .get(row)
                    .map(String::as_str)
                    .unwrap_or("")
            };
            let (start, end) =
                Self::selection_columns(line, row, &selection, 0, visible_width(line));
            let plain = strip_terminal_sequences(&slice_by_column(
                line,
                start,
                end.saturating_sub(start),
                true,
            ));
            lines.push(js_trim_end(&plain).to_owned());
        }
        let text = lines.join("\n");
        (!text.is_empty()).then_some(text)
    }
    /// True means delivery was requested, not that the OS clipboard accepted it.
    pub fn request_copy_active_selection(&self, host: &mut impl ComponentSelectionHost) -> bool {
        if let Some(text) = self.active_text(host) {
            host.request_copy_text(text);
            true
        } else {
            false
        }
    }
    /// Selection fallback, including press/move/release. Pass the SAME gesture
    /// controller that routed the event so release-click capture/focus effects
    /// survive. ComponentGesture supplies it to its host's selection callback.
    pub fn handle_mouse_event(
        &mut self,
        host: &mut impl ComponentSelectionHost,
        gesture: &mut ComponentGesture,
        raw: SgrMouseEvent,
    ) {
        let button = raw.button & 3;
        if button != 0 && !(raw.release && button == 3) {
            return;
        }
        let scroll = self
            .state
            .anchor
            .as_ref()
            .and_then(|p| p.scroll_view.as_ref());
        let point = self.selection_point(host, raw, scroll);
        if raw.release {
            if !self.state.press_active {
                return;
            }
            self.state.press_active = false;
            self.stop_auto_scroll(host);
            if self.state.anchor.is_none() {
                return;
            }
            self.update_focus(host, point.clone());
            let anchor = self.state.anchor.as_ref().unwrap();
            let is_click = !self.state.dragged
                && anchor.scroll_view == point.scroll_view
                && anchor.row == point.row
                && anchor.col == point.col;
            let clicked_url = if is_click {
                self.state.pressed_url.clone()
            } else {
                None
            };
            self.state.pressed_url = None;
            if let Some(url) = clicked_url.filter(|url| !url.is_empty() && host.has_url_opener()) {
                self.state.anchor = None;
                self.state.focus = None;
                let _ = host.open_selection_url(&url);
                host.request_render();
                return;
            }
            if is_click {
                let (columns, rows) = host.terminal_size();
                let mut click = create_mouse_event(
                    TuiMouseEventType::Click,
                    raw.button,
                    raw.x,
                    raw.y,
                    columns,
                    rows,
                );
                click.click_count =
                    Some(self.state.last_click.as_ref().map_or(1, |c| c.count).into());
                let overlay = host.dispatch_mouse_to_overlay(&click);
                let result = overlay.result.or_else(|| {
                    if overlay.hit {
                        None
                    } else {
                        host.dispatch_mouse_to_layout(&click)
                    }
                });
                if let Some(result) = result {
                    let render = gesture.apply_dispatch_result(host, &click, &result);
                    self.clear(host);
                    if render {
                        host.request_render();
                    }
                    return;
                }
            }
            if self.copy_on_select {
                self.request_copy_active_selection(host);
            }
            host.request_render();
            return;
        }
        if raw.button & 32 != 0 {
            if !self.state.press_active || self.state.anchor.is_none() {
                return;
            }
            self.state.dragged = true;
            self.state.last_click = None;
            self.state.pressed_url = None;
            self.update_focus(host, point);
            self.update_auto_scroll(host, raw);
            host.request_render();
            return;
        }
        self.stop_auto_scroll(host);
        self.state.press_active = true;
        let scroll = if !host.selection_has_overlay() {
            host.selection_frame()
                .and_then(|f| get_scroll_views_at(f, raw.x, raw.y).into_iter().next())
        } else {
            None
        };
        let anchor = self.selection_point(host, raw, scroll.as_ref());
        let word = self.word_selection(host, &anchor);
        let count = self.click_count(host, &anchor, word.as_ref());
        let range = match count {
            2 => word,
            3 => Some(self.line_selection(host, &anchor)),
            _ => None,
        };
        self.state.granularity = match (count, range.is_some()) {
            (2, true) => SelectionGranularity::Word,
            (3, true) => SelectionGranularity::Line,
            _ => SelectionGranularity::Character,
        };
        self.state.anchor = Some(
            range
                .as_ref()
                .map_or_else(|| anchor.clone(), |r| r.start.clone()),
        );
        self.state.focus = Some(range.as_ref().map_or(anchor, |r| r.end.clone()));
        self.state.dragged = false;
        self.state.pressed_url = if range.is_some() {
            None
        } else {
            let (columns, rows) = host.terminal_size();
            get_osc8_link_at_column(
                host.selection_screen()
                    .get(clamp_pointer(raw.y, rows))
                    .map(String::as_str)
                    .unwrap_or(""),
                clamp_pointer(raw.x, columns),
            )
            .map(str::to_owned)
        };
        self.state.initial_range = range;
        host.request_render();
    }
}
