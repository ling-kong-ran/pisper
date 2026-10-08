//! Actual-source gesture traces with explicit synchronous host seams.
use crate::tui::component::{
    Component, TuiMouseButton, TuiMouseEvent, TuiMouseEventResult, TuiMouseEventType,
};
use crate::tui::component_gesture::*;
use crate::tui::component_mouse::*;
use crate::tui::components::container::Container;
use crate::tui::layout::{render_layout_frame, LayoutFrame};
use crate::tui::mouse_dispatch::create_mouse_event;
use crate::tui::viewport_mouse::SgrMouseEvent;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::Arc;

type Trace = Rc<RefCell<Vec<Value>>>;
type Objects = Rc<RefCell<HashMap<String, ComponentHandle>>>;
fn kind(s: &str) -> TuiMouseEventType {
    match s {
        "press" => TuiMouseEventType::Press,
        "release" => TuiMouseEventType::Release,
        "click" => TuiMouseEventType::Click,
        "move" => TuiMouseEventType::Move,
        "drag" => TuiMouseEventType::Drag,
        "wheel" => TuiMouseEventType::Wheel,
        _ => panic!("{s}"),
    }
}
fn kind_name(kind: TuiMouseEventType) -> &'static str {
    match kind {
        TuiMouseEventType::Press => "press",
        TuiMouseEventType::Release => "release",
        TuiMouseEventType::Click => "click",
        TuiMouseEventType::Move => "move",
        TuiMouseEventType::Drag => "drag",
        TuiMouseEventType::Wheel => "wheel",
    }
}
fn event_value(e: &TuiMouseEvent) -> Value {
    let button = match e.button {
        TuiMouseButton::Left => "left",
        TuiMouseButton::Right => "right",
        TuiMouseButton::Middle => "middle",
        TuiMouseButton::None => "none",
    };
    let mut v = json!({"type":kind_name(e.event_type),"button":button,"x":e.x,"y":e.y,"screenX":e.screen_x,"screenY":e.screen_y,"width":e.width,"height":e.height,"shift":e.shift,"alt":e.alt,"ctrl":e.ctrl});
    if let Some(n) = e.click_count {
        v["clickCount"] = json!(n);
    }
    if let Some(n) = e.wheel_delta {
        v["wheelDelta"] = json!(n);
    }
    v
}
fn raw(v: &Value) -> SgrMouseEvent {
    SgrMouseEvent {
        button: v["button"].as_i64().unwrap(),
        x: v["x"].as_i64().unwrap(),
        y: v["y"].as_i64().unwrap(),
        release: v["release"].as_bool().unwrap(),
    }
}
fn raw_value(r: SgrMouseEvent) -> Value {
    json!({"button":r.button,"x":r.x,"y":r.y,"release":r.release})
}
fn flags(v: &Value) -> TuiMouseEventResult {
    TuiMouseEventResult {
        handled: v["handled"].as_bool().unwrap_or(false),
        capture: v["capture"].as_bool().unwrap_or(false),
        focus: v["focus"].as_bool().unwrap_or(false),
        render: v["render"].as_bool(),
    }
}
fn target(v: &Value, objects: &HashMap<String, ComponentHandle>) -> ComponentMouseTarget {
    ComponentMouseTarget {
        component: objects[v["id"].as_str().unwrap()].clone(),
        origin_x: v["originX"].as_i64().unwrap(),
        origin_y: v["originY"].as_i64().unwrap(),
        width: v["width"].as_u64().unwrap() as usize,
        height: v["height"].as_u64().unwrap() as usize,
    }
}
struct Probe {
    id: String,
    state: Rc<RefCell<Value>>,
    trace: Trace,
    objects: Weak<RefCell<HashMap<String, ComponentHandle>>>,
}
impl Component for Probe {
    fn render(&mut self, _: usize) -> Vec<String> {
        vec![self.id.clone(), self.id.clone()]
    }
    fn mouse_action(&mut self, e: &TuiMouseEvent) -> Option<MouseAction> {
        self.trace
            .borrow_mut()
            .push(json!({"op":"mouse","id":self.id,"event":event_value(e)}));
        let s = self.state.borrow();
        let v = s["responses"]
            .get(kind_name(e.event_type))
            .unwrap_or(&s["response"]);
        if v.is_null() {
            return None;
        }
        if v.get("forward").is_some() {
            let objects = self.objects.upgrade().unwrap();
            let objects = objects.borrow();
            Some(MouseAction::Dispatched(ComponentMouseResult {
                result: flags(&v["flags"]),
                target: target(&v["forward"], &objects),
                focus_target: v["focusTarget"].as_str().map(|id| objects[id].clone()),
            }))
        } else {
            Some(MouseAction::Direct(flags(v)))
        }
    }
}
struct Host {
    objects: Objects,
    states: HashMap<String, Rc<RefCell<Value>>>,
    root: ComponentHandle,
    container: Rc<RefCell<Container>>,
    frame: Option<LayoutFrame>,
    trace: Trace,
    config: Value,
    columns: usize,
    rows: usize,
    now: i64,
    drag: bool,
    focused: Option<ComponentHandle>,
}
impl Host {
    fn new() -> Self {
        let objects = Rc::new(RefCell::new(HashMap::new()));
        let trace = Rc::new(RefCell::new(Vec::new()));
        let mut states = HashMap::new();
        for id in ["a", "b", "owner"] {
            let state = Rc::new(RefCell::new(
                json!({"response":{"handled":true},"responses":{}}),
            ));
            let c = ComponentHandle::new(Probe {
                id: id.into(),
                state: state.clone(),
                trace: trace.clone(),
                objects: Rc::downgrade(&objects),
            });
            objects.borrow_mut().insert(id.into(), c);
            states.insert(id.into(), state);
        }
        let (root, container) = ComponentHandle::with_shared(Container::new(vec![
            objects.borrow()["a"].clone(),
            objects.borrow()["b"].clone(),
        ]));
        objects.borrow_mut().insert("root".into(), root.clone());
        let mut host = Self {
            objects,
            states,
            root,
            container,
            frame: None,
            trace,
            config: json!({}),
            columns: 12,
            rows: 8,
            now: 1000,
            drag: false,
            focused: None,
        };
        host.layout();
        host
    }
    fn object(&self, id: &str) -> ComponentHandle {
        self.objects.borrow()[id].clone()
    }
    fn name(&self, c: &ComponentHandle) -> String {
        self.objects
            .borrow()
            .iter()
            .find(|(_, v)| *v == c)
            .map_or_else(|| "detached".into(), |(k, _)| k.clone())
    }
    fn record(&self, v: Value) {
        self.trace.borrow_mut().push(v);
    }
    fn raw_hook(&self, op: &str, key: &str, r: SgrMouseEvent) -> bool {
        self.record(json!({"op":op,"raw":raw_value(r)}));
        self.config[key].as_bool().unwrap_or(false)
    }
    fn layout(&mut self) {
        self.frame = Some(render_layout_frame(
            &mut self.root,
            self.columns,
            self.rows,
            Arc::new(|| {}),
        ));
    }
    fn target_value(&self, t: Option<&ComponentMouseTarget>) -> Value {
        t.map_or(Value::Null,|t|json!({"id":self.name(&t.component),"originX":t.origin_x,"originY":t.origin_y,"width":t.width,"height":t.height}))
    }
    fn snapshot(&self, g: &ComponentGesture) -> Value {
        json!({"capture":self.target_value(g.capture()),"pressTarget":self.target_value(g.press_target()),"point":g.press_point().map(|p|json!({"x":p.x,"y":p.y})),"moved":g.press_moved(),"lastClick":g.last_click().map(|c|json!({"id":self.name(&c.component),"timestamp":c.timestamp_ms,"count":c.count,"x":c.x,"y":c.y})),"focused":self.focused.as_ref().map(|c|self.name(c))})
    }
    fn step(&mut self, g: &mut ComponentGesture, op: &Value) -> Value {
        match op["op"].as_str().unwrap() {
            "raw" => g.handle_mouse_event(self, raw(&op["raw"])),
            "response" => {
                *self.states[op["id"].as_str().unwrap()].borrow_mut() = json!({"response":op["response"],"responses":op.get("responses").cloned().unwrap_or(json!({}))})
            }
            "config" => {
                for (k, v) in op["value"].as_object().unwrap() {
                    self.config[k] = v.clone();
                }
                if let Some(v) = op["value"]["drag"].as_bool() {
                    self.drag = v;
                }
            }
            "time" => self.now = op["value"].as_i64().unwrap(),
            "size" => {
                self.columns = op["columns"].as_u64().unwrap() as usize;
                self.rows = op["rows"].as_u64().unwrap() as usize;
            }
            "layout" => self.layout(),
            "remove" => {
                self.container
                    .borrow_mut()
                    .remove_child(&self.object(op["id"].as_str().unwrap()));
            }
            "add" => self
                .container
                .borrow_mut()
                .add_child(self.object(op["id"].as_str().unwrap())),
            "delegate" => {
                self.container
                    .borrow_mut()
                    .set_input_handler(if op["value"].as_bool().unwrap() {
                        Some(Box::new(|_| {}))
                    } else {
                        None
                    })
            }
            "apply" => {
                let e = create_mouse_event(
                    kind(op["type"].as_str().unwrap()),
                    op["button"].as_i64().unwrap_or(0),
                    op["x"].as_i64().unwrap_or(1),
                    op["y"].as_i64().unwrap_or(0),
                    self.columns,
                    self.rows,
                );
                let result = ComponentMouseResult {
                    result: flags(&op["flags"]),
                    target: target(&op["target"], &self.objects.borrow()),
                    focus_target: op["focusTarget"].as_str().map(|id| self.object(id)),
                };
                return json!(g.apply_dispatch_result(self, &e, &result));
            }
            "clear" => g.clear_gesture(),
            "focusOut" => {
                g.on_focus_out();
                self.drag = false;
            }
            "start" => {
                g.on_terminal_start();
                self.drag = false;
            }
            "stop" => {
                g.on_terminal_stop();
                self.drag = false;
            }
            s => panic!("{s}"),
        }
        Value::Null
    }
}
impl ComponentGestureHost for Host {
    fn terminal_size(&self) -> (usize, usize) {
        (self.columns, self.rows)
    }
    fn now_ms(&mut self) -> i64 {
        self.record(json!({"op":"now","value":self.now}));
        self.now
    }
    fn handle_search_mouse_event(&mut self, r: SgrMouseEvent) -> bool {
        self.raw_hook("search", "search", r)
    }
    fn dispatch_mouse_to_overlay(&mut self, e: &TuiMouseEvent) -> OverlayMouseDispatch {
        self.record(json!({"op":"overlay","event":event_value(e)}));
        let o = &self.config["overlay"];
        let result = o
            .get("target")
            .map(|v| target(v, &self.objects.borrow()))
            .and_then(|t| dispatch_mouse_to_target(e, &t));
        OverlayMouseDispatch {
            hit: o["hit"].as_bool().unwrap_or(false),
            result,
        }
    }
    fn handle_scroll_to_end_indicator_mouse_event(&mut self, r: SgrMouseEvent) -> bool {
        self.raw_hook("indicator", "indicator", r)
    }
    fn handle_scrollbar_mouse_event(&mut self, r: SgrMouseEvent) -> bool {
        let handled = self.raw_hook("scrollbar", "scrollbar", r);
        if let Some(d) = self.config["dragAfter"].as_bool() {
            self.drag = d;
        }
        handled
    }
    fn scrollbar_drag_active(&self) -> bool {
        self.drag
    }
    fn update_scrollbar_hover(&mut self, x: i64, y: i64) {
        self.record(json!({"op":"hover","x":x,"y":y}));
    }
    fn stop_scrollbar_hover(&mut self) {
        self.record(json!({"op":"stopHover"}));
    }
    fn dispatch_mouse_to_layout(&mut self, e: &TuiMouseEvent) -> Option<ComponentMouseResult> {
        self.record(json!({"op":"layout","event":event_value(e)}));
        dispatch_mouse_to_layout(self.frame.as_ref(), e)
    }
    fn resolve_mouse_focus_target(&mut self, c: &ComponentHandle) -> ComponentHandle {
        let id = self.name(c);
        self.record(json!({"op":"resolveFocus","id":id}));
        self.config["focusMap"][&id]
            .as_str()
            .map_or_else(|| c.clone(), |id| self.object(id))
    }
    fn focused_component(&self) -> Option<ComponentHandle> {
        self.record(json!({"op":"getFocus"}));
        self.focused.clone()
    }
    fn set_focus(&mut self, c: ComponentHandle) {
        self.record(json!({"op":"setFocus","id":self.name(&c)}));
        self.focused = Some(c);
    }
    fn clear_text_selection(&mut self) {
        self.record(json!({"op":"clearSelection"}));
    }
    fn request_render(&mut self) {
        self.record(json!({"op":"render"}));
    }
    fn handle_right_click_paste(&mut self, r: SgrMouseEvent) -> bool {
        self.raw_hook("paste", "paste", r)
    }
    fn handle_selection_mouse_event(&mut self, r: SgrMouseEvent, _gesture: &mut ComponentGesture) {
        self.record(json!({"op":"selection","raw":raw_value(r)}));
    }
}
fn run_group(group: &str) {
    let fixture: Value =
        serde_json::from_str(include_str!("../component_gesture/fixtures.json")).unwrap();
    let mut steps = 0;
    for case in fixture[group].as_array().unwrap() {
        let mut host = Host::new();
        let mut gesture = ComponentGesture::default();
        for (index, op) in case["ops"].as_array().unwrap().iter().enumerate() {
            let value = host.step(&mut gesture, op);
            let state = host.snapshot(&gesture);
            let trace = std::mem::take(&mut *host.trace.borrow_mut());
            assert_eq!(
                json!({"value":value,"state":state,"trace":trace}),
                case["expected"][index],
                "{group}/{} step {index}: {op}",
                case["name"]
            );
            steps += 1;
        }
    }
    println!(
        "{group}: {} actual-source cases / {steps} state+trace steps",
        fixture[group].as_array().unwrap().len()
    );
}
#[test]
fn actual_gesture_release_click_and_movement() {
    run_group("gestures");
}
#[test]
fn actual_host_routing_and_apply_order() {
    run_group("routes");
}
#[test]
fn actual_click_cycles_and_clock_boundaries() {
    run_group("clicks");
}
#[test]
fn actual_retained_and_forwarded_targets() {
    run_group("retained");
}
#[test]
fn actual_lifecycle_mouse_state_projection() {
    run_group("lifecycle");
}

// Rust ownership contracts beyond the source trace corpus. Probe registries use
// weak back-links so only real frame/gesture/click owners extend lifetime.
fn press_raw() -> SgrMouseEvent {
    SgrMouseEvent {
        button: 0,
        x: 1,
        y: 0,
        release: false,
    }
}
fn detach_a(host: &mut Host) {
    host.container.borrow_mut().clear();
    host.layout(); // Container deliberately keeps stale cache until rendering.
    host.objects.borrow_mut().remove("a");
}
#[test]
fn press_then_click_history_owns_removed_component_until_focus_out_or_start() {
    for start in [false, true] {
        let mut host = Host::new();
        let weak = host.object("a").downgrade();
        let mut g = ComponentGesture::default();
        g.handle_mouse_event(&mut host, press_raw());
        detach_a(&mut host);
        assert!(weak.upgrade().is_some());
        assert!(g.capture().is_none());
        g.handle_mouse_event(
            &mut host,
            SgrMouseEvent {
                release: true,
                ..press_raw()
            },
        );
        assert!(g.press_target().is_none());
        assert!(g.last_click().is_some());
        assert!(weak.upgrade().is_some());
        g.on_terminal_stop();
        assert!(weak.upgrade().is_some());
        if start {
            g.on_terminal_start();
        } else {
            g.on_focus_out();
        }
        assert!(weak.upgrade().is_none());
    }
}
#[test]
fn capture_without_press_point_releases_ownership_without_creating_click_history() {
    let mut host = Host::new();
    let weak = host.object("a").downgrade();
    let mut g = ComponentGesture::default();
    let event = create_mouse_event(TuiMouseEventType::Move, 35, 1, 0, 12, 8);
    let result = ComponentMouseResult {
        result: TuiMouseEventResult {
            capture: true,
            ..Default::default()
        },
        target: ComponentMouseTarget {
            component: host.object("a"),
            origin_x: 3,
            origin_y: 4,
            width: 2,
            height: 5,
        },
        focus_target: None,
    };
    assert!(!g.apply_dispatch_result(&mut host, &event, &result));
    drop(result);
    detach_a(&mut host);
    assert!(weak.upgrade().is_some());
    assert!(g.press_point().is_none());
    g.handle_mouse_event(
        &mut host,
        SgrMouseEvent {
            release: true,
            ..press_raw()
        },
    );
    assert!(g.capture().is_none());
    assert!(g.last_click().is_none());
    assert!(weak.upgrade().is_none());
}
#[test]
fn dropping_active_gesture_does_not_require_a_registry_cleanup() {
    let mut host = Host::new();
    let weak = host.object("a").downgrade();
    let mut g = ComponentGesture::default();
    host.states["a"].borrow_mut()["response"] = json!({"capture":true});
    g.handle_mouse_event(&mut host, press_raw());
    detach_a(&mut host);
    assert!(g.capture().is_some());
    assert!(weak.upgrade().is_some());
    drop(g);
    assert!(weak.upgrade().is_none());
}
#[test]
fn gesture_callback_can_edit_parent_then_removed_leaf_receives_release_and_click() {
    struct Mutator {
        parent: Weak<RefCell<Container>>,
        seen: Rc<RefCell<Vec<TuiMouseEventType>>>,
    }
    impl Component for Mutator {
        fn render(&mut self, _: usize) -> Vec<String> {
            vec!["mouse".into()]
        }
        fn handle_mouse(&mut self, e: &TuiMouseEvent) -> Option<TuiMouseEventResult> {
            self.seen.borrow_mut().push(e.event_type);
            if e.event_type == TuiMouseEventType::Press {
                let parent = self.parent.upgrade().unwrap();
                let mut parent = parent.borrow_mut();
                parent.clear();
                parent.set_input_handler(None);
            }
            Some(TuiMouseEventResult {
                handled: true,
                focus: true,
                capture: true,
                render: Some(false),
            })
        }
    }
    let mut host = Host::new();
    let seen = Rc::new(RefCell::new(Vec::new()));
    let child = ComponentHandle::new(Mutator {
        parent: Rc::downgrade(&host.container),
        seen: seen.clone(),
    });
    host.objects.borrow_mut().insert("a".into(), child.clone());
    {
        let mut parent = host.container.borrow_mut();
        parent.clear();
        parent.add_child(child.clone());
        parent.set_input_handler(Some(Box::new(|_| {})));
    }
    host.layout();
    let mut g = ComponentGesture::default();
    g.handle_mouse_event(&mut host, press_raw());
    assert!(host.container.borrow().children.is_empty());
    assert_eq!(host.focused, Some(child.clone()));
    assert_eq!(g.capture().unwrap().component, child);
    host.layout();
    g.handle_mouse_event(
        &mut host,
        SgrMouseEvent {
            release: true,
            ..press_raw()
        },
    );
    assert_eq!(
        *seen.borrow(),
        [
            TuiMouseEventType::Press,
            TuiMouseEventType::Release,
            TuiMouseEventType::Click
        ]
    );
    assert!(g.capture().is_none());
    assert!(g.press_target().is_none());
    assert!(!host.trace.borrow().iter().any(|e| e["op"] == "render"));
}
