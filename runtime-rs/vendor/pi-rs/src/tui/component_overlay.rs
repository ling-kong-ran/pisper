//! Owning mouse-facing overlay helpers from actual `TuiBase`.
//!
//! Current stack visibility/containment decides focus ownership. Last-rendered
//! rectangles decide hits, even after an entry is hidden or removed. These are
//! distinct inputs: do not rebuild saved geometry from the current tree.
//! This is NOT show/hide/focus-restore/compositing or the legacy OS host. A host
//! supplies its current stack and rendered rectangles, then can call these
//! helpers from ComponentGestureHost. Single-threaded, finite cell geometry;
//! arbitrary JS getter/array mutation or cyclic component trees are not modeled.
use crate::tui::component::TuiMouseEvent;
use crate::tui::component_gesture::OverlayMouseDispatch;
use crate::tui::component_mouse::ComponentHandle;
use crate::tui::mouse_dispatch::{retarget_mouse_event, MouseDispatchTarget};

pub type OverlayVisibility = Box<dyn FnMut(usize, usize) -> bool>;

/// Mouse-facing projection of a CURRENT overlay entry. nonCapturing and
/// focusOrder intentionally are not inputs: neither affects these helpers.
pub struct ComponentOverlay {
    pub component: ComponentHandle,
    pub hidden: bool,
    pub visible: Option<OverlayVisibility>,
}
impl ComponentOverlay {
    pub fn new(component: ComponentHandle) -> Self {
        Self {
            component,
            hidden: false,
            visible: None,
        }
    }
    /// Hidden short-circuits the predicate; otherwise evaluate it on EVERY call
    /// with current dimensions, before doing structural containment.
    pub fn is_visible(&mut self, columns: usize, rows: usize) -> bool {
        !self.hidden
            && self
                .visible
                .as_mut()
                .is_none_or(|predicate| predicate(columns, rows))
    }
}

/// Last-rendered terminal-relative rectangle retaining the original component.
/// Its order must be the renderer's visual order, not current insertion order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderedComponentOverlay {
    pub component: ComponentHandle,
    pub row: i64,
    pub col: i64,
    pub width: usize,
    pub height: usize,
}

/// Real upstream containment descends ONLY through Container instances. A
/// MouseRegion's child is a mouse route, not a structural Container child.
/// Children are looked up live (including hidden Stack entries); no layout,
/// render cache, transient path or global registry is used.
pub fn contains_component(root: &ComponentHandle, target: &ComponentHandle) -> bool {
    if root == target {
        return true;
    }
    if !root.with_mut(|component| component.is_container_component()) {
        return false;
    }
    let mut index = 0;
    while let Some(child) = root.with_mut(|component| component.mouse_child(index)) {
        // Release the parent's RefCell borrow before descending.
        if contains_component(&child, target) {
            return true;
        }
        index += 1;
    }
    false
}

/// Most-recent CURRENT visible stack entry containing the requested component.
/// This is focus ownership only, not TuiBase.setFocus/overlay restore policy.
pub fn resolve_mouse_focus_target(
    overlays: &mut [ComponentOverlay],
    component: &ComponentHandle,
    columns: usize,
    rows: usize,
) -> ComponentHandle {
    for overlay in overlays.iter_mut().rev() {
        if overlay.is_visible(columns, rows) && contains_component(&overlay.component, component) {
            return overlay.component.clone();
        }
    }
    component.clone()
}

/// Topmost RENDERED rectangle wins. A declining/missing handler still consumes
/// the hit: never fall through to lower overlays. Current visibility/stack
/// membership are deliberately not checked, exactly as in upstream.
pub fn dispatch_mouse_to_overlay(
    rendered: &[RenderedComponentOverlay],
    event: &TuiMouseEvent,
) -> OverlayMouseDispatch {
    for layout in rendered.iter().rev() {
        let x = i128::from(event.screen_x);
        let y = i128::from(event.screen_y);
        let col = i128::from(layout.col);
        let row = i128::from(layout.row);
        if x < col || x >= col + layout.width as i128 || y < row || y >= row + layout.height as i128
        {
            continue;
        }
        let target = MouseDispatchTarget {
            component: layout.component.clone(),
            origin_x: layout.col,
            origin_y: layout.row,
            width: layout.width,
            height: layout.height,
        };
        let mut result = target
            .component
            .dispatch(&retarget_mouse_event(event, &target));
        if let Some(result) = &mut result {
            if result.result.focus {
                // Keep concrete nested target/geometry/capture unchanged.
                result.focus_target = Some(target.component);
            }
        }
        return OverlayMouseDispatch { hit: true, result };
    }
    OverlayMouseDispatch::default()
}
