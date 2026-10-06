//! Every step compares values AND callbacks against unmodified actual sources.
use crate::tui::component::{
    Component, TuiMouseButton, TuiMouseEvent, TuiMouseEventResult, TuiMouseEventType,
};
use crate::tui::component_mouse::*;
use crate::tui::components::container::Container;
use crate::tui::components::mouse_region::MouseRegion;
use crate::tui::components::scroll_view::{ScrollView, ScrollViewOptions, ScrollViewScrollbar};
use crate::tui::components::stack::{
    HStack, StackAlign, StackBasis, StackEntry, StackEntryOptions, StackOptions, VStack,
};
use crate::tui::layout::{render_layout_frame, LayoutBoxId, LayoutFrame, LayoutRect};
use crate::tui::layout_node::{ComponentCacheId, LayoutNode};
use crate::tui::rendered_lines::RenderedLines;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

type Trace = Arc<Mutex<Vec<Value>>>;
fn fixture() -> Value {
    serde_json::from_str(include_str!("../component_mouse/fixtures.json")).unwrap()
}
fn number(v: &Value) -> f64 {
    v.as_f64().unwrap_or_else(|| match v.as_str().unwrap() {
        "NaN" => f64::NAN,
        "Infinity" => f64::INFINITY,
        "-Infinity" => f64::NEG_INFINITY,
        n => panic!("{n}"),
    })
}
fn event_type(v: &Value) -> TuiMouseEventType {
    match v.as_str().unwrap() {
        "press" => TuiMouseEventType::Press,
        "release" => TuiMouseEventType::Release,
        "move" => TuiMouseEventType::Move,
        "drag" => TuiMouseEventType::Drag,
        "click" => TuiMouseEventType::Click,
        "wheel" => TuiMouseEventType::Wheel,
        s => panic!("{s}"),
    }
}
fn event(v: &Value) -> TuiMouseEvent {
    TuiMouseEvent {
        event_type: event_type(&v["type"]),
        button: match v["button"].as_str().unwrap() {
            "left" => TuiMouseButton::Left,
            "middle" => TuiMouseButton::Middle,
            "right" => TuiMouseButton::Right,
            "none" => TuiMouseButton::None,
            s => panic!("{s}"),
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
        wheel_delta: v.get("wheelDelta").map(number),
        click_count: v.get("clickCount").map(|x| x.as_u64().unwrap() as u32),
    }
}
fn event_value(e: &TuiMouseEvent) -> Value {
    let mut v = json!({"type":match e.event_type {TuiMouseEventType::Press=>"press",TuiMouseEventType::Release=>"release",TuiMouseEventType::Move=>"move",TuiMouseEventType::Drag=>"drag",TuiMouseEventType::Click=>"click",TuiMouseEventType::Wheel=>"wheel"},"button":match e.button {TuiMouseButton::Left=>"left",TuiMouseButton::Middle=>"middle",TuiMouseButton::Right=>"right",TuiMouseButton::None=>"none"},"x":e.x,"y":e.y,"screenX":e.screen_x,"screenY":e.screen_y,"width":e.width,"height":e.height,"shift":e.shift,"alt":e.alt,"ctrl":e.ctrl});
    if let Some(n) = e.wheel_delta {
        v["wheelDelta"] = if n.is_nan() {
            json!("NaN")
        } else if n == f64::INFINITY {
            json!("Infinity")
        } else if n == f64::NEG_INFINITY {
            json!("-Infinity")
        } else if n.fract() == 0.0 && n.abs() <= 9_007_199_254_740_991.0 {
            // JS JSON.stringify has one number kind; serde_json distinguishes 0/0.0.
            json!(n as i64)
        } else {
            json!(n)
        };
    }
    if let Some(n) = e.click_count {
        v["clickCount"] = json!(n);
    }
    v
}
fn flags(v: &Value) -> TuiMouseEventResult {
    TuiMouseEventResult {
        handled: v["handled"].as_bool().unwrap_or(false),
        capture: v["capture"].as_bool().unwrap_or(false),
        focus: v["focus"].as_bool().unwrap_or(false),
        render: v["render"].as_bool(),
    }
}
fn flags_value(r: Option<TuiMouseEventResult>) -> Value {
    r.map_or(
        Value::Null,
        |r| json!({"handled":r.handled,"capture":r.capture,"focus":r.focus,"render":r.render}),
    )
}

#[derive(Clone)]
struct Probe {
    id: String,
    state: Rc<RefCell<Value>>,
    trace: Trace,
}
impl Component for Probe {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.trace
            .lock()
            .unwrap()
            .push(json!({"op":"render","id":self.id,"width":width}));
        self.state.borrow()["lines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().replace("{w}", &width.to_string()))
            .collect()
    }
    fn invalidate(&mut self) {
        self.trace
            .lock()
            .unwrap()
            .push(json!({"op":"invalidate","id":self.id}));
    }
    fn handle_mouse(&mut self, event: &TuiMouseEvent) -> Option<TuiMouseEventResult> {
        let event = event_value(event);
        self.trace
            .lock()
            .unwrap()
            .push(json!({"op":"mouse","id":self.id,"event":event}));
        let state = self.state.borrow();
        let response = state["responses"]
            .get(event["type"].as_str().unwrap())
            .unwrap_or(&state["response"]);
        (!response.is_null()).then(|| flags(response))
    }
}
enum Content {
    Leaf(Probe),
    Container(Container),
    HStack(HStack),
    VStack(VStack),
    Scroll(ScrollView),
    Region(MouseRegion),
}
impl Content {
    fn component(&mut self) -> &mut dyn Component {
        match self {
            Self::Leaf(c) => c,
            Self::Container(c) => c,
            Self::HStack(c) => c,
            Self::VStack(c) => c,
            Self::Scroll(c) => c,
            Self::Region(c) => c,
        }
    }
    fn uses_container_mouse_handler(&self) -> bool {
        matches!(
            self,
            Self::Container(_) | Self::HStack(_) | Self::VStack(_) | Self::Scroll(_)
        )
    }
}
// A custom upstream layout-node handler uses the same layout implementation but
// has different method identity. This adapter explicitly models that contract.
struct Driven {
    content: Content,
    mouse_override: Option<Probe>,
}
impl Component for Driven {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.content.component().render(width)
    }
    fn render_layout_lines(&mut self, width: usize) -> RenderedLines {
        self.content.component().render_layout_lines(width)
    }
    fn layout_node_mut(&mut self) -> Option<LayoutNode<'_>> {
        self.content.component().layout_node_mut()
    }
    fn prepare_mouse_children(&mut self) {
        self.content.component().prepare_mouse_children();
    }
    fn mouse_child(&mut self, index: usize) -> Option<ComponentHandle> {
        self.content.component().mouse_child(index)
    }
    fn uses_container_mouse_handler(&self) -> bool {
        self.mouse_override.is_none() && self.content.uses_container_mouse_handler()
    }
    fn delegates_mouse_focus(&self) -> bool {
        match &self.content {
            Content::Container(c) => c.delegates_mouse_focus(),
            _ => false,
        }
    }
    fn mouse_action(&mut self, event: &TuiMouseEvent) -> Option<MouseAction> {
        if let Some(probe) = &mut self.mouse_override {
            probe.handle_mouse(event).map(MouseAction::Direct)
        } else {
            self.content.component().mouse_action(event)
        }
    }
    fn handle_mouse(&mut self, event: &TuiMouseEvent) -> Option<TuiMouseEventResult> {
        if let Some(probe) = &mut self.mouse_override {
            probe.handle_mouse(event)
        } else {
            self.content.component().handle_mouse(event)
        }
    }
    fn handle_input(&mut self, data: &str) {
        self.content.component().handle_input(data);
    }
    fn invalidate(&mut self) {
        self.content.component().invalidate();
    }
}
fn input(container: &mut Container, trace: &Trace, id: &str, enabled: bool) {
    let id = id.to_owned();
    let trace = trace.clone();
    container.set_input_handler(enabled.then(|| {
        Box::new(move |data: &str| {
            trace
                .lock()
                .unwrap()
                .push(json!({"op":"input","id":id,"data":data}));
        }) as crate::tui::components::container::ContainerInputHandler
    }));
}
fn entry_options(v: &Value) -> StackEntryOptions {
    StackEntryOptions {
        basis: v["basis"].as_f64().map(StackBasis::Size),
        grow: v["grow"].as_f64(),
        shrink: v["shrink"].as_f64(),
        min_size: v["minSize"].as_f64(),
        max_size: v["maxSize"].as_f64(),
        visible: v["hidden"].as_bool().unwrap_or(false).then(|| {
            Arc::new(|_: &crate::tui::components::stack::LayoutViewport| false)
                as crate::tui::components::stack::StackVisibility
        }),
    }
}
fn rect(v: &Value) -> LayoutRect {
    LayoutRect {
        x: v["x"].as_i64().unwrap(),
        y: v["y"].as_i64().unwrap(),
        width: v["width"].as_u64().unwrap() as usize,
        height: v["height"].as_u64().unwrap() as usize,
    }
}
fn rect_value(r: LayoutRect) -> Value {
    json!({"x":r.x,"y":r.y,"width":r.width,"height":r.height})
}
fn box_value(f: &LayoutFrame, i: LayoutBoxId, names: &HashMap<ComponentCacheId, String>) -> Value {
    let b = &f.boxes[i];
    json!({"id":names[&b.component.as_ref().expect("owning frame").id()],"rect":rect_value(b.rect),"clip":rect_value(b.clip),"children":b.children.iter().map(|&i|box_value(f,i,names)).collect::<Vec<_>>()})
}
fn result_value(
    r: Option<&ComponentMouseResult>,
    names: &HashMap<ComponentCacheId, String>,
) -> Value {
    r.map_or(Value::Null, |r| {
        let mut v = flags_value(Some(r.result)); let t = &r.target;
        v["target"]=json!({"id":names[&t.component.id()],"originX":t.origin_x,"originY":t.origin_y,"width":t.width,"height":t.height});
        v["focusTarget"]=json!(r.focus_target.as_ref().map(|c| &names[&c.id()])); v
    })
}
fn run_case(case: &Value) -> usize {
    let trace = Trace::default();
    let mut objects: HashMap<String, ComponentHandle> = HashMap::new();
    let mut controls = HashMap::new();
    let mut states = HashMap::new();
    let mut names = HashMap::new();
    let mut frames: HashMap<String, LayoutFrame> = HashMap::new();
    let mut saved: HashMap<String, ComponentMouseTarget> = HashMap::new();
    let mut current: Option<String> = None;
    for spec in case["nodes"].as_array().unwrap() {
        let id = spec["id"].as_str().unwrap();
        let state = Rc::new(RefCell::new(spec.clone()));
        let probe = Probe {
            id: id.into(),
            state: state.clone(),
            trace: trace.clone(),
        };
        let content = match spec["kind"].as_str().unwrap() {
            "leaf" => Content::Leaf(probe.clone()),
            "container" => {
                let mut c = Container::new(
                    spec["children"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|id| objects[id.as_str().unwrap()].clone())
                        .collect(),
                );
                input(&mut c, &trace, id, spec["input"].as_bool().unwrap_or(false));
                Content::Container(c)
            }
            "region" => {
                let mut probe = probe.clone();
                Content::Region(MouseRegion::new(
                    objects[spec["child"].as_str().unwrap()].clone(),
                    Box::new(move |e| probe.handle_mouse(e)),
                ))
            }
            "scroll" => Content::Scroll(ScrollView::new(
                Box::new(objects[spec["child"].as_str().unwrap()].clone()),
                ScrollViewOptions {
                    scrollbar: Some(if spec["options"]["scrollbar"] == "always" {
                        ScrollViewScrollbar::Always
                    } else {
                        ScrollViewScrollbar::Hidden
                    }),
                    timer_scheduler: Some(Arc::new(|_, _| {})),
                    follow_end: spec["options"]["followEnd"].as_bool().unwrap_or(false),
                    primary: spec["options"]["primary"].as_bool().unwrap_or(false),
                    ..ScrollViewOptions::default()
                },
            )),
            kind @ ("hstack" | "vstack") => {
                let entries = spec["children"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|child| {
                        let (id, options) = if let Some(id) = child.as_str() {
                            (id, StackEntryOptions::default())
                        } else {
                            (
                                child["id"].as_str().unwrap(),
                                entry_options(&child["options"]),
                            )
                        };
                        StackEntry::new(Box::new(objects[id].clone()), options)
                    })
                    .collect();
                let options = StackOptions {
                    gap: spec["options"]["gap"].as_f64(),
                    align: StackAlign::Stretch,
                };
                if kind == "hstack" {
                    Content::HStack(HStack::new(entries, options))
                } else {
                    Content::VStack(VStack::new(entries, options))
                }
            }
            kind => panic!("unknown kind {kind}"),
        };
        let (handle, control) = ComponentHandle::with_shared(Driven {
            content,
            mouse_override: spec["override"].as_bool().unwrap_or(false).then_some(probe),
        });
        names.insert(handle.id(), id.to_owned());
        objects.insert(id.into(), handle);
        controls.insert(id.to_owned(), control);
        states.insert(id.to_owned(), state);
    }
    let ops = case["ops"].as_array().unwrap();
    let expected = case["expected"].as_array().unwrap();
    assert_eq!(ops.len(), expected.len());
    for (index, op) in ops.iter().enumerate() {
        let id = op["id"].as_str().unwrap_or("");
        let mut value = Value::Null;
        match op["op"].as_str().unwrap() {
            "render" => {
                value = json!(objects
                    .get_mut(id)
                    .unwrap()
                    .render(op["width"].as_u64().unwrap() as usize))
            }
            "layout" => {
                let name = op["name"].as_str().unwrap_or("current").to_owned();
                let callback_trace = trace.clone();
                let f = render_layout_frame(
                    objects.get_mut(op["root"].as_str().unwrap()).unwrap(),
                    op["width"].as_u64().unwrap() as usize,
                    op["height"].as_u64().unwrap() as usize,
                    Arc::new(move || {
                        callback_trace
                            .lock()
                            .unwrap()
                            .push(json!({"op":"requestRender"}))
                    }),
                );
                value = json!({"lines":(0..f.lines.len()).map(|i|f.lines.get(i)).collect::<Vec<_>>(),"root":box_value(&f,f.root,&names)});
                current = Some(name.clone());
                frames.insert(name, f);
            }
            kind @ ("hit" | "dispatch" | "target") => {
                let event = event(&op["event"]);
                if kind == "hit" && op.get("frame").is_some() {
                    current = op["frame"].as_str().map(String::from);
                }
                let r = match kind {
                    "hit" => {
                        dispatch_mouse_to_layout(current.as_ref().map(|name| &frames[name]), &event)
                    }
                    "target" => {
                        dispatch_mouse_to_target(&event, &saved[op["saved"].as_str().unwrap()])
                    }
                    _ => objects[id].dispatch(&event),
                };
                if let (Some(name), Some(result)) = (op["save"].as_str(), &r) {
                    saved.insert(name.into(), result.target.clone());
                }
                value = result_value(r.as_ref(), &names);
            }
            "lines" => states[id].borrow_mut()["lines"] = op["lines"].clone(),
            "response" => {
                let mut s = states[id].borrow_mut();
                s["response"] = op["response"].clone();
                s["responses"] = op["responses"].clone();
            }
            "add" => {
                let child = objects[op["child"].as_str().unwrap()].clone();
                match &mut controls[id].borrow_mut().content {
                    Content::Container(c) => c.add_child(child),
                    Content::HStack(c) => {
                        c.add_child(Box::new(child), entry_options(&op["options"]));
                    }
                    Content::VStack(c) => {
                        c.add_child(Box::new(child), entry_options(&op["options"]));
                    }
                    _ => panic!("add"),
                }
            }
            "remove" => {
                let child = &objects[op["child"].as_str().unwrap()];
                match &mut controls[id].borrow_mut().content {
                    Content::Container(c) => {
                        c.remove_child(child);
                    }
                    Content::HStack(c) => {
                        c.remove_child_handle(child);
                    }
                    Content::VStack(c) => {
                        c.remove_child_handle(child);
                    }
                    _ => panic!("remove"),
                }
            }
            "clear" => match &mut controls[id].borrow_mut().content {
                Content::Container(c) => c.clear(),
                Content::HStack(c) => c.clear(),
                Content::VStack(c) => c.clear(),
                _ => panic!("clear"),
            },
            "reverse" => match &mut controls[id].borrow_mut().content {
                Content::Container(c) => c.children.reverse(),
                _ => panic!("reverse"),
            },
            "input" => objects
                .get_mut(id)
                .unwrap()
                .handle_input(op["data"].as_str().unwrap()),
            "delegate" => match &mut controls[id].borrow_mut().content {
                Content::Container(c) => input(c, &trace, id, op["value"].as_bool().unwrap()),
                _ => panic!("delegate"),
            },
            "invalidate" => objects.get_mut(id).unwrap().invalidate(),
            "geometry" => {
                let f = frames
                    .get_mut(op["frame"].as_str().unwrap_or("current"))
                    .unwrap();
                let mut i = f.root;
                for n in op["path"].as_array().unwrap() {
                    i = f.boxes[i].children[n.as_u64().unwrap() as usize];
                }
                let b = &mut f.boxes[i];
                if let Some(r) = op.get("rect") {
                    b.rect = rect(r);
                }
                if let Some(r) = op.get("clip") {
                    b.clip = rect(r);
                }
                if let Some(layer) = op["layer"].as_i64() {
                    b.layer = layer as i32;
                }
            }
            op => panic!("unknown operation {op}"),
        }
        let actual = json!({"value":value,"trace":std::mem::take(&mut *trace.lock().unwrap())});
        assert_eq!(
            actual, expected[index],
            "case={} step={} op={}",
            case["name"], index, op
        );
    }
    ops.len()
}
fn group(name: &str, cases: usize, steps: usize) {
    let f = fixture();
    let values = f[name].as_array().unwrap();
    assert_eq!(values.len(), cases);
    assert_eq!(values.iter().map(run_case).sum::<usize>(), steps);
}
#[test]
fn container_cache_and_focus_match_actual_source() {
    group("containers", 8, 676);
}
#[test]
fn layout_routing_aliases_and_priority_match_actual_source() {
    group("layouts", 34, 728);
}
#[test]
fn stale_frames_targets_and_mutations_match_actual_source() {
    group("mutations", 8, 70);
}
#[test]
fn mouse_region_child_first_matches_actual_source() {
    group("regions", 20, 60);
}

#[test]
fn direct_layout_node_and_opaque_region_handlers_match_actual_source() {
    group("directLayouts", 7, 53);
}

// Rust ownership/adapter contract tests, not additional JS-number parity claims.
fn press() -> TuiMouseEvent {
    crate::tui::mouse_dispatch::create_mouse_event(TuiMouseEventType::Press, 0, 0, 0, 8, 8)
}
struct CellLeaf;
impl Component for CellLeaf {
    fn render(&mut self, _: usize) -> Vec<String> {
        vec!["x".into()]
    }
    fn handle_mouse(&mut self, _: &TuiMouseEvent) -> Option<TuiMouseEventResult> {
        Some(TuiMouseEventResult {
            capture: true,
            ..TuiMouseEventResult::default()
        })
    }
}
#[test]
fn owning_identity_is_not_path_or_zero_sized_address() {
    let a = ComponentHandle::new(CellLeaf);
    let b = ComponentHandle::new(CellLeaf);
    assert_ne!(a, b);
    assert_eq!(a, ComponentHandle::from_box(Box::new(a.clone())));
    assert_eq!(a, ComponentHandle::new(Box::new(Box::new(a.clone()))));
    let (root, stack) = ComponentHandle::with_shared(VStack::new(
        vec![
            StackEntry::new(
                Box::new(a.clone()),
                StackEntryOptions {
                    visible: Some(Arc::new(|_| false)),
                    ..StackEntryOptions::default()
                },
            ),
            StackEntry::from(Box::new(b.clone()) as Box<dyn Component>),
            StackEntry::from(Box::new(a.clone()) as Box<dyn Component>),
        ],
        StackOptions::default(),
    ));
    assert_eq!(root.resolve_path(&[]), Some(root.clone()));
    assert_eq!(root.resolve_path(&[0]), Some(a.clone()));
    assert_eq!(root.resolve_path(&[1]), Some(b.clone()));
    assert_eq!(root.resolve_path(&[2]), Some(a.clone()));
    assert_eq!(root.resolve_path(&[3]), None);
    assert_eq!(root.resolve_path(&[0, 0]), None);
    stack.borrow_mut().remove_child_handle(&a).unwrap();
    assert_eq!(root.resolve_path(&[0]), Some(b));
    assert_eq!(root.resolve_path(&[1]), Some(a));
}
struct DroppingLeaf(Rc<std::cell::Cell<usize>>);
impl Drop for DroppingLeaf {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}
impl Component for DroppingLeaf {
    fn render(&mut self, _: usize) -> Vec<String> {
        vec!["x".into()]
    }
    fn handle_mouse(&mut self, _: &TuiMouseEvent) -> Option<TuiMouseEventResult> {
        Some(TuiMouseEventResult {
            capture: true,
            ..TuiMouseEventResult::default()
        })
    }
}
#[test]
fn old_frames_and_targets_retain_removed_objects_then_release_them() {
    let drops = Rc::new(std::cell::Cell::new(0));
    let child = ComponentHandle::new(DroppingLeaf(drops.clone()));
    let weak = child.downgrade();
    let (mut root, stack) = ComponentHandle::with_shared(VStack::new(
        vec![StackEntry::from(
            Box::new(child.clone()) as Box<dyn Component>
        )],
        StackOptions::default(),
    ));
    let frame = render_layout_frame(&mut root, 8, 5, Arc::new(|| {}));
    let saved = dispatch_mouse_to_layout(Some(&frame), &press())
        .unwrap()
        .target;
    stack.borrow_mut().remove_child_handle(&child).unwrap();
    stack
        .borrow_mut()
        .add_child(Box::new(CellLeaf), StackEntryOptions::default());
    drop(child);
    assert!(weak.upgrade().is_some());
    assert_eq!(drops.get(), 0);
    assert_ne!(root.resolve_path(&[0]), Some(saved.component.clone()));
    let new_frame = render_layout_frame(&mut root, 3, 2, Arc::new(|| {}));
    let old_result = dispatch_mouse_to_layout(Some(&frame), &press()).unwrap();
    assert_eq!(old_result.target.component, saved.component);
    drop(old_result);
    let event = TuiMouseEvent {
        event_type: TuiMouseEventType::Drag,
        screen_x: -9,
        screen_y: 14,
        x: 77,
        y: 77,
        width: 1,
        height: 1,
        ..press()
    };
    let old_result = dispatch_mouse_to_target(&event, &saved).unwrap();
    assert_eq!(old_result.target, saved);
    drop(old_result);
    assert_eq!((saved.width, saved.height), (8, 1));
    drop(frame);
    assert!(weak.upgrade().is_some());
    drop(saved);
    assert!(weak.upgrade().is_none());
    assert_eq!(drops.get(), 1);
    drop(new_frame);
    drop(root);
    drop(stack);
    assert_eq!(drops.get(), 1);
}
#[test]
fn container_snapshot_owns_removed_children_until_next_render() {
    let drops = Rc::new(std::cell::Cell::new(0));
    let child = ComponentHandle::new(DroppingLeaf(drops.clone()));
    let weak = child.downgrade();
    let (mut root, container) = ComponentHandle::with_shared(Container::new(vec![child.clone()]));
    root.render(8);
    container.borrow_mut().remove_child(&child);
    drop(child);
    root.invalidate();
    assert!(root.dispatch(&press()).is_some());
    assert!(root
        .dispatch(&TuiMouseEvent {
            width: 3,
            ..press()
        })
        .is_none());
    assert!(root.dispatch(&press()).is_some());
    assert!(weak.upgrade().is_some());
    assert!(root.render(8).is_empty());
    assert!(weak.upgrade().is_none());
    assert_eq!(drops.get(), 1);
}
struct CallbackLeaf(Box<dyn FnMut()>);
impl Component for CallbackLeaf {
    fn render(&mut self, _: usize) -> Vec<String> {
        vec!["x".into()]
    }
    fn handle_mouse(&mut self, _: &TuiMouseEvent) -> Option<TuiMouseEventResult> {
        (self.0)();
        Some(TuiMouseEventResult {
            focus: true,
            ..TuiMouseEventResult::default()
        })
    }
}
#[test]
fn forwarding_releases_parent_borrow_and_checks_focus_delegation_after_callback() {
    let (mut root, parent) = ComponentHandle::with_shared(Container::default());
    let weak = Rc::downgrade(&parent);
    let enable = Rc::new(std::cell::Cell::new(false));
    let requested = enable.clone();
    let child = ComponentHandle::new(CallbackLeaf(Box::new(move || {
        let parent = weak.upgrade().unwrap();
        let mut parent = parent.borrow_mut();
        parent.clear();
        parent.set_input_handler(requested.get().then(|| {
            Box::new(|_: &str| {}) as crate::tui::components::container::ContainerInputHandler
        }));
    })));
    parent.borrow_mut().add_child(child.clone());
    parent
        .borrow_mut()
        .set_input_handler(Some(Box::new(|_| {})));
    root.render(8);
    let first = root.dispatch(&press()).unwrap();
    assert_eq!(first.focus_target, Some(child.clone()));
    enable.set(true);
    let second = root.dispatch(&press()).unwrap();
    assert_eq!(second.focus_target, Some(root));
    assert_eq!(second.target.component, child);
}
#[test]
fn mouse_region_releases_its_borrow_before_child_callback() {
    let link: Rc<RefCell<Option<WeakComponentHandle>>> = Rc::default();
    let back = link.clone();
    let child = ComponentHandle::new(CallbackLeaf(Box::new(move || {
        let region = back.borrow().as_ref().unwrap().upgrade().unwrap();
        region.with_mut(|c| assert!(c.mouse_child(0).is_some()));
    })));
    let region = ComponentHandle::new(MouseRegion::new(
        child.clone(),
        Box::new(|_| panic!("handled child forbids fallback")),
    ));
    *link.borrow_mut() = Some(region.downgrade());
    let result = region.dispatch(&press()).unwrap();
    assert_eq!(result.target.component, child);
    assert_eq!(result.focus_target, Some(child));
}
#[test]
fn replacing_scroll_child_preserves_old_frame_identity_and_new_live_path() {
    let (mut root, scroll) = ComponentHandle::with_shared(ScrollView::new(
        Box::new(CellLeaf),
        ScrollViewOptions {
            scrollbar: Some(ScrollViewScrollbar::Hidden),
            timer_scheduler: Some(Arc::new(|_, _| {})),
            ..ScrollViewOptions::default()
        },
    ));
    let old = root.resolve_path(&[0]).unwrap();
    assert_eq!(root.resolve_path(&[1]), None);
    let frame = render_layout_frame(&mut root, 8, 4, Arc::new(|| {}));
    *scroll.borrow_mut().child() = Box::new(CellLeaf);
    let new = root.resolve_path(&[0]).unwrap();
    assert_ne!(old, new);
    assert_eq!(
        dispatch_mouse_to_layout(Some(&frame), &press())
            .unwrap()
            .target
            .component,
        old
    );
    let frame2 = render_layout_frame(&mut root, 4, 3, Arc::new(|| {}));
    assert_eq!(
        dispatch_mouse_to_layout(Some(&frame2), &press())
            .unwrap()
            .target
            .component,
        new
    );
}
#[test]
fn zero_width_boxes_keep_handles_and_borrow_only_frames_do_not_invent_them() {
    let child = ComponentHandle::new(CellLeaf);
    let mut root = ComponentHandle::new(HStack::new(
        vec![StackEntry::new(
            Box::new(child.clone()),
            StackEntryOptions {
                basis: Some(StackBasis::Size(0.0)),
                max_size: Some(0.0),
                ..StackEntryOptions::default()
            },
        )],
        StackOptions::default(),
    ));
    let frame = render_layout_frame(&mut root, 4, 3, Arc::new(|| {}));
    let b = &frame.boxes[frame.root_box().children[0]];
    assert_eq!(b.component, Some(child));
    assert_eq!(b.clip.width, 0);
    assert!(dispatch_mouse_to_layout(Some(&frame), &press()).is_none());
    let frame = render_layout_frame(&mut CellLeaf, 4, 3, Arc::new(|| {}));
    assert!(frame.root_box().component.is_none());
    assert!(dispatch_mouse_to_layout(Some(&frame), &press()).is_none());
}
#[derive(Clone)]
struct SharedSparse {
    id: ComponentCacheId,
    calls: Rc<std::cell::Cell<usize>>,
}
impl Component for SharedSparse {
    fn render(&mut self, _: usize) -> Vec<String> {
        panic!("must preserve sparse render hook")
    }
    fn render_layout_lines(&mut self, _: usize) -> RenderedLines {
        self.calls.set(self.calls.get() + 1);
        RenderedLines::sparse(2, [(1, "x".into())].into())
    }
    fn layout_cache_id(&self) -> Option<ComponentCacheId> {
        Some(self.id)
    }
    fn is_focusable(&self) -> bool {
        true
    }
    fn focused(&self) -> bool {
        true
    }
    fn wants_key_release(&self) -> bool {
        true
    }
}
#[test]
fn boxed_handles_preserve_sparse_hooks_and_explicit_shared_render_cache_identity() {
    let calls = Rc::new(std::cell::Cell::new(0));
    let sparse = SharedSparse {
        id: ComponentCacheId::new(),
        calls: calls.clone(),
    };
    let a = ComponentHandle::from_box(Box::new(sparse.clone()));
    let b = ComponentHandle::from_box(Box::new(sparse));
    assert_ne!(a, b);
    assert_eq!(a.layout_cache_id(), b.layout_cache_id());
    assert!(a.is_focusable());
    assert!(a.focused());
    assert!(a.wants_key_release());
    let mut root = ComponentHandle::new(VStack::new(
        vec![
            StackEntry::from(Box::new(a) as Box<dyn Component>),
            StackEntry::from(Box::new(b) as Box<dyn Component>),
        ],
        StackOptions::default(),
    ));
    let frame = render_layout_frame(&mut root, 8, 4, Arc::new(|| {}));
    assert_eq!(calls.get(), 1);
    let ids = &frame.root_box().children;
    assert_eq!(ids.len(), 2);
    for &id in ids {
        assert_eq!(frame.boxes[id].lines.as_ref().unwrap().get(0), None);
        assert_eq!(frame.boxes[id].lines.as_ref().unwrap().get(1), Some("x"));
    }
}
