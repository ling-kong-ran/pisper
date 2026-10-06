//! Owning `TuiBase` focus and overlay lifecycle (not the legacy screen host).
//!
//! Components, overlay entries, and rendered mouse targets have separate owning
//! identities. A removed handle remains usable: `hide`/`focus` check membership,
//! but upstream `setHidden`/`unfocus` do not. Host effects run synchronously.
//! The host must call [`ComponentFocus::restore_before_input`] after its input
//! filters and before delivering keyboard input. Filtering, input-handler
//! presence, immediate rendering, layout/compositing and OS IO remain host work.
//!
//! Single-threaded like ComponentHandle. Self-reentrant callbacks/visibility
//! predicates and arbitrary JS mutation of the original options object are not
//! supported. Use weak component back-links; cycles in preFocus traversal are
//! detected but cycles in actual component child trees are not supported.
use crate::tui::component_mouse::ComponentHandle;
use crate::tui::component_overlay::{contains_component, ComponentOverlay, OverlayVisibility};
use crate::tui::overlay::OverlayBounds;
use std::cell::RefCell;
use std::collections::HashSet;
use std::fmt;
use std::rc::Rc;

/// Required live host services. No inert default implementations: cursor and
/// render requests are applied now, not returned in a deferred action list.
pub trait ComponentFocusHost {
    fn terminal_columns(&mut self) -> usize;
    fn terminal_rows(&mut self) -> usize;
    fn mounted_roots(&mut self) -> Vec<ComponentHandle>;
    fn hide_cursor(&mut self);
    fn request_render(&mut self);
}

#[derive(Default)]
pub struct ComponentOverlayOptions {
    pub non_capturing: bool,
    pub visible: Option<OverlayVisibility>,
}

struct Entry {
    mouse: ComponentOverlay,
    pre_focus: Option<ComponentHandle>,
    non_capturing: bool,
    focus_order: f64,
    bounds: Option<OverlayBounds>,
}

/// Owning entry identity, not component identity. Clones address the same entry.
/// Methods with host effects live on ComponentFocus and require this handle.
#[derive(Clone)]
pub struct ComponentOverlayHandle {
    owner: Rc<()>,
    entry: Rc<RefCell<Entry>>,
}
impl ComponentOverlayHandle {
    pub fn component(&self) -> ComponentHandle {
        self.entry.borrow().mouse.component.clone()
    }
    pub fn pre_focus(&self) -> Option<ComponentHandle> {
        self.entry.borrow().pre_focus.clone()
    }
    pub fn is_hidden(&self) -> bool {
        self.entry.borrow().mouse.hidden
    }
    pub fn non_capturing(&self) -> bool {
        self.entry.borrow().non_capturing
    }
    pub fn focus_order(&self) -> f64 {
        self.entry.borrow().focus_order
    }
    /// Raw last-rendered bounds, without visibility or membership evaluation.
    /// Use ComponentFocus::get_bounds for the upstream handle query.
    pub fn stored_bounds(&self) -> Option<OverlayBounds> {
        self.entry.borrow().bounds
    }
}
impl PartialEq for ComponentOverlayHandle {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.entry, &other.entry)
    }
}
impl Eq for ComponentOverlayHandle {}
impl fmt::Debug for ComponentOverlayHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ComponentOverlayHandle")
            .field("identity", &Rc::as_ptr(&self.entry))
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OverlayFocusResume {
    RestoreOverlay,
    /// None is an explicit null target, not missing unfocus options.
    FocusTarget(Option<ComponentHandle>),
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum OverlayFocusRestore {
    #[default]
    Inactive,
    Eligible {
        overlay: ComponentOverlayHandle,
    },
    Blocked {
        overlay: ComponentOverlayHandle,
        blocked_by: ComponentHandle,
        resume: OverlayFocusResume,
    },
}
impl OverlayFocusRestore {
    fn overlay(&self) -> Option<&ComponentOverlayHandle> {
        match self {
            Self::Inactive => None,
            Self::Eligible { overlay } | Self::Blocked { overlay, .. } => Some(overlay),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum OverlayUnfocusTarget {
    /// `handle.unfocus()` with no options.
    #[default]
    Fallback,
    /// `handle.unfocus({target})`, including an explicit null target.
    Target(Option<ComponentHandle>),
}

/// Rust-only misuse: JS handles close over their originating TUI implicitly.
/// Rejected before callbacks/effects, without conflating it with a stale entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OverlayOwnerMismatch;
impl fmt::Display for OverlayOwnerMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("overlay handle belongs to a different focus controller")
    }
}
impl std::error::Error for OverlayOwnerMismatch {}

pub struct ComponentFocus {
    owner: Rc<()>,
    focused: Option<ComponentHandle>,
    overlays: Vec<ComponentOverlayHandle>,
    restore: OverlayFocusRestore,
    // Use JS-number increment/comparison behavior, including ties at 2^53.
    focus_order: f64,
}
impl Default for ComponentFocus {
    fn default() -> Self {
        Self {
            owner: Rc::new(()),
            focused: None,
            overlays: Vec::new(),
            restore: OverlayFocusRestore::Inactive,
            focus_order: 0.0,
        }
    }
}
impl ComponentFocus {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn focused_component(&self) -> Option<ComponentHandle> {
        self.focused.clone()
    }
    /// Insertion order, not compositor visual order.
    pub fn overlays(&self) -> &[ComponentOverlayHandle] {
        &self.overlays
    }
    /// Raw saved restore state. Temporary invisibility does NOT erase it.
    pub fn restore_state(&self) -> &OverlayFocusRestore {
        &self.restore
    }
    pub fn focus_order_counter(&self) -> f64 {
        self.focus_order
    }
    fn check_owner(&self, handle: &ComponentOverlayHandle) -> Result<(), OverlayOwnerMismatch> {
        if Rc::ptr_eq(&self.owner, &handle.owner) {
            Ok(())
        } else {
            Err(OverlayOwnerMismatch)
        }
    }
    fn contains(&self, handle: &ComponentOverlayHandle) -> bool {
        self.overlays.contains(handle)
    }
    fn is_visible(handle: &ComponentOverlayHandle, host: &mut impl ComponentFocusHost) -> bool {
        let mut entry = handle.entry.borrow_mut();
        if entry.mouse.hidden {
            return false;
        }
        if entry.mouse.visible.is_none() {
            return true;
        }
        // Do not even read the dimensions when hidden or without a predicate.
        let columns = host.terminal_columns();
        let rows = host.terminal_rows();
        entry.mouse.is_visible(columns, rows)
    }
    fn first_visible_for(
        &self,
        component: Option<&ComponentHandle>,
        host: &mut impl ComponentFocusHost,
    ) -> Option<ComponentOverlayHandle> {
        let component = component?;
        self.overlays
            .iter()
            .find(|overlay| overlay.component() == *component && Self::is_visible(overlay, host))
            .cloned()
    }
    fn visible_restore(&self, host: &mut impl ComponentFocusHost) -> OverlayFocusRestore {
        let Some(overlay) = self.restore.overlay() else {
            return OverlayFocusRestore::Inactive;
        };
        if !self.contains(overlay) || !Self::is_visible(overlay, host) {
            return OverlayFocusRestore::Inactive;
        }
        self.restore.clone()
    }
    fn clear_restore_for(&mut self, overlay: &ComponentOverlayHandle) {
        if self.restore.overlay() == Some(overlay) {
            self.restore = OverlayFocusRestore::Inactive;
        }
    }
    fn resolve_resume(
        &mut self,
        overlay: &ComponentOverlayHandle,
        resume: &OverlayFocusResume,
    ) -> Option<ComponentHandle> {
        match resume {
            OverlayFocusResume::RestoreOverlay => Some(overlay.component()),
            OverlayFocusResume::FocusTarget(target) => {
                self.restore = OverlayFocusRestore::Inactive;
                target.clone()
            }
        }
    }
    fn is_ancestor(&self, overlay: &ComponentOverlayHandle, component: &ComponentHandle) -> bool {
        let mut visited = HashSet::new();
        let mut current = overlay.pre_focus();
        while let Some(item) = current {
            if !visited.insert(item.id()) {
                break;
            }
            if item == *component {
                return true;
            }
            current = self
                .overlays
                .iter()
                .find(|overlay| overlay.component() == item)
                .and_then(ComponentOverlayHandle::pre_focus);
        }
        false
    }
    fn is_mounted(component: &ComponentHandle, host: &mut impl ComponentFocusHost) -> bool {
        host.mounted_roots()
            .iter()
            .any(|root| contains_component(root, component))
    }
    fn retarget_pre_focus(&mut self, removed: &ComponentOverlayHandle) {
        let component = removed.component();
        let pre_focus = removed.pre_focus();
        for overlay in &self.overlays {
            if overlay != removed && overlay.pre_focus().as_ref() == Some(&component) {
                overlay.entry.borrow_mut().pre_focus = pre_focus.clone();
            }
        }
    }
    fn topmost_visible(
        &self,
        host: &mut impl ComponentFocusHost,
    ) -> Option<ComponentOverlayHandle> {
        let mut topmost: Option<ComponentOverlayHandle> = None;
        for overlay in &self.overlays {
            if overlay.non_capturing() || !Self::is_visible(overlay, host) {
                continue;
            }
            if topmost
                .as_ref()
                .is_none_or(|top| overlay.focus_order() > top.focus_order())
            {
                topmost = Some(overlay.clone());
            }
        }
        topmost
    }
    pub fn set_focus(
        &mut self,
        component: Option<ComponentHandle>,
        host: &mut impl ComponentFocusHost,
    ) {
        self.set_focus_internal(component, false, host);
    }
    fn set_focus_internal(
        &mut self,
        component: Option<ComponentHandle>,
        preserve_restore: bool,
        host: &mut impl ComponentFocusHost,
    ) {
        let previous = self.focused.clone();
        let mut next = component;
        let previous_overlay = self.first_visible_for(previous.as_ref(), host);
        let next_is_overlay = next.as_ref().is_some_and(|next| {
            self.overlays
                .iter()
                .any(|overlay| overlay.component() == *next)
        });
        let restore = self.visible_restore(host);
        if let Some(target) = next.as_ref().filter(|_| !next_is_overlay).cloned() {
            match &restore {
                OverlayFocusRestore::Blocked {
                    overlay,
                    blocked_by,
                    resume,
                } if Some(blocked_by) == previous.as_ref() => {
                    if matches!(resume, OverlayFocusResume::FocusTarget(_))
                        || !Self::is_mounted(blocked_by, host)
                    {
                        next = self.resolve_resume(overlay, resume);
                    } else {
                        self.restore = OverlayFocusRestore::Blocked {
                            overlay: overlay.clone(),
                            blocked_by: target,
                            resume: resume.clone(),
                        };
                    }
                }
                _ => {
                    if let Some(overlay) = previous_overlay {
                        if restore.overlay() == Some(&overlay)
                            && !self.is_ancestor(&overlay, &target)
                        {
                            self.restore = OverlayFocusRestore::Blocked {
                                overlay,
                                blocked_by: target,
                                resume: OverlayFocusResume::RestoreOverlay,
                            };
                        }
                    }
                }
            }
        } else if next.is_none() {
            if let OverlayFocusRestore::Blocked {
                overlay,
                blocked_by,
                resume,
            } = &restore
            {
                if Some(blocked_by) == previous.as_ref() {
                    next = self.resolve_resume(overlay, resume);
                } else if !preserve_restore {
                    self.restore = OverlayFocusRestore::Inactive;
                }
            } else if !preserve_restore {
                self.restore = OverlayFocusRestore::Inactive;
            }
        }
        // Same-target calls still run BOTH setters, in this exact order.
        if let Some(old) = &self.focused {
            old.with_mut(|component| {
                if component.is_focusable() {
                    component.set_focused(false);
                }
            });
        }
        self.focused = next;
        if let Some(next) = &self.focused {
            next.with_mut(|component| {
                if component.is_focusable() {
                    component.set_focused(true);
                }
            });
        }
        if let Some(overlay) = self.first_visible_for(self.focused.as_ref(), host) {
            self.restore = OverlayFocusRestore::Eligible { overlay };
        }
    }
    pub fn show_overlay(
        &mut self,
        component: ComponentHandle,
        options: ComponentOverlayOptions,
        host: &mut impl ComponentFocusHost,
    ) -> ComponentOverlayHandle {
        self.focus_order += 1.0;
        let overlay = ComponentOverlayHandle {
            owner: self.owner.clone(),
            entry: Rc::new(RefCell::new(Entry {
                mouse: ComponentOverlay {
                    component: component.clone(),
                    hidden: false,
                    visible: options.visible,
                },
                pre_focus: self.focused.clone(),
                non_capturing: options.non_capturing,
                focus_order: self.focus_order,
                bounds: None,
            })),
        };
        self.overlays.push(overlay.clone());
        if !options.non_capturing && Self::is_visible(&overlay, host) {
            self.set_focus(Some(component), host);
        }
        host.hide_cursor();
        host.request_render();
        overlay
    }
    pub fn hide(
        &mut self,
        overlay: &ComponentOverlayHandle,
        host: &mut impl ComponentFocusHost,
    ) -> Result<(), OverlayOwnerMismatch> {
        self.check_owner(overlay)?;
        let Some(index) = self.overlays.iter().position(|entry| entry == overlay) else {
            return Ok(());
        };
        self.clear_restore_for(overlay);
        self.retarget_pre_focus(overlay);
        self.overlays.remove(index);
        if self.focused.as_ref() == Some(&overlay.component()) {
            let target = self
                .topmost_visible(host)
                .map(|entry| entry.component())
                .or_else(|| overlay.pre_focus());
            self.set_focus(target, host);
        }
        if self.overlays.is_empty() {
            host.hide_cursor();
        }
        host.request_render();
        Ok(())
    }
    /// Removes the insertion-stack last entry, not the highest focus order.
    pub fn hide_overlay(&mut self, host: &mut impl ComponentFocusHost) {
        if let Some(overlay) = self.overlays.last().cloned() {
            self.hide(&overlay, host).expect("internal overlay owner");
        }
    }
    pub fn set_hidden(
        &mut self,
        overlay: &ComponentOverlayHandle,
        hidden: bool,
        host: &mut impl ComponentFocusHost,
    ) -> Result<(), OverlayOwnerMismatch> {
        self.check_owner(overlay)?;
        if overlay.is_hidden() == hidden {
            return Ok(());
        }
        overlay.entry.borrow_mut().mouse.hidden = hidden;
        if hidden {
            self.clear_restore_for(overlay);
            if self.focused.as_ref() == Some(&overlay.component()) {
                let target = self
                    .topmost_visible(host)
                    .map(|entry| entry.component())
                    .or_else(|| overlay.pre_focus());
                self.set_focus(target, host);
            }
        } else if !overlay.non_capturing() && Self::is_visible(overlay, host) {
            self.focus_order += 1.0;
            overlay.entry.borrow_mut().focus_order = self.focus_order;
            self.set_focus(Some(overlay.component()), host);
        }
        host.request_render();
        Ok(())
    }
    pub fn focus(
        &mut self,
        overlay: &ComponentOverlayHandle,
        host: &mut impl ComponentFocusHost,
    ) -> Result<(), OverlayOwnerMismatch> {
        self.check_owner(overlay)?;
        if !self.contains(overlay) || !Self::is_visible(overlay, host) {
            return Ok(());
        }
        self.focus_order += 1.0;
        overlay.entry.borrow_mut().focus_order = self.focus_order;
        self.set_focus(Some(overlay.component()), host);
        host.request_render();
        Ok(())
    }
    pub fn unfocus(
        &mut self,
        overlay: &ComponentOverlayHandle,
        target: OverlayUnfocusTarget,
        host: &mut impl ComponentFocusHost,
    ) -> Result<(), OverlayOwnerMismatch> {
        self.check_owner(overlay)?;
        let is_focused = self.focused.as_ref() == Some(&overlay.component());
        let restore = self.restore.clone();
        let pending = restore.overlay() == Some(overlay);
        if !is_focused && !pending {
            return Ok(());
        }
        if let OverlayFocusRestore::Blocked {
            overlay: entry,
            blocked_by,
            ..
        } = &restore
        {
            if entry == overlay && self.focused.as_ref() == Some(blocked_by) {
                self.restore = match target {
                    OverlayUnfocusTarget::Target(target) => OverlayFocusRestore::Blocked {
                        overlay: overlay.clone(),
                        blocked_by: blocked_by.clone(),
                        resume: OverlayFocusResume::FocusTarget(target),
                    },
                    OverlayUnfocusTarget::Fallback => OverlayFocusRestore::Inactive,
                };
                host.request_render();
                return Ok(());
            }
        }
        self.clear_restore_for(overlay);
        if is_focused || matches!(target, OverlayUnfocusTarget::Target(_)) {
            // Upstream evaluates fallback/visibility even for an explicit target.
            let fallback = self
                .topmost_visible(host)
                .filter(|top| top != overlay)
                .map(|top| top.component())
                .or_else(|| overlay.pre_focus());
            self.set_focus(
                match target {
                    OverlayUnfocusTarget::Fallback => fallback,
                    OverlayUnfocusTarget::Target(target) => target,
                },
                host,
            );
        }
        host.request_render();
        Ok(())
    }
    pub fn is_focused(
        &self,
        overlay: &ComponentOverlayHandle,
    ) -> Result<bool, OverlayOwnerMismatch> {
        self.check_owner(overlay)?;
        Ok(self.focused.as_ref() == Some(&overlay.component()))
    }
    /// Renderer seam: save a value copy; queries still enforce membership and
    /// visibility. Does not render, compute bounds, or rebuild mouse frames.
    pub fn set_rendered_bounds(
        &mut self,
        overlay: &ComponentOverlayHandle,
        bounds: Option<OverlayBounds>,
    ) -> Result<(), OverlayOwnerMismatch> {
        self.check_owner(overlay)?;
        overlay.entry.borrow_mut().bounds = bounds;
        Ok(())
    }
    pub fn get_bounds(
        &self,
        overlay: &ComponentOverlayHandle,
        host: &mut impl ComponentFocusHost,
    ) -> Result<Option<OverlayBounds>, OverlayOwnerMismatch> {
        self.check_owner(overlay)?;
        Ok(
            if self.contains(overlay) && Self::is_visible(overlay, host) {
                overlay.stored_bounds()
            } else {
                None
            },
        )
    }
    pub fn has_overlay(&self, host: &mut impl ComponentFocusHost) -> bool {
        self.overlays
            .iter()
            .any(|overlay| Self::is_visible(overlay, host))
    }
    pub fn is_overlay_focused(&self, host: &mut impl ComponentFocusHost) -> bool {
        self.overlays.iter().any(|overlay| {
            self.focused.as_ref() == Some(&overlay.component()) && Self::is_visible(overlay, host)
        })
    }
    /// Current reverse insertion stack for mouse focus ownership, not visual
    /// focusOrder. Shares ComponentOverlay visibility and structural containment
    /// with component_overlay's slice helper without cloning/moving predicates.
    pub fn resolve_mouse_focus_target(
        &self,
        component: &ComponentHandle,
        host: &mut impl ComponentFocusHost,
    ) -> ComponentHandle {
        for overlay in self.overlays.iter().rev() {
            if Self::is_visible(overlay, host)
                && contains_component(&overlay.component(), component)
            {
                return overlay.component();
            }
        }
        component.clone()
    }
    /// Exactly the focus portion of TuiBase.handleTerminalInput (1042–1068).
    /// Returns an owning target for subsequent input delivery, after releasing
    /// controller borrows. No key-release filtering/handler/render scheduling.
    pub fn restore_before_input(
        &mut self,
        host: &mut impl ComponentFocusHost,
    ) -> Option<ComponentHandle> {
        let focused_overlay = self
            .overlays
            .iter()
            .find(|overlay| self.focused.as_ref() == Some(&overlay.component()))
            .cloned();
        if let Some(overlay) = focused_overlay {
            if !Self::is_visible(&overlay, host) {
                if let Some(top) = self.topmost_visible(host) {
                    self.set_focus(Some(top.component()), host);
                } else {
                    self.set_focus_internal(overlay.pre_focus(), true, host);
                }
            }
        }
        let focus_is_overlay = self
            .overlays
            .iter()
            .any(|overlay| self.focused.as_ref() == Some(&overlay.component()));
        if !focus_is_overlay {
            match self.visible_restore(host) {
                OverlayFocusRestore::Eligible { overlay } => {
                    self.set_focus(Some(overlay.component()), host)
                }
                OverlayFocusRestore::Blocked {
                    overlay,
                    blocked_by,
                    resume,
                } if self.focused.as_ref() != Some(&blocked_by) => {
                    let target = self.resolve_resume(&overlay, &resume);
                    self.set_focus(target, host);
                }
                _ => {}
            }
        }
        self.focused.clone()
    }
}
