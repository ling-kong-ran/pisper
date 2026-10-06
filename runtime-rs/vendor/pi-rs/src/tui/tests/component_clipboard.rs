//! Actual-source clipboard/flash differential tests; services are controlled inputs.
use crate::tui::component::{Component, TuiMouseEvent};
use crate::tui::component_clipboard::{
    copy_text_to_clipboard, ClipboardCompletion, ClipboardDelivery, ClipboardResult,
    ComponentClipboardHost,
};
use crate::tui::component_gesture::{ComponentGesture, ComponentGestureHost, OverlayMouseDispatch};
use crate::tui::component_mouse::{ComponentHandle, ComponentMouseResult};
use crate::tui::component_selection::{
    ComponentSelection, ComponentSelectionHost, SelectionGranularity, SelectionPoint,
    SelectionRange, SelectionWordSegment,
};
use crate::tui::components::alt_screen_flash::{
    AltScreenFlashContainer, AltScreenFlashHost, FlashId,
};
use crate::tui::layout::LayoutFrame;
use crate::tui::viewport_mouse::SgrMouseEvent;
use futures::channel::oneshot;
use futures::task::noop_waker;
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::future::ready;
use std::rc::Rc;
use std::task::{Context, Poll};

type Trace = Rc<RefCell<Vec<Value>>>;
type DeliverySender = oneshot::Sender<Result<ClipboardResult, String>>;
fn record(trace: &Trace, value: Value) {
    trace.borrow_mut().push(value);
}
fn wire(value: f64) -> Value {
    if value.is_nan() {
        json!("NaN")
    } else if value == f64::INFINITY {
        json!("Infinity")
    } else if value == f64::NEG_INFINITY {
        json!("-Infinity")
    } else if value.fract() == 0.0 && value.abs() < i64::MAX as f64 {
        json!(value as i64)
    } else {
        json!(value)
    }
}
fn number(value: &Value) -> f64 {
    match value.as_str() {
        Some("NaN") => f64::NAN,
        Some("Infinity") => f64::INFINITY,
        Some("-Infinity") => f64::NEG_INFINITY,
        _ => value.as_f64().unwrap(),
    }
}
fn delivery_value(value: &Value) -> ClipboardResult {
    if let Some(b) = value.as_bool() {
        ClipboardResult::Boolean(b)
    } else if let Some(s) = value.as_str() {
        ClipboardResult::Message(s.to_owned())
    } else {
        ClipboardResult::Other
    }
}
#[derive(Clone)]
struct Timer {
    id: FlashId,
    duration: f64,
    active: bool,
    unreferenced: bool,
}
#[derive(Default)]
struct TimerHost {
    trace: Trace,
    timers: BTreeMap<u64, Timer>,
    next: u64,
}
impl AltScreenFlashHost for TimerHost {
    fn set_flash_timeout(&mut self, id: FlashId, duration_ms: f64) -> u64 {
        self.next += 1;
        let timer = self.next;
        record(
            &self.trace,
            json!({"op":"timeout","timer":timer,"duration":wire(duration_ms)}),
        );
        self.timers.insert(
            timer,
            Timer {
                id,
                duration: duration_ms,
                active: true,
                unreferenced: false,
            },
        );
        timer
    }
    fn unref_flash_timeout(&mut self, timer: u64) {
        self.timers.get_mut(&timer).unwrap().unreferenced = true;
        record(&self.trace, json!({"op":"unref","timer":timer}));
    }
    fn clear_flash_timeout(&mut self, timer: u64) {
        record(&self.trace, json!({"op":"cancel","timer":timer}));
        self.timers.get_mut(&timer).unwrap().active = false;
    }
    fn request_flash_render(&mut self) {
        record(&self.trace, json!({"op":"render"}));
    }
}
struct Services {
    injected: bool,
    mode: RefCell<Value>,
    next_request: Cell<u64>,
    trace: Trace,
    deliveries: RefCell<BTreeMap<u64, Option<DeliverySender>>>,
    flashes: RefCell<AltScreenFlashContainer>,
    timers: RefCell<TimerHost>,
    write_error: RefCell<Option<String>>,
    flash_error: RefCell<Option<String>>,
}
impl Services {
    fn new(spec: &Value) -> Rc<Self> {
        let trace = Trace::default();
        Rc::new(Self {
            injected: spec["injected"].as_bool().unwrap(),
            mode: RefCell::new(spec["mode"].clone()),
            next_request: Cell::new(0),
            trace: trace.clone(),
            deliveries: RefCell::new(BTreeMap::new()),
            flashes: RefCell::new(AltScreenFlashContainer::default()),
            timers: RefCell::new(TimerHost {
                trace,
                ..TimerHost::default()
            }),
            write_error: RefCell::new(None),
            flash_error: RefCell::new(None),
        })
    }
    fn settle(&self, request: u64, value: Result<ClipboardResult, String>) {
        if let Some(sender) = self
            .deliveries
            .borrow_mut()
            .get_mut(&request)
            .expect("known request")
            .take()
        {
            sender.send(value).expect("retained future");
        }
    }
    fn fire(&self, token: u64) {
        record(&self.trace, json!({"op":"fire","timer":token}));
        let id = {
            let mut timers = self.timers.borrow_mut();
            let t = timers.timers.get_mut(&token).unwrap();
            t.active = false;
            t.id.clone()
        };
        self.flashes
            .borrow_mut()
            .expire(&id, &mut *self.timers.borrow_mut());
    }
}
impl ComponentClipboardHost for Services {
    type Error = String;
    fn has_copy_selection(&self) -> bool {
        self.injected
    }
    fn copy_selection(&self, text: String) -> Result<ClipboardDelivery<String>, String> {
        assert!(self.injected, "no accidental fallback service");
        let request = self.next_request.get() + 1;
        self.next_request.set(request);
        record(
            &self.trace,
            json!({"op":"copy","request":request,"text":text}),
        );
        let mode = self.mode.borrow();
        match mode["kind"].as_str().unwrap() {
            "throw" => Err(mode["error"].as_str().unwrap().to_owned()),
            "reject" => Ok(Box::pin(ready(Err(mode["error"]
                .as_str()
                .unwrap()
                .to_owned())))),
            "ready" => Ok(Box::pin(ready(Ok(delivery_value(&mode["value"]))))),
            "deferred" => {
                let (tx, rx) = oneshot::channel();
                self.deliveries.borrow_mut().insert(request, Some(tx));
                Ok(Box::pin(async move {
                    rx.await.expect("service retained until settled")
                }))
            }
            v => panic!("unknown service mode {v}"),
        }
    }
    fn write_clipboard_sequence(&self, sequence: &str) -> Result<(), String> {
        record(&self.trace, json!({"op":"write","sequence":sequence}));
        if let Some(error) = self.write_error.borrow().as_ref() {
            return Err(error.clone());
        }
        Ok(())
    }
    fn flash_clipboard(&self, message: &str, duration_ms: Option<f64>) -> Result<(), String> {
        record(
            &self.trace,
            json!({"op":"flash","message":message,"duration":duration_ms.map(wire)}),
        );
        if let Some(error) = self.flash_error.borrow().as_ref() {
            return Err(error.clone());
        }
        self.flashes.borrow_mut().flash(
            &mut *self.timers.borrow_mut(),
            message.to_owned(),
            duration_ms,
        );
        Ok(())
    }
}
struct Task {
    state: Value,
    future: Option<ClipboardCompletion<String>>,
}
type Tasks = Rc<RefCell<Vec<Task>>>;
fn start(tasks: &Tasks, name: &str, future: ClipboardCompletion<String>) {
    assert!(!tasks.borrow().iter().any(|t| t.state["name"] == name));
    tasks.borrow_mut().push(Task {
        state: json!({"name":name,"status":"pending"}),
        future: Some(future),
    });
}
fn drain(tasks: &Tasks) {
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    for task in tasks.borrow_mut().iter_mut() {
        if let Some(future) = task.future.as_mut() {
            if let Poll::Ready(result) = future.as_mut().poll(&mut cx) {
                task.future = None;
                match result {
                    Ok(value) => {
                        task.state["status"] = json!("ready");
                        task.state["value"] = json!(value);
                    }
                    Err(error) => {
                        task.state["status"] = json!("error");
                        task.state["error"] = json!(error);
                    }
                }
            }
        }
    }
}
// Deliberately empty-tree, no-overlay/non-scroll Selection host. Both source and
// Rust use real selection events/word geometry. Not a simulated selection range.
struct SelectionHost {
    clipboard: Rc<Services>,
    tasks: Tasks,
    auto: u64,
    screen: Vec<String>,
    columns: usize,
    rows: usize,
    now: i64,
    segments: Value,
}
impl ComponentGestureHost for SelectionHost {
    fn terminal_size(&self) -> (usize, usize) {
        (self.columns, self.rows)
    }
    fn now_ms(&mut self) -> i64 {
        record(&self.clipboard.trace, json!({"op":"now","value":self.now}));
        self.now
    }
    fn handle_search_mouse_event(&mut self, _: SgrMouseEvent) -> bool {
        panic!("no outer gesture in this fixture")
    }
    fn dispatch_mouse_to_overlay(&mut self, _: &TuiMouseEvent) -> OverlayMouseDispatch {
        OverlayMouseDispatch::default()
    }
    fn handle_scroll_to_end_indicator_mouse_event(&mut self, _: SgrMouseEvent) -> bool {
        panic!("no indicator")
    }
    fn handle_scrollbar_mouse_event(&mut self, _: SgrMouseEvent) -> bool {
        panic!("no scrollbar")
    }
    fn scrollbar_drag_active(&self) -> bool {
        false
    }
    fn update_scrollbar_hover(&mut self, _: i64, _: i64) {
        panic!("no scrollbar")
    }
    fn stop_scrollbar_hover(&mut self) {
        panic!("no scrollbar")
    }
    fn dispatch_mouse_to_layout(&mut self, _: &TuiMouseEvent) -> Option<ComponentMouseResult> {
        None
    }
    fn resolve_mouse_focus_target(&mut self, _: &ComponentHandle) -> ComponentHandle {
        panic!("empty tree")
    }
    fn focused_component(&self) -> Option<ComponentHandle> {
        None
    }
    fn set_focus(&mut self, _: ComponentHandle) {
        panic!("empty tree")
    }
    fn clear_text_selection(&mut self) {
        panic!("direct selection controls clear")
    }
    fn request_render(&mut self) {
        record(&self.clipboard.trace, json!({"op":"render"}));
    }
    fn handle_right_click_paste(&mut self, _: SgrMouseEvent) -> bool {
        panic!("no paste")
    }
    fn handle_selection_mouse_event(&mut self, _: SgrMouseEvent, _: &mut ComponentGesture) {
        panic!("direct selection")
    }
}
impl ComponentSelectionHost for SelectionHost {
    fn selection_frame(&self) -> Option<&LayoutFrame> {
        None
    }
    fn selection_screen(&self) -> &[String] {
        &self.screen
    }
    fn selection_has_overlay(&mut self) -> bool {
        false
    }
    fn selection_word_segments(&mut self, plain: &str) -> Vec<SelectionWordSegment> {
        record(&self.clipboard.trace, json!({"op":"segments","line":plain}));
        self.segments[plain]
            .as_array()
            .expect("external Intl input exists")
            .iter()
            .map(|p| SelectionWordSegment {
                text: p["text"].as_str().unwrap().to_owned(),
                is_word_like: p["isWordLike"].as_bool().unwrap(),
            })
            .collect()
    }
    fn start_selection_interval(&mut self, _: u64) -> u64 {
        panic!("non-scroll selection fixture")
    }
    fn cancel_selection_interval(&mut self, _: u64) {
        panic!("non-scroll selection fixture")
    }
    fn has_url_opener(&self) -> bool {
        false
    }
    fn open_selection_url(&mut self, _: &str) -> Result<(), String> {
        panic!("no opener")
    }
    fn request_copy_text(&mut self, text: String) {
        self.auto += 1;
        let future = copy_text_to_clipboard(self.clipboard.clone(), text);
        start(&self.tasks, &format!("auto-{}", self.auto), future);
    }
}
fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().to_owned())
        .collect()
}
fn point(p: Option<&SelectionPoint>) -> Value {
    p.map_or(
        Value::Null,
        |p| json!({"row":p.row,"col":p.col,"boundary":p.boundary}),
    )
}
fn range(r: Option<&SelectionRange>) -> Value {
    r.map_or(
        Value::Null,
        |r| json!({"start":point(Some(&r.start)),"end":point(Some(&r.end))}),
    )
}
struct Harness {
    host: SelectionHost,
    selection: ComponentSelection,
    gesture: ComponentGesture,
}
impl Harness {
    fn new(spec: &Value, segments: &Value) -> Self {
        let mut selection = ComponentSelection::default();
        selection.set_copy_on_select(spec["copyOnSelect"].as_bool().unwrap());
        Self {
            host: SelectionHost {
                clipboard: Services::new(spec),
                tasks: Tasks::default(),
                auto: 0,
                screen: strings(&spec["screen"]),
                columns: spec["columns"].as_u64().unwrap() as usize,
                rows: spec["rows"].as_u64().unwrap() as usize,
                now: 1000,
                segments: segments.clone(),
            },
            selection,
            gesture: ComponentGesture::default(),
        }
    }
    fn snapshot(&self) -> Value {
        let h = &self.host.clipboard;
        let s = self.selection.state();
        let f = h.flashes.borrow();
        json!({
            "trace":h.trace.borrow().clone(),"tasks":self.host.tasks.borrow().iter().map(|t|t.state.clone()).collect::<Vec<_>>(),
            "flash":{"nextId":f.next_id(),"entries":f.entries().iter().map(|e|json!({"id":e.id.sequence(),"message":e.message,"timer":e.timer})).collect::<Vec<_>>()},
            "timers":h.timers.borrow().timers.iter().map(|(id,t)|json!({"timer":id,"duration":wire(t.duration),"active":t.active,"unreferenced":t.unreferenced})).collect::<Vec<_>>(),
            "selection":{"anchor":point(s.anchor.as_ref()),"focus":point(s.focus.as_ref()),"granularity":match s.granularity {SelectionGranularity::Character=>"character",SelectionGranularity::Word=>"word",SelectionGranularity::Line=>"line"},"initialRange":range(s.initial_range.as_ref()),"lastClick":s.last_click.as_ref().map(|c|json!({"timestamp":c.timestamp,"count":c.count,"row":c.row,"wordStart":c.word_start,"wordEnd":c.word_end})),"pressActive":s.press_active,"dragged":s.dragged,"pressedUrl":s.pressed_url,"autoScrollDirection":s.auto_scroll_direction,"dragPointer":s.drag_pointer.map(|(x,y)|json!({"x":x,"y":y})),"timer":s.timer,"copyOnSelect":self.selection.copy_on_select(),"bounds":range(self.selection.bounds().as_ref()),"text":self.selection.active_text(&self.host)}
        })
    }
    fn step(&mut self, op: &Value) -> Value {
        let h = self.host.clipboard.clone();
        h.trace.borrow_mut().clear();
        let mut result = Value::Null;
        match op["op"].as_str().unwrap() {
            "mode" => *h.mode.borrow_mut() = op["value"].clone(),
            "errors" => {
                *h.write_error.borrow_mut() = op["write"].as_str().map(str::to_owned);
                *h.flash_error.borrow_mut() = op["flash"].as_str().map(str::to_owned);
            }
            "copyText" => start(
                &self.host.tasks,
                op["task"].as_str().unwrap(),
                copy_text_to_clipboard(h.clone(), op["text"].as_str().unwrap().to_owned()),
            ),
            "copyActive" => start(
                &self.host.tasks,
                op["task"].as_str().unwrap(),
                self.selection
                    .copy_active_selection_to_clipboard(&self.host, h.clone()),
            ),
            "resolve" => h.settle(
                op["request"].as_u64().unwrap(),
                Ok(delivery_value(&op["value"])),
            ),
            "reject" => h.settle(
                op["request"].as_u64().unwrap(),
                Err(op["error"].as_str().unwrap().to_owned()),
            ),
            "flash" => {
                if let Err(error) = h.flash_clipboard(
                    op["message"].as_str().unwrap(),
                    op.get("duration").map(number),
                ) {
                    result = json!({"error":error});
                }
            }
            "render" => {
                result = json!(h
                    .flashes
                    .borrow_mut()
                    .render(op["width"].as_u64().unwrap() as usize))
            }
            "dispose" => h.flashes.borrow_mut().dispose(&mut *h.timers.borrow_mut()),
            "invalidate" => h.flashes.borrow_mut().invalidate(),
            "fire" => h.fire(op["timer"].as_u64().unwrap()),
            "select" => {
                let v = &op["raw"];
                self.selection.handle_mouse_event(
                    &mut self.host,
                    &mut self.gesture,
                    SgrMouseEvent {
                        button: v["button"].as_i64().unwrap(),
                        x: v["x"].as_i64().unwrap(),
                        y: v["y"].as_i64().unwrap(),
                        release: v["release"].as_bool().unwrap(),
                    },
                );
            }
            "clear" => self.selection.clear(&mut self.host),
            "screen" => self.host.screen = strings(&op["lines"]),
            "copyOnSelect" => self
                .selection
                .set_copy_on_select(op["value"].as_bool().unwrap()),
            "time" => self.host.now = op["value"].as_i64().unwrap(),
            "drain" => {}
            v => panic!("unknown operation {v}"),
        }
        let before = self.snapshot();
        drain(&self.host.tasks);
        let mut after = self.snapshot();
        after["result"] = result;
        after["beforeDrain"] = before;
        after
    }
}
fn fixtures() -> Value {
    serde_json::from_str(include_str!("../component_clipboard/fixtures.json")).unwrap()
}
fn compare(group: &str, count: usize, steps: usize) {
    let fixture = fixtures();
    let cases = fixture[group].as_array().unwrap();
    assert_eq!(cases.len(), count);
    let mut total = 0;
    for c in cases {
        let mut h = Harness::new(c, &fixture["wordSegments"]);
        let ops = c["ops"].as_array().unwrap();
        let expected = c["expected"].as_array().unwrap();
        assert_eq!(ops.len(), expected.len());
        for (i, op) in ops.iter().enumerate() {
            let actual = h.step(op);
            assert_eq!(actual, expected[i], "{group}/{} step{i} {op}", c["name"]);
            total += 1;
        }
    }
    assert_eq!(total, steps);
}
#[test]
fn injected_delivery_matches_actual_source() {
    compare("delivery", 35, 151);
}
#[test]
fn osc52_utf8_bytes_match_actual_source() {
    compare("osc52", 97, 291);
}
#[test]
fn active_and_release_selection_copy_matches_actual_source() {
    compare("selection", 85, 501);
}
#[test]
fn flash_stack_timer_render_matches_actual_source() {
    compare("flashes", 128, 748);
}
#[test]
fn interleaved_completions_match_actual_source() {
    compare("sequences", 10, 190);
}

#[test]
fn injected_call_is_eager_but_flash_waits_for_poll_unlike_osc52() {
    let injected = Services::new(&json!({"injected":true,"mode":{"kind":"ready","value":true}}));
    let mut future = copy_text_to_clipboard(injected.clone(), "alpha".into());
    assert_eq!(
        *injected.trace.borrow(),
        vec![json!({"op":"copy","request":1,"text":"alpha"})]
    );
    assert!(injected.flashes.borrow().entries().is_empty());
    let waker = noop_waker();
    assert_eq!(
        future.as_mut().poll(&mut Context::from_waker(&waker)),
        Poll::Ready(Ok(true))
    );
    assert_eq!(injected.flashes.borrow().entries()[0].message, "Copied!");
    let fallback = Services::new(&json!({"injected":false,"mode":{"kind":"ready","value":true}}));
    let _unpolled = copy_text_to_clipboard(fallback.clone(), "f".into());
    assert_eq!(
        fallback.trace.borrow()[0],
        json!({"op":"write","sequence":"\u{1b}]52;c;Zg==\u{7}"})
    );
    assert_eq!(fallback.flashes.borrow().entries().len(), 1);
}
#[test]
fn pending_future_owns_host_without_borrowing_it_and_propagates_rejection() {
    let host = Services::new(&json!({"injected":true,"mode":{"kind":"deferred"}}));
    let weak = Rc::downgrade(&host);
    let mut future = copy_text_to_clipboard(host.clone(), "first".into());
    let waker = noop_waker();
    assert!(future
        .as_mut()
        .poll(&mut Context::from_waker(&waker))
        .is_pending());
    // Other operations and mutations remain possible while the first is pending.
    *host.mode.borrow_mut() = json!({"kind":"ready","value":true});
    let mut other = copy_text_to_clipboard(host.clone(), "second".into());
    assert_eq!(
        other.as_mut().poll(&mut Context::from_waker(&waker)),
        Poll::Ready(Ok(true))
    );
    drop(other);
    host.settle(1, Err("exact rejection".into()));
    drop(host);
    assert!(weak.upgrade().is_some());
    assert_eq!(
        future.as_mut().poll(&mut Context::from_waker(&waker)),
        Poll::Ready(Err("exact rejection".into()))
    );
    drop(future);
    assert!(weak.upgrade().is_none());
}
#[test]
fn flash_timer_identity_is_container_owned_and_dispose_preserves_sequence() {
    let mut a = AltScreenFlashContainer::default();
    let mut b = AltScreenFlashContainer::default();
    let mut host = TimerHost::default();
    a.flash(&mut host, "first".into(), None);
    b.flash(&mut host, "second".into(), None);
    let aid = a.entries()[0].id.clone();
    let bid = b.entries()[0].id.clone();
    assert_eq!(aid.sequence(), bid.sequence());
    assert_ne!(aid, bid);
    host.trace.borrow_mut().clear();
    b.expire(&aid, &mut host);
    assert_eq!(b.entries().len(), 1);
    assert!(host.trace.borrow().is_empty());
    a.dispose(&mut host);
    assert_eq!(
        host.trace.borrow().as_slice(),
        [json!({"op":"cancel","timer":1})]
    );
    host.trace.borrow_mut().clear();
    a.expire(&aid, &mut host);
    assert!(host.trace.borrow().is_empty());
    a.flash(&mut host, "after".into(), Some(-1.0));
    assert_eq!(a.entries()[0].id.sequence(), 1);
    assert_eq!(host.timers[&3].duration, 0.0);
    b.expire(&bid, &mut host);
    assert!(b.entries().is_empty());
    assert_eq!(a.entries().len(), 1);
}
