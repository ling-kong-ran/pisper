//! Actual-source fixtures: manual frames, real layout, signed geometry and
//! actual gesture routing with explicitly traced unrelated host services.
use crate::tui::component::{Component, TuiMouseEvent};
use crate::tui::component_gesture::{ComponentGesture, ComponentGestureHost, OverlayMouseDispatch};
use crate::tui::component_mouse::{ComponentHandle, ComponentMouseResult};
use crate::tui::component_screen_widgets::*;
use crate::tui::component_selection::{SelectionPoint, SelectionRange};
use crate::tui::component_selection_paint::apply_selection;
use crate::tui::components::alt_screen_flash::{
    AltScreenFlashContainer, AltScreenFlashHost, FlashId,
};
use crate::tui::components::scroll_view::{
    ScrollHandle, ScrollView, ScrollViewOptions, ScrollViewScrollToOptions, ScrollViewScrollbar,
};
use crate::tui::components::stack::{
    StackBasis, StackEntry, StackEntryOptions, StackOptions, VStack,
};
use crate::tui::layout::{
    get_scroll_view_box_id, get_scrollbar_geometry, render_layout_frame, LayoutBox, LayoutFrame,
    LayoutRect,
};
use crate::tui::rendered_lines::RenderedLines;
use crate::tui::viewport_mouse::SgrMouseEvent;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
type Trace = Arc<Mutex<Vec<Value>>>;
fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().into())
        .collect()
}
fn n(v: &Value) -> usize {
    v.as_u64().unwrap() as usize
}
fn raw(v: &Value) -> SgrMouseEvent {
    SgrMouseEvent {
        x: v["x"].as_i64().unwrap(),
        y: v["y"].as_i64().unwrap(),
        button: v["button"].as_i64().unwrap(),
        release: v["release"].as_bool().unwrap(),
    }
}
fn rect(v: &Value) -> LayoutRect {
    LayoutRect {
        x: v["x"].as_i64().unwrap(),
        y: v["y"].as_i64().unwrap(),
        width: n(&v["width"]),
        height: n(&v["height"]),
    }
}
fn rect_value(r: LayoutRect) -> Value {
    json!({"x":r.x,"y":r.y,"width":r.width,"height":r.height})
}
fn bar(v: &str) -> ScrollViewScrollbar {
    match v {
        "hidden" => ScrollViewScrollbar::Hidden,
        "auto" => ScrollViewScrollbar::Auto,
        "always" => ScrollViewScrollbar::Always,
        _ => panic!("bar"),
    }
}
struct Lines(Vec<String>);
impl Component for Lines {
    fn render(&mut self, _: usize) -> Vec<String> {
        self.0.clone()
    }
}
struct Timers {
    trace: Trace,
    next: u64,
    pending: BTreeMap<u64, FlashId>,
}
impl AltScreenFlashHost for Timers {
    fn set_flash_timeout(&mut self, id: FlashId, duration_ms: f64) -> u64 {
        self.next += 1;
        self.pending.insert(self.next, id);
        self.trace
            .lock()
            .unwrap()
            .push(json!({"op":"timeout","timer":self.next,"duration":duration_ms as u64}));
        self.next
    }
    fn unref_flash_timeout(&mut self, timer: u64) {
        self.trace
            .lock()
            .unwrap()
            .push(json!({"op":"unref","timer":timer}));
    }
    fn clear_flash_timeout(&mut self, timer: u64) {
        self.pending.remove(&timer);
        self.trace
            .lock()
            .unwrap()
            .push(json!({"op":"cancel","timer":timer}));
    }
    fn request_flash_render(&mut self) {
        self.trace.lock().unwrap().push(json!({"op":"render"}));
    }
}
struct Harness {
    trace: Trace,
    columns: usize,
    rows: usize,
    screen: Vec<String>,
    negative: Value,
    lines: Vec<String>,
    label: Option<String>,
    scrolls: BTreeMap<String, ScrollHandle>,
    components: BTreeMap<String, ComponentHandle>,
    root: VStack,
    frame: Option<LayoutFrame>,
    indicator: ScrollToEndIndicator,
    flashes: AltScreenFlashContainer,
    timers: Timers,
}
impl Harness {
    fn new(spec: &Value) -> Self {
        let trace = Trace::default();
        let lines = strings(&spec["lines"]);
        let mut scrolls = BTreeMap::new();
        let mut components = BTreeMap::new();
        for id in ["primary", "other", "implicit"] {
            let view = ScrollView::new(
                Box::new(Lines(if id == "implicit" {
                    vec![]
                } else {
                    lines.clone()
                })),
                ScrollViewOptions {
                    follow_end: id != "primary" || spec["follow"].as_bool().unwrap(),
                    primary: true,
                    scrollbar: Some(if id == "primary" {
                        bar(spec["bar"].as_str().unwrap())
                    } else {
                        ScrollViewScrollbar::Hidden
                    }),
                    timer_scheduler: Some(Arc::new(|_, _| {})),
                    ..Default::default()
                },
            );
            scrolls.insert(id.into(), view.state());
            components.insert(id.into(), ComponentHandle::new(view));
        }
        let root = VStack::new(
            vec![
                StackEntry::new(
                    Box::new(components["primary"].clone()),
                    StackEntryOptions {
                        basis: Some(StackBasis::Size(0.0)),
                        grow: Some(1.0),
                        min_size: Some(1.0),
                        ..Default::default()
                    },
                ),
                StackEntry::new(
                    Box::new(Lines(vec!["editor".into(), "footer".into()])),
                    StackEntryOptions {
                        basis: Some(StackBasis::Auto),
                        min_size: Some(1.0),
                        ..Default::default()
                    },
                ),
            ],
            StackOptions::default(),
        );
        let timers = Timers {
            trace: trace.clone(),
            next: 0,
            pending: BTreeMap::new(),
        };
        Self {
            trace,
            columns: n(&spec["columns"]),
            rows: n(&spec["rows"]),
            screen: strings(&spec["screen"]),
            negative: json!({}),
            lines,
            label: spec["label"].as_str().map(String::from),
            scrolls,
            components,
            root,
            frame: None,
            indicator: ScrollToEndIndicator::default(),
            flashes: AltScreenFlashContainer::default(),
            timers,
        }
    }
    fn record(&self, op: &str) {
        self.trace.lock().unwrap().push(json!({"op":op}));
    }
    fn renderer(&self) -> Arc<dyn Fn() + Send + Sync> {
        let trace = self.trace.clone();
        Arc::new(move || trace.lock().unwrap().push(json!({"op":"render"})))
    }
    fn click(&self, event: SgrMouseEvent) -> bool {
        self.indicator.handle_mouse_event(
            event,
            self.frame.as_ref(),
            &self.scrolls["implicit"],
            || self.record("render"),
        )
    }
    fn name(&self, s: Option<&ScrollHandle>) -> Value {
        s.and_then(|s| {
            self.scrolls
                .iter()
                .find_map(|(id, h)| (s == h).then(|| json!(id)))
        })
        .unwrap_or(Value::Null)
    }
    fn snapshot(&self) -> Value {
        let scrolls:serde_json::Map<String,Value>=self.scrolls.iter().map(|(id,h)|{let s=h.snapshot();(id.clone(),json!({"top":s.scroll_top,"following":s.following_end,"visible":s.scrollbar_visible,"content":s.content_height,"viewport":s.viewport_height}))}).collect();
        let layout = self.frame.as_ref().map(|f| {
            fn visit(frame: &LayoutFrame, index: usize, order: &mut Vec<usize>) {
                order.push(index);
                for &child in &frame.boxes[index].children { visit(frame, child, order); }
            }
            let mut order = Vec::new();
            visit(f, f.root, &mut order);
            json!({"primary":self.name(f.primary_scroll_view.as_ref()),"boxes":order.into_iter().map(|i|{let b=&f.boxes[i];json!({"rect":rect_value(b.rect),"clip":rect_value(b.clip),"scroll":self.name(b.scroll_view.as_ref())})}).collect::<Vec<_>>()})
        });
        json!({"screen":self.screen,"negative":self.negative,"rect":self.indicator.rect().map(|r|json!({"row":r.row,"column":r.column,"width":r.width})),"scrolls":scrolls,"flashes":self.flashes.entries().iter().map(|e|json!({"id":e.id.sequence(),"message":e.message,"timer":e.timer})).collect::<Vec<_>>(),"nextId":self.flashes.next_id(),"layout":layout})
    }
    fn step(&mut self, op: &Value, gesture: &mut ComponentGesture) -> Value {
        let width = op["width"].as_u64().map_or(self.columns, |n| n as usize);
        let height = op["height"].as_u64().map_or(self.rows, |n| n as usize);
        match op["op"].as_str().unwrap() {
            "frame" => {
                let id = op["id"].as_str().unwrap();
                let b = LayoutBox {
                    component_path: vec![],
                    component: Some(self.components[id].clone()),
                    rect: rect(&op["rect"]),
                    clip: rect(&op["clip"]),
                    children: vec![],
                    parent: None,
                    lines: None,
                    line_offset: None,
                    scroll_view: (!op["missing"].as_bool().unwrap_or(false))
                        .then(|| self.scrolls[id].clone()),
                    scroll_content_lines: Some(RenderedLines::dense(self.lines.clone())),
                    layer: 0,
                };
                self.frame = Some(LayoutFrame {
                    root: 0,
                    boxes: vec![b],
                    width: self.columns,
                    height: self.rows,
                    lines: RenderedLines::dense(vec![]),
                    primary_scroll_view: (!op["implicit"].as_bool().unwrap_or(false))
                        .then(|| self.scrolls[id].clone()),
                });
            }
            "clearFrame" => self.frame = None,
            "layout" => {
                let render = self.renderer();
                let f = match op["root"].as_str() {
                    Some("primary") => render_layout_frame(
                        &mut self.components["primary"].clone(),
                        width,
                        height,
                        render,
                    ),
                    Some("implicit") => render_layout_frame(
                        &mut self.components["implicit"].clone(),
                        width,
                        height,
                        render,
                    ),
                    _ => render_layout_frame(&mut self.root, width, height, render),
                };
                self.screen = (0..f.lines.len())
                    .map(|i| f.lines.get(i).unwrap().into())
                    .collect();
                self.frame = Some(f);
                self.negative = json!({});
            }
            "scroll" => {
                let s = &self.scrolls[op["id"].as_str().unwrap()];
                s.update_layout(n(&op["content"]), n(&op["viewport"]), self.renderer());
                s.scroll_to(
                    n(&op["top"]),
                    ScrollViewScrollToOptions {
                        disable_follow: op["disableFollow"].as_bool().unwrap(),
                    },
                );
            }
            "bar" => self.scrolls[op["id"].as_str().unwrap_or("primary")]
                .set_scrollbar(bar(op["value"].as_str().unwrap())),
            "end" => self.scrolls[op["id"].as_str().unwrap_or("primary")].scroll_to_end(),
            "label" => self.label = op["text"].as_str().map(String::from),
            "screen" => {
                self.screen = strings(&op["lines"]);
                self.negative = json!({});
            }
            "indicator" => {
                let trace = self.trace.clone();
                let label = self.label.clone();
                let mut callback = || {
                    let text = label.as_ref().unwrap().clone();
                    trace
                        .lock()
                        .unwrap()
                        .push(json!({"op":"label","text":text}));
                    if text == "THROW" {
                        Err("label rejected")
                    } else {
                        Ok(text)
                    }
                };
                let callback = if self.label.is_some() {
                    Some(&mut callback as &mut dyn FnMut() -> Result<String, &'static str>)
                } else {
                    None
                };
                match self.indicator.composite(
                    &self.screen,
                    self.frame.as_ref().unwrap(),
                    &self.scrolls["implicit"],
                    width,
                    callback,
                ) {
                    Ok(out) => {
                        self.screen = out.screen;
                        self.negative = json!({});
                        if let Some((r, line)) = out.negative_row {
                            self.negative[r.to_string()] = json!(line);
                        }
                    }
                    Err(e) => return json!({"error":e}),
                }
            }
            "flashes" => {
                self.screen = composite_flashes(&self.screen, &mut self.flashes, width, height);
                self.negative = json!({});
            }
            "flash" => self.flashes.flash(
                &mut self.timers,
                op["message"].as_str().unwrap().into(),
                Some(op["duration"].as_f64().unwrap()),
            ),
            "expire" => {
                if let Some(id) = self.timers.pending.remove(&op["timer"].as_u64().unwrap()) {
                    self.flashes.expire(&id, &mut self.timers);
                }
            }
            "dispose" => self.flashes.dispose(&mut self.timers),
            "click" => return json!(self.click(raw(&op["raw"]))),
            "raw" => gesture.handle_mouse_event(self, raw(&op["raw"])),
            "paint" => {
                let point = |v: &Value, boundary| SelectionPoint {
                    row: n(&v[0]),
                    col: n(&v[1]),
                    boundary,
                    scroll_view: None,
                };
                let b = SelectionRange {
                    start: point(&op["start"], false),
                    end: point(&op["end"], op["boundary"].as_bool().unwrap_or(false)),
                };
                self.screen =
                    apply_selection(&self.screen, Some(&b), self.frame.as_ref(), self.columns);
                self.negative = json!({});
            }
            x => panic!("unknown {x}"),
        }
        Value::Null
    }
}
impl ComponentGestureHost for Harness {
    fn terminal_size(&self) -> (usize, usize) {
        (self.columns, self.rows)
    }
    fn now_ms(&mut self) -> i64 {
        panic!("unexpected clock in routing seam")
    }
    fn handle_search_mouse_event(&mut self, _: SgrMouseEvent) -> bool {
        self.record("search");
        false
    }
    fn dispatch_mouse_to_overlay(&mut self, _: &TuiMouseEvent) -> OverlayMouseDispatch {
        self.record("overlay");
        OverlayMouseDispatch::default()
    }
    fn handle_scroll_to_end_indicator_mouse_event(&mut self, e: SgrMouseEvent) -> bool {
        self.click(e)
    }
    fn handle_scrollbar_mouse_event(&mut self, e: SgrMouseEvent) -> bool {
        self.record("scrollbar");
        let Some(f) = &self.frame else { return false };
        let s = f
            .primary_scroll_view
            .as_ref()
            .unwrap_or(&self.scrolls["implicit"]);
        let g = get_scroll_view_box_id(f, s).and_then(|i| get_scrollbar_geometry(f, i, false));
        g.is_some_and(|g| {
            !e.release
                && e.button == 0
                && e.x == g.column
                && e.y >= g.track_top
                && e.y < g.track_top + g.track_height as i64
        })
    }
    fn scrollbar_drag_active(&self) -> bool {
        false
    }
    fn update_scrollbar_hover(&mut self, _: i64, _: i64) {
        self.record("hover");
    }
    fn stop_scrollbar_hover(&mut self) {
        panic!("unexpected overlay hit")
    }
    fn dispatch_mouse_to_layout(&mut self, _: &TuiMouseEvent) -> Option<ComponentMouseResult> {
        self.record("layout");
        None
    }
    fn resolve_mouse_focus_target(&mut self, _: &ComponentHandle) -> ComponentHandle {
        panic!("no focus response")
    }
    fn focused_component(&self) -> Option<ComponentHandle> {
        None
    }
    fn set_focus(&mut self, _: ComponentHandle) {
        panic!("no focus response")
    }
    fn clear_text_selection(&mut self) {
        panic!("no component response")
    }
    fn request_render(&mut self) {
        self.record("render");
    }
    fn handle_right_click_paste(&mut self, _: SgrMouseEvent) -> bool {
        self.record("paste");
        false
    }
    fn handle_selection_mouse_event(&mut self, _: SgrMouseEvent, _: &mut ComponentGesture) {
        self.record("selection");
    }
}
fn corpus() -> Value {
    serde_json::from_str(include_str!("../component_screen_widgets/fixtures.json")).unwrap()
}
fn replay(group: &str) {
    for case in corpus()[group].as_array().unwrap() {
        let mut h = Harness::new(case);
        let mut gesture = ComponentGesture::default();
        for (i, op) in case["ops"].as_array().unwrap().iter().enumerate() {
            h.trace.lock().unwrap().clear();
            let value = h.step(op, &mut gesture);
            let got = json!({"value":value,"state":h.snapshot(),"trace":*h.trace.lock().unwrap()});
            assert_eq!(
                got, case["expected"][i],
                "{group}/{} step {i}: {op}",
                case["name"]
            );
        }
    }
}
#[test]
fn actual_source_flash_composition() {
    replay("flashes");
}
#[test]
fn actual_source_indicator() {
    replay("indicator");
}
#[test]
fn actual_source_indicator_clicks() {
    replay("clicks");
}
#[test]
fn actual_source_real_layout_and_selection_paint() {
    replay("composed");
}
#[test]
fn actual_source_gesture_routing_seams() {
    replay("routing");
}
#[test]
fn actual_source_signed_geometry() {
    replay("signed");
}

#[test]
fn zero_height_keeps_all_flash_rows_and_empty_stack_does_not_pad() {
    let trace = Trace::default();
    let mut timers = Timers {
        trace,
        next: 0,
        pending: BTreeMap::new(),
    };
    let mut f = AltScreenFlashContainer::default();
    assert!(composite_flashes(&[], &mut f, 20, 4).is_empty());
    f.flash(&mut timers, "First".into(), None);
    f.flash(&mut timers, "Second".into(), None);
    let rows = composite_flashes(&[], &mut f, 20, 0);
    assert_eq!(rows.len(), 2);
    assert!(rows[0].contains("First"));
    assert!(rows[1].contains("Second"));
    assert_eq!(composite_flashes(&[], &mut f, 0, 0), Vec::<String>::new());
    f.dispose(&mut timers);
    assert!(timers.pending.is_empty());
}
#[test]
fn label_not_called_when_ineligible_and_rejection_clears_rect() {
    let data = corpus();
    let case = &data["indicator"][0];
    let mut h = Harness::new(case);
    let mut g = ComponentGesture::default();
    for op in case["ops"].as_array().unwrap().iter().take(3) {
        h.step(op, &mut g);
    }
    assert!(h.indicator.rect().is_some());
    let mut reject = || Err::<String, _>("failure");
    let prior = h.screen.clone();
    assert_eq!(
        h.indicator.composite(
            &prior,
            h.frame.as_ref().unwrap(),
            &h.scrolls["implicit"],
            20,
            Some(&mut reject)
        ),
        Err("failure")
    );
    assert!(h.indicator.rect().is_none());
    assert_eq!(prior, h.screen);
    h.scrolls["primary"].scroll_to_end();
    let mut forbidden =
        || -> Result<String, ()> { panic!("following scroll cannot request label") };
    h.indicator
        .composite(
            &prior,
            h.frame.as_ref().unwrap(),
            &h.scrolls["implicit"],
            20,
            Some(&mut forbidden),
        )
        .unwrap();
}
#[test]
fn hit_scrolls_current_primary_before_explicit_render_and_keeps_published_rect() {
    let data = corpus();
    let case = &data["clicks"][0];
    let mut h = Harness::new(case);
    let mut g = ComponentGesture::default();
    for op in case["ops"].as_array().unwrap().iter().take(3) {
        h.step(op, &mut g);
    }
    let rect = h.indicator.rect().unwrap();
    let target = h.scrolls["other"].clone();
    target.update_layout(30, 3, Arc::new(|| {}));
    target.scroll_to(2, Default::default());
    h.frame.as_mut().unwrap().primary_scroll_view = Some(target.clone());
    let event = SgrMouseEvent {
        x: rect.column as i64,
        y: rect.row as i64,
        button: 0,
        release: false,
    };
    assert!(h.indicator.handle_mouse_event(
        event,
        h.frame.as_ref(),
        &h.scrolls["implicit"],
        || {
            let s = target.snapshot();
            assert_eq!(s.scroll_top, 27);
            assert!(s.following_end);
        }
    ));
    assert_eq!(h.indicator.rect(), Some(rect));
    assert_eq!(h.scrolls["primary"].snapshot().scroll_top, 0);
}
