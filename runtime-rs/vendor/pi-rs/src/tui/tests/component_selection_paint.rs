//! Actual-source paint replay, using the verified selection host scaffold.
//! External Intl segmentation is
//! recorded as a required input service, not used as an expected range/text.
//! No tests open URLs, access the clipboard, run timer workers or use real IO.
use crate::tui::component::{
    Component, TuiMouseButton, TuiMouseEvent, TuiMouseEventResult, TuiMouseEventType,
};
use crate::tui::component_focus::*;
use crate::tui::component_gesture::*;
use crate::tui::component_mouse::*;
use crate::tui::component_overlay::{dispatch_mouse_to_overlay, RenderedComponentOverlay};
use crate::tui::component_selection::*;
use crate::tui::component_selection_paint::{apply_selection, apply_selection_highlight};
use crate::tui::components::container::Container;
use crate::tui::components::mouse_region::MouseRegion;
use crate::tui::components::scroll_view::{
    ScrollHandle, ScrollView, ScrollViewOptions, ScrollViewScrollToOptions, ScrollViewScrollbar,
};
use crate::tui::layout::{render_layout_frame, LayoutBox, LayoutFrame, LayoutRect};
use crate::tui::rendered_lines::RenderedLines;
use crate::tui::viewport_mouse::SgrMouseEvent;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
type Trace = Arc<Mutex<Vec<Value>>>;
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

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().to_owned())
        .collect()
}
fn raw(v: &Value) -> SgrMouseEvent {
    SgrMouseEvent {
        button: v["button"].as_i64().unwrap(),
        x: v["x"].as_i64().unwrap(),
        y: v["y"].as_i64().unwrap(),
        release: v["release"].as_bool().unwrap(),
    }
}
fn response(
    trace: &Trace,
    id: &str,
    state: &RefCell<Value>,
    e: &TuiMouseEvent,
) -> Option<TuiMouseEventResult> {
    let event = event_value(e);
    trace
        .lock()
        .unwrap()
        .push(json!({"op":"mouse","id":id,"event":event}));
    let state = state.borrow();
    let v = &state["responses"][event["type"].as_str().unwrap()];
    (!v.is_null()).then(|| TuiMouseEventResult {
        handled: v["handled"].as_bool().unwrap_or(false),
        focus: v["focus"].as_bool().unwrap_or(false),
        capture: v["capture"].as_bool().unwrap_or(false),
        render: v["render"].as_bool(),
    })
}
struct Leaf {
    id: String,
    state: Rc<RefCell<Value>>,
    trace: Trace,
}
impl Component for Leaf {
    fn render(&mut self, _: usize) -> Vec<String> {
        strings(&self.state.borrow()["lines"])
    }
    fn handle_mouse(&mut self, e: &TuiMouseEvent) -> Option<TuiMouseEventResult> {
        response(&self.trace, &self.id, &self.state, e)
    }
    fn is_focusable(&self) -> bool {
        true
    }
    fn focused(&self) -> bool {
        self.state.borrow()["focused"].as_bool().unwrap()
    }
    fn set_focused(&mut self, value: bool) {
        self.trace
            .lock()
            .unwrap()
            .push(json!({"op":"focused","id":self.id,"value":value}));
        self.state.borrow_mut()["focused"] = json!(value);
    }
}
struct Io {
    trace: Trace,
    columns: usize,
    rows: usize,
    now: i64,
    next_timer: u64,
    objects: BTreeMap<String, ComponentHandle>,
    states: BTreeMap<String, Rc<RefCell<Value>>>,
    containers: BTreeMap<String, Rc<RefCell<Container>>>,
    scrolls: BTreeMap<String, ScrollHandle>,
    frame: Option<LayoutFrame>,
    paint_frame: Option<LayoutFrame>,
    screen: Vec<String>,
    rendered: Vec<RenderedComponentOverlay>,
    handles: BTreeMap<String, ComponentOverlayHandle>,
    url_mode: Option<String>,
    segments: Value,
}
impl Io {
    fn record(&self, v: Value) {
        self.trace.lock().unwrap().push(v);
    }
    fn name(&self, c: &ComponentHandle) -> String {
        self.objects
            .iter()
            .find(|(_, v)| *v == c)
            .unwrap()
            .0
            .clone()
    }
    fn point(&self, p: Option<&SelectionPoint>) -> Value {
        p.map_or(Value::Null,|p| json!({"row":p.row,"col":p.col,"scrollView":p.scroll_view.as_ref().map(|s| self.scrolls.iter().find(|(_,v)| *v==s).unwrap().0),"boundary":p.boundary}))
    }
    fn range(&self, r: Option<&SelectionRange>) -> Value {
        r.map_or(
            Value::Null,
            |r| json!({"start":self.point(Some(&r.start)),"end":self.point(Some(&r.end))}),
        )
    }
    fn target(&self, t: Option<&ComponentMouseTarget>) -> Value {
        t.map_or(Value::Null,|t| json!({"id":self.name(&t.component),"originX":t.origin_x,"originY":t.origin_y,"width":t.width,"height":t.height}))
    }
}
impl ComponentFocusHost for Io {
    fn terminal_columns(&mut self) -> usize {
        self.columns
    }
    fn terminal_rows(&mut self) -> usize {
        self.rows
    }
    fn mounted_roots(&mut self) -> Vec<ComponentHandle> {
        self.objects.values().cloned().collect()
    }
    fn hide_cursor(&mut self) {
        self.record(json!({"op":"hideCursor"}));
    }
    fn request_render(&mut self) {
        self.record(json!({"op":"render"}));
    }
}
struct Harness {
    io: Io,
    focus: ComponentFocus,
    selection: Option<ComponentSelection>,
}
impl Harness {
    fn new(spec: &Value, segments: &Value) -> Self {
        let trace = Trace::default();
        let mut objects: BTreeMap<String, ComponentHandle> = BTreeMap::new();
        let mut states = BTreeMap::new();
        let mut containers = BTreeMap::new();
        let mut scrolls = BTreeMap::new();
        for node in spec["nodes"].as_array().unwrap() {
            let id = node["id"].as_str().unwrap().to_owned();
            let state = Rc::new(RefCell::new(node.clone()));
            let handle = match node["kind"].as_str().unwrap() {
                "leaf" => {
                    state.borrow_mut()["focused"] = json!(false);
                    ComponentHandle::new(Leaf {
                        id: id.clone(),
                        state: state.clone(),
                        trace: trace.clone(),
                    })
                }
                "container" => {
                    let children = strings(&node["children"])
                        .into_iter()
                        .map(|s| objects[&s].clone())
                        .collect();
                    let (h, c) = ComponentHandle::with_shared(Container::new(children));
                    containers.insert(id.clone(), c);
                    h
                }
                "region" => {
                    let id = id.clone();
                    let state = state.clone();
                    let trace = trace.clone();
                    ComponentHandle::new(MouseRegion::new(
                        objects[node["child"].as_str().unwrap()].clone(),
                        Box::new(move |e| response(&trace, &id, &state, e)),
                    ))
                }
                "scroll" => {
                    let view = ScrollView::new(
                        Box::new(objects[node["child"].as_str().unwrap()].clone()),
                        ScrollViewOptions {
                            scrollbar: Some(ScrollViewScrollbar::Hidden),
                            follow_end: node["followEnd"].as_bool().unwrap_or(false),
                            timer_scheduler: Some(Arc::new(|_, _| {
                                panic!("hidden scrollbars must not start worker timers")
                            })),
                            ..ScrollViewOptions::default()
                        },
                    );
                    scrolls.insert(id.clone(), view.state());
                    ComponentHandle::new(view)
                }
                kind => panic!("bad node {kind}"),
            };
            objects.insert(id.clone(), handle);
            states.insert(id, state);
        }
        let mut selection = ComponentSelection::default();
        selection.set_copy_on_select(spec["copyOnSelect"].as_bool().unwrap_or(true));
        Self {
            io: Io {
                trace,
                columns: spec["columns"].as_u64().unwrap_or(20) as usize,
                rows: spec["rows"].as_u64().unwrap_or(4) as usize,
                now: 1000,
                next_timer: 0,
                objects,
                states,
                containers,
                scrolls,
                frame: None,
                paint_frame: None,
                screen: strings(&spec["screen"]),
                rendered: vec![],
                handles: BTreeMap::new(),
                url_mode: spec["urlMode"].as_str().map(str::to_owned),
                segments: segments.clone(),
            },
            focus: ComponentFocus::default(),
            selection: Some(selection),
        }
    }
    // Synchronous temporary ownership, never a RefCell borrow across callback.
    // The empty slot catches unsupported self-reentry rather than dropping it.
    fn with_selection<T>(&mut self, f: impl FnOnce(&mut ComponentSelection, &mut Self) -> T) -> T {
        let mut selection = self.selection.take().expect("selection host re-entry");
        let value = f(&mut selection, self);
        self.selection = Some(selection);
        value
    }
    fn snapshot(&self, g: &ComponentGesture) -> Value {
        let sel = self.selection.as_ref().unwrap();
        let s = sel.state();
        let last=s.last_click.as_ref().map(|c| json!({"timestamp":c.timestamp,"count":c.count,"row":c.row,"scrollView":c.scroll_view.as_ref().map(|s|self.io.scrolls.iter().find(|(_,v)| *v==s).unwrap().0),"wordStart":c.word_start,"wordEnd":c.word_end}));
        let flags: BTreeMap<_, _> = self
            .io
            .states
            .iter()
            .filter(|(_, s)| s.borrow()["kind"] == "leaf")
            .map(|(id, s)| (id, s.borrow()["focused"].clone()))
            .collect();
        let scrolls: BTreeMap<_, _> = self
            .io
            .scrolls
            .iter()
            .map(|(id, s)| {
                let s = s.snapshot();
                (id, json!({"top":s.scroll_top,"following":s.following_end}))
            })
            .collect();
        json!({"anchor":self.io.point(s.anchor.as_ref()),"focus":self.io.point(s.focus.as_ref()),"granularity":match s.granularity {SelectionGranularity::Character=>"character",SelectionGranularity::Word=>"word",SelectionGranularity::Line=>"line"},"initialRange":self.io.range(s.initial_range.as_ref()),"lastClick":last,"dragPointer":s.drag_pointer.map(|(x,y)|json!({"x":x,"y":y})),"direction":s.auto_scroll_direction,"timer":s.timer,"pressActive":s.press_active,"pressedUrl":s.pressed_url,"dragged":s.dragged,"copyOnSelect":sel.copy_on_select(),"bounds":self.io.range(sel.bounds().as_ref()),"text":sel.active_text(self),"focused":self.focus.focused_component().as_ref().map(|c|self.io.name(c)),"flags":flags,"scrolls":scrolls,"gesture":{"capture":self.io.target(g.capture()),"pressTarget":self.io.target(g.press_target()),"point":g.press_point().map(|p|json!({"x":p.x,"y":p.y})),"moved":g.press_moved(),"lastClick":g.last_click().map(|c|json!({"id":self.io.name(&c.component),"timestamp":c.timestamp_ms,"count":c.count,"x":c.x,"y":c.y}))}})
    }
    fn step(&mut self, g: &mut ComponentGesture, op: &Value) -> Value {
        let id = op["id"].as_str().unwrap_or("");
        match op["op"].as_str().unwrap() {
            "savePaintFrame" => self.io.paint_frame = self.io.frame.clone(),
            "highlight" => return json!(apply_selection_highlight(op["text"].as_str().unwrap())),
            "paint" | "paintBounds" => {
                let before = self.snapshot(g);
                let input = op
                    .get("screen")
                    .map(strings)
                    .unwrap_or_else(|| self.io.screen.clone());
                let frame = match op["layout"].as_str() {
                    Some("none") => None,
                    Some("saved") => self.io.paint_frame.as_ref(),
                    _ => self.io.frame.as_ref(),
                };
                let layout_before = format!("{frame:?}");
                let selection = self.selection.as_ref().unwrap();
                let painted = if op["op"] == "paint" {
                    selection.apply_selection(&input, frame, self.io.columns)
                } else {
                    let point = |p: &Value| SelectionPoint {
                        row: p["row"].as_u64().unwrap() as usize,
                        col: p["col"].as_u64().unwrap() as usize,
                        boundary: p["boundary"].as_bool().unwrap_or(false),
                        scroll_view: p["scrollView"]
                            .as_str()
                            .map(|id| self.io.scrolls[id].clone()),
                    };
                    let bounds = op["bounds"].as_object().map(|_| SelectionRange {
                        start: point(&op["bounds"]["start"]),
                        end: point(&op["bounds"]["end"]),
                    });
                    apply_selection(&input, bounds.as_ref(), frame, self.io.columns)
                };
                assert_eq!(layout_before, format!("{frame:?}"), "paint mutated frame");
                assert_eq!(
                    before,
                    self.snapshot(g),
                    "paint mutated selection or scroll"
                );
                assert_eq!(
                    input,
                    op.get("screen")
                        .map(strings)
                        .unwrap_or_else(|| self.io.screen.clone())
                );
                return json!(painted);
            }
            "selection" => self.with_selection(|s, h| s.handle_mouse_event(h, g, raw(&op["raw"]))),
            "raw" => g.handle_mouse_event(self, raw(&op["raw"])),
            "tick" => self.with_selection(|s, h| s.auto_scroll_tick(h)),
            "clear" => self.with_selection(|s, h| s.clear(h)),
            "stopAuto" => self.with_selection(|s, h| s.stop_auto_scroll(h)),
            "text" => return json!(self.selection.as_ref().unwrap().active_text(self)),
            "has" => return json!(self.selection.as_ref().unwrap().active_text(self).is_some()),
            "copy" => return json!(self.with_selection(|s, h| s.request_copy_active_selection(h))),
            "copyOnSelect" => {
                let s = self.selection.as_mut().unwrap();
                s.set_copy_on_select(op["value"].as_bool().unwrap());
                return json!(s.copy_on_select());
            }
            "time" => self.io.now = op["value"].as_i64().unwrap(),
            "screen" => self.io.screen = strings(&op["lines"]),
            "size" => {
                self.io.columns = op["columns"].as_u64().unwrap() as usize;
                self.io.rows = op["rows"].as_u64().unwrap() as usize;
            }
            "frame" => {
                if let Some(specs) = op["boxes"].as_array() {
                    let rect = |v: &Value| LayoutRect {
                        x: v["x"].as_i64().unwrap(),
                        y: v["y"].as_i64().unwrap(),
                        width: v["width"].as_u64().unwrap() as usize,
                        height: v["height"].as_u64().unwrap() as usize,
                    };
                    let mut boxes: Vec<_> = specs
                        .iter()
                        .map(|b| {
                            let id = b["id"].as_str().unwrap();
                            LayoutBox {
                                component_path: vec![],
                                component: Some(self.io.objects[id].clone()),
                                rect: rect(&b["rect"]),
                                clip: rect(&b["clip"]),
                                children: b["children"]
                                    .as_array()
                                    .unwrap()
                                    .iter()
                                    .map(|v| v.as_u64().unwrap() as usize)
                                    .collect(),
                                parent: None,
                                lines: None,
                                line_offset: None,
                                scroll_view: self.io.scrolls.get(id).cloned(),
                                scroll_content_lines: b
                                    .get("lines")
                                    .map(|l| RenderedLines::dense(strings(l))),
                                layer: b["layer"].as_i64().unwrap_or(0) as i32,
                            }
                        })
                        .collect();
                    for i in 0..boxes.len() {
                        for child in boxes[i].children.clone() {
                            boxes[child].parent = Some(i);
                        }
                    }
                    self.io.frame = Some(LayoutFrame {
                        root: 0,
                        boxes,
                        width: self.io.columns,
                        height: self.io.rows,
                        lines: RenderedLines::dense(vec![]),
                        primary_scroll_view: None,
                    });
                } else {
                    self.io.frame = None;
                }
            }
            "renderFrame" => {
                let trace = self.io.trace.clone();
                let mut root = self.io.objects[id].clone();
                let frame = render_layout_frame(
                    &mut root,
                    self.io.columns,
                    self.io.rows,
                    Arc::new(move || trace.lock().unwrap().push(json!({"op":"layoutRender"}))),
                );
                if op["screen"] == true {
                    self.io.screen = (0..frame.lines.len())
                        .map(|i| frame.lines.get(i).unwrap_or("").to_owned())
                        .collect();
                }
                self.io.frame = Some(frame);
            }
            "columns" => {
                let point = |v: &Value| SelectionPoint {
                    row: v["row"].as_u64().unwrap() as usize,
                    col: v["col"].as_u64().unwrap() as usize,
                    scroll_view: None,
                    boundary: v["boundary"].as_bool().unwrap_or(false),
                };
                let range = SelectionRange {
                    start: point(&op["start"]),
                    end: point(&op["end"]),
                };
                let (start, end) = ComponentSelection::selection_columns(
                    op["line"].as_str().unwrap(),
                    op["row"].as_u64().unwrap() as usize,
                    &range,
                    op["min"].as_u64().unwrap() as usize,
                    op["max"].as_u64().unwrap() as usize,
                );
                return json!({"start":start,"end":end});
            }
            "scroll" => {
                let trace = self.io.trace.clone();
                let sid = id.to_owned();
                let s = &self.io.scrolls[id];
                s.update_layout(
                    op["content"].as_u64().unwrap() as usize,
                    op["viewport"].as_u64().unwrap() as usize,
                    Arc::new(move || {
                        trace
                            .lock()
                            .unwrap()
                            .push(json!({"op":"scrollRender","id":sid}))
                    }),
                );
                s.scroll_to(
                    op["top"].as_u64().unwrap() as usize,
                    ScrollViewScrollToOptions::default(),
                );
            }
            "point" => {
                let p = self.selection.as_ref().unwrap().selection_point(
                    self,
                    raw(&op["raw"]),
                    self.io.scrolls.get(id),
                );
                return self.io.point(Some(&p));
            }
            "word" | "line" => {
                let p = SelectionPoint {
                    row: op["row"].as_u64().unwrap() as usize,
                    col: op["col"].as_u64().unwrap() as usize,
                    scroll_view: self.io.scrolls.get(id).cloned(),
                    boundary: false,
                };
                let result = self.with_selection(|s, h| {
                    if op["op"] == "word" {
                        s.word_selection(h, &p)
                    } else {
                        Some(s.line_selection(h, &p))
                    }
                });
                return self.io.range(result.as_ref());
            }
            "render" => {
                self.io.objects[id].with_mut(|c| {
                    c.render(op["width"].as_u64().map_or(self.io.columns, |v| v as usize));
                });
            }
            "responses" => self.io.states[id].borrow_mut()["responses"] = op["value"].clone(),
            "lines" => self.io.states[id].borrow_mut()["lines"] = op["value"].clone(),
            "children" => {
                self.io.containers[id].borrow_mut().children = strings(&op["ids"])
                    .iter()
                    .map(|id| self.io.objects[id].clone())
                    .collect()
            }
            "focus" => self
                .focus
                .set_focus(self.io.objects.get(id).cloned(), &mut self.io),
            "show" => {
                let h = self.focus.show_overlay(
                    self.io.objects[id].clone(),
                    ComponentOverlayOptions {
                        non_capturing: op["nonCapturing"].as_bool().unwrap_or(true),
                        visible: None,
                    },
                    &mut self.io,
                );
                self.io
                    .handles
                    .insert(op["key"].as_str().unwrap().to_owned(), h);
            }
            "hide" => {
                let h = self.io.handles[op["key"].as_str().unwrap()].clone();
                self.focus.hide(&h, &mut self.io).unwrap();
            }
            "hidden" => {
                let h = self.io.handles[op["key"].as_str().unwrap()].clone();
                self.focus
                    .set_hidden(&h, op["value"].as_bool().unwrap(), &mut self.io)
                    .unwrap();
            }
            "overlayFrame" => {
                self.io.rendered = op["layouts"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|b| RenderedComponentOverlay {
                        component: self.io.handles[b["key"].as_str().unwrap()].component(),
                        row: b["row"].as_i64().unwrap(),
                        col: b["col"].as_i64().unwrap(),
                        width: b["width"].as_u64().unwrap() as usize,
                        height: b["height"].as_u64().unwrap() as usize,
                    })
                    .collect()
            }
            name => panic!("unknown op {name}"),
        }
        Value::Null
    }
}
impl ComponentGestureHost for Harness {
    fn terminal_size(&self) -> (usize, usize) {
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
        dispatch_mouse_to_layout(self.io.frame.as_ref(), e)
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
        self.with_selection(|s, h| s.clear(h));
    }
    fn request_render(&mut self) {
        self.io.request_render();
    }
    fn handle_right_click_paste(&mut self, r: SgrMouseEvent) -> bool {
        self.io.record(json!({"op":"paste","raw":raw_value(r)}));
        false
    }
    fn handle_selection_mouse_event(&mut self, r: SgrMouseEvent, g: &mut ComponentGesture) {
        self.with_selection(|s, h| s.handle_mouse_event(h, g, r));
    }
}
impl ComponentSelectionHost for Harness {
    fn selection_frame(&self) -> Option<&LayoutFrame> {
        self.io.frame.as_ref()
    }
    fn selection_screen(&self) -> &[String] {
        &self.io.screen
    }
    fn selection_has_overlay(&mut self) -> bool {
        self.io.record(json!({"op":"hasOverlay"}));
        self.focus.has_overlay(&mut self.io)
    }
    fn selection_word_segments(&mut self, line: &str) -> Vec<SelectionWordSegment> {
        self.io.record(json!({"op":"segments","line":line}));
        self.io
            .segments
            .get(line)
            .expect("unrecorded external Intl segmentation input")
            .as_array()
            .unwrap()
            .iter()
            .map(|s| SelectionWordSegment {
                text: s["text"].as_str().unwrap().to_owned(),
                is_word_like: s["isWordLike"].as_bool().unwrap(),
            })
            .collect()
    }
    fn start_selection_interval(&mut self, millis: u64) -> u64 {
        self.io.next_timer += 1;
        let token = self.io.next_timer;
        self.io
            .record(json!({"op":"interval","token":token,"millis":millis}));
        self.io.record(json!({"op":"unref","token":token}));
        token
    }
    fn cancel_selection_interval(&mut self, token: u64) {
        self.io.record(json!({"op":"cancel","token":token}));
    }
    fn has_url_opener(&self) -> bool {
        self.io.url_mode.is_some()
    }
    fn open_selection_url(&mut self, url: &str) -> Result<(), String> {
        self.io.record(json!({"op":"url","url":url}));
        if self.io.url_mode.as_deref() == Some("error") {
            Err("controlled opener failure".into())
        } else {
            Ok(())
        }
    }
    fn request_copy_text(&mut self, text: String) {
        self.io.record(json!({"op":"copy","text":text}));
    }
}
fn fixture() -> Value {
    serde_json::from_str(include_str!("../component_selection_paint/fixtures.json")).unwrap()
}
fn verify(group: &str) {
    let f = fixture();
    for case in f[group].as_array().unwrap() {
        let mut h = Harness::new(case, &f["wordSegments"]);
        let mut g = ComponentGesture::default();
        for (i, op) in case["ops"].as_array().unwrap().iter().enumerate() {
            let value = h.step(&mut g, op);
            let state = h.snapshot(&g);
            let trace = std::mem::take(&mut *h.io.trace.lock().unwrap());
            let actual = json!({"value":value,"state":state,"trace":trace});
            assert_eq!(
                actual, case["expected"][i],
                "{group}/{} step {i} op {op}",
                case["name"]
            );
        }
    }
}
#[test]
fn actual_selection_paint_highlights() {
    verify("highlights");
}
#[test]
fn actual_selection_paint_screen() {
    verify("screen");
}
#[test]
fn actual_selection_paint_scroll() {
    verify("scroll");
}
#[test]
fn actual_selection_paint_composed() {
    verify("composed");
}
#[test]
fn actual_selection_paint_sequences() {
    verify("sequences");
}

#[test]
fn inverse_is_reasserted_after_internal_reset_without_normalizing_tokens() {
    assert_eq!(
        apply_selection_highlight("\x1b[1mal\x1b[0mpha"),
        "\x1b[7m\x1b[1m\x1b[7mal\x1b[0m\x1b[7mpha\x1b[27m"
    );
    assert_eq!(
        apply_selection_highlight("\x1b]8;;https://example.com/m\x07X\x1b]8;;\x07"),
        "\x1b[7m\x1b]8;;https://example.com/m\x07X\x1b]8;;\x07\x1b[27m"
    );
}

#[test]
fn normalized_bounds_keep_scroll_identity_after_registry_and_current_frame_removal() {
    let f = fixture();
    let case = f["composed"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "forward-reset")
        .unwrap();
    let mut h = Harness::new(case, &f["wordSegments"]);
    let mut g = ComponentGesture::default();
    for op in case["ops"].as_array().unwrap().iter().take(3) {
        h.step(&mut g, op);
    }
    let selection = h.selection.take().unwrap();
    let saved = h.io.frame.take().unwrap();
    let expected = selection.apply_selection(&h.io.screen, Some(&saved), h.io.columns);
    assert_ne!(expected, h.io.screen);
    h.io.scrolls.remove("scroll");
    h.io.objects.remove("scroll");
    assert_eq!(
        selection.apply_selection(&h.io.screen, Some(&saved), h.io.columns),
        expected
    );
    assert_eq!(
        selection.apply_selection(&h.io.screen, None, h.io.columns),
        h.io.screen
    );
    assert!(selection.bounds().unwrap().start.scroll_view.is_some());
}

#[test]
fn negative_projected_start_row_does_not_restrict_visible_first_row_columns() {
    let f = fixture();
    let case = f["scroll"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "negative-start-row-not-clamped")
        .unwrap();
    let mut h = Harness::new(case, &f["wordSegments"]);
    let mut g = ComponentGesture::default();
    for op in case["ops"].as_array().unwrap().iter().take(2) {
        h.step(&mut g, op);
    }
    let scroll = Some(h.io.scrolls["scroll"].clone());
    let bounds = SelectionRange {
        start: SelectionPoint {
            row: 0,
            col: 15,
            boundary: false,
            scroll_view: scroll.clone(),
        },
        end: SelectionPoint {
            row: 2,
            col: 3,
            boundary: false,
            scroll_view: scroll,
        },
    };
    let painted = apply_selection(
        &h.io.screen,
        Some(&bounds),
        h.io.frame.as_ref(),
        h.io.columns,
    );
    assert_eq!(painted[0], "\x1b[7mHEAD\x1b[27mER");
    assert_eq!(painted[1..], h.io.screen[1..]);
}
