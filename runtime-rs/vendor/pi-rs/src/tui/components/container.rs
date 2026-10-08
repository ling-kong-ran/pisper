//! Actual Container child forwarding and render-time geometry snapshots.
use crate::tui::component::{Component, TuiMouseEvent};
use crate::tui::component_mouse::{
    container_mouse_action, valid_container_row, ComponentHandle, MouseAction,
};

pub type ContainerInputHandler = Box<dyn FnMut(&str)>;
#[derive(Default)]
pub struct Container {
    /// Live children; snapshots intentionally survive add/remove/clear/invalidate.
    pub children: Vec<ComponentHandle>,
    mouse_layout: Option<(usize, Vec<(ComponentHandle, usize)>)>,
    input: Option<ContainerInputHandler>,
}
impl Container {
    pub fn new(children: Vec<ComponentHandle>) -> Self {
        Self {
            children,
            ..Self::default()
        }
    }
    pub fn add_child(&mut self, child: ComponentHandle) {
        self.children.push(child);
    }
    pub fn remove_child(&mut self, child: &ComponentHandle) -> Option<ComponentHandle> {
        let index = self.children.iter().position(|c| c == child)?;
        Some(self.children.remove(index))
    }
    pub fn clear(&mut self) {
        self.children.clear();
    }
    pub fn set_input_handler(&mut self, input: Option<ContainerInputHandler>) {
        self.input = input;
    }
}
impl Component for Container {
    fn render(&mut self, width: usize) -> Vec<String> {
        let mut lines = Vec::new();
        let mut children = Vec::new();
        for child in &mut self.children {
            let rendered = child.render(width);
            children.push((child.clone(), rendered.len()));
            lines.extend(rendered);
        }
        self.mouse_layout = Some((width, children));
        lines
    }
    fn invalidate(&mut self) {
        for child in &mut self.children {
            child.invalidate();
        }
    }
    fn mouse_child(&mut self, index: usize) -> Option<ComponentHandle> {
        self.children.get(index).cloned()
    }
    fn is_container_component(&self) -> bool {
        true
    }
    fn uses_container_mouse_handler(&self) -> bool {
        true
    }
    fn delegates_mouse_focus(&self) -> bool {
        self.input.is_some()
    }
    fn handle_input(&mut self, data: &str) {
        if let Some(input) = &mut self.input {
            input(data);
        }
    }
    fn mouse_action(&mut self, event: &TuiMouseEvent) -> Option<MouseAction> {
        if !valid_container_row(event) {
            return None;
        }
        if let Some((width, children)) = &self.mouse_layout {
            if *width == event.width {
                return container_mouse_action(children, event);
            }
        }
        // Do not replace the last render snapshot on a width mismatch.
        let children: Vec<_> = self
            .children
            .iter_mut()
            .map(|child| {
                let height = child.render(event.width).len();
                (child.clone(), height)
            })
            .collect();
        container_mouse_action(&children, event)
    }
}
