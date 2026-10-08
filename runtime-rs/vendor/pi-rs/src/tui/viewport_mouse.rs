//! Alternate-screen wheel and scrollbar routing over live viewport frames.
//!
//! This ports the actual `TuiAltScreen` parse/routeWheel/scrollbar methods, not
//! the full input dispatcher. Hosts must still give overlays/components/search
//! their upstream precedence and wire selection clearing, focus and rendering.
//! Captured scrollbar drags retain their live ScrollHandle even as frames change.
use crate::tui::components::scroll_view::{RequestRender, ScrollHandle, ScrollViewScrollToOptions};
use crate::tui::layout::{
    get_scroll_view_box_id, get_scroll_views_at, get_scrollbar_geometry, LayoutFrame,
    ScrollbarGeometry,
};
use crate::tui::wheel_scroll::{WheelScrollAccelerator, WheelScrollLines};

/// Upstream `ALT_WHEEL_SCROLL_MULTIPLIER` (tui-alt-screen.ts): Alt+wheel moves
/// five times as far.
pub const ALT_WHEEL_SCROLL_MULTIPLIER: f64 = 5.0;

/// SGR coordinates are signed: the protocol accepts a zero field (cell -1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SgrMouseEvent {
    pub button: i64,
    pub x: i64,
    pub y: i64,
    pub release: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WheelDirection {
    Up,
    Down,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WheelEvent {
    pub direction: WheelDirection,
    pub x: i64,
    pub y: i64,
    pub button: i64,
}

fn decimal(units: &[u16]) -> Option<i64> {
    // Keep the exact JS integer domain. Huge decimal/Infinity terminal fields
    // are deliberately rejected, rather than silently rounding/wrapping them.
    const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;
    if units.is_empty() {
        return None;
    }
    units.iter().try_fold(0_i64, |value, &unit| {
        if !(48..=57).contains(&unit) {
            return None;
        }
        let next = value.checked_mul(10)?.checked_add(i64::from(unit - 48))?;
        (next <= MAX_SAFE_INTEGER).then_some(next)
    })
}

/// Exact SGR syntax for finite safe-integer fields; no whitespace/trailing data.
/// Raw UTF-16 access also supports X10 code-unit parsing without lossy decoding.
pub fn parse_sgr_mouse_event_utf16(data: &[u16]) -> Option<SgrMouseEvent> {
    if !data.starts_with(&[27, 91, 60]) || data.len() < 4 {
        return None;
    }
    let release = match data.last()? {
        77 => false,
        109 => true,
        _ => return None,
    };
    let mut fields = data[3..data.len() - 1].split(|&unit| unit == 59);
    let button = decimal(fields.next()?)?;
    let x = decimal(fields.next()?)? - 1;
    let y = decimal(fields.next()?)? - 1;
    if fields.next().is_some() {
        return None;
    }
    Some(SgrMouseEvent {
        button,
        x,
        y,
        release,
    })
}
pub fn parse_sgr_mouse_event(data: &str) -> Option<SgrMouseEvent> {
    parse_sgr_mouse_event_utf16(&data.encode_utf16().collect::<Vec<_>>())
}

/// Vertical SGR or six-code-unit X10 wheel reports. Horizontal reports are not
/// wheel events here; SGR release reports are accepted exactly like presses.
pub fn parse_wheel_event_utf16(data: &[u16]) -> Option<WheelEvent> {
    let (button, x, y) = if let Some(event) = parse_sgr_mouse_event_utf16(data) {
        (event.button, event.x, event.y)
    } else if data.len() == 6 && data.starts_with(&[27, 91, 77]) {
        (
            i64::from(data[3]) - 32,
            i64::from(data[4]) - 33,
            i64::from(data[5]) - 33,
        )
    } else {
        return None;
    };
    if button & 64 == 0 {
        return None;
    }
    let direction = match button & 3 {
        0 => WheelDirection::Up,
        1 => WheelDirection::Down,
        _ => return None,
    };
    Some(WheelEvent {
        direction,
        x,
        y,
        button,
    })
}
pub fn parse_wheel_event(data: &str) -> Option<WheelEvent> {
    parse_wheel_event_utf16(&data.encode_utf16().collect::<Vec<_>>())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScrollbarTarget {
    pub scroll_view: ScrollHandle,
    pub geometry: ScrollbarGeometry,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScrollbarDrag {
    pub scroll_view: ScrollHandle,
    pub grab_offset: usize,
}

/// Stateful viewport-scroll consumer, independent of a particular OS event loop.
/// The supplied implicit scroll is the host's fallback document, not a dummy
/// recreated per event. `request_render` must schedule on the owning host loop.
pub struct ViewportScrollMouse {
    implicit_scroll: ScrollHandle,
    wheel_scroll: WheelScrollAccelerator,
    request_render: RequestRender,
    scrollbar_hover: Option<ScrollHandle>,
    scrollbar_drag: Option<ScrollbarDrag>,
}
impl ViewportScrollMouse {
    /// Upstream `TuiAltScreen` constructor: the wheel-line option defaults to
    /// 1 line (not `"auto"`); acceleration uses the platform/SSH detection.
    pub fn new(
        implicit_scroll: ScrollHandle,
        wheel_scroll_lines: Option<f64>,
        request_render: RequestRender,
    ) -> Self {
        Self {
            implicit_scroll,
            wheel_scroll: WheelScrollAccelerator::new(
                wheel_scroll_lines.map_or(WheelScrollLines::Fixed(1.0), WheelScrollLines::Fixed),
                None,
            ),
            request_render,
            scrollbar_hover: None,
            scrollbar_drag: None,
        }
    }

    /// Upstream `setWheelScrollLines`: reconfigure and reset the gesture state.
    pub fn set_wheel_scroll_lines(&mut self, lines: WheelScrollLines) {
        self.wheel_scroll.set_lines(lines);
    }
    /// Upstream `routeWheel` wheel-delta step: the accelerated line count for
    /// a wheel event in `direction` (-1 | 1) at time `now` (ms), times the
    /// Alt multiplier when the SGR button code has bit 3 set.
    pub fn wheel_scroll_lines(&mut self, button: i64, now: f64, direction: i64) -> f64 {
        let lines = self.wheel_scroll.next(direction, now);
        let scaled = if button & 8 != 0 {
            lines * ALT_WHEEL_SCROLL_MULTIPLIER
        } else {
            lines
        };
        direction as f64 * scaled
    }
    pub fn scrollbar_hover(&self) -> Option<&ScrollHandle> {
        self.scrollbar_hover.as_ref()
    }
    pub fn scrollbar_drag(&self) -> Option<&ScrollbarDrag> {
        self.scrollbar_drag.as_ref()
    }
    pub fn scrollbar_target_at(
        frame: Option<&LayoutFrame>,
        has_overlay: bool,
        x: i64,
        y: i64,
        include_hidden_auto: bool,
    ) -> Option<ScrollbarTarget> {
        if has_overlay {
            return None;
        }
        let frame = frame?;
        for scroll_view in get_scroll_views_at(frame, x, y) {
            let Some(index) = get_scroll_view_box_id(frame, &scroll_view) else {
                continue;
            };
            let Some(geometry) = get_scrollbar_geometry(frame, index, include_hidden_auto) else {
                continue;
            };
            if x == geometry.column
                && y >= geometry.track_top
                && (y - geometry.track_top) < geometry.track_height as i64
            {
                return Some(ScrollbarTarget {
                    scroll_view,
                    geometry,
                });
            }
        }
        None
    }
    fn set_scrollbar_hover(&mut self, scroll: Option<ScrollHandle>) {
        if self.scrollbar_hover == scroll {
            return;
        }
        if let Some(old) = &self.scrollbar_hover {
            old.set_scrollbar_active(false);
        }
        self.scrollbar_hover = scroll;
        if let Some(new) = &self.scrollbar_hover {
            new.set_scrollbar_active(true);
        }
    }
    pub fn update_scrollbar_hover(
        &mut self,
        frame: Option<&LayoutFrame>,
        has_overlay: bool,
        x: i64,
        y: i64,
    ) {
        self.set_scrollbar_hover(
            Self::scrollbar_target_at(frame, has_overlay, x, y, true).map(|t| t.scroll_view),
        );
    }
    pub fn stop_scrollbar_hover(&mut self) {
        self.set_scrollbar_hover(None);
    }
    pub fn stop_scrollbar_drag(&mut self) {
        self.scrollbar_drag = None;
    }
    /// Route after overlay/component handling has declined the wheel event.
    /// Upstream containment breaks the hit chain, NOT the separate primary
    /// fallback. A primary not already visited still receives unused delta.
    /// `now` is the monotonic clock in milliseconds (upstream performance.now).
    pub fn route_wheel(
        &mut self,
        frame: Option<&LayoutFrame>,
        has_overlay: bool,
        event: WheelEvent,
        now: f64,
    ) {
        let direction = match event.direction {
            WheelDirection::Up => -1.0,
            WheelDirection::Down => 1.0,
        };
        let mut remaining = self.wheel_scroll_lines(event.button, now, direction as i64);
        let mut seen = Vec::new();
        for scroll in frame
            .map(|f| get_scroll_views_at(f, event.x, event.y))
            .unwrap_or_default()
        {
            seen.push(scroll.clone());
            remaining = scroll.scroll_by_number(remaining);
            if remaining == 0.0 || scroll.snapshot().overscroll_contain {
                break;
            }
        }
        let primary = frame
            .and_then(|f| f.primary_scroll_view.as_ref())
            .unwrap_or(&self.implicit_scroll);
        if remaining != 0.0 && !seen.contains(primary) {
            primary.scroll_by_number(remaining);
        }
        self.update_scrollbar_hover(frame, has_overlay, event.x, event.y);
        (self.request_render)();
    }
    fn scroll_to_pointer(
        scroll: &ScrollHandle,
        geometry: ScrollbarGeometry,
        pointer_y: i64,
        grab_offset: usize,
    ) {
        let max_offset = geometry.track_height.saturating_sub(geometry.thumb_height);
        let offset = (pointer_y as f64 - geometry.track_top as f64 - grab_offset as f64)
            .max(0.0)
            .min(max_offset as f64);
        let top = if max_offset == 0 {
            0.0
        } else {
            (offset / max_offset as f64 * geometry.max_scroll_top as f64).round()
        };
        scroll.scroll_to_number(top, ScrollViewScrollToOptions::default());
    }
    /// A new valid left-button drag clears host text-selection state before
    /// hover/scroll callbacks. Active drags consume reports outside bounds,
    /// through overlays and even when their geometry disappears; release ends
    /// capture without an extra scroll/render request. This method itself does
    /// not perform the outer dispatcher's post-event hover update.
    pub fn handle_scrollbar_mouse_event(
        &mut self,
        frame: Option<&LayoutFrame>,
        has_overlay: bool,
        event: SgrMouseEvent,
        clear_selection: impl FnOnce(),
    ) -> bool {
        if let Some(drag) = &self.scrollbar_drag {
            if event.release {
                self.stop_scrollbar_drag();
                return true;
            }
            if let Some((frame, index)) = frame.and_then(|frame| {
                get_scroll_view_box_id(frame, &drag.scroll_view).map(|i| (frame, i))
            }) {
                if let Some(geometry) = get_scrollbar_geometry(frame, index, false) {
                    Self::scroll_to_pointer(&drag.scroll_view, geometry, event.y, drag.grab_offset);
                }
            }
            return true;
        }
        if event.release || event.button & 32 != 0 || event.button & 3 != 0 {
            return false;
        }
        let Some(target) = Self::scrollbar_target_at(frame, has_overlay, event.x, event.y, false)
        else {
            return false;
        };
        clear_selection();
        self.set_scrollbar_hover(Some(target.scroll_view.clone()));
        let geometry = target.geometry;
        let on_thumb = event.y >= geometry.thumb_top
            && event.y - geometry.thumb_top < geometry.thumb_height as i64;
        let grab_offset = if on_thumb {
            (event.y - geometry.thumb_top) as usize
        } else {
            geometry.thumb_height / 2
        };
        if !on_thumb {
            Self::scroll_to_pointer(&target.scroll_view, geometry, event.y, grab_offset);
        }
        self.scrollbar_drag = Some(ScrollbarDrag {
            scroll_view: target.scroll_view,
            grab_offset,
        });
        true
    }
}
