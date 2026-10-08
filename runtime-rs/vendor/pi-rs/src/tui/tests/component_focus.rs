//! Actual-source focus lifecycle: per-step values, all retained identities,
//! raw restore state, setter/visibility/host order and real mouse composition.
//! Plain-input bridge releases the component borrow before synchronous callback
//! commands. This does not model arbitrary self-reentrant input handlers.
use crate::tui::component::{
    Component, TuiMouseButton, TuiMouseEvent, TuiMouseEventResult, TuiMouseEventType,
};
use crate::tui::component_focus::*;
use crate::tui::component_gesture::*;
use crate::tui::component_mouse::*;
use crate::tui::component_overlay::{dispatch_mouse_to_overlay, RenderedComponentOverlay};
use crate::tui::components::container::Container;
use crate::tui::overlay::OverlayBounds;
use crate::tui::viewport_mouse::SgrMouseEvent;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
type Trace = Rc<RefCell<Vec<Value>>>;
fn raw_value(r: SgrMouseEvent) -> Value {
    json!({"button":r.button,"x":r.x,"y":r.y,"release":r.release})
}
fn event_value(e: &TuiMouseEvent) -> Value {
    let kind = match e.event_type {
        TuiMouseEventType::Press => "press",
        TuiMouseEventType::Release => "release",
        TuiMouseEventType::Click => "click",
        TuiMouseEventType::Move => "move",
        TuiMouseEventType::Drag => "drag",
        TuiMouseEventType::Wheel => "wheel",
    };
    let button = match e.button {
        TuiMouseButton::Left => "left",
        TuiMouseButton::Right => "right",
        TuiMouseButton::Middle => "middle",
        TuiMouseButton::None => "none",
    };
    let mut v = json!({"type":kind,"button":button,"x":e.x,"y":e.y,"screenX":e.screen_x,"screenY":e.screen_y,"width":e.width,"height":e.height,"shift":e.shift,"alt":e.alt,"ctrl":e.ctrl});
    if let Some(n) = e.click_count {
        v["clickCount"] = json!(n);
    }
    if let Some(n) = e.wheel_delta {
        v["wheelDelta"] = json!(n);
    }
    v
}
fn bounds_value(b: Option<OverlayBounds>) -> Value {
    b.map_or(
        Value::Null,
        |b| json!({"row":b.row,"col":b.col,"width":b.width,"height":b.height}),
    )
}
struct Probe {
    id: String,
    state: Rc<RefCell<Value>>,
    trace: Trace,
    children: Option<Rc<RefCell<Container>>>,
    structural: bool,
}
impl Component for Probe {
    fn render(&mut self, width: usize) -> Vec<String> {
        if self.structural {
            self.children.as_ref().unwrap().borrow_mut().render(width)
        } else {
            vec![self.id.clone(), self.id.clone()]
        }
    }
    fn mouse_child(&mut self, index: usize) -> Option<ComponentHandle> {
        self.children
            .as_ref()?
            .borrow()
            .children
            .get(index)
            .cloned()
    }
    fn is_container_component(&self) -> bool {
        self.structural
    }
    fn uses_container_mouse_handler(&self) -> bool {
        self.structural && self.state.borrow().get("mouse").is_none()
    }
    fn delegates_mouse_focus(&self) -> bool {
        self.state.borrow()["input"] != false
    }
    fn is_focusable(&self) -> bool {
        self.state.borrow()["focusable"] != false
    }
    fn focused(&self) -> bool {
        self.state.borrow()["focused"].as_bool().unwrap()
    }
    fn set_focused(&mut self, value: bool) {
        self.trace
            .borrow_mut()
            .push(json!({"op":"focused","id":self.id,"value":value}));
        self.state.borrow_mut()["focused"] = json!(value);
    }
    fn handle_input(&mut self, data: &str) {
        self.trace
            .borrow_mut()
            .push(json!({"op":"input","id":self.id,"data":data}));
    }
    fn mouse_action(&mut self, event: &TuiMouseEvent) -> Option<MouseAction> {
        if let Some(response) = self.state.borrow().get("mouse") {
            self.trace
                .borrow_mut()
                .push(json!({"op":"mouse","id":self.id,"event":event_value(event)}));
            if response.is_null() {
                return None;
            }
            return Some(MouseAction::Direct(TuiMouseEventResult {
                handled: response["handled"].as_bool().unwrap_or(false),
                capture: response["capture"].as_bool().unwrap_or(false),
                focus: response["focus"].as_bool().unwrap_or(false),
                render: response["render"].as_bool(),
            }));
        }
        if self.structural {
            self.children.as_ref()?.borrow_mut().mouse_action(event)
        } else {
            None
        }
    }
}
struct Io {
    trace: Trace,
    objects: BTreeMap<String, ComponentHandle>,
    states: BTreeMap<String, Rc<RefCell<Value>>>,
    containers: BTreeMap<String, Rc<RefCell<Container>>>,
    handles: BTreeMap<String, ComponentOverlayHandle>,
    visibility: BTreeMap<String, Rc<RefCell<Value>>>,
    rendered: Vec<RenderedComponentOverlay>,
    roots: Vec<ComponentHandle>,
    columns: usize,
    rows: usize,
    now: i64,
    on_input: Value,
}
impl Io {
    fn record(&self, v: Value) {
        self.trace.borrow_mut().push(v);
    }
    fn object(&self, id: &str) -> ComponentHandle {
        self.objects[id].clone()
    }
    fn name(&self, c: &ComponentHandle) -> String {
        self.objects
            .iter()
            .find(|(_, v)| *v == c)
            .unwrap()
            .0
            .clone()
    }
    fn key(&self, h: &ComponentOverlayHandle) -> String {
        self.handles
            .iter()
            .find(|(_, v)| *v == h)
            .unwrap()
            .0
            .clone()
    }
    fn target_value(&self, t: Option<&ComponentMouseTarget>) -> Value {
        t.map_or(Value::Null,|t|json!({"id":self.name(&t.component),"originX":t.origin_x,"originY":t.origin_y,"width":t.width,"height":t.height}))
    }
}
impl ComponentFocusHost for Io {
    fn terminal_columns(&mut self) -> usize {
        self.record(json!({"op":"columns","value":self.columns}));
        self.columns
    }
    fn terminal_rows(&mut self) -> usize {
        self.record(json!({"op":"rows","value":self.rows}));
        self.rows
    }
    fn mounted_roots(&mut self) -> Vec<ComponentHandle> {
        self.record(json!({"op":"roots"}));
        self.roots.clone()
    }
    fn hide_cursor(&mut self) {
        self.record(json!({"op":"hideCursor"}));
    }
    fn request_render(&mut self) {
        self.record(json!({"op":"render"}));
    }
}
struct Harness {
    focus: ComponentFocus,
    io: Io,
}
impl Harness {
    fn new(case: &Value) -> Self {
        let trace = Rc::new(RefCell::new(Vec::new()));
        let mut objects: BTreeMap<String, ComponentHandle> = BTreeMap::new();
        let mut states = BTreeMap::new();
        let mut containers = BTreeMap::new();
        for spec in case["nodes"].as_array().unwrap() {
            let id = spec["id"].as_str().unwrap().to_owned();
            let mut value = spec.clone();
            value["focused"] = json!(false);
            let state = Rc::new(RefCell::new(value));
            let children = spec.get("children").map(|v| {
                Rc::new(RefCell::new(Container::new(
                    v.as_array()
                        .unwrap()
                        .iter()
                        .map(|id| objects[id.as_str().unwrap()].clone())
                        .collect(),
                )))
            });
            if let Some(children) = &children {
                containers.insert(id.clone(), children.clone());
            }
            let probe = Probe {
                id: id.clone(),
                state: state.clone(),
                trace: trace.clone(),
                children,
                structural: spec["kind"] == "container",
            };
            objects.insert(id.clone(), ComponentHandle::new(probe));
            states.insert(id, state);
        }
        Self {
            focus: ComponentFocus::new(),
            io: Io {
                trace,
                objects,
                states,
                containers,
                handles: BTreeMap::new(),
                visibility: BTreeMap::new(),
                rendered: Vec::new(),
                roots: Vec::new(),
                columns: 80,
                rows: 24,
                now: 1000,
                on_input: case["onInput"].clone(),
            },
        }
    }
    fn snapshot(&self, g: &ComponentGesture) -> Value {
        let io = &self.io;
        let restore = match self.focus.restore_state() {
            OverlayFocusRestore::Inactive => json!({"status":"inactive"}),
            OverlayFocusRestore::Eligible { overlay } => {
                json!({"status":"eligible","overlay":io.key(overlay)})
            }
            OverlayFocusRestore::Blocked {
                overlay,
                blocked_by,
                resume,
            } => {
                json!({"status":"blocked","overlay":io.key(overlay),"blockedBy":io.name(blocked_by),"resume":match resume {OverlayFocusResume::RestoreOverlay=>json!({"status":"restore-overlay"}),OverlayFocusResume::FocusTarget(target)=>json!({"status":"focus-target","target":target.as_ref().map(|c|io.name(c))})}})
            }
        };
        let entries:BTreeMap<_,_>=io.handles.iter().map(|(k,h)|(k.clone(),json!({"component":io.name(&h.component()),"preFocus":h.pre_focus().as_ref().map(|c|io.name(c)),"hidden":h.is_hidden(),"nonCapturing":h.non_capturing(),"focusOrder":h.focus_order() as u64,"bounds":bounds_value(h.stored_bounds())}))).collect();
        let flags: BTreeMap<_, _> = io
            .states
            .iter()
            .map(|(id, s)| {
                (
                    id.clone(),
                    if s.borrow()["focusable"] == false {
                        Value::Null
                    } else {
                        s.borrow()["focused"].clone()
                    },
                )
            })
            .collect();
        json!({"focused":self.focus.focused_component().as_ref().map(|c|io.name(c)),"restore":restore,"counter":self.focus.focus_order_counter() as u64,"stack":self.focus.overlays().iter().map(|h|io.key(h)).collect::<Vec<_>>(),"entries":entries,"flags":flags,"gesture":{"capture":io.target_value(g.capture()),"pressTarget":io.target_value(g.press_target()),"point":g.press_point().map(|p|json!({"x":p.x,"y":p.y})),"moved":g.press_moved(),"lastClick":g.last_click().map(|c|json!({"id":io.name(&c.component),"timestamp":c.timestamp_ms,"count":c.count,"x":c.x,"y":c.y}))}})
    }
    fn step(&mut self, g: &mut ComponentGesture, op: &Value) -> Value {
        let id = op["id"].as_str().unwrap_or("");
        let key = op["key"].as_str().unwrap_or("");
        let h = self.io.handles.get(key).cloned();
        match op["op"].as_str().unwrap() {
            "set" => self
                .focus
                .set_focus(self.io.objects.get(id).cloned(), &mut self.io),
            "show" => {
                let state = Rc::new(RefCell::new(json!({"value":op["visible"],"index":0})));
                self.io.visibility.insert(key.into(), state.clone());
                let visible=op.get("visible").map(|_|{
                    let trace=self.io.trace.clone();let key=key.to_owned();
                    Box::new(move|columns,rows|{
                        let mut s=state.borrow_mut();let mut v=s["value"].clone();
                        if let Some(values)=v.as_array(){let i=s["index"].as_u64().unwrap() as usize;s["index"]=json!(i+1);v=values[i.min(values.len()-1)].clone();}
                        let value=v==true||(v.is_object()&&columns>=v["columns"].as_u64().unwrap_or(0) as usize&&rows>=v["rows"].as_u64().unwrap_or(0) as usize);
                        trace.borrow_mut().push(json!({"op":"visible","key":key,"columns":columns,"rows":rows,"value":value}));value
                    }) as crate::tui::component_overlay::OverlayVisibility
                });
                let h = self.focus.show_overlay(
                    self.io.object(id),
                    ComponentOverlayOptions {
                        non_capturing: op["nonCapturing"].as_bool().unwrap_or(false),
                        visible,
                    },
                    &mut self.io,
                );
                self.io.handles.insert(key.into(), h);
            }
            "focus" => self.focus.focus(h.as_ref().unwrap(), &mut self.io).unwrap(),
            "hide" => self.focus.hide(h.as_ref().unwrap(), &mut self.io).unwrap(),
            "hidden" => self
                .focus
                .set_hidden(
                    h.as_ref().unwrap(),
                    op["value"].as_bool().unwrap(),
                    &mut self.io,
                )
                .unwrap(),
            "unfocus" => {
                let target = op
                    .get("target")
                    .map_or(OverlayUnfocusTarget::Fallback, |v| {
                        OverlayUnfocusTarget::Target(v.as_str().map(|s| self.io.object(s)))
                    });
                self.focus
                    .unfocus(h.as_ref().unwrap(), target, &mut self.io)
                    .unwrap();
            }
            "pop" => self.focus.hide_overlay(&mut self.io),
            "visibility" => {
                let mut s = self.io.visibility[key].borrow_mut();
                s["value"] = op["value"].clone();
                s["index"] = json!(0);
            }
            "size" => {
                self.io.columns = op["columns"].as_u64().unwrap() as usize;
                self.io.rows = op["rows"].as_u64().unwrap() as usize;
            }
            "roots" => {
                self.io.roots = op["ids"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| self.io.object(v.as_str().unwrap()))
                    .collect()
            }
            "children" => {
                self.io.containers[id].borrow_mut().children = op["ids"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| self.io.object(v.as_str().unwrap()))
                    .collect()
            }
            "input" => {
                // Actual TuiAltScreen's constructor installs handleViewportInput;
                // for these plain keys it calls shouldDeferViewportInputToOverlay
                // (line 725) before TuiBase's restoration block. Keep this probe
                // in the test host, NOT inside the reusable base focus controller.
                self.focus.is_overlay_focused(&mut self.io);
                if let Some(target) = self.focus.restore_before_input(&mut self.io) {
                    let name = self.io.name(&target);
                    let data = op["data"].as_str().unwrap();
                    if self.io.states[&name].borrow()["input"] != false {
                        target.with_mut(|c| c.handle_input(data));
                        // Required borrow boundary; commands finish synchronously before render.
                        let actions = self.io.on_input[&name][data]
                            .as_array()
                            .cloned()
                            .unwrap_or_default();
                        for action in actions {
                            self.step(g, &action);
                        }
                        self.io.record(json!({"op":"immediate"}));
                    }
                }
            }
            "has" => return json!(self.focus.has_overlay(&mut self.io)),
            "overlayFocused" => return json!(self.focus.is_overlay_focused(&mut self.io)),
            "isHidden" => return json!(h.as_ref().unwrap().is_hidden()),
            "isFocused" => return json!(self.focus.is_focused(h.as_ref().unwrap()).unwrap()),
            "bounds" => {
                let v = &op["value"];
                let b = (!v.is_null()).then(|| OverlayBounds {
                    row: v["row"].as_u64().unwrap() as usize,
                    col: v["col"].as_u64().unwrap() as usize,
                    width: v["width"].as_u64().unwrap() as usize,
                    height: v["height"].as_u64().unwrap() as usize,
                });
                self.focus
                    .set_rendered_bounds(h.as_ref().unwrap(), b)
                    .unwrap();
            }
            "getBounds" => {
                let mut b = self
                    .focus
                    .get_bounds(h.as_ref().unwrap(), &mut self.io)
                    .unwrap();
                let value = bounds_value(b);
                if op["mutate"] == true {
                    if let Some(b) = &mut b {
                        b.row = 999;
                    }
                }
                return value;
            }
            "owner" => {
                let c = self
                    .focus
                    .resolve_mouse_focus_target(&self.io.object(id), &mut self.io);
                return json!(self.io.name(&c));
            }
            "render" => {
                self.io
                    .object(id)
                    .with_mut(|c| c.render(op["width"].as_u64().unwrap() as usize));
            }
            "frame" => {
                self.io.rendered = op["layouts"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|r| RenderedComponentOverlay {
                        component: self.io.handles[r["key"].as_str().unwrap()].component(),
                        row: r["row"].as_i64().unwrap(),
                        col: r["col"].as_i64().unwrap(),
                        width: r["width"].as_u64().unwrap() as usize,
                        height: r["height"].as_u64().unwrap() as usize,
                    })
                    .collect()
            }
            "raw" => {
                let r = &op["raw"];
                g.handle_mouse_event(
                    self,
                    SgrMouseEvent {
                        button: r["button"].as_i64().unwrap(),
                        x: r["x"].as_i64().unwrap(),
                        y: r["y"].as_i64().unwrap(),
                        release: r["release"].as_bool().unwrap(),
                    },
                );
            }
            "time" => self.io.now = op["value"].as_i64().unwrap(),
            s => panic!("unknown operation {s}"),
        }
        Value::Null
    }
}
impl ComponentGestureHost for Harness {
    fn terminal_size(&self) -> (usize, usize) {
        self.io
            .record(json!({"op":"columns","value":self.io.columns}));
        self.io.record(json!({"op":"rows","value":self.io.rows}));
        (self.io.columns, self.io.rows)
    }
    fn now_ms(&mut self) -> i64 {
        self.io.record(json!({"op":"now","value":self.io.now}));
        self.io.now
    }
    fn handle_search_mouse_event(&mut self, r: SgrMouseEvent) -> bool {
        self.io.record(json!({"op":"search","raw":raw_value(r)}));
        false
    }
    fn dispatch_mouse_to_overlay(&mut self, e: &TuiMouseEvent) -> OverlayMouseDispatch {
        dispatch_mouse_to_overlay(&self.io.rendered, e)
    }
    fn handle_scroll_to_end_indicator_mouse_event(&mut self, r: SgrMouseEvent) -> bool {
        self.io.record(json!({"op":"indicator","raw":raw_value(r)}));
        false
    }
    fn handle_scrollbar_mouse_event(&mut self, r: SgrMouseEvent) -> bool {
        self.io.record(json!({"op":"scrollbar","raw":raw_value(r)}));
        false
    }
    fn scrollbar_drag_active(&self) -> bool {
        false
    }
    fn update_scrollbar_hover(&mut self, x: i64, y: i64) {
        self.io.record(json!({"op":"hover","x":x,"y":y}));
    }
    fn stop_scrollbar_hover(&mut self) {
        self.io.record(json!({"op":"stopHover"}));
    }
    fn dispatch_mouse_to_layout(&mut self, e: &TuiMouseEvent) -> Option<ComponentMouseResult> {
        self.io
            .record(json!({"op":"layout","event":event_value(e)}));
        None
    }
    fn resolve_mouse_focus_target(&mut self, c: &ComponentHandle) -> ComponentHandle {
        self.focus.resolve_mouse_focus_target(c, &mut self.io)
    }
    fn focused_component(&self) -> Option<ComponentHandle> {
        self.focus.focused_component()
    }
    fn set_focus(&mut self, c: ComponentHandle) {
        self.focus.set_focus(Some(c), &mut self.io);
    }
    fn clear_text_selection(&mut self) {
        self.io.record(json!({"op":"clearSelection"}));
    }
    fn request_render(&mut self) {
        self.io.request_render();
    }
    fn handle_right_click_paste(&mut self, r: SgrMouseEvent) -> bool {
        self.io.record(json!({"op":"paste","raw":raw_value(r)}));
        false
    }
    fn handle_selection_mouse_event(&mut self, r: SgrMouseEvent, _gesture: &mut ComponentGesture) {
        self.io.record(json!({"op":"selection","raw":raw_value(r)}));
    }
}
fn verify(group: &str) {
    let fixture: Value =
        serde_json::from_str(include_str!("../component_focus/fixtures.json")).unwrap();
    for case in fixture[group].as_array().unwrap() {
        let mut host = Harness::new(case);
        let mut g = ComponentGesture::default();
        for (index, op) in case["ops"].as_array().unwrap().iter().enumerate() {
            let value = host.step(&mut g, op);
            let state = host.snapshot(&g);
            let trace = std::mem::take(&mut *host.io.trace.borrow_mut());
            assert_eq!(
                json!({"value":value,"state":state,"trace":trace}),
                case["expected"][index],
                "{group}/{} step{index} {op}",
                case["name"]
            );
        }
    }
}
#[test]
fn actual_overlay_lifecycle_and_guards() {
    verify("lifecycle");
}
#[test]
fn actual_eligible_blocked_resume_and_ancestry() {
    verify("restore");
}
#[test]
fn actual_visibility_short_circuits_and_bounds() {
    verify("visibility");
}
#[test]
fn actual_independent_entry_identity_and_stale_handles() {
    verify("identity");
}
#[test]
fn actual_gesture_overlay_and_focus_composition() {
    verify("composed");
}
#[test]
fn actual_deterministic_focus_sequences() {
    verify("sequences");
}

// Native Rust contracts independent of the fixture's name/registry ownership.
struct QuietHost {
    roots: Vec<ComponentHandle>,
    calls: Vec<&'static str>,
}
impl ComponentFocusHost for QuietHost {
    fn terminal_columns(&mut self) -> usize {
        self.calls.push("columns");
        80
    }
    fn terminal_rows(&mut self) -> usize {
        self.calls.push("rows");
        24
    }
    fn mounted_roots(&mut self) -> Vec<ComponentHandle> {
        self.calls.push("roots");
        self.roots.clone()
    }
    fn hide_cursor(&mut self) {
        self.calls.push("cursor");
    }
    fn request_render(&mut self) {
        self.calls.push("render");
    }
}
fn quiet_host() -> QuietHost {
    QuietHost {
        roots: Vec::new(),
        calls: Vec::new(),
    }
}
struct Plain;
impl Component for Plain {
    fn render(&mut self, _: usize) -> Vec<String> {
        panic!("focus must not render")
    }
}
#[test]
fn foreign_controller_rejects_before_all_effects_but_stale_origin_is_valid() {
    let mut origin = ComponentFocus::new();
    let mut other = ComponentFocus::new();
    let mut host = quiet_host();
    let component = ComponentHandle::new(Plain);
    let handle = origin.show_overlay(
        component.clone(),
        ComponentOverlayOptions::default(),
        &mut host,
    );
    host.calls.clear();
    assert_eq!(other.hide(&handle, &mut host), Err(OverlayOwnerMismatch));
    assert_eq!(other.focus(&handle, &mut host), Err(OverlayOwnerMismatch));
    assert_eq!(
        other.set_hidden(&handle, true, &mut host),
        Err(OverlayOwnerMismatch)
    );
    assert_eq!(
        other.unfocus(&handle, OverlayUnfocusTarget::Target(None), &mut host),
        Err(OverlayOwnerMismatch)
    );
    assert_eq!(other.is_focused(&handle), Err(OverlayOwnerMismatch));
    assert_eq!(
        other.get_bounds(&handle, &mut host),
        Err(OverlayOwnerMismatch)
    );
    assert_eq!(
        other.set_rendered_bounds(&handle, None),
        Err(OverlayOwnerMismatch)
    );
    assert!(host.calls.is_empty());
    assert!(!handle.is_hidden());
    assert_eq!(other.focused_component(), None);
    origin.hide(&handle, &mut host).unwrap();
    host.calls.clear();
    origin.focus(&handle, &mut host).unwrap();
    origin.hide(&handle, &mut host).unwrap();
    assert!(host.calls.is_empty());
    origin.set_hidden(&handle, true, &mut host).unwrap();
    origin.set_hidden(&handle, false, &mut host).unwrap();
    assert_eq!(origin.focused_component(), Some(component));
    assert_eq!(host.calls, vec!["render", "render"]);
    assert!(origin.overlays().is_empty());
    assert_eq!(origin.restore_state(), &OverlayFocusRestore::Inactive);
}
#[test]
fn focused_pre_focus_and_blocked_targets_are_owned_without_registry() {
    let mut focus = ComponentFocus::new();
    let mut host = quiet_host();
    let base = ComponentHandle::new(Plain);
    let overlay = ComponentHandle::new(Plain);
    let replacement = ComponentHandle::new(Plain);
    let base_weak = base.downgrade();
    let overlay_weak = overlay.downgrade();
    let replacement_weak = replacement.downgrade();
    focus.set_focus(Some(base.clone()), &mut host);
    let handle = focus.show_overlay(
        overlay.clone(),
        ComponentOverlayOptions::default(),
        &mut host,
    );
    focus.set_focus(Some(replacement.clone()), &mut host);
    assert!(matches!(
        focus.restore_state(),
        OverlayFocusRestore::Blocked { .. }
    ));
    drop(base);
    drop(overlay);
    drop(replacement);
    assert!(base_weak.upgrade().is_some());
    assert!(overlay_weak.upgrade().is_some());
    assert!(replacement_weak.upgrade().is_some());
    focus.set_focus(None, &mut host);
    assert!(replacement_weak.upgrade().is_none());
    let retained = focus.restore_state().clone();
    focus.hide(&handle, &mut host).unwrap();
    drop(handle);
    drop(focus);
    // A raw restore snapshot also owns the entry and its preFocus, no integer registry.
    assert!(base_weak.upgrade().is_some());
    assert!(overlay_weak.upgrade().is_some());
    drop(retained);
    assert!(base_weak.upgrade().is_none());
    assert!(overlay_weak.upgrade().is_none());
}
struct Setter {
    value: bool,
    callback: Box<dyn FnMut(bool)>,
}
impl Component for Setter {
    fn render(&mut self, _: usize) -> Vec<String> {
        panic!("focus must not render")
    }
    fn is_focusable(&self) -> bool {
        true
    }
    fn focused(&self) -> bool {
        self.value
    }
    fn set_focused(&mut self, value: bool) {
        self.value = value;
        (self.callback)(value);
    }
}
#[test]
fn focus_setters_release_old_component_before_borrowing_new_and_run_on_same_target() {
    let order = Rc::new(RefCell::new(Vec::new()));
    let seen = order.clone();
    let (old, typed) = ComponentHandle::with_shared(Setter {
        value: false,
        callback: Box::new(move |v| seen.borrow_mut().push(("old", v))),
    });
    let weak = Rc::downgrade(&typed);
    let seen = order.clone();
    let new = ComponentHandle::new(Setter {
        value: false,
        callback: Box::new(move |v| {
            let old = weak.upgrade().unwrap();
            let guard = old.try_borrow_mut().expect("old setter borrow released");
            assert!(!guard.value);
            seen.borrow_mut().push(("new", v));
        }),
    });
    let mut focus = ComponentFocus::new();
    let mut host = quiet_host();
    focus.set_focus(Some(old), &mut host);
    focus.set_focus(Some(new.clone()), &mut host);
    focus.set_focus(Some(new), &mut host);
    assert_eq!(
        *order.borrow(),
        vec![
            ("old", true),
            ("old", false),
            ("new", true),
            ("new", false),
            ("new", true)
        ]
    );
    assert!(host.calls.is_empty());
}
struct MountedProbe {
    parent: std::rc::Weak<RefCell<Container>>,
    called: Rc<RefCell<bool>>,
}
impl Component for MountedProbe {
    fn render(&mut self, _: usize) -> Vec<String> {
        panic!("mounted check must not render")
    }
    fn is_container_component(&self) -> bool {
        let parent = self.parent.upgrade().unwrap();
        let _guard = parent
            .try_borrow_mut()
            .expect("parent released during mounted traversal");
        *self.called.borrow_mut() = true;
        false
    }
}
#[test]
fn mounted_lookup_descends_live_tree_without_holding_parent_borrow() {
    let (parent, typed) = ComponentHandle::with_shared(Container::default());
    let called = Rc::new(RefCell::new(false));
    typed
        .borrow_mut()
        .add_child(ComponentHandle::new(MountedProbe {
            parent: Rc::downgrade(&typed),
            called: called.clone(),
        }));
    let mut focus = ComponentFocus::new();
    let mut host = quiet_host();
    host.roots.push(parent);
    let overlay = ComponentHandle::new(Plain);
    let blocker = ComponentHandle::new(Plain);
    focus.show_overlay(
        overlay.clone(),
        ComponentOverlayOptions::default(),
        &mut host,
    );
    focus.set_focus(Some(blocker), &mut host);
    focus.set_focus(Some(ComponentHandle::new(Plain)), &mut host);
    assert!(*called.borrow());
    assert_eq!(focus.focused_component(), Some(overlay));
}
