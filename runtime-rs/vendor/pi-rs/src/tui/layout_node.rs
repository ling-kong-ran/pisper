//! Mutable, safe trait-object layout discovery; no downcasts or pointer identity.
use crate::tui::component::Component;
use crate::tui::components::scroll_view::ScrollHandle;
use crate::tui::components::stack::{StackAlign, StackEntry, StackKind};
use std::sync::atomic::{AtomicU64, Ordering};

/// Explicit identity for multiple component adapters sharing the same render
/// state. Ordinary uniquely-owned children use their original tree path instead,
/// so distinct zero-sized components never collide in the render cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ComponentCacheId(u64);
impl ComponentCacheId {
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .expect("component identity exhausted");
        Self(id)
    }
}
impl Default for ComponentCacheId {
    fn default() -> Self {
        Self::new()
    }
}

pub enum LayoutNode<'a> {
    Stack {
        kind: StackKind,
        entries: &'a mut [StackEntry],
        gap: f64,
        align: StackAlign,
    },
    Scroll {
        component: &'a mut dyn Component,
        state: ScrollHandle,
    },
}
