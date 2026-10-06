//! Mouse dispatch primitives from actual tui.ts and TuiAltScreen helpers.
//!
//! `T` is a stable, cloneable host-owned component handle, not a transient tree
//! path or an address. An owning handle can retain a removed component during
//! capture. The host is responsible for safe handle resolution and lifetime;
//! this module does not yet install the live layout/Container/overlay dispatcher.
//! Cell coordinates are signed integers. Coordinate differences saturate only
//! outside i64's representable range (not a JS-number parity claim there).
use crate::tui::component::{
    Component, TuiMouseButton, TuiMouseEvent, TuiMouseEventResult, TuiMouseEventType,
};
use crate::tui::viewport_mouse::SgrMouseEvent;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MouseDispatchTarget<T> {
    pub component: T,
    pub origin_x: i64,
    pub origin_y: i64,
    pub width: usize,
    pub height: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MouseDispatchResult<T> {
    pub result: TuiMouseEventResult,
    pub target: MouseDispatchTarget<T>,
    /// May be a delegating parent, not the concrete mouse recipient.
    pub focus_target: Option<T>,
}
impl<T> MouseDispatchResult<T> {
    /// The caller resolves/applies focus first. Explicit render=false still
    /// suppresses a render even when focus changes or the event is a press.
    pub fn wants_render(&self, event_type: TuiMouseEventType, focus_changed: bool) -> bool {
        self.result
            .render
            .unwrap_or_else(|| focus_changed || self.result.wants_render(event_type))
    }
}

/// A component may decline, return flags, or forward an already dispatched
/// nested target. The latter passes through without rebuilding its coordinates.
pub enum MouseHandlerResult<T> {
    Direct(TuiMouseEventResult),
    Dispatched(MouseDispatchResult<T>),
}

pub fn dispatch_mouse_event<T: Clone>(
    component: T,
    event: &TuiMouseEvent,
    handler: impl FnOnce(&TuiMouseEvent) -> Option<MouseHandlerResult<T>>,
) -> Option<MouseDispatchResult<T>> {
    let result = match handler(event)? {
        MouseHandlerResult::Dispatched(result) => return Some(result),
        MouseHandlerResult::Direct(result) => result,
    };
    if !result.handled && !result.capture && !result.focus {
        return None;
    }
    Some(MouseDispatchResult {
        focus_target: result.focus.then(|| component.clone()),
        result: TuiMouseEventResult {
            handled: true,
            ..result
        },
        target: MouseDispatchTarget {
            component,
            origin_x: event.screen_x.saturating_sub(event.x),
            origin_y: event.screen_y.saturating_sub(event.y),
            width: event.width,
            height: event.height,
        },
    })
}

/// Adapter for today's Component flag-returning API. Nested/Container adapters
/// use dispatch_mouse_event and MouseHandlerResult::Dispatched directly.
pub fn dispatch_component_mouse<T: Clone>(
    component: &mut dyn Component,
    identity: T,
    event: &TuiMouseEvent,
) -> Option<MouseDispatchResult<T>> {
    dispatch_mouse_event(identity, event, |event| {
        component
            .handle_mouse(event)
            .map(MouseHandlerResult::Direct)
    })
}

pub fn retarget_mouse_event<T>(
    event: &TuiMouseEvent,
    target: &MouseDispatchTarget<T>,
) -> TuiMouseEvent {
    TuiMouseEvent {
        x: event.screen_x.saturating_sub(target.origin_x),
        y: event.screen_y.saturating_sub(target.origin_y),
        width: target.width,
        height: target.height,
        ..event.clone()
    }
}

pub fn decode_mouse_button(button: i64) -> TuiMouseButton {
    match button & 3 {
        0 => TuiMouseButton::Left,
        1 => TuiMouseButton::Middle,
        2 => TuiMouseButton::Right,
        _ => TuiMouseButton::None,
    }
}

pub fn mouse_event_type(raw: SgrMouseEvent) -> TuiMouseEventType {
    if raw.release {
        TuiMouseEventType::Release
    } else if raw.button & 32 != 0 {
        if decode_mouse_button(raw.button) == TuiMouseButton::None {
            TuiMouseEventType::Move
        } else {
            TuiMouseEventType::Drag
        }
    } else {
        TuiMouseEventType::Press
    }
}

/// Width/height remain finite cell counts; they are clamped to at least one.
/// Extra wheel_delta/click_count can be assigned on the returned event.
pub fn create_mouse_event(
    event_type: TuiMouseEventType,
    button: i64,
    x: i64,
    y: i64,
    columns: usize,
    rows: usize,
) -> TuiMouseEvent {
    TuiMouseEvent {
        event_type,
        button: if event_type == TuiMouseEventType::Wheel {
            TuiMouseButton::None
        } else {
            decode_mouse_button(button)
        },
        x,
        y,
        screen_x: x,
        screen_y: y,
        width: columns.max(1),
        height: rows.max(1),
        shift: button & 4 != 0,
        alt: button & 8 != 0,
        ctrl: button & 16 != 0,
        wheel_delta: None,
        click_count: None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComponentClick<T> {
    pub component: T,
    pub timestamp_ms: i64,
    pub count: u32,
    pub x: i64,
    pub y: i64,
}

/// Upstream cycles 1,2,3,1 within 500ms for the same component and screen cell.
/// An injected wall clock preserves backward-time behavior without OS sleeps.
pub struct ComponentClickTracker<T> {
    previous: Option<ComponentClick<T>>,
}
impl<T> Default for ComponentClickTracker<T> {
    fn default() -> Self {
        Self { previous: None }
    }
}
impl<T: Clone + PartialEq> ComponentClickTracker<T> {
    pub fn previous(&self) -> Option<&ComponentClick<T>> {
        self.previous.as_ref()
    }
    pub fn clear(&mut self) {
        self.previous = None;
    }
    pub fn count(&mut self, component: T, x: i64, y: i64, now_ms: i64) -> u32 {
        let count = self
            .previous
            .as_ref()
            .filter(|old| {
                i128::from(now_ms) - i128::from(old.timestamp_ms) <= 500
                    && old.component == component
                    && old.x == x
                    && old.y == y
            })
            .map_or(1, |old| old.count % 3 + 1);
        self.previous = Some(ComponentClick {
            component,
            timestamp_ms: now_ms,
            count,
            x,
            y,
        });
        count
    }
}
