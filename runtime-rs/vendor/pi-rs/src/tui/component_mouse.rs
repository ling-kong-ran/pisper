//! Single-threaded owning component identity and composite mouse routing.
//!
//! Frames/targets retain the exact component, even after removal or path shifts.
//! Use `ComponentHandle::dispatch` for composite results; the old flag-only API
//! cannot express a nested target. This is not yet the screen/gesture event loop.
//! RefCell guards are released before forwarding mouse callbacks to a child.
//! Re-entering the same component while its own callback/render is borrowed is
//! not supported; use weak references for back-links to avoid strong cycles.
use crate::tui::component::{Component, TuiMouseEvent, TuiMouseEventResult};
use crate::tui::layout::{get_layout_boxes_at, LayoutFrame};
use crate::tui::layout_node::ComponentCacheId;
use crate::tui::mouse_dispatch::{
    dispatch_mouse_event, retarget_mouse_event, MouseDispatchResult, MouseDispatchTarget,
    MouseHandlerResult,
};
use crate::tui::rendered_lines::RenderedLines;
use std::cell::RefCell;
use std::collections::HashSet;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::rc::{Rc, Weak};

pub type ComponentMouseResult = MouseDispatchResult<ComponentHandle>;
pub type ComponentMouseTarget = MouseDispatchTarget<ComponentHandle>;

/// A short-borrow action plan. Forwarding happens after releasing the parent.
pub enum MouseAction {
    Direct(TuiMouseEventResult),
    Dispatched(ComponentMouseResult),
    Forward {
        child: ComponentHandle,
        event: TuiMouseEvent,
        /// Container checks its *current* input-handler presence after the child.
        delegate_focus: bool,
        /// MouseRegion invokes its own handler only when the child declines.
        fallback_to_self: bool,
    },
}

#[derive(Clone)]
pub struct ComponentHandle {
    id: ComponentCacheId,
    component: Rc<RefCell<dyn Component>>,
}
impl ComponentHandle {
    pub fn new<C: Component + 'static>(component: C) -> Self {
        Self::with_shared(component).0
    }
    /// The typed reference shares the very same mutable component state.
    pub fn with_shared<C: Component + 'static>(component: C) -> (Self, Rc<RefCell<C>>) {
        let existing = component.component_handle();
        let shared = Rc::new(RefCell::new(component));
        let handle = existing.unwrap_or_else(|| Self {
            id: ComponentCacheId::new(),
            component: shared.clone(),
        });
        (handle, shared)
    }
    /// Keeps an existing wrapper's identity instead of allocating another one.
    pub fn from_box(component: Box<dyn Component>) -> Self {
        component
            .component_handle()
            .unwrap_or_else(|| Self::new(component))
    }
    pub fn id(&self) -> ComponentCacheId {
        self.id
    }
    pub fn downgrade(&self) -> WeakComponentHandle {
        WeakComponentHandle {
            id: self.id,
            component: Rc::downgrade(&self.component),
        }
    }
    pub fn with_mut<R>(&self, f: impl FnOnce(&mut dyn Component) -> R) -> R {
        f(&mut *self.component.borrow_mut())
    }
    /// Resolve only the CURRENT tree, including hidden children. Never use this
    /// to re-resolve a retained frame or captured target after tree mutation.
    pub fn resolve_path(&self, path: &[usize]) -> Option<Self> {
        let mut component = self.clone();
        for &index in path {
            component = component.with_mut(|c| c.mouse_child(index))?;
        }
        Some(component)
    }
    pub fn dispatch(&self, event: &TuiMouseEvent) -> Option<ComponentMouseResult> {
        let action = self.with_mut(|c| c.mouse_action(event))?;
        match action {
            MouseAction::Direct(flags) => dispatch_mouse_event(self.clone(), event, |_| {
                Some(MouseHandlerResult::Direct(flags))
            }),
            MouseAction::Dispatched(result) => Some(result),
            MouseAction::Forward {
                child,
                event: child_event,
                delegate_focus,
                fallback_to_self,
            } => {
                let result = child.dispatch(&child_event);
                if let Some(mut result) = result {
                    if delegate_focus
                        && result.result.focus
                        && self.component.borrow().delegates_mouse_focus()
                    {
                        result.focus_target = Some(self.clone());
                    }
                    Some(result)
                } else if fallback_to_self {
                    let flags = self.with_mut(|c| c.handle_mouse(event))?;
                    dispatch_mouse_event(self.clone(), event, |_| {
                        Some(MouseHandlerResult::Direct(flags))
                    })
                } else {
                    None
                }
            }
        }
    }
}
impl fmt::Debug for ComponentHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ComponentHandle").field(&self.id).finish()
    }
}
impl PartialEq for ComponentHandle {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
impl Eq for ComponentHandle {}
impl Hash for ComponentHandle {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}
#[derive(Clone)]
pub struct WeakComponentHandle {
    id: ComponentCacheId,
    component: Weak<RefCell<dyn Component>>,
}
impl WeakComponentHandle {
    pub fn upgrade(&self) -> Option<ComponentHandle> {
        Some(ComponentHandle {
            id: self.id,
            component: self.component.upgrade()?,
        })
    }
}

impl Component for ComponentHandle {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.with_mut(|c| c.render(width))
    }
    fn render_layout_lines(&mut self, width: usize) -> RenderedLines {
        self.with_mut(|c| c.render_layout_lines(width))
    }
    // A RefMut cannot escape as a LayoutNode. The layout engine unwraps this
    // handle while retaining the guard locally, before inspecting the node.
    fn component_handle(&self) -> Option<ComponentHandle> {
        Some(self.clone())
    }
    fn layout_cache_id(&self) -> Option<ComponentCacheId> {
        self.component.borrow().layout_cache_id().or(Some(self.id))
    }
    fn prepare_mouse_children(&mut self) {
        self.with_mut(|c| c.prepare_mouse_children());
    }
    fn mouse_child(&mut self, index: usize) -> Option<ComponentHandle> {
        self.with_mut(|c| c.mouse_child(index))
    }
    fn mouse_action(&mut self, event: &TuiMouseEvent) -> Option<MouseAction> {
        self.dispatch(event).map(MouseAction::Dispatched)
    }
    fn handle_mouse(&mut self, event: &TuiMouseEvent) -> Option<TuiMouseEventResult> {
        self.dispatch(event).map(|r| r.result)
    }
    fn is_container_component(&self) -> bool {
        self.component.borrow().is_container_component()
    }
    fn uses_container_mouse_handler(&self) -> bool {
        self.component.borrow().uses_container_mouse_handler()
    }
    fn delegates_mouse_focus(&self) -> bool {
        self.component.borrow().delegates_mouse_focus()
    }
    fn handle_input(&mut self, data: &str) {
        self.with_mut(|c| c.handle_input(data));
    }
    fn wants_key_release(&self) -> bool {
        self.component.borrow().wants_key_release()
    }
    fn invalidate(&mut self) {
        self.with_mut(|c| c.invalidate());
    }
    fn is_focusable(&self) -> bool {
        self.component.borrow().is_focusable()
    }
    fn focused(&self) -> bool {
        self.component.borrow().focused()
    }
    fn set_focused(&mut self, focused: bool) {
        self.with_mut(|c| c.set_focused(focused));
    }
}

/// Convert a Box child once, without changing its state or using raw addresses.
/// Rendering bare legacy trees does not require this conversion.
pub(crate) fn ensure_handle(component: &mut Box<dyn Component>) -> ComponentHandle {
    if let Some(handle) = component.component_handle() {
        return handle;
    }
    struct Empty;
    impl Component for Empty {
        fn render(&mut self, _: usize) -> Vec<String> {
            Vec::new()
        }
    }
    let previous = std::mem::replace(component, Box::new(Empty));
    let handle = ComponentHandle::from_box(previous);
    *component = Box::new(handle.clone());
    handle
}

pub(crate) fn valid_container_row(event: &TuiMouseEvent) -> bool {
    event.y >= 0 && (event.y as u128) < event.height as u128
}
/// All children have already been measured, even if the first one is hit.
pub(crate) fn container_mouse_action(
    children: &[(ComponentHandle, usize)],
    event: &TuiMouseEvent,
) -> Option<MouseAction> {
    if !valid_container_row(event) {
        return None;
    }
    let mut child_y = 0_u128;
    for (child, height) in children {
        let end = child_y + *height as u128;
        if (event.y as u128) >= child_y && (event.y as u128) < end {
            return Some(MouseAction::Forward {
                child: child.clone(),
                event: TuiMouseEvent {
                    y: (event.y as u128 - child_y) as i64,
                    height: *height,
                    ..event.clone()
                },
                delegate_focus: true,
                fallback_to_self: false,
            });
        }
        child_y = end;
    }
    None
}

/// Mirrors the actual layout router: visual priority, identity dedup, then skip
/// layout nodes with the inherited Container handler (without marking visited).
/// Borrow-only frames have no owning identities and are intentionally skipped.
pub fn dispatch_mouse_to_layout(
    frame: Option<&LayoutFrame>,
    event: &TuiMouseEvent,
) -> Option<ComponentMouseResult> {
    let mut visited = HashSet::new();
    for b in get_layout_boxes_at(frame?, event.screen_x, event.screen_y) {
        let Some(component) = &b.component else {
            continue;
        };
        if visited.contains(&component.id()) {
            continue;
        }
        let skip = component
            .with_mut(|c| c.uses_container_mouse_handler() && c.layout_node_mut().is_some());
        if skip {
            continue;
        }
        visited.insert(component.id());
        let local = TuiMouseEvent {
            x: event.screen_x.saturating_sub(b.rect.x),
            y: event.screen_y.saturating_sub(b.rect.y),
            width: b.rect.width,
            height: b.rect.height,
            ..event.clone()
        };
        if let Some(result) = component.dispatch(&local) {
            return Some(result);
        }
    }
    None
}
pub fn dispatch_mouse_to_target(
    event: &TuiMouseEvent,
    target: &ComponentMouseTarget,
) -> Option<ComponentMouseResult> {
    target
        .component
        .dispatch(&retarget_mouse_event(event, target))
}
