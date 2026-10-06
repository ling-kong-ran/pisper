//! Direct-render ports of stack.ts, h-stack.ts and v-stack.ts.
//!
//! Allocation retains JS number arithmetic, including non-finite helper inputs.
//! The Component boundary remains UTF-8 with usize cell widths; non-finite or
//! unrepresentable *render* dimensions are not a JS-compatible input domain.
//! Legacy child edits use entry indices; interactive owning adapters retain
//! stable identities independent of entry order. Mutable layout-tree discovery is
//! provided for the separate viewport engine; these standalone render methods
//! intentionally keep their different measurement and rendering behavior.
use crate::tui::component::{Component, TuiMouseEvent};
use crate::tui::component_mouse::{
    container_mouse_action, ensure_handle, valid_container_row, ComponentHandle, MouseAction,
};
use crate::tui::overlay::composite_tui_line;
use crate::tui::utils::visible_width;
use std::sync::Arc;

pub const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayoutViewport {
    pub width: usize,
    pub height: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum StackBasis {
    #[default]
    Auto,
    Size(f64),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StackAlign {
    #[default]
    Stretch,
    Start,
    Center,
    End,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StackKind {
    Horizontal,
    Vertical,
}

pub type StackVisibility = Arc<dyn Fn(&LayoutViewport) -> bool + Send + Sync>;

#[derive(Clone, Default)]
pub struct StackEntryOptions {
    pub basis: Option<StackBasis>,
    pub grow: Option<f64>,
    pub shrink: Option<f64>,
    pub min_size: Option<f64>,
    pub max_size: Option<f64>,
    pub visible: Option<StackVisibility>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct StackOptions {
    pub gap: Option<f64>,
    pub align: StackAlign,
}

pub struct StackEntry {
    pub component: Box<dyn Component>,
    pub options: StackEntryOptions,
}

impl StackEntry {
    pub fn new(component: Box<dyn Component>, options: StackEntryOptions) -> Self {
        Self { component, options }
    }
}

impl From<Box<dyn Component>> for StackEntry {
    fn from(component: Box<dyn Component>) -> Self {
        Self::new(component, StackEntryOptions::default())
    }
}

/// Borrowed counterpart of upstream's readonly stack layout-node metadata.
/// Mutable trait-object discovery is available via Component::layout_node_mut.
pub struct StackLayoutNode<'a> {
    pub kind: StackKind,
    pub entries: &'a [StackEntry],
    pub gap: f64,
    pub align: StackAlign,
}

/// Shared storage/lifecycle; the boolean selects horizontal vs vertical rendering.
pub struct Stack<const HORIZONTAL: bool> {
    entries: Vec<StackEntry>,
    gap: f64,
    align: StackAlign,
}

pub type HStack = Stack<true>;
pub type VStack = Stack<false>;

impl<const H: bool> Default for Stack<H> {
    fn default() -> Self {
        Self::new(Vec::new(), StackOptions::default())
    }
}

impl<const H: bool> Stack<H> {
    pub fn new(children: Vec<StackEntry>, options: StackOptions) -> Self {
        let mut stack = Self {
            entries: Vec::new(),
            gap: normalize_size(options.gap, 0.0),
            align: options.align,
        };
        for child in children {
            stack.add_child(child.component, child.options);
        }
        stack
    }

    pub fn add_child(
        &mut self,
        component: Box<dyn Component>,
        mut options: StackEntryOptions,
    ) -> usize {
        options.grow = options.grow.map(|v| normalize_size(Some(v), 0.0));
        options.shrink = options.shrink.map(|v| normalize_size(Some(v), 1.0));
        options.min_size = options.min_size.map(|v| normalize_size(Some(v), 0.0));
        options.max_size = options
            .max_size
            .map(|v| normalize_size(Some(v), MAX_SAFE_INTEGER));
        let index = self.entries.len();
        self.entries.push(StackEntry { component, options });
        index
    }

    /// Legacy index-based editing; use remove_child_handle for object identity.
    pub fn remove_child(&mut self, index: usize) -> Option<StackEntry> {
        (index < self.entries.len()).then(|| self.entries.remove(index))
    }

    /// Remove the first occurrence, preserving aliased siblings like upstream.
    pub fn remove_child_handle(&mut self, handle: &ComponentHandle) -> Option<StackEntry> {
        self.prepare_mouse_children();
        let index = self
            .entries
            .iter()
            .position(|e| e.component.component_handle().as_ref() == Some(handle))?;
        Some(self.entries.remove(index))
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn layout_node(&self) -> StackLayoutNode<'_> {
        StackLayoutNode {
            kind: if H {
                StackKind::Horizontal
            } else {
                StackKind::Vertical
            },
            entries: &self.entries,
            gap: self.gap,
            align: self.align,
        }
    }

    fn visible_indices(&self, width: usize) -> Vec<usize> {
        visible_stack_entries(
            &self.entries,
            &LayoutViewport {
                width,
                height: MAX_SAFE_INTEGER as u64,
            },
        )
    }

    fn render_vertical(&mut self, width: usize) -> Vec<String> {
        let width = width.max(1);
        let indices = self.visible_indices(width);
        let rendered: Vec<_> = indices
            .iter()
            .map(|&i| self.entries[i].component.render(width))
            .collect();
        let options: Vec<_> = indices
            .iter()
            .map(|&i| self.entries[i].options.clone())
            .collect();
        let intrinsic: Vec<_> = rendered.iter().map(|lines| lines.len() as f64).collect();
        let sizes = allocate_stack_sizes(&options, &intrinsic, None, self.gap);
        let mut result = Vec::new();
        for (index, lines) in rendered.into_iter().enumerate() {
            if index > 0 {
                result.extend(std::iter::repeat_n(String::new(), self.gap as usize));
            }
            let size = sizes[index] as usize;
            let mut lines = lines;
            lines.truncate(size);
            lines.resize(size, String::new());
            result.extend(lines);
        }
        result
    }

    fn render_horizontal(&mut self, width: usize) -> Vec<String> {
        let width = width.max(1);
        let indices = self.visible_indices(width);
        if indices.is_empty() {
            return Vec::new();
        }
        // Upstream standalone HStack measures every visible child, even with a
        // fixed basis. The later viewport layout engine has different caching.
        let intrinsic: Vec<_> = indices
            .iter()
            .map(|&i| {
                self.entries[i]
                    .component
                    .render(width)
                    .iter()
                    .map(|line| visible_width(line))
                    .max()
                    .unwrap_or(0) as f64
            })
            .collect();
        let options: Vec<_> = indices
            .iter()
            .map(|&i| self.entries[i].options.clone())
            .collect();
        let sizes = allocate_stack_sizes(&options, &intrinsic, Some(width as f64), self.gap);
        let widths: Vec<_> = sizes.iter().map(|&size| size as usize).collect();
        let rendered: Vec<_> = indices
            .iter()
            .zip(&widths)
            .map(|(&i, &child_width)| {
                if child_width == 0 {
                    Vec::new()
                } else {
                    self.entries[i].component.render(child_width)
                }
            })
            .collect();
        let height = rendered.iter().map(Vec::len).max().unwrap_or(0);
        let mut result = vec![String::new(); height];
        let mut x: usize = 0;
        for (lines, child_width) in rendered.into_iter().zip(widths) {
            let offset = match self.align {
                StackAlign::Center => (height - lines.len()) / 2,
                StackAlign::End => height - lines.len(),
                StackAlign::Stretch | StackAlign::Start => 0,
            };
            for (row, line) in lines.iter().enumerate() {
                let target = row + offset;
                result[target] = composite_tui_line(&result[target], line, x, child_width, width);
            }
            x = x
                .saturating_add(child_width)
                .saturating_add(self.gap as usize);
        }
        result
    }
}

impl<const H: bool> Component for Stack<H> {
    fn prepare_mouse_children(&mut self) {
        for entry in &mut self.entries {
            ensure_handle(&mut entry.component);
        }
    }
    fn is_container_component(&self) -> bool {
        true
    }
    fn uses_container_mouse_handler(&self) -> bool {
        true
    }
    fn mouse_action(&mut self, event: &TuiMouseEvent) -> Option<MouseAction> {
        if !valid_container_row(event) {
            return None;
        }
        self.prepare_mouse_children();
        // Stack.render never sets Container.mouseLayout. Direct dispatch always
        // measures ALL children at event.width, including hidden entries.
        let children: Vec<_> = self
            .entries
            .iter_mut()
            .map(|entry| {
                let height = entry.component.render(event.width).len();
                (
                    entry.component.component_handle().expect("prepared child"),
                    height,
                )
            })
            .collect();
        container_mouse_action(&children, event)
    }
    fn layout_node_mut(&mut self) -> Option<crate::tui::layout_node::LayoutNode<'_>> {
        Some(crate::tui::layout_node::LayoutNode::Stack {
            kind: if H {
                StackKind::Horizontal
            } else {
                StackKind::Vertical
            },
            entries: &mut self.entries,
            gap: self.gap,
            align: self.align,
        })
    }

    fn render(&mut self, width: usize) -> Vec<String> {
        if H {
            self.render_horizontal(width)
        } else {
            self.render_vertical(width)
        }
    }

    fn invalidate(&mut self) {
        // Includes hidden entries, exactly like the inherited Container method.
        for entry in &mut self.entries {
            entry.component.invalidate();
        }
    }
}

/// Visible entry indices retain order and support later mutable child rendering.
pub fn visible_stack_entries(entries: &[StackEntry], viewport: &LayoutViewport) -> Vec<usize> {
    entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            entry
                .options
                .visible
                .as_ref()
                .is_none_or(|visible| visible(viewport))
                .then_some(index)
        })
        .collect()
}

fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.max(b)
    }
}

fn js_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.min(b)
    }
}

fn normalize_size(value: Option<f64>, fallback: f64) -> f64 {
    value
        .filter(|v| v.is_finite())
        .map_or(fallback, |v| js_max(0.0, v.floor()))
}

fn clamp_size(size: f64, entry: &StackEntryOptions) -> f64 {
    let min = js_max(0.0, entry.min_size.unwrap_or(0.0).floor());
    let max = js_max(min, entry.max_size.unwrap_or(MAX_SAFE_INTEGER).floor());
    js_max(min, js_min(max, js_max(0.0, size.floor())))
}

fn distribute(sizes: &mut [f64], entries: &[StackEntryOptions], amount: f64, grow: bool) {
    let mut remaining = amount;
    while remaining > 0.0 {
        let candidates: Vec<_> = entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                let eligible = if grow {
                    entry.grow.unwrap_or(0.0) > 0.0
                        && sizes[index] < entry.max_size.unwrap_or(MAX_SAFE_INTEGER)
                } else {
                    entry.shrink.unwrap_or(1.0) > 0.0
                        && sizes[index] > entry.min_size.unwrap_or(0.0)
                };
                eligible.then_some(index)
            })
            .collect();
        if candidates.is_empty() {
            return;
        }
        let weight = |index: usize, sizes: &[f64]| {
            if grow {
                entries[index].grow.unwrap_or(0.0)
            } else {
                entries[index].shrink.unwrap_or(1.0) * js_max(1.0, sizes[index])
            }
        };
        let total_weight: f64 = candidates.iter().map(|&index| weight(index, sizes)).sum();
        let mut distributed = 0.0;
        for index in candidates {
            if remaining <= 0.0 {
                break;
            }
            let proposed = js_max(
                1.0,
                (remaining * weight(index, sizes) / total_weight).floor(),
            );
            let capacity = if grow {
                entries[index].max_size.unwrap_or(MAX_SAFE_INTEGER) - sizes[index]
            } else {
                sizes[index] - entries[index].min_size.unwrap_or(0.0)
            };
            let delta = js_min(remaining, js_min(proposed, capacity));
            if delta <= 0.0 {
                continue;
            }
            sizes[index] += if grow { delta } else { -delta };
            remaining -= delta;
            distributed += delta;
        }
        if distributed == 0.0 {
            return;
        }
    }
}

/// Exact ordered upstream allocator, not a CSS flexbox substitute. In each
/// round later entries use the updated remainder but the original total weight.
/// This helper does not normalize options; Stack::add_child does that separately.
pub fn allocate_stack_sizes(
    entries: &[StackEntryOptions],
    intrinsic_sizes: &[f64],
    available_size: Option<f64>,
    gap: f64,
) -> Vec<f64> {
    let mut sizes: Vec<_> = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let basis = match entry.basis {
                None | Some(StackBasis::Auto) => intrinsic_sizes.get(index).copied().unwrap_or(0.0),
                Some(StackBasis::Size(value)) => value,
            };
            clamp_size(basis, entry)
        })
        .collect();
    let Some(available_size) = available_size else {
        return sizes;
    };
    let content_size = js_max(
        0.0,
        available_size.floor() - entries.len().saturating_sub(1) as f64 * gap,
    );
    let total: f64 = sizes.iter().sum();
    if total < content_size {
        distribute(&mut sizes, entries, content_size - total, true);
    } else if total > content_size {
        distribute(&mut sizes, entries, total - content_size, false);
    }
    sizes
}
