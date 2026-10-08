//! Actual TuiBase overlay helpers plus real gesture composition; other host
//! features remain explicit controlled seams, not a full screen integration.
use crate::tui::component::{
    Component, TuiMouseButton, TuiMouseEvent, TuiMouseEventResult, TuiMouseEventType,
};
use crate::tui::component_gesture::*;
use crate::tui::component_mouse::*;
use crate::tui::component_overlay;
use crate::tui::component_overlay::{
    contains_component, ComponentOverlay, RenderedComponentOverlay,
};
use crate::tui::components::container::Container;
use crate::tui::components::mouse_region::MouseRegion;
use crate::tui::components::scroll_view::{ScrollView, ScrollViewOptions};
use crate::tui::components::stack::{HStack, StackEntry, StackEntryOptions, StackOptions, VStack};
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

fn event(v: &Value) -> TuiMouseEvent {
    TuiMouseEvent {
        event_type: kind(v["type"].as_str().unwrap()),
        button: match v["button"].as_str().unwrap() {
            "left" => TuiMouseButton::Left,
            "right" => TuiMouseButton::Right,
            "middle" => TuiMouseButton::Middle,
            _ => TuiMouseButton::None,
        },
        x: v["x"].as_i64().unwrap(),
        y: v["y"].as_i64().unwrap(),
        screen_x: v["screenX"].as_i64().unwrap(),
        screen_y: v["screenY"].as_i64().unwrap(),
        width: v["width"].as_u64().unwrap() as usize,
        height: v["height"].as_u64().unwrap() as usize,
        shift: v["shift"].as_bool().unwrap(),
        alt: v["alt"].as_bool().unwrap(),
        ctrl: v["ctrl"].as_bool().unwrap(),
        wheel_delta: v["wheelDelta"].as_f64(),
        click_count: v["clickCount"].as_u64().map(|n| n as u32),
    }
}
struct Probe {
    id: String,
    state: Rc<RefCell<Value>>,
    trace: Trace,
    objects: Weak<RefCell<HashMap<String, ComponentHandle>>>,
    children: Option<Vec<ComponentHandle>>,
    structural: bool,
}
impl Component for Probe {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.children.as_mut().map_or_else(
            || vec![self.id.clone(), self.id.clone()],
            |children| children.iter_mut().flat_map(|c| c.render(width)).collect(),
        )
    }
    fn is_container_component(&self) -> bool {
        self.structural
    }
    fn mouse_child(&mut self, index: usize) -> Option<ComponentHandle> {
        self.children.as_ref()?.get(index).cloned()
    }
    fn mouse_action(&mut self, e: &TuiMouseEvent) -> Option<MouseAction> {
        let s = self.state.borrow();
        if s["noHandler"].as_bool().unwrap_or(false) {
            return None;
        }
        self.trace
            .borrow_mut()
            .push(json!({"op":"mouse","id":self.id,"event":event_value(e)}));
        let v = &s["response"];
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
    containers: HashMap<String, Rc<RefCell<Container>>>,
    overlays: Vec<ComponentOverlay>,
    keys: Vec<String>,
    entries: HashMap<String, ComponentHandle>,
    visible_states: HashMap<String, Rc<RefCell<Value>>>,
    rendered: Vec<RenderedComponentOverlay>,
    saved: HashMap<String, ComponentMouseTarget>,
    trace: Trace,
    columns: usize,
    rows: usize,
    now: i64,
    focused: Option<ComponentHandle>,
}
impl Host {
    fn new(nodes: &Value) -> Self {
        let objects: Objects = Rc::new(RefCell::new(HashMap::new()));
        let trace = Rc::new(RefCell::new(Vec::new()));
        let mut states = HashMap::new();
        let mut containers = HashMap::new();
        for spec in nodes.as_array().unwrap() {
            let id = spec["id"].as_str().unwrap().to_owned();
            let state = Rc::new(RefCell::new(spec.clone()));
            states.insert(id.clone(), state.clone());
            let get = |s: &str| objects.borrow()[s].clone();
            let children = || {
                spec["children"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|s| get(s.as_str().unwrap()))
                    .collect::<Vec<_>>()
            };
            let probe = |children, structural| Probe {
                id: id.clone(),
                state: state.clone(),
                trace: trace.clone(),
                objects: Rc::downgrade(&objects),
                children,
                structural,
            };
            let h = match spec["kind"].as_str().unwrap() {
                "leaf" => ComponentHandle::new(probe(None, false)),
                "foreign" => ComponentHandle::new(probe(Some(children()), false)),
                "container" if spec["override"].as_bool().unwrap_or(false) => {
                    ComponentHandle::new(probe(Some(children()), true))
                }
                "container" => {
                    let mut c = Container::default();
                    for child in children() {
                        c.add_child(child);
                    }
                    if spec["input"].as_bool().unwrap_or(false) {
                        c.set_input_handler(Some(Box::new(|_| {})));
                    }
                    let (h, c) = ComponentHandle::with_shared(c);
                    containers.insert(id.clone(), c);
                    h
                }
                "region" => {
                    let trace = trace.clone();
                    let id = id.clone();
                    ComponentHandle::new(MouseRegion::new(
                        get(spec["child"].as_str().unwrap()),
                        Box::new(move |e| {
                            trace
                                .borrow_mut()
                                .push(json!({"op":"mouse","id":id,"event":event_value(e)}));
                            let s = state.borrow();
                            (!s["response"].is_null()).then(|| flags(&s["response"]))
                        }),
                    ))
                }
                "scroll" => ComponentHandle::new(ScrollView::new(
                    Box::new(get(spec["child"].as_str().unwrap())),
                    ScrollViewOptions::default(),
                )),
                name @ ("hstack" | "vstack") => {
                    let entries=children().into_iter().map(|h|StackEntry::new(Box::new(h),StackEntryOptions{visible:spec["hiddenChild"].as_bool().unwrap_or(false).then(||Arc::new(|_:&crate::tui::components::stack::LayoutViewport|false) as crate::tui::components::stack::StackVisibility),..Default::default()})).collect();
                    let options = StackOptions {
                        gap: Some(0.0),
                        ..Default::default()
                    };
                    if name == "hstack" {
                        ComponentHandle::new(HStack::new(entries, options))
                    } else {
                        ComponentHandle::new(VStack::new(entries, options))
                    }
                }
                x => panic!("{x}"),
            };
            objects.borrow_mut().insert(id, h);
        }
        Self {
            objects,
            states,
            containers,
            overlays: Vec::new(),
            keys: Vec::new(),
            entries: HashMap::new(),
            visible_states: HashMap::new(),
            rendered: Vec::new(),
            saved: HashMap::new(),
            trace,
            columns: 30,
            rows: 20,
            now: 1000,
            focused: None,
        }
    }
    fn object(&self, id: &str) -> ComponentHandle {
        self.objects.borrow()[id].clone()
    }
    fn name(&self, c: &ComponentHandle) -> String {
        self.objects
            .borrow()
            .iter()
            .find(|(_, v)| *v == c)
            .unwrap()
            .0
            .clone()
    }
    fn record(&self, v: Value) {
        self.trace.borrow_mut().push(v);
    }
    fn target_value(&self, t: Option<&ComponentMouseTarget>) -> Value {
        t.map_or(Value::Null,|t|json!({"id":self.name(&t.component),"originX":t.origin_x,"originY":t.origin_y,"width":t.width,"height":t.height}))
    }
    fn result_value(&self, r: Option<&ComponentMouseResult>) -> Value {
        r.map_or(Value::Null,|r|json!({"handled":r.result.handled,"capture":r.result.capture,"focus":r.result.focus,"render":r.result.render,"target":self.target_value(Some(&r.target)),"focusTarget":r.focus_target.as_ref().map(|c|self.name(c))}))
    }
    fn snapshot(&self, g: &ComponentGesture) -> Value {
        json!({"capture":self.target_value(g.capture()),"pressTarget":self.target_value(g.press_target()),"point":g.press_point().map(|p|json!({"x":p.x,"y":p.y})),"moved":g.press_moved(),"lastClick":g.last_click().map(|c|json!({"id":self.name(&c.component),"timestamp":c.timestamp_ms,"count":c.count,"x":c.x,"y":c.y})),"focused":self.focused.as_ref().map(|c|self.name(c))})
    }
    fn step(&mut self, g: &mut ComponentGesture, op: &Value) -> Value {
        let id = op["id"].as_str().unwrap_or("");
        let key = op["key"].as_str().unwrap_or("");
        match op["op"].as_str().unwrap() {
            "stack" => {
                self.overlays.clear();
                self.keys.clear();
                for s in op["entries"].as_array().unwrap() {
                    let key = s["key"].as_str().unwrap().to_owned();
                    let mut e =
                        ComponentOverlay::new(self.object(s["component"].as_str().unwrap()));
                    e.hidden = s["hidden"].as_bool().unwrap_or(false);
                    let state = Rc::new(RefCell::new(s.clone()));
                    self.visible_states.insert(key.clone(), state.clone());
                    self.entries.insert(key.clone(), e.component.clone());
                    self.keys.push(key.clone());
                    if s.get("visible").is_some() {
                        let trace = self.trace.clone();
                        e.visible = Some(Box::new(move |columns, rows| {
                            trace.borrow_mut().push(
                                json!({"op":"visible","key":key,"columns":columns,"rows":rows}),
                            );
                            let s = state.borrow();
                            let v = &s["visible"];
                            v.as_bool().unwrap_or(false)
                                || (v.is_object()
                                    && columns >= v["columns"].as_u64().unwrap_or(0) as usize
                                    && rows >= v["rows"].as_u64().unwrap_or(0) as usize)
                        }));
                    }
                    self.overlays.push(e);
                }
            }
            "frame" => {
                self.rendered = op["layouts"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|x| RenderedComponentOverlay {
                        component: self.entries[x["key"].as_str().unwrap()].clone(),
                        row: x["row"].as_i64().unwrap(),
                        col: x["col"].as_i64().unwrap(),
                        width: x["width"].as_u64().unwrap() as usize,
                        height: x["height"].as_u64().unwrap() as usize,
                    })
                    .collect();
            }
            "contains" => {
                return json!(contains_component(
                    &self.object(op["root"].as_str().unwrap()),
                    &self.object(id)
                ))
            }
            "owner" => {
                let c = self.resolve_mouse_focus_target(&self.object(id));
                return json!(self.name(&c));
            }
            "hit" => {
                let r = self.dispatch_mouse_to_overlay(&event(&op["event"]));
                if let (Some(save), Some(result)) = (op["save"].as_str(), r.result.as_ref()) {
                    self.saved.insert(save.to_owned(), result.target.clone());
                }
                return json!({"hit":r.hit,"result":self.result_value(r.result.as_ref())});
            }
            "target" => {
                return self.result_value(
                    dispatch_mouse_to_target(
                        &event(&op["event"]),
                        &self.saved[op["saved"].as_str().unwrap()],
                    )
                    .as_ref(),
                )
            }
            "render" => {
                return json!(self
                    .object(id)
                    .render(op["width"].as_u64().unwrap() as usize))
            }
            "hidden" => {
                let i = self.keys.iter().position(|x| x == key).unwrap();
                self.overlays[i].hidden = op["value"].as_bool().unwrap();
            }
            "visibility" => self.visible_states[key].borrow_mut()["visible"] = op["value"].clone(),
            "drop" => {
                let i = self.keys.iter().position(|x| x == key).unwrap();
                self.keys.remove(i);
                self.overlays.remove(i);
            }
            "reverse" => {
                self.keys.reverse();
                self.overlays.reverse();
            }
            "frameReverse" => self.rendered.reverse(),
            "remove" => {
                self.containers[op["root"].as_str().unwrap()]
                    .borrow_mut()
                    .remove_child(&self.object(id));
            }
            "add" => self.containers[op["root"].as_str().unwrap()]
                .borrow_mut()
                .add_child(self.object(id)),
            "response" => self.states[id].borrow_mut()["response"] = op["response"].clone(),
            "size" => {
                self.columns = op["columns"].as_u64().unwrap() as usize;
                self.rows = op["rows"].as_u64().unwrap() as usize;
            }
            "raw" => g.handle_mouse_event(self, raw(&op["raw"])),
            "time" => self.now = op["value"].as_i64().unwrap(),
            x => panic!("{x}"),
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
        self.record(json!({"op":"search","raw":raw_value(r)}));
        false
    }
    fn dispatch_mouse_to_overlay(&mut self, e: &TuiMouseEvent) -> OverlayMouseDispatch {
        self.record(json!({"op":"overlay","event":event_value(e)}));
        component_overlay::dispatch_mouse_to_overlay(&self.rendered, e)
    }
    fn handle_scroll_to_end_indicator_mouse_event(&mut self, r: SgrMouseEvent) -> bool {
        self.record(json!({"op":"indicator","raw":raw_value(r)}));
        false
    }
    fn handle_scrollbar_mouse_event(&mut self, r: SgrMouseEvent) -> bool {
        self.record(json!({"op":"scrollbar","raw":raw_value(r)}));
        false
    }
    fn scrollbar_drag_active(&self) -> bool {
        false
    }
    fn update_scrollbar_hover(&mut self, x: i64, y: i64) {
        self.record(json!({"op":"hover","x":x,"y":y}));
    }
    fn stop_scrollbar_hover(&mut self) {
        self.record(json!({"op":"stopHover"}));
    }
    fn dispatch_mouse_to_layout(&mut self, e: &TuiMouseEvent) -> Option<ComponentMouseResult> {
        self.record(json!({"op":"layout","event":event_value(e)}));
        None
    }
    fn resolve_mouse_focus_target(&mut self, c: &ComponentHandle) -> ComponentHandle {
        self.record(json!({"op":"resolve","id":self.name(c)}));
        component_overlay::resolve_mouse_focus_target(
            &mut self.overlays,
            c,
            self.columns,
            self.rows,
        )
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
        self.record(json!({"op":"paste","raw":raw_value(r)}));
        false
    }
    fn handle_selection_mouse_event(&mut self, r: SgrMouseEvent, _gesture: &mut ComponentGesture) {
        self.record(json!({"op":"selection","raw":raw_value(r)}));
    }
}
fn verify(group: &str) {
    let fixture: Value =
        serde_json::from_str(include_str!("../component_overlay/fixtures.json")).unwrap();
    for case in fixture[group].as_array().unwrap() {
        let mut host = Host::new(&case["nodes"]);
        let mut gesture = ComponentGesture::default();
        for (index, op) in case["ops"].as_array().unwrap().iter().enumerate() {
            let value = host.step(&mut gesture, op);
            let state = host.snapshot(&gesture);
            let trace = std::mem::take(&mut *host.trace.borrow_mut());
            let actual = json!({"value":value,"state":state,"trace":trace});
            assert_eq!(
                actual, case["expected"][index],
                "{group}/{} step{index} {op}",
                case["name"]
            );
        }
    }
}
#[test]
fn actual_structural_containment_and_ownership() {
    verify("ownership");
}
#[test]
fn actual_visibility_and_current_stack_order() {
    verify("visibility");
}
#[test]
fn actual_rendered_overlay_hit_and_focus_retarget() {
    verify("hits");
}
#[test]
fn actual_stale_frame_and_current_tree_mutations() {
    verify("mutations");
}
#[test]
fn actual_overlay_helpers_composed_with_gesture_controller() {
    verify("gestures");
}

type ControlHandler = Box<dyn FnMut(&TuiMouseEvent) -> Option<TuiMouseEventResult>>;
struct Control(ControlHandler);
impl Component for Control {
    fn render(&mut self, _: usize) -> Vec<String> {
        vec!["control".into()]
    }
    fn handle_mouse(&mut self, event: &TuiMouseEvent) -> Option<TuiMouseEventResult> {
        (self.0)(event)
    }
}
fn control() -> ComponentHandle {
    ComponentHandle::new(Control(Box::new(|_| {
        Some(TuiMouseEventResult {
            focus: true,
            capture: true,
            render: Some(false),
            ..Default::default()
        })
    })))
}
fn rectangle(component: ComponentHandle) -> RenderedComponentOverlay {
    RenderedComponentOverlay {
        component,
        row: 2,
        col: 3,
        width: 6,
        height: 4,
    }
}
fn press(x: i64, y: i64) -> TuiMouseEvent {
    crate::tui::mouse_dispatch::create_mouse_event(TuiMouseEventType::Press, 0, x, y, 30, 20)
}
#[test]
fn nested_box_and_handle_adapters_preserve_structural_marker() {
    let child = control();
    let mut container = Container::default();
    container.add_child(child.clone());
    let mut boxed: Box<dyn Component> = Box::new(Box::new(container));
    assert!(boxed.is_container_component());
    assert_eq!(boxed.mouse_child(0), Some(child.clone()));
    let root = ComponentHandle::from_box(boxed);
    let another = ComponentHandle::from_box(Box::new(root.clone()));
    assert_eq!(root, another);
    assert!(root.is_container_component());
    assert!(contains_component(&root, &child));
    let region = ComponentHandle::new(MouseRegion::new(child.clone(), Box::new(|_| None)));
    assert!(!region.is_container_component());
    assert!(!contains_component(&region, &child));
    assert!(contains_component(&region, &region));
}
#[test]
fn retained_frame_and_concrete_target_release_ownership_separately() {
    let child = control();
    let child_weak = child.downgrade();
    let mut c = Container::default();
    c.add_child(child.clone());
    let root = ComponentHandle::new(c);
    let root_weak = root.downgrade();
    let mut current = vec![ComponentOverlay::new(root.clone())];
    let mut rendered = vec![rectangle(root.clone())];
    drop(child);
    drop(root);
    current.clear();
    assert!(root_weak.upgrade().is_some());
    let saved = component_overlay::dispatch_mouse_to_overlay(&rendered, &press(3, 2))
        .result
        .unwrap()
        .target;
    rendered.clear();
    assert!(root_weak.upgrade().is_none());
    assert!(child_weak.upgrade().is_some());
    let mut release = press(-10, 40);
    release.event_type = TuiMouseEventType::Release;
    let result = dispatch_mouse_to_target(&release, &saved).unwrap();
    assert_eq!(result.target, saved);
    drop(result);
    drop(saved);
    assert!(child_weak.upgrade().is_none());
}
#[test]
fn child_can_remove_itself_before_overlay_focus_owner_is_applied() {
    let (root, parent) = ComponentHandle::with_shared(Container::default());
    let weak_parent = Rc::downgrade(&parent);
    let self_ref = Rc::new(RefCell::new(None::<WeakComponentHandle>));
    let weak_self = self_ref.clone();
    let child = ComponentHandle::new(Control(Box::new(move |_| {
        let child = weak_self.borrow().as_ref().unwrap().upgrade().unwrap();
        weak_parent
            .upgrade()
            .unwrap()
            .borrow_mut()
            .remove_child(&child);
        Some(TuiMouseEventResult {
            focus: true,
            capture: true,
            render: Some(false),
            ..Default::default()
        })
    })));
    *self_ref.borrow_mut() = Some(child.downgrade());
    parent.borrow_mut().add_child(child.clone());
    let rendered = vec![rectangle(root.clone())];
    let result = component_overlay::dispatch_mouse_to_overlay(&rendered, &press(3, 2))
        .result
        .unwrap();
    assert!(!contains_component(&root, &child));
    assert_eq!(result.target.component, child);
    assert_eq!(result.focus_target, Some(root.clone()));
    assert_eq!(result.result.render, Some(false));
    let mut current = vec![ComponentOverlay::new(root.clone())];
    assert_eq!(
        component_overlay::resolve_mouse_focus_target(
            &mut current,
            &result.target.component,
            30,
            20
        ),
        child
    );
    assert_eq!(
        component_overlay::resolve_mouse_focus_target(
            &mut current,
            result.focus_target.as_ref().unwrap(),
            30,
            20
        ),
        root
    );
    assert!(dispatch_mouse_to_target(&press(20, 20), &result.target).is_some());
}
#[test]
fn containment_does_not_render_hidden_stack_or_scroll_children() {
    struct NoRender;
    impl Component for NoRender {
        fn render(&mut self, _: usize) -> Vec<String> {
            panic!("containment must not render")
        }
    }
    let child = ComponentHandle::new(NoRender);
    let scroll = ComponentHandle::new(ScrollView::new(
        Box::new(child.clone()),
        ScrollViewOptions::default(),
    ));
    let stack = ComponentHandle::new(VStack::new(
        vec![StackEntry::new(
            Box::new(scroll.clone()),
            StackEntryOptions {
                visible: Some(Arc::new(|_| false)),
                ..Default::default()
            },
        )],
        StackOptions::default(),
    ));
    assert!(contains_component(&stack, &child));
    assert!(contains_component(&stack, &scroll));
    assert!(!contains_component(&stack, &control()));
}
#[test]
fn extreme_integer_rectangle_edges_are_safe_without_js_parity_claim() {
    let trace = Rc::new(RefCell::new(Vec::new()));
    let seen = trace.clone();
    let child = ComponentHandle::new(Control(Box::new(move |e| {
        seen.borrow_mut().push((e.x, e.y));
        Some(TuiMouseEventResult {
            handled: true,
            ..Default::default()
        })
    })));
    let mut layout = RenderedComponentOverlay {
        component: child,
        row: i64::MAX,
        col: i64::MAX,
        width: 1,
        height: 1,
    };
    assert!(
        component_overlay::dispatch_mouse_to_overlay(&[layout.clone()], &press(i64::MAX, i64::MAX))
            .hit
    );
    assert!(
        !component_overlay::dispatch_mouse_to_overlay(
            &[layout.clone()],
            &press(i64::MAX - 1, i64::MAX)
        )
        .hit
    );
    layout.col = i64::MIN;
    layout.row = i64::MIN;
    assert!(
        component_overlay::dispatch_mouse_to_overlay(&[layout.clone()], &press(i64::MIN, i64::MIN))
            .hit
    );
    layout.width = 0;
    assert!(
        !component_overlay::dispatch_mouse_to_overlay(&[layout], &press(i64::MIN, i64::MIN)).hit
    );
    assert_eq!(*trace.borrow(), vec![(0, 0), (0, 0)]);
}
