//! Child-first mouse wrapper, deliberately NOT a layout node.
use crate::tui::component::{Component, TuiMouseEvent, TuiMouseEventResult};
use crate::tui::component_mouse::{ComponentHandle, MouseAction};

pub type MouseRegionHandler = Box<dyn FnMut(&TuiMouseEvent) -> Option<TuiMouseEventResult>>;
pub struct MouseRegion {
    child: ComponentHandle,
    on_mouse: MouseRegionHandler,
}
impl MouseRegion {
    pub fn new(child: ComponentHandle, on_mouse: MouseRegionHandler) -> Self {
        Self { child, on_mouse }
    }
    pub fn child(&self) -> &ComponentHandle {
        &self.child
    }
}
impl Component for MouseRegion {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.child.render(width)
    }
    fn invalidate(&mut self) {
        self.child.invalidate();
    }
    fn mouse_child(&mut self, index: usize) -> Option<ComponentHandle> {
        (index == 0).then(|| self.child.clone())
    }
    fn mouse_action(&mut self, event: &TuiMouseEvent) -> Option<MouseAction> {
        Some(MouseAction::Forward {
            child: self.child.clone(),
            event: event.clone(),
            delegate_focus: false,
            fallback_to_self: true,
        })
    }
    fn handle_mouse(&mut self, event: &TuiMouseEvent) -> Option<TuiMouseEventResult> {
        (self.on_mouse)(event)
    }
}
