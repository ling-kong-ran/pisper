//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Port of `modes/interactive/chat-viewport.ts` (46 lines, sha256
//! `24bbdd06a5e17d6c0631f7de51329eb7bb4ecd4a460dec7b3d593ff9e6219960`):
//! the shared fullscreen transcript + fixed input-dock layout factory
//! ([`create_chat_viewport`]).
//!
//! `chat-viewport.ts` is a pure composition helper over `@earendil-works/pi-tui`
//! primitives; the port mirrors it one-to-one over the already-ported tui
//! widgets (`ScrollView`, `VStack`). Upstream mounts the *same* transcript
//! instance in the root `VStack` and returns it; the port expresses that shared
//! ownership with `ComponentHandle::with_shared`
//! (`Rc<RefCell<…>>`). Upstream spreads
//! `scrollbarTrackStyle`/`scrollbarThumbStyle` into the options only when
//! defined; the Rust [`ScrollViewOptions`] fields are `Option`s, so the
//! conditional spread becomes a plain assignment.

use std::cell::RefCell;
use std::rc::Rc;

use crate::tui::component::Component;
use crate::tui::component_mouse::ComponentHandle;
use crate::tui::components::scroll_view::{
    ScrollStyle, ScrollView, ScrollViewOptions, ScrollViewScrollbar,
};
use crate::tui::components::stack::{StackBasis, StackEntry, StackEntryOptions, VStack};

/// Upstream `ChatViewportOptions`. Every field is a component the factory
/// mounts into the fixed layout.
pub struct ChatViewportOptions {
    pub document: Box<dyn Component>,
    pub pending_messages: Box<dyn Component>,
    pub status: Box<dyn Component>,
    pub editor: Box<dyn Component>,
    pub footer: Box<dyn Component>,
    pub widgets_above: Option<Box<dyn Component>>,
    pub widgets_below: Option<Box<dyn Component>>,
    pub scrollbar: Option<ScrollViewScrollbar>,
    pub scrollbar_track_style: Option<ScrollStyle>,
    pub scrollbar_thumb_style: Option<ScrollStyle>,
}

/// Upstream `ChatViewport` return shape (`root` + `transcript`). The dock is
/// additionally exposed as a shared handle because the port mounts it by value
/// inside the root stack (same instance, shared ownership).
pub struct ChatViewport {
    pub root: VStack,
    /// The transcript mounted inside [`ChatViewport::root`].
    pub transcript: Rc<RefCell<ScrollView>>,
    /// The input dock (second root entry); wired into the interactive shell
    /// in r19+.
    #[allow(dead_code)]
    dock: Rc<RefCell<VStack>>,
}

fn entry(component: Box<dyn Component>, shrink: f64, min_size: f64) -> StackEntry {
    StackEntry::new(
        component,
        StackEntryOptions {
            shrink: Some(shrink),
            min_size: Some(min_size),
            ..StackEntryOptions::default()
        },
    )
}

/// Shared fullscreen transcript and fixed input-dock layout
/// (upstream `createChatViewport`).
pub fn create_chat_viewport(options: ChatViewportOptions) -> ChatViewport {
    let mut scroll_options = ScrollViewOptions {
        follow_end: true, // upstream `follow: "end"`
        primary: true,
        overscroll_contain: true, // upstream `overscroll: "chain"`
        ..ScrollViewOptions::default()
    };
    // `options.scrollbar ?? "auto"` plus the two conditional style spreads.
    scroll_options.scrollbar = Some(options.scrollbar.unwrap_or(ScrollViewScrollbar::Auto));
    scroll_options.scrollbar_track_style = options.scrollbar_track_style;
    scroll_options.scrollbar_thumb_style = options.scrollbar_thumb_style;

    let transcript = ScrollView::new(options.document, scroll_options);
    let (transcript_handle, transcript_shared) = ComponentHandle::with_shared(transcript);

    let mut dock_children = vec![
        entry(options.pending_messages, 1.0, 0.0),
        entry(options.status, 1.0, 0.0),
    ];
    if let Some(widgets_above) = options.widgets_above {
        dock_children.push(entry(widgets_above, 1.0, 0.0));
    }
    // `{ component: options.editor, shrink: 1, minSize: 3 }`
    dock_children.push(entry(options.editor, 1.0, 3.0));
    if let Some(widgets_below) = options.widgets_below {
        dock_children.push(entry(widgets_below, 1.0, 0.0));
    }
    dock_children.push(entry(options.footer, 1.0, 0.0));
    let (dock_handle, dock_shared) =
        ComponentHandle::with_shared(VStack::new(dock_children, Default::default()));

    let root = VStack::new(
        vec![
            // `{ component: transcript, basis: 0, grow: 1, shrink: 1, minSize: 1 }`
            StackEntry::new(
                Box::new(transcript_handle),
                StackEntryOptions {
                    basis: Some(StackBasis::Size(0.0)),
                    grow: Some(1.0),
                    shrink: Some(1.0),
                    min_size: Some(1.0),
                    ..StackEntryOptions::default()
                },
            ),
            // `{ component: dock, basis: "auto", grow: 0, shrink: 1, minSize: 1 }`
            StackEntry::new(
                Box::new(dock_handle),
                StackEntryOptions {
                    grow: Some(0.0),
                    shrink: Some(1.0),
                    min_size: Some(1.0),
                    ..StackEntryOptions::default()
                },
            ),
        ],
        Default::default(),
    );

    ChatViewport {
        root,
        transcript: transcript_shared,
        dock: dock_shared,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixedLines(Vec<&'static str>);
    impl Component for FixedLines {
        fn render(&mut self, _width: usize) -> Vec<String> {
            self.0.iter().map(|s| s.to_string()).collect()
        }
    }

    fn base_options() -> ChatViewportOptions {
        ChatViewportOptions {
            document: Box::new(FixedLines(vec!["doc"])),
            pending_messages: Box::new(FixedLines(vec!["pending"])),
            status: Box::new(FixedLines(vec!["status"])),
            editor: Box::new(FixedLines(vec!["editor"])),
            footer: Box::new(FixedLines(vec!["footer"])),
            widgets_above: None,
            widgets_below: None,
            scrollbar: None,
            scrollbar_track_style: None,
            scrollbar_thumb_style: None,
        }
    }

    /// Oracle: tests/fixtures/interactive_r19_oracle scenario `chat_viewport`
    /// (`full`): transcript options and the full 5-entry dock layout.
    #[test]
    fn full_layout_matches_oracle() {
        let mut opts = base_options();
        opts.widgets_above = Some(Box::new(FixedLines(vec!["above"])));
        opts.widgets_below = Some(Box::new(FixedLines(vec!["below"])));
        opts.scrollbar = Some(ScrollViewScrollbar::Always);
        opts.scrollbar_track_style = Some(std::sync::Arc::new(|t: &str| t.to_string()));
        opts.scrollbar_thumb_style = Some(std::sync::Arc::new(|t: &str| t.to_string()));
        let viewport = create_chat_viewport(opts);

        let transcript = viewport.transcript.borrow();
        assert!(matches!(
            transcript.scrollbar(),
            ScrollViewScrollbar::Always
        ));
        assert!(transcript.is_following_end());

        let mut dock = viewport.dock.borrow_mut();
        let dock_layout = match dock.layout_node_mut() {
            Some(crate::tui::layout_node::LayoutNode::Stack { entries, .. }) => entries
                .iter()
                .map(|e| (e.options.shrink, e.options.min_size))
                .collect::<Vec<_>>(),
            _other => panic!("dock is not a stack"),
        };
        assert_eq!(
            dock_layout,
            vec![
                (Some(1.0), Some(0.0)),
                (Some(1.0), Some(0.0)),
                (Some(1.0), Some(0.0)),
                (Some(1.0), Some(3.0)),
                (Some(1.0), Some(0.0)),
                (Some(1.0), Some(0.0)),
            ],
            "pending/status/above/editor(min 3)/below/footer"
        );
    }

    /// Oracle scenario `chat_viewport` (`minimal`): no widget rows and the
    /// default scrollbar resolves to `auto`.
    #[test]
    fn minimal_layout_matches_oracle() {
        let viewport = create_chat_viewport(base_options());
        let transcript = viewport.transcript.borrow();
        assert!(matches!(transcript.scrollbar(), ScrollViewScrollbar::Auto));
        let mut dock = viewport.dock.borrow_mut();
        let dock_layout = match dock.layout_node_mut() {
            Some(crate::tui::layout_node::LayoutNode::Stack { entries, .. }) => entries
                .iter()
                .map(|e| (e.options.shrink, e.options.min_size))
                .collect::<Vec<_>>(),
            _other => panic!("dock is not a stack"),
        };
        assert_eq!(
            dock_layout,
            vec![
                (Some(1.0), Some(0.0)),
                (Some(1.0), Some(0.0)),
                (Some(1.0), Some(3.0)),
                (Some(1.0), Some(0.0)),
            ]
        );
    }
}
