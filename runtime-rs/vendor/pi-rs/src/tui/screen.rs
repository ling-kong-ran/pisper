//! Port of the `TuiBase` input-dispatch and focus assembly from upstream
//! `packages/tui/src/tui.ts` (handleTerminalInput + Container), feeding the
//! differential frame planner in [`crate::tui::renderer`].
//!
//! Disclosed substitutions:
//! - JS component identity (`this.focusedComponent === component`) becomes an
//!   index into the screen's child list.
//! - Upstream input listeners are a `Set<TuiInputListener>`; the port keeps an
//!   ordered Vec with add/remove returning and taking a listener id.
//! - The Node `process.nextTick` render scheduling becomes a boolean render
//!   request the host loop polls; the 16 ms throttle constant is preserved as
//!   [`MIN_RENDER_INTERVAL_MS`] for the host scheduler.

use crate::tui::component::Component;
use crate::tui::keys::is_key_release;

/// Upstream `MIN_RENDER_INTERVAL_MS` (16 ms frame throttle).
pub const MIN_RENDER_INTERVAL_MS: u64 = 16;

/// Upstream `TuiInputListenerResult`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InputListenerResult {
    /// Stop propagation.
    pub consume: bool,
    /// Replace the data seen by later listeners and the focused component.
    pub data: Option<String>,
}

/// A screen that owns root components and routes input
/// (upstream `TuiBase` minus the Node render scheduling).
pub struct TuiScreen {
    children: Vec<Box<dyn Component>>,
    focused: Option<usize>,
    input_listeners: Vec<(u64, InputListener)>,
    next_listener_id: u64,
    render_requested: bool,
    stopped: bool,
}

type InputListener = Box<dyn FnMut(&str) -> Option<InputListenerResult> + Send>;

impl Default for TuiScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl TuiScreen {
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
            focused: None,
            input_listeners: Vec::new(),
            next_listener_id: 0,
            render_requested: false,
            stopped: false,
        }
    }

    /// Upstream `addChild`.
    pub fn add_child(&mut self, child: Box<dyn Component>) {
        self.children.push(child);
    }

    pub fn children_len(&self) -> usize {
        self.children.len()
    }

    /// Upstream `removeChild` — identity becomes the child index.
    pub fn remove_child(&mut self, index: usize) {
        if index < self.children.len() {
            if self.focused == Some(index) {
                self.focused = None;
            }
            self.children.remove(index);
        }
    }

    pub fn clear_children(&mut self) {
        self.children.clear();
        self.focused = None;
    }

    /// Upstream `setFocus` — focus the child at `index`.
    pub fn set_focus(&mut self, index: Option<usize>) {
        if let Some(index) = index {
            if index >= self.children.len() {
                return;
            }
            if let Some(previous) = self.focused {
                self.children[previous].set_focused(false);
            }
            self.focused = Some(index);
            self.children[index].set_focused(true);
        } else {
            if let Some(previous) = self.focused {
                self.children[previous].set_focused(false);
            }
            self.focused = None;
        }
        self.request_render();
    }

    pub fn focused_index(&self) -> Option<usize> {
        self.focused
    }

    /// Upstream `addInputListener`: returns the listener id for removal.
    pub fn add_input_listener(
        &mut self,
        listener: impl FnMut(&str) -> Option<InputListenerResult> + Send + 'static,
    ) -> u64 {
        let id = self.next_listener_id;
        self.next_listener_id += 1;
        self.input_listeners.push((id, Box::new(listener)));
        id
    }

    pub fn remove_input_listener(&mut self, id: u64) {
        self.input_listeners
            .retain(|(listener_id, _)| *listener_id != id);
    }

    /// Upstream `requestRender`: the host loop polls
    /// [`TuiScreen::take_render_request`].
    pub fn request_render(&mut self) {
        self.render_requested = true;
    }

    pub fn take_render_request(&mut self) -> bool {
        std::mem::take(&mut self.render_requested)
    }

    pub fn stop(&mut self) {
        self.stopped = true;
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped
    }

    /// Render all children (upstream `Container.render`).
    pub fn render(&mut self, width: usize) -> Vec<String> {
        let mut lines = Vec::new();
        for child in &mut self.children {
            lines.extend(child.render(width));
        }
        lines
    }

    /// Upstream `invalidate`.
    pub fn invalidate(&mut self) {
        for child in &mut self.children {
            child.invalidate();
        }
    }

    /// Upstream `handleTerminalInput` core: listener chain, key-release
    /// filtering, focus dispatch and the render request.
    pub fn handle_terminal_input(&mut self, data: &str) -> bool {
        if self.stopped {
            return false;
        }

        let mut current = data.to_string();

        // Input listeners: each may consume the input or rewrite it.
        for (_, listener) in &mut self.input_listeners {
            if let Some(result) = listener(&current) {
                if result.consume {
                    return false;
                }
                if let Some(replacement) = result.data {
                    current = replacement;
                }
            }
        }

        if current.is_empty() {
            return false;
        }

        // Dispatch to the focused component, filtering key releases unless the
        // component opts in.
        if let Some(index) = self.focused {
            let wants_release = self.children[index].wants_key_release();
            if is_key_release(current.as_str()) && !wants_release {
                return false;
            }
            self.children[index].handle_input(current.as_str());
            // Input is latency-sensitive: bypass the throttled timer path.
            self.render_requested = true;
            return true;
        }

        false
    }
}
