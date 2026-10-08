//! Owning component mouse gestures from actual `TuiAltScreen.handleMouseEvent`.
//!
//! This controller executes synchronous host callbacks in upstream order. It is
//! not the legacy screen/OS event loop. A host must implement search, overlay,
//! scrollbar, focus, selection and paste; none silently default to a no-op.
//! Feed non-wheel SGR events here. The viewport input layer routes wheel events
//! separately and may reuse [`ComponentGesture::apply_dispatch_result`].
//! Owning targets retain removed components and their original geometry. Calls
//! are single-threaded and non-reentrant; do not put handles on timer workers.
use crate::tui::component::{TuiMouseEvent, TuiMouseEventType};
use crate::tui::component_mouse::{
    dispatch_mouse_to_target, ComponentHandle, ComponentMouseResult, ComponentMouseTarget,
};
use crate::tui::mouse_dispatch::{
    create_mouse_event, mouse_event_type, ComponentClick, ComponentClickTracker,
};
use crate::tui::viewport_mouse::SgrMouseEvent;

/// A hit without a result still blocks the layout behind the overlay.
#[derive(Default)]
pub struct OverlayMouseDispatch {
    pub hit: bool,
    pub result: Option<ComponentMouseResult>,
}

/// Required integration seams, invoked immediately rather than returned as an
/// action list. A caller may use `component_mouse::dispatch_mouse_to_layout` in
/// its layout implementation. Focus resolution must account for live overlays.
pub trait ComponentGestureHost {
    fn terminal_size(&self) -> (usize, usize);
    fn now_ms(&mut self) -> i64;
    fn handle_search_mouse_event(&mut self, raw: SgrMouseEvent) -> bool;
    fn dispatch_mouse_to_overlay(&mut self, event: &TuiMouseEvent) -> OverlayMouseDispatch;
    fn handle_scroll_to_end_indicator_mouse_event(&mut self, raw: SgrMouseEvent) -> bool;
    fn handle_scrollbar_mouse_event(&mut self, raw: SgrMouseEvent) -> bool;
    fn scrollbar_drag_active(&self) -> bool;
    fn update_scrollbar_hover(&mut self, x: i64, y: i64);
    fn stop_scrollbar_hover(&mut self);
    fn dispatch_mouse_to_layout(&mut self, event: &TuiMouseEvent) -> Option<ComponentMouseResult>;
    fn resolve_mouse_focus_target(&mut self, component: &ComponentHandle) -> ComponentHandle;
    fn focused_component(&self) -> Option<ComponentHandle>;
    fn set_focus(&mut self, component: ComponentHandle);
    fn clear_text_selection(&mut self);
    fn request_render(&mut self);
    fn handle_right_click_paste(&mut self, raw: SgrMouseEvent) -> bool;
    /// Selection receives this live controller so its release-click focus/capture
    /// effects are synchronous and persist in the ongoing mouse event stream.
    fn handle_selection_mouse_event(&mut self, raw: SgrMouseEvent, gesture: &mut ComponentGesture);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MousePressPoint {
    pub x: i64,
    pub y: i64,
}

#[derive(Default)]
pub struct ComponentGesture {
    capture: Option<ComponentMouseTarget>,
    press_target: Option<ComponentMouseTarget>,
    press_point: Option<MousePressPoint>,
    press_moved: bool,
    clicks: ComponentClickTracker<ComponentHandle>,
}
impl ComponentGesture {
    pub fn capture(&self) -> Option<&ComponentMouseTarget> {
        self.capture.as_ref()
    }
    pub fn press_target(&self) -> Option<&ComponentMouseTarget> {
        self.press_target.as_ref()
    }
    pub fn press_point(&self) -> Option<MousePressPoint> {
        self.press_point
    }
    pub fn press_moved(&self) -> bool {
        self.press_moved
    }
    pub fn last_click(&self) -> Option<&ComponentClick<ComponentHandle>> {
        self.clicks.previous()
    }
    /// Clear the active gesture, not click history. Used after release/stop.
    pub fn clear_gesture(&mut self) {
        self.capture = None;
        self.press_target = None;
        self.press_point = None;
        self.press_moved = false;
    }
    /// Mouse-state portion only: the host still owns the rest of focus-out.
    pub fn on_focus_out(&mut self) {
        self.clear_gesture();
        self.clicks.clear();
    }
    /// Mouse-state portion of beforeTerminalStart, not terminal startup itself.
    pub fn on_terminal_start(&mut self) {
        self.clear_gesture();
        self.clicks.clear();
    }
    /// Actual beforeTerminalStop leaves lastComponentClick intact. The next
    /// start clears it. This hook does not stop selection/scrollbar/OS timers.
    pub fn on_terminal_stop(&mut self) {
        self.clear_gesture();
    }
    fn event(
        &self,
        host: &impl ComponentGestureHost,
        raw: SgrMouseEvent,
        kind: TuiMouseEventType,
    ) -> TuiMouseEvent {
        let (columns, rows) = host.terminal_size();
        create_mouse_event(kind, raw.button, raw.x, raw.y, columns, rows)
    }
    /// Resolve even when focus is not requested, then focus, capture, and decide
    /// render. Explicit false overrides both focus change and event defaults.
    /// No render is scheduled by this method; the caller combines release/click.
    pub fn apply_dispatch_result(
        &mut self,
        host: &mut impl ComponentGestureHost,
        event: &TuiMouseEvent,
        result: &ComponentMouseResult,
    ) -> bool {
        let requested = result
            .focus_target
            .as_ref()
            .unwrap_or(&result.target.component);
        let focus_target = host.resolve_mouse_focus_target(requested);
        let focus_changed =
            result.result.focus && host.focused_component().as_ref() != Some(&focus_target);
        if result.result.focus {
            host.set_focus(focus_target);
        }
        if result.result.capture {
            self.capture = Some(result.target.clone());
        }
        result.wants_render(event.event_type, focus_changed)
    }
    pub fn handle_mouse_event(&mut self, host: &mut impl ComponentGestureHost, raw: SgrMouseEvent) {
        let kind = mouse_event_type(raw);
        let event = self.event(host, raw, kind);
        if let Some(target) = self
            .capture
            .as_ref()
            .or(self.press_target.as_ref())
            .cloned()
        {
            if self
                .press_point
                .is_some_and(|p| p.x != raw.x || p.y != raw.y)
            {
                self.press_moved = true;
                self.clicks.clear();
            }
            let mut render = false;
            if let Some(result) = dispatch_mouse_to_target(&event, &target) {
                render = self.apply_dispatch_result(host, &event, &result);
            }
            if raw.release {
                if !self.press_moved
                    && self.press_point == Some(MousePressPoint { x: raw.x, y: raw.y })
                {
                    // Read current terminal size and clock AFTER release callback.
                    let count =
                        self.clicks
                            .count(target.component.clone(), raw.x, raw.y, host.now_ms());
                    let mut click = self.event(host, raw, TuiMouseEventType::Click);
                    click.click_count = Some(count);
                    if let Some(result) = dispatch_mouse_to_target(&click, &target) {
                        // Do not short-circuit applying click flags when release renders.
                        render = self.apply_dispatch_result(host, &click, &result) || render;
                    }
                }
                self.clear_gesture();
            }
            if render {
                host.request_render();
            }
            return;
        }
        if host.handle_search_mouse_event(raw) {
            return;
        }
        let overlay = host.dispatch_mouse_to_overlay(&event);
        if !overlay.hit {
            if host.handle_scroll_to_end_indicator_mouse_event(raw) {
                return;
            }
            let handled = host.handle_scrollbar_mouse_event(raw);
            if !host.scrollbar_drag_active() {
                host.update_scrollbar_hover(raw.x, raw.y);
            }
            if handled {
                return;
            }
        } else {
            host.stop_scrollbar_hover();
        }
        let result = overlay.result.or_else(|| {
            if overlay.hit {
                None
            } else {
                host.dispatch_mouse_to_layout(&event)
            }
        });
        if let Some(result) = result {
            let render = self.apply_dispatch_result(host, &event, &result);
            if kind == TuiMouseEventType::Press {
                host.clear_text_selection();
                self.press_target = Some(result.target);
                self.press_point = Some(MousePressPoint { x: raw.x, y: raw.y });
                self.press_moved = false;
            }
            if render {
                host.request_render();
            }
            return;
        }
        if host.handle_right_click_paste(raw) {
            return;
        }
        host.handle_selection_mouse_event(raw, self);
    }
}
