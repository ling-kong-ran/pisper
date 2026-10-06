//! Differential fixtures from actual upstream layout.ts/ScrollView/Text, with
//! exact bytes, render/visibility/style/callback traces, geometry and hit paths.
use crate::tui::component::Component;
use crate::tui::components::scroll_view::{
    ScrollHandle, ScrollTimerCallback, ScrollTimerScheduler, ScrollView, ScrollViewOptions,
    ScrollViewScrollToOptions, ScrollViewScrollbar,
};
use crate::tui::components::stack::{
    HStack, StackAlign, StackBasis, StackEntry, StackEntryOptions, StackOptions, VStack,
};
use crate::tui::components::text::Text;
use crate::tui::layout::*;
use crate::tui::layout_node::ComponentCacheId;
use crate::tui::rendered_lines::RenderedLines;
use crate::tui::terminal_image::*;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

type Trace = Arc<Mutex<Vec<Value>>>;
#[derive(Default)]
struct Clock {
    now: u64,
    next: u64,
    jobs: Vec<(u64, u64, ScrollTimerCallback)>,
}
fn scheduler(clock: Arc<Mutex<Clock>>) -> ScrollTimerScheduler {
    Arc::new(move |delay, callback| {
        let mut c = clock.lock().unwrap();
        c.next += 1;
        let deadline = c.now + delay.as_millis() as u64;
        let next = c.next;
        c.jobs.push((deadline, next, callback));
    })
}
fn advance(clock: &Arc<Mutex<Clock>>, millis: u64) {
    let until = clock.lock().unwrap().now + millis;
    loop {
        let callback = {
            let mut c = clock.lock().unwrap();
            let found = c
                .jobs
                .iter()
                .enumerate()
                .filter(|(_, job)| job.0 <= until)
                .min_by_key(|(_, job)| (job.0, job.1))
                .map(|(i, _)| i);
            found.map(|i| {
                let (time, _, callback) = c.jobs.remove(i);
                c.now = time;
                callback
            })
        };
        if let Some(callback) = callback {
            callback();
        } else {
            break;
        }
    }
    clock.lock().unwrap().now = until;
}
struct LeafState {
    spec: Value,
    lines: Vec<String>,
    text: Option<Text>,
    calls: usize,
}
#[derive(Clone)]
struct Probe {
    id: String,
    cache_id: ComponentCacheId,
    state: Arc<Mutex<LeafState>>,
    trace: Trace,
}
impl Probe {
    fn rendered(&mut self, width: usize) -> RenderedLines {
        let mut s = self.state.lock().unwrap();
        s.calls += 1;
        self.trace
            .lock()
            .unwrap()
            .push(json!({"op":"render","id":self.id,"width":width,"call":s.calls}));
        if let Some(text) = &mut s.text {
            return RenderedLines::dense(text.render(width));
        }
        if !s.spec["sparse"].is_null() {
            let sparse = &s.spec["sparse"];
            return RenderedLines::sparse(
                sparse["length"].as_u64().unwrap() as usize,
                sparse["entries"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|e| {
                        (
                            e[0].as_u64().unwrap() as usize,
                            e[1].as_str().unwrap().into(),
                        )
                    })
                    .collect(),
            );
        }
        let lines = if let Some(count) = s.spec["repeat"].as_u64() {
            vec![s.lines.first().cloned().unwrap_or_default(); count as usize]
        } else {
            s.lines.clone()
        };
        RenderedLines::dense(
            lines
                .iter()
                .enumerate()
                .map(|(i, line)| {
                    line.replace("{w}", &width.to_string())
                        .replace("{fill}", &"x".repeat(width.saturating_sub(1)))
                        .replace("{n}", &i.to_string())
                        .replace("{call}", &s.calls.to_string())
                })
                .collect(),
        )
    }
}
impl Component for Probe {
    fn render(&mut self, width: usize) -> Vec<String> {
        match self.rendered(width) {
            RenderedLines::Dense(lines) => (*lines).clone(),
            _ => panic!("sparse test unexpectedly rendered through Vec API"),
        }
    }
    fn render_layout_lines(&mut self, width: usize) -> RenderedLines {
        self.rendered(width)
    }
    fn layout_cache_id(&self) -> Option<ComponentCacheId> {
        Some(self.cache_id)
    }
    fn invalidate(&mut self) {
        let mut s = self.state.lock().unwrap();
        if let Some(text) = &mut s.text {
            text.invalidate();
        } else {
            self.trace
                .lock()
                .unwrap()
                .push(json!({"op":"invalidate","id":self.id}));
        }
    }
}
fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|v| v.iter().map(|s| s.as_str().unwrap().into()).collect())
        .unwrap_or_default()
}
fn number(v: &Value) -> f64 {
    match v.as_str() {
        Some("NaN") => f64::NAN,
        Some("Infinity") => f64::INFINITY,
        Some("-Infinity") => f64::NEG_INFINITY,
        _ => v.as_f64().unwrap(),
    }
}
fn bar(v: &str) -> ScrollViewScrollbar {
    match v {
        "auto" => ScrollViewScrollbar::Auto,
        "always" => ScrollViewScrollbar::Always,
        _ => ScrollViewScrollbar::Hidden,
    }
}
fn bar_name(v: ScrollViewScrollbar) -> &'static str {
    match v {
        ScrollViewScrollbar::Auto => "auto",
        ScrollViewScrollbar::Always => "always",
        ScrollViewScrollbar::Hidden => "hidden",
    }
}
struct Harness {
    trace: Trace,
    clock: Arc<Mutex<Clock>>,
    probes: HashMap<String, Probe>,
    names: HashMap<ComponentPath, String>,
    scrolls: Vec<(String, ScrollHandle)>,
}
impl Harness {
    fn new() -> Self {
        Self {
            trace: Trace::default(),
            clock: Arc::default(),
            probes: HashMap::new(),
            names: HashMap::new(),
            scrolls: Vec::new(),
        }
    }
    fn build(&mut self, spec: &Value, path: ComponentPath) -> Box<dyn Component> {
        let kind = spec["kind"].as_str().unwrap();
        if kind == "alias" {
            let target = spec["target"].as_str().unwrap();
            self.names.insert(path, target.into());
            return Box::new(self.probes[target].clone());
        }
        let id = spec["id"].as_str().unwrap().to_owned();
        self.names.insert(path.clone(), id.clone());
        match kind {
            "leaf" | "text" => {
                let text = (kind == "text").then(|| {
                    Text::with_options(
                        spec["text"].as_str().unwrap(),
                        0,
                        0,
                        spec["background"].as_str().map(|background| {
                            let background = background.to_owned();
                            Arc::new(move |s: &str| format!("{background}{s}\x1b[49m"))
                                as crate::tui::components::text::BgFn
                        }),
                    )
                });
                let probe = Probe {
                    id: id.clone(),
                    cache_id: ComponentCacheId::new(),
                    state: Arc::new(Mutex::new(LeafState {
                        spec: spec.clone(),
                        lines: strings(&spec["lines"]),
                        text,
                        calls: 0,
                    })),
                    trace: self.trace.clone(),
                };
                self.probes.insert(id, probe.clone());
                Box::new(probe)
            }
            "scroll" => {
                let options = &spec["options"];
                let style = |kind: &'static str| {
                    options["style"].as_str().map(|style| {
                        let style = style.to_owned();
                        let trace = self.trace.clone();
                        let id = id.clone();
                        Arc::new(move |s: &str| {
                            trace
                                .lock()
                                .unwrap()
                                .push(json!({"op":"style","id":id,"kind":kind,"text":s}));
                            if style == "plain" {
                                s.to_owned()
                            } else {
                                format!(
                                    "\x1b[38;5;{}m{s}\x1b[39m",
                                    if kind == "track" { 2 } else { 1 }
                                )
                            }
                        })
                            as crate::tui::components::scroll_view::ScrollStyle
                    })
                };
                let options = ScrollViewOptions {
                    follow_end: options["follow"] == "end",
                    primary: options["primary"].as_bool().unwrap_or(false),
                    overscroll_contain: options["overscroll"] == "contain",
                    scrollbar: options["scrollbar"].as_str().map(bar),
                    scrollbar_track_style: style("track"),
                    scrollbar_thumb_style: style("thumb"),
                    scrollbar_hide_delay_ms: (!options["scrollbarHideDelayMs"].is_null())
                        .then(|| number(&options["scrollbarHideDelayMs"])),
                    timer_scheduler: Some(scheduler(self.clock.clone())),
                };
                let mut child_path = path;
                child_path.push(0);
                let child = self.build(&spec["child"], child_path);
                let view = ScrollView::new(child, options);
                self.scrolls.push((id, view.state()));
                Box::new(view)
            }
            "hstack" | "vstack" => {
                let entries=spec["children"].as_array().unwrap().iter().enumerate().map(|(i,entry)|{
                    let o=&entry["options"];let mut child_path=path.clone();child_path.push(i);let child=self.build(&entry["node"],child_path);
                    let options=StackEntryOptions {basis:if o["basis"].is_null() {None} else if o["basis"]=="auto" {Some(StackBasis::Auto)} else {Some(StackBasis::Size(number(&o["basis"])))},grow:o["grow"].as_f64(),shrink:o["shrink"].as_f64(),min_size:o["minSize"].as_f64(),max_size:o["maxSize"].as_f64(),visible:o["visible"].as_str().map(|visible|{let visible=visible.to_owned();let trace=self.trace.clone();let id=id.clone();Arc::new(move |viewport:&crate::tui::components::stack::LayoutViewport| {trace.lock().unwrap().push(json!({"op":"visible","id":id,"index":i,"width":viewport.width,"height":viewport.height}));match visible.as_str() {"never"=>false,"wide"=>viewport.width>=6,"short"=>viewport.height<=4,_=>true}}) as crate::tui::components::stack::StackVisibility})};
                    StackEntry::new(child,options)
                }).collect();
                let options = StackOptions {
                    gap: spec["options"]["gap"].as_f64(),
                    align: match spec["options"]["align"].as_str() {
                        Some("start") => StackAlign::Start,
                        Some("center") => StackAlign::Center,
                        Some("end") => StackAlign::End,
                        _ => StackAlign::Stretch,
                    },
                };
                if kind == "hstack" {
                    Box::new(HStack::new(entries, options))
                } else {
                    Box::new(VStack::new(entries, options))
                }
            }
            _ => panic!("unknown kind {kind}"),
        }
    }
    fn name(&self, b: &LayoutBox) -> &str {
        &self.names[&b.component_path]
    }
    fn scroll_name(&self, s: &ScrollHandle) -> &str {
        &self.scrolls.iter().find(|(_, other)| other == s).unwrap().0
    }
    fn scroll(&self, id: &str) -> &ScrollHandle {
        &self.scrolls.iter().find(|(name, _)| name == id).unwrap().1
    }
    fn dump_box(&self, f: &LayoutFrame, i: usize) -> Value {
        let b = &f.boxes[i];
        json!({"id":self.name(b),"rect":rect(b.rect),"clip":rect(b.clip),"parent":b.parent.map(|p|self.name(&f.boxes[p])),"layer":b.layer,"lines":line_summary(b.lines.as_ref()),"lineOffset":b.line_offset,"scroll":b.scroll_view.as_ref().map(|s|self.scroll_name(s)),"scrollContent":line_summary(b.scroll_content_lines.as_ref()),"geometry":geometry(get_scrollbar_geometry(f,i,false)),"hiddenGeometry":geometry(get_scrollbar_geometry(f,i,true)),"children":b.children.iter().map(|&c|self.dump_box(f,c)).collect::<Vec<_>>()})
    }
    fn dump_frame(&self, f: &LayoutFrame) -> Value {
        let (w, h) = (f.width as i64, f.height as i64);
        let mut points = vec![
            (-1, 0),
            (0, -1),
            (w, 0),
            (0, h),
            (0, 0),
            (w - 1, h - 1),
            (w / 2, h / 2),
        ];
        if w * h <= 40 {
            for y in 0..h {
                for x in 0..w {
                    points.push((x, y));
                }
            }
        }
        json!({"width":w,"height":h,"lines":(0..f.lines.len()).map(|i|f.lines.get(i)).collect::<Vec<_>>(),"primary":f.primary_scroll_view.as_ref().map(|s|self.scroll_name(s)),"root":self.dump_box(f,f.root),"hits":points.into_iter().map(|(x,y)|json!({"x":x,"y":y,"boxes":get_layout_boxes_at(f,x,y).into_iter().map(|b|self.name(b)).collect::<Vec<_>>(),"scrolls":get_scroll_views_at(f,x,y).iter().map(|s|self.scroll_name(s)).collect::<Vec<_>>()})).collect::<Vec<_>>(),"scrollBoxes":self.scrolls.iter().map(|(id,s)|json!([id,get_scroll_view_box(f,s).map(|b|self.name(b))])).collect::<Vec<_>>()})
    }
    fn run(&mut self, case: &Value) -> Vec<Value> {
        for image in case["images"].as_array().unwrap() {
            register_kitty_image_metadata(metadata(image));
        }
        let mut root = self.build(&case["tree"], vec![]);
        let mut frame = None;
        let mut outputs = vec![];
        for step in case["steps"].as_array().unwrap() {
            let mut result = Value::Null;
            let mut rendered = Value::Null;
            let id = step["id"].as_str().unwrap_or("");
            match step["op"].as_str().unwrap() {
                "render" => {
                    let trace = self.trace.clone();
                    let f = render_layout_frame(
                        &mut *root,
                        step["width"]
                            .as_u64()
                            .unwrap_or_else(|| case["width"].as_u64().unwrap())
                            as usize,
                        step["height"]
                            .as_u64()
                            .unwrap_or_else(|| case["height"].as_u64().unwrap())
                            as usize,
                        Arc::new(move || trace.lock().unwrap().push(json!({"op":"requestRender"}))),
                    );
                    rendered = self.dump_frame(&f);
                    frame = Some(f);
                }
                "by" => {
                    let value = self.scroll(id).scroll_by_number(number(&step["value"]));
                    result = if value.fract() == 0.0 {
                        json!(value as i64)
                    } else {
                        json!(value)
                    };
                }
                "to" => self.scroll(id).scroll_to_number(
                    number(&step["value"]),
                    ScrollViewScrollToOptions {
                        disable_follow: step["disableFollow"].as_bool().unwrap_or(false),
                    },
                ),
                "start" => self.scroll(id).scroll_to_start(),
                "end" => self.scroll(id).scroll_to_end(),
                "active" => self
                    .scroll(id)
                    .set_scrollbar_active(step["value"].as_bool().unwrap()),
                "bar" => self
                    .scroll(id)
                    .set_scrollbar(bar(step["value"].as_str().unwrap())),
                "advance" => advance(&self.clock, step["value"].as_u64().unwrap()),
                "lines" => self.probes[id].state.lock().unwrap().lines = strings(&step["value"]),
                "text" => self.probes[id]
                    .state
                    .lock()
                    .unwrap()
                    .text
                    .as_mut()
                    .unwrap()
                    .set_text(step["value"].as_str().unwrap()),
                "invalidate" => root.invalidate(),
                other => panic!("unknown op {other}"),
            }
            let states:Vec<_>=self.scrolls.iter().map(|(id,s)|{let state=s.snapshot();json!({"id":id,"top":state.scroll_top,"following":state.following_end,"viewport":state.viewport_height,"scrollbar":bar_name(state.scrollbar),"visible":state.scrollbar_visible,"active":state.scrollbar_active,"primary":state.primary,"overscroll":if state.overscroll_contain {"contain"} else {"chain"}})}).collect();
            let live: Vec<_> = frame
                .as_ref()
                .map(|f| {
                    self.scrolls
                        .iter()
                        .map(|(id, s)| {
                            let b = get_scroll_view_box_id(f, s);
                            json!([
                                id,
                                geometry(b.and_then(|b| get_scrollbar_geometry(f, b, false))),
                                geometry(b.and_then(|b| get_scrollbar_geometry(f, b, true)))
                            ])
                        })
                        .collect()
                })
                .unwrap_or_default();
            outputs.push(json!({"result":result,"frame":rendered,"states":states,"liveGeometry":live,"trace":std::mem::take(&mut *self.trace.lock().unwrap())}));
        }
        outputs
    }
}
fn line_summary(lines: Option<&RenderedLines>) -> Value {
    lines.map(|lines|json!({"length":lines.len(),"entries":lines.present().map(|(i,s)|json!([i,s])).collect::<Vec<_>>()})).unwrap_or_default()
}
fn rect(r: LayoutRect) -> Value {
    json!({"x":r.x,"y":r.y,"width":r.width,"height":r.height})
}
fn geometry(g: Option<ScrollbarGeometry>) -> Value {
    g.map(|g|json!({"column":g.column,"trackTop":g.track_top,"trackHeight":g.track_height,"thumbTop":g.thumb_top,"thumbHeight":g.thumb_height,"maxScrollTop":g.max_scroll_top})).unwrap_or_default()
}
fn metadata(v: &Value) -> KittyImageMetadata {
    KittyImageMetadata {
        image_id: v["imageId"].as_u64().unwrap(),
        columns: v["columns"].as_u64().unwrap() as usize,
        rows: v["rows"].as_u64().unwrap() as usize,
        width_px: v["widthPx"].as_u64().unwrap() as usize,
        height_px: v["heightPx"].as_u64().unwrap() as usize,
    }
}
fn first_diff(actual: &Value, expected: &Value, path: &str) -> Option<String> {
    if actual == expected {
        return None;
    }
    match (actual, expected) {
        (Value::Object(a), Value::Object(e)) if a.len() == e.len() => {
            for (k, v) in e {
                if let Some(diff) =
                    first_diff(a.get(k).unwrap_or(&Value::Null), v, &format!("{path}.{k}"))
                {
                    return Some(diff);
                }
            }
        }
        (Value::Array(a), Value::Array(e)) if a.len() == e.len() => {
            for (i, (a, e)) in a.iter().zip(e).enumerate() {
                if let Some(diff) = first_diff(a, e, &format!("{path}[{i}]")) {
                    return Some(diff);
                }
            }
        }
        _ => {}
    }
    let shorten = |v: &Value| v.to_string().chars().take(400).collect::<String>();
    Some(format!(
        "{path}: actual={} expected={}",
        shorten(actual),
        shorten(expected)
    ))
}
fn corpus() -> Value {
    serde_json::from_str(include_str!("../layout/fixtures.json")).unwrap()
}
#[test]
fn viewport_layout_matches_actual_upstream_frames_traces_and_lifecycles() {
    let data = corpus();
    let cases = data["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 346);
    assert_eq!(
        cases
            .iter()
            .map(|c| c["steps"].as_array().unwrap().len())
            .sum::<usize>(),
        1289
    );
    let mut failures = vec![];
    for (i, case) in cases.iter().enumerate() {
        let outputs = Harness::new().run(case);
        if let Some(diff) = first_diff(&json!(outputs), &case["outputs"], "outputs") {
            if failures.len() < 8 {
                eprintln!("{}: {diff}", case["id"]);
                std::fs::write(
                    format!("target/layout-difference-{i}.json"),
                    serde_json::to_vec_pretty(&outputs).unwrap(),
                )
                .unwrap();
            }
            failures.push(case["id"].as_str().unwrap());
        }
    }
    assert!(
        failures.is_empty(),
        "{} differing layout cases: {failures:?}",
        failures.len()
    );
}
#[test]
fn kitty_encoding_and_crop_match_actual_upstream_bytes() {
    let data = corpus();
    let cases = data["kitty"].as_array().unwrap();
    assert_eq!(cases.len(), 108);
    let mut registry = KittyImageRegistry::default();
    for (i, case) in cases.iter().enumerate() {
        let actual = if case["op"] == "encode" {
            let o = &case["options"];
            encode_kitty(
                case["data"].as_str().unwrap(),
                KittyOptions {
                    columns: o["columns"].as_u64().map(|n| n as usize),
                    rows: o["rows"].as_u64().map(|n| n as usize),
                    image_id: o["imageId"].as_u64(),
                    move_cursor: o["moveCursor"].as_bool(),
                },
            )
        } else {
            let meta = metadata(&case["metadata"]);
            registry.register(meta);
            let line = case["line"].as_str().unwrap();
            assert_eq!(registry.get(line), Some(meta));
            registry.crop(
                line,
                case["hidden"].as_i64().unwrap(),
                case["visible"].as_i64().unwrap(),
            )
        };
        assert_eq!(actual, case["expected"].as_str().unwrap(), "Kitty case {i}");
    }
}
#[test]
fn scroll_native_timer_notifies_without_a_host_poll() {
    let (tx, rx) = std::sync::mpsc::channel();
    let view = ScrollView::new(
        Box::new(Text::new("content")),
        ScrollViewOptions {
            scrollbar: Some(ScrollViewScrollbar::Auto),
            scrollbar_hide_delay_ms: Some(20.0),
            ..Default::default()
        },
    );
    let state = view.state();
    state.update_layout(
        8,
        4,
        Arc::new(move || {
            let _ = tx.send(());
        }),
    );
    state.scroll_by(1);
    rx.recv_timeout(Duration::from_secs(5))
        .expect("scroll change callback");
    rx.recv_timeout(Duration::from_secs(5))
        .expect("timer hide callback without polling");
    assert!(!state.snapshot().scrollbar_visible);
}
#[test]
fn scroll_timer_cancellation_drop_and_reentrant_callbacks_are_safe() {
    let clock = Arc::new(Mutex::new(Clock::default()));
    let notifications = Arc::new(Mutex::new(0));
    let view = ScrollView::new(
        Box::new(Text::new("content")),
        ScrollViewOptions {
            scrollbar: Some(ScrollViewScrollbar::Auto),
            timer_scheduler: Some(scheduler(clock.clone())),
            scrollbar_hide_delay_ms: Some(10.0),
            ..Default::default()
        },
    );
    let state = view.state();
    let count = notifications.clone();
    let reentrant = state.clone();
    state.update_layout(
        10,
        3,
        Arc::new(move || {
            let _ = reentrant.snapshot();
            *count.lock().unwrap() += 1;
        }),
    );
    state.scroll_by(1);
    state.set_scrollbar_active(true);
    advance(&clock, 20);
    assert!(state.snapshot().scrollbar_visible);
    assert_eq!(*notifications.lock().unwrap(), 2);
    state.set_scrollbar_active(false);
    state.set_scrollbar(ScrollViewScrollbar::Hidden);
    advance(&clock, 20);
    assert_eq!(*notifications.lock().unwrap(), 4);
    // Replace the deliberately self-capturing callback before drop.
    let count = notifications.clone();
    state.update_layout(10, 3, Arc::new(move || *count.lock().unwrap() += 1));
    state.set_scrollbar(ScrollViewScrollbar::Auto);
    state.scroll_by(1);
    let before = *notifications.lock().unwrap();
    drop(state);
    drop(view);
    advance(&clock, 20);
    assert_eq!(*notifications.lock().unwrap(), before);
}
#[test]
fn distinct_zero_sized_components_do_not_share_the_frame_cache() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    struct Zero;
    impl Component for Zero {
        fn render(&mut self, _: usize) -> Vec<String> {
            vec![CALLS.fetch_add(1, Ordering::SeqCst).to_string()]
        }
    }
    CALLS.store(0, Ordering::SeqCst);
    let mut stack = VStack::new(
        vec![
            StackEntry::new(Box::new(Zero), StackEntryOptions::default()),
            StackEntry::new(Box::new(Zero), StackEntryOptions::default()),
        ],
        StackOptions::default(),
    );
    let frame = render_layout_frame(&mut stack, 4, 2, Arc::new(|| {}));
    assert_eq!(
        frame.lines.present().map(|(_, s)| s).collect::<Vec<_>>(),
        ["0", "1"]
    );
    assert_eq!(CALLS.load(Ordering::SeqCst), 2);
    let frame = render_layout_frame(&mut stack, 4, 2, Arc::new(|| {}));
    assert_eq!(
        frame.lines.present().map(|(_, s)| s).collect::<Vec<_>>(),
        ["2", "3"]
    );
}
#[test]
fn kitty_registry_refreshes_insertion_order_and_evicts_at_one_thousand() {
    let mut registry = KittyImageRegistry::default();
    let meta = |id| KittyImageMetadata {
        image_id: id,
        columns: 2,
        rows: 3,
        width_px: 100,
        height_px: 101,
    };
    let line = |id| format!("prefix\x1b_Ga=T,i={id};AAAA\x1b\\suffix");
    for id in 0..1000 {
        registry.register(meta(id));
    }
    registry.register(meta(0));
    registry.register(meta(1000));
    assert_eq!(registry.get(&line(0)), Some(meta(0)));
    assert_eq!(registry.get(&line(1)), None);
    assert_eq!(registry.transmission_generation(&line(0)), Some(1001));
    for invalid in [
        "\x1b_Gi=-1;",
        "\x1b_Gii=0;",
        "\x1b_Gi=0x;",
        "\x1b_Gi=;",
        "\x1b_Gi=0",
    ] {
        assert_eq!(registry.get(invalid), None);
    }
}
