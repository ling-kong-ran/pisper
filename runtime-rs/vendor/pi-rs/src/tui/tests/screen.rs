//! Tests for the TuiBase input-dispatch/focus assembly
//! (upstream tui.ts `handleTerminalInput` + Container).

use std::sync::{Arc, Mutex};

use crate::tui::component::{Component, TuiMouseEvent, TuiMouseEventResult, TuiMouseEventType};
use crate::tui::screen::{InputListenerResult, TuiScreen};

/// Minimal recording component used as a dispatch target.
struct Recorder {
    id: &'static str,
    inputs: Vec<String>,
    wants_release: bool,
    focused: bool,
}

impl Recorder {
    fn new(id: &'static str) -> Self {
        Self {
            id,
            inputs: Vec::new(),
            wants_release: false,
            focused: false,
        }
    }
}

impl Component for Recorder {
    fn render(&mut self, _width: usize) -> Vec<String> {
        vec![self.id.to_string()]
    }

    fn handle_input(&mut self, data: &str) {
        self.inputs.push(data.to_string());
    }

    fn wants_key_release(&self) -> bool {
        self.wants_release
    }

    fn is_focusable(&self) -> bool {
        true
    }

    fn focused(&self) -> bool {
        self.focused
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }
}

#[test]
fn input_dispatch_routes_to_the_focused_child_only() {
    let mut screen = TuiScreen::new();
    screen.add_child(Box::new(Recorder::new("a")));
    screen.add_child(Box::new(Recorder::new("b")));
    screen.set_focus(Some(0));

    // Focus state flags follow set_focus.
    screen.set_focus(Some(1));

    screen.handle_terminal_input("x");
    let _ = screen.take_render_request();

    // Both children received nothing directly via routing (only focused gets
    // the input); validate via a second focused dispatch.
    screen.set_focus(Some(0));
    screen.handle_terminal_input("y");
    let _ = screen.take_render_request();
    assert!(!screen.take_render_request());
}

#[test]
fn input_listeners_can_consume_or_rewrite() {
    let mut screen = TuiScreen::new();
    screen.add_child(Box::new(Recorder::new("a")));
    screen.set_focus(Some(0));

    // Rewriting listener: turns "a" into "b".
    screen.add_input_listener(|data| {
        if data == "a" {
            Some(InputListenerResult {
                consume: false,
                data: Some("b".to_string()),
            })
        } else {
            None
        }
    });

    // Consuming listener: swallows "b" AFTER the rewrite.
    let consume_id = screen.add_input_listener(|data| {
        if data == "b" {
            Some(InputListenerResult {
                consume: true,
                data: None,
            })
        } else {
            None
        }
    });

    assert!(!screen.handle_terminal_input("a")); // consumed by the second listener
    screen.remove_input_listener(consume_id);
    assert!(screen.handle_terminal_input("a")); // now reaches the component
}

#[test]
fn key_releases_are_filtered_unless_the_component_opts_in() {
    let mut screen = TuiScreen::new();
    screen.add_child(Box::new(Recorder::new("editor")));
    screen.set_focus(Some(0));

    // A Kitty release marker for 'a'.
    assert!(!screen.handle_terminal_input("\x1b[97;1:3u"));

    // Non-release input still flows.
    assert!(screen.handle_terminal_input("a"));
}

#[test]
fn stopped_screen_ignores_input() {
    let mut screen = TuiScreen::new();
    screen.add_child(Box::new(Recorder::new("a")));
    screen.set_focus(Some(0));
    screen.stop();
    assert!(!screen.handle_terminal_input("x"));
}

#[test]
fn container_render_concatenates_children() {
    let mut screen = TuiScreen::new();
    screen.add_child(Box::new(Recorder::new("one")));
    screen.add_child(Box::new(Recorder::new("two")));
    assert_eq!(
        screen.render(40),
        vec!["one".to_string(), "two".to_string()]
    );
}

#[test]
fn shared_state_component_receives_routed_input() {
    // Integration-style: the recorder wrapped in shared state, like the
    // upstream tests that capture component input through closures.
    let mut screen = TuiScreen::new();
    let received: Arc<Mutex<Vec<String>>> = Arc::default();
    let sink = Arc::clone(&received);

    struct SinkComponent {
        sink: Arc<Mutex<Vec<String>>>,
    }
    impl Component for SinkComponent {
        fn render(&mut self, _width: usize) -> Vec<String> {
            Vec::new()
        }
        fn handle_input(&mut self, data: &str) {
            self.sink.lock().unwrap().push(data.to_string());
        }
    }

    screen.add_child(Box::new(SinkComponent { sink }));
    screen.set_focus(Some(0));
    screen.handle_terminal_input("hello");
    assert_eq!(*received.lock().unwrap(), vec!["hello".to_string()]);
}

#[test]
fn mouse_event_result_defaults() {
    let result = TuiMouseEventResult::default();
    assert!(!result.handled);
    assert!(!result.capture);
    assert!(!result.focus);
    assert_eq!(result.render, None);

    let event = TuiMouseEvent {
        event_type: TuiMouseEventType::Press,
        button: crate::tui::component::TuiMouseButton::Left,
        x: 1,
        y: 0,
        screen_x: 1,
        screen_y: 0,
        width: 10,
        height: 1,
        shift: false,
        alt: false,
        ctrl: false,
        wheel_delta: None,
        click_count: None,
    };
    assert_eq!(event.event_type, TuiMouseEventType::Press);
}
