//! The component interface and mouse-event vocabulary from upstream
//! `packages/tui/src/tui.ts` (lines 21-168) — split out early because every
//! widget depends on it. The `TUI` class itself (differential rendering,
//! overlays, focus) joins in a later slice.
//!
//! Disclosed substitutions:
//! - JS classes implement `Component` structurally; Rust uses a trait.
//!   `render(&mut self)` takes `&mut` because components cache rendered state
//!   and update bookkeeping (e.g. Input's horizontal scroll origin) during
//!   render.
//! - `Focusable` is a JS property-presence check (`"focused" in component`);
//!   the trait expresses it as [`Component::is_focusable`] plus
//!   focused accessor methods, defaulting to non-focusable.

/// Upstream `TuiMouseEventType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TuiMouseEventType {
    Press,
    Release,
    Move,
    Drag,
    Click,
    Wheel,
}

/// Upstream `TuiMouseButton`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TuiMouseButton {
    Left,
    Middle,
    Right,
    None,
}

/// Upstream `TuiMouseEvent`: normalized cell-based mouse event with
/// zero-based coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct TuiMouseEvent {
    pub event_type: TuiMouseEventType,
    pub button: TuiMouseButton,
    /// Signed coordinates local to the receiving component (capture can leave bounds).
    pub x: i64,
    pub y: i64,
    /// Signed absolute terminal coordinates; SGR zero fields decode to -1.
    pub screen_x: i64,
    pub screen_y: i64,
    /// Current component bounds.
    pub width: usize,
    pub height: usize,
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
    /// JS-number logical lines (fractional/non-finite values are preserved).
    /// Negative values scroll up; individual consumers decide truthiness.
    pub wheel_delta: Option<f64>,
    /// Consecutive click count when type is Click.
    pub click_count: Option<u32>,
}

/// Upstream `TuiMouseEventResult`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TuiMouseEventResult {
    /// Stop propagation and suppress renderer-level fallback behavior.
    pub handled: bool,
    /// Route subsequent drag/release events to this component. Implies handled.
    pub capture: bool,
    /// Give keyboard focus to this component. Implies handled.
    pub focus: bool,
    /// Explicitly request or suppress a render.
    pub render: Option<bool>,
}

impl TuiMouseEventResult {
    /// Upstream default render request: press/click/drag/wheel render,
    /// move/release do not.
    pub fn wants_render(&self, event_type: TuiMouseEventType) -> bool {
        match self.render {
            Some(requested) => requested,
            None => matches!(
                event_type,
                TuiMouseEventType::Press
                    | TuiMouseEventType::Click
                    | TuiMouseEventType::Drag
                    | TuiMouseEventType::Wheel
            ),
        }
    }
}

/// Cursor position marker - APC sequence, zero-width; components emit it at
/// the cursor position when focused and the renderer positions the hardware
/// cursor there.
pub const CURSOR_MARKER: &str = "\x1b_pi:c\x07";

/// Upstream `Component` interface.
pub trait Component {
    /// Render the component to lines for the given viewport width.
    fn render(&mut self, width: usize) -> Vec<String>;

    /// Sparse-capable viewport rendering; existing widgets keep their Vec API.
    fn render_layout_lines(&mut self, width: usize) -> crate::tui::rendered_lines::RenderedLines {
        crate::tui::rendered_lines::RenderedLines::dense(self.render(width))
    }

    /// Discover stack/scroll children without unsafe downcasts.
    fn layout_node_mut(&mut self) -> Option<crate::tui::layout_node::LayoutNode<'_>> {
        None
    }

    /// Override only for adapters sharing one logical component. The same ID
    /// promises the same mutable render state, not merely identical content.
    fn layout_cache_id(&self) -> Option<crate::tui::layout_node::ComponentCacheId> {
        None
    }

    /// Owning identity, when this is an adapter installed by ComponentHandle.
    fn component_handle(&self) -> Option<crate::tui::component_mouse::ComponentHandle> {
        None
    }

    /// Install owning adapters for discoverable children on the interactive path.
    fn prepare_mouse_children(&mut self) {}

    /// Current-tree path discovery, not a persistent identity or capture lookup.
    fn mouse_child(
        &mut self,
        index: usize,
    ) -> Option<crate::tui::component_mouse::ComponentHandle> {
        self.prepare_mouse_children();
        match self.layout_node_mut()? {
            crate::tui::layout_node::LayoutNode::Stack { entries, .. } => {
                entries.get(index)?.component.component_handle()
            }
            crate::tui::layout_node::LayoutNode::Scroll { component, .. } if index == 0 => {
                component.component_handle()
            }
            _ => None,
        }
    }

    /// Structural equivalent of `instanceof Container`, independent of mouse
    /// overrides or layout-node discovery. Container subclasses opt in; an
    /// arbitrary wrapper exposing mouse_child (e.g. MouseRegion) does not.
    fn is_container_component(&self) -> bool {
        false
    }

    /// Composite-aware companion to the legacy flag-only mouse handler.
    fn mouse_action(
        &mut self,
        event: &TuiMouseEvent,
    ) -> Option<crate::tui::component_mouse::MouseAction> {
        self.handle_mouse(event)
            .map(crate::tui::component_mouse::MouseAction::Direct)
    }

    /// Explicit equivalent of upstream's inherited Container method identity.
    /// A custom mouse-action override on a layout node must return false here.
    fn uses_container_mouse_handler(&self) -> bool {
        false
    }

    /// Container's optional keyboard handler is present (not Focusable).
    fn delegates_mouse_focus(&self) -> bool {
        false
    }

    /// Optional handler for keyboard input when the component has focus.
    fn handle_input(&mut self, _data: &str) {}

    /// Optional normalized mouse handler.
    fn handle_mouse(&mut self, _event: &TuiMouseEvent) -> Option<TuiMouseEventResult> {
        None
    }

    /// If true, the component receives key release events (Kitty protocol).
    fn wants_key_release(&self) -> bool {
        false
    }

    /// Invalidate any cached rendering state.
    fn invalidate(&mut self) {}

    /// `Focusable` marker: whether this component can receive focus and show
    /// the hardware cursor. Default: not focusable (upstream components
    /// without a `focused` property).
    fn is_focusable(&self) -> bool {
        false
    }

    /// Current focus state (only meaningful for focusable components).
    fn focused(&self) -> bool {
        false
    }

    /// Set by the TUI when focus changes.
    fn set_focused(&mut self, _focused: bool) {}
}

/// Upstream `isFocusable`.
pub fn is_focusable(component: &dyn Component) -> bool {
    component.is_focusable()
}

/// Box adapters preserve every component hook, including sparse rendering and
/// explicit cache identities; they do not silently erase composite behavior.
impl<T: Component + ?Sized> Component for Box<T> {
    fn render(&mut self, width: usize) -> Vec<String> {
        (**self).render(width)
    }
    fn render_layout_lines(&mut self, width: usize) -> crate::tui::rendered_lines::RenderedLines {
        (**self).render_layout_lines(width)
    }
    fn layout_node_mut(&mut self) -> Option<crate::tui::layout_node::LayoutNode<'_>> {
        (**self).layout_node_mut()
    }
    fn layout_cache_id(&self) -> Option<crate::tui::layout_node::ComponentCacheId> {
        (**self).layout_cache_id()
    }
    fn component_handle(&self) -> Option<crate::tui::component_mouse::ComponentHandle> {
        (**self).component_handle()
    }
    fn prepare_mouse_children(&mut self) {
        (**self).prepare_mouse_children();
    }
    fn mouse_child(
        &mut self,
        index: usize,
    ) -> Option<crate::tui::component_mouse::ComponentHandle> {
        (**self).mouse_child(index)
    }
    fn is_container_component(&self) -> bool {
        (**self).is_container_component()
    }
    fn mouse_action(
        &mut self,
        event: &TuiMouseEvent,
    ) -> Option<crate::tui::component_mouse::MouseAction> {
        (**self).mouse_action(event)
    }
    fn uses_container_mouse_handler(&self) -> bool {
        (**self).uses_container_mouse_handler()
    }
    fn delegates_mouse_focus(&self) -> bool {
        (**self).delegates_mouse_focus()
    }
    fn handle_input(&mut self, data: &str) {
        (**self).handle_input(data);
    }
    fn handle_mouse(&mut self, event: &TuiMouseEvent) -> Option<TuiMouseEventResult> {
        (**self).handle_mouse(event)
    }
    fn wants_key_release(&self) -> bool {
        (**self).wants_key_release()
    }
    fn invalidate(&mut self) {
        (**self).invalidate();
    }
    fn is_focusable(&self) -> bool {
        (**self).is_focusable()
    }
    fn focused(&self) -> bool {
        (**self).focused()
    }
    fn set_focused(&mut self, focused: bool) {
        (**self).set_focused(focused);
    }
}
