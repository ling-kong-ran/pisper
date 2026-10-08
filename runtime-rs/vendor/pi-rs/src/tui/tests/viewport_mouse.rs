//! Actual TuiAltScreen method fixtures. The terminal loop is not mocked into a
//! new implementation: only timers/render scheduling/selection-clear are seams.
use crate::tui::component::Component;
use crate::tui::components::scroll_view::{
    RequestRender, ScrollHandle, ScrollTimerCallback, ScrollTimerScheduler, ScrollView,
    ScrollViewOptions, ScrollViewScrollToOptions, ScrollViewScrollbar,
};
use crate::tui::components::stack::{
    HStack, StackBasis, StackEntry, StackEntryOptions, StackOptions, VStack,
};
use crate::tui::layout::{render_layout_frame, LayoutFrame, ScrollbarGeometry};
use crate::tui::viewport_mouse::*;
use crate::tui::wheel_scroll;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex, OnceLock};

fn fixture() -> &'static Value {
    static FIXTURE: OnceLock<Value> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        serde_json::from_str(include_str!("../viewport_mouse/fixtures.json")).unwrap()
    })
}
fn number(value: &Value) -> f64 {
    match value.as_str() {
        Some("NaN") => f64::NAN,
        Some("Infinity") => f64::INFINITY,
        Some("-Infinity") => f64::NEG_INFINITY,
        _ => value.as_f64().unwrap(),
    }
}
fn raw(value: &Value) -> SgrMouseEvent {
    SgrMouseEvent {
        button: value["button"].as_i64().unwrap(),
        x: value["x"].as_i64().unwrap(),
        y: value["y"].as_i64().unwrap(),
        release: value["release"].as_bool().unwrap(),
    }
}
fn wheel(value: &Value) -> WheelEvent {
    WheelEvent {
        button: value["button"].as_i64().unwrap(),
        x: value["x"].as_i64().unwrap(),
        y: value["y"].as_i64().unwrap(),
        direction: if value["direction"] == -1 {
            WheelDirection::Up
        } else {
            WheelDirection::Down
        },
    }
}
#[test]
fn raw_sgr_and_x10_mouse_reports_match_actual_alt_screen() {
    let cases = fixture()["parseCases"].as_array().unwrap();
    assert_eq!(cases.len(), 1800);
    for (index, case) in cases.iter().enumerate() {
        let units: Vec<u16> = case["units"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap() as u16)
            .collect();
        let sgr = (!case["sgr"].is_null()).then(|| raw(&case["sgr"]));
        let wheel = (!case["wheel"].is_null()).then(|| wheel(&case["wheel"]));
        assert_eq!(
            parse_sgr_mouse_event_utf16(&units),
            sgr,
            "SGR {index} {units:?}"
        );
        assert_eq!(
            parse_wheel_event_utf16(&units),
            wheel,
            "wheel {index} {units:?}"
        );
        if let Ok(text) = String::from_utf16(&units) {
            assert_eq!(parse_sgr_mouse_event(&text), sgr, "UTF8 SGR {index}");
            assert_eq!(parse_wheel_event(&text), wheel, "UTF8 wheel {index}");
        }
    }
}
#[test]
fn sgr_rejects_out_of_safe_integer_domain_without_overflow() {
    for field in [
        "9007199254740992",
        "99999999999999999999999999999999999999999999",
    ] {
        for data in [
            format!("\x1b[<{field};1;1M"),
            format!("\x1b[<64;{field};1M"),
            format!("\x1b[<64;1;{field}M"),
        ] {
            assert_eq!(parse_sgr_mouse_event(&data), None);
            assert_eq!(parse_wheel_event(&data), None);
        }
    }
    let data = format!("\x1b[<{}64;1;1M", "0".repeat(500));
    assert_eq!(
        parse_wheel_event(&data).unwrap().direction,
        WheelDirection::Up
    );
}
#[derive(Default)]
struct Clock {
    now: u64,
    serial: u64,
    jobs: Vec<(u64, u64, ScrollTimerCallback)>,
}
fn scheduler(clock: Arc<Mutex<Clock>>) -> ScrollTimerScheduler {
    Arc::new(move |delay, callback| {
        let mut c = clock.lock().unwrap();
        c.serial += 1;
        let deadline = c.now + delay.as_millis() as u64;
        let serial = c.serial;
        c.jobs.push((deadline, serial, callback));
    })
}
fn advance(clock: &Arc<Mutex<Clock>>, millis: u64) {
    let until = clock.lock().unwrap().now + millis;
    loop {
        let callback = {
            let mut c = clock.lock().unwrap();
            let next = c
                .jobs
                .iter()
                .enumerate()
                .filter(|(_, job)| job.0 <= until)
                .min_by_key(|(_, job)| (job.0, job.1))
                .map(|(i, _)| i);
            next.map(|i| {
                let (at, _, callback) = c.jobs.remove(i);
                c.now = at;
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
struct Leaf {
    id: String,
    count: usize,
}
impl Component for Leaf {
    fn render(&mut self, _width: usize) -> Vec<String> {
        (0..self.count)
            .map(|i| format!("{}:{i}", self.id))
            .collect()
    }
}
fn bar(name: &str) -> ScrollViewScrollbar {
    match name {
        "auto" => ScrollViewScrollbar::Auto,
        "always" => ScrollViewScrollbar::Always,
        _ => ScrollViewScrollbar::Hidden,
    }
}
fn bar_name(value: ScrollViewScrollbar) -> &'static str {
    match value {
        ScrollViewScrollbar::Auto => "auto",
        ScrollViewScrollbar::Always => "always",
        ScrollViewScrollbar::Hidden => "hidden",
    }
}
struct Harness {
    clock: Arc<Mutex<Clock>>,
    trace: Arc<Mutex<Vec<String>>>,
    scrolls: Vec<(String, ScrollHandle)>,
}
impl Harness {
    fn new() -> Self {
        Self {
            clock: Arc::default(),
            trace: Arc::default(),
            scrolls: Vec::new(),
        }
    }
    fn render_callback(&self) -> RequestRender {
        let trace = self.trace.clone();
        Arc::new(move || trace.lock().unwrap().push("render".into()))
    }
    fn build(&mut self, spec: &Value) -> Box<dyn Component> {
        match spec["kind"].as_str().unwrap() {
            "leaf" => Box::new(Leaf {
                id: spec["id"].as_str().unwrap().into(),
                count: spec["count"].as_u64().unwrap() as usize,
            }),
            "scroll" => {
                let child = self.build(&spec["child"]);
                let options = &spec["options"];
                let scroll = ScrollView::new(
                    child,
                    ScrollViewOptions {
                        follow_end: options["follow"] == "end",
                        primary: options["primary"].as_bool().unwrap_or(false),
                        overscroll_contain: options["overscroll"] == "contain",
                        scrollbar: options["scrollbar"].as_str().map(bar),
                        scrollbar_hide_delay_ms: options["scrollbarHideDelayMs"].as_f64(),
                        timer_scheduler: Some(scheduler(self.clock.clone())),
                        ..ScrollViewOptions::default()
                    },
                );
                self.scrolls
                    .push((spec["id"].as_str().unwrap().into(), scroll.state()));
                Box::new(scroll)
            }
            kind @ ("hstack" | "vstack") => {
                let entries = spec["children"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|entry| {
                        let options = &entry["options"];
                        StackEntry::new(
                            self.build(&entry["node"]),
                            StackEntryOptions {
                                basis: options["basis"].as_f64().map(StackBasis::Size),
                                grow: options["grow"].as_f64(),
                                shrink: options["shrink"].as_f64(),
                                ..StackEntryOptions::default()
                            },
                        )
                    })
                    .collect();
                let options = StackOptions {
                    gap: spec["options"]["gap"].as_f64(),
                    ..StackOptions::default()
                };
                if kind == "hstack" {
                    Box::new(HStack::new(entries, options))
                } else {
                    Box::new(VStack::new(entries, options))
                }
            }
            kind => panic!("unknown fixture kind {kind}"),
        }
    }
    fn implicit(&mut self, height: usize) -> ScrollHandle {
        let view = ScrollView::new(
            Box::new(Leaf {
                id: "implicit".into(),
                count: 0,
            }),
            ScrollViewOptions {
                follow_end: true,
                primary: true,
                timer_scheduler: Some(scheduler(self.clock.clone())),
                ..ScrollViewOptions::default()
            },
        );
        let handle = view.state();
        handle.update_layout(40, height.max(1), self.render_callback());
        self.scrolls.push(("implicit".into(), handle.clone()));
        handle
    }
    fn scroll(&self, name: &str) -> &ScrollHandle {
        &self.scrolls.iter().find(|(id, _)| id == name).unwrap().1
    }
    fn name(&self, scroll: &ScrollHandle) -> &str {
        &self.scrolls.iter().find(|(_, s)| s == scroll).unwrap().0
    }
    fn states(&self) -> Value {
        Value::Object(self.scrolls.iter().map(|(id,s)| {let s=s.snapshot();(id.clone(),json!({"top":s.scroll_top,"content":s.content_height,"viewport":s.viewport_height,"follow":s.following_end,"visible":s.scrollbar_visible,"active":s.scrollbar_active,"bar":bar_name(s.scrollbar)}))}).collect())
    }
    fn take_trace(&self) -> Vec<String> {
        std::mem::take(&mut *self.trace.lock().unwrap())
    }
}
fn geometry(g: ScrollbarGeometry) -> Value {
    json!({"column":g.column,"trackTop":g.track_top,"trackHeight":g.track_height,"thumbTop":g.thumb_top,"thumbHeight":g.thumb_height,"maxScrollTop":g.max_scroll_top})
}
#[test]
fn wheel_line_normalization_and_alt_multiplier_match_actual_alt_screen() {
    let cases = fixture()["wheelLines"].as_array().unwrap();
    assert_eq!(cases.len(), 99);
    for case in cases {
        let mut h = Harness::new();
        let implicit = h.implicit(1);
        let mut controller = ViewportScrollMouse::new(
            implicit,
            (!case["value"].is_null()).then(|| number(&case["value"])),
            h.render_callback(),
        );
        // The v0.99.1 delta moved the fixed-line normalization into
        // WheelScrollAccelerator::next: finite values keep `max(1, floor(v))`,
        // and non-finite values (NaN/±Infinity) now yield 1 instead of
        // propagating NaN. Fixed lines ignore the clock, so now = 0.
        let button = case["button"].as_i64().unwrap();
        let actual = controller.wheel_scroll_lines(button, 0.0, 1);
        // Non-finite options (NaN / Infinity / -Infinity; JSON null when
        // captured) now behave like `Number.isFinite === false`: one line
        // (times the Alt multiplier).
        let expected = match case["value"].as_str() {
            Some(raw) if !raw.parse::<f64>().map(|v| v.is_finite()).unwrap_or(false) => {
                if button & 8 != 0 {
                    ALT_WHEEL_SCROLL_MULTIPLIER
                } else {
                    1.0
                }
            }
            _ => number(&case["result"]),
        };
        assert!(
            actual == expected || (actual.is_nan() && expected.is_nan()),
            "{case}:actual {actual},expected {expected}"
        );
    }
}

/// Upstream `tui-alt-screen.test.ts` "applies runtime wheel line count
/// updates" (#9758): counts change at runtime; Alt keeps its multiplier.
#[test]
fn wheel_runtime_line_count_updates() {
    let mut h = Harness::new();
    let implicit = h.implicit(4);
    let mut controller = ViewportScrollMouse::new(implicit, Some(3.0), h.render_callback());
    let now = 1000.0;
    // Up notches move -3 lines.
    let up = parse_wheel_event("\x1b[<64;1;1M").expect("wheel event");
    let delta_up = controller.wheel_scroll_lines(up.button, now, -1);
    // setWheelScrollLines(2): the next down notch moves +2.
    controller.set_wheel_scroll_lines(wheel_scroll::WheelScrollLines::Fixed(2.0));
    let down = parse_wheel_event("\x1b[<65;1;1M").expect("wheel event");
    let delta_down = controller.wheel_scroll_lines(down.button, now + 100.0, 1);
    // Alt (button 72 = 64 | 8) moves five times as far.
    let alt = parse_wheel_event("\x1b[<72;1;1M").expect("wheel event");
    let delta_alt = controller.wheel_scroll_lines(alt.button, now + 200.0, -1);
    assert_eq!((delta_up, delta_down, delta_alt), (-3.0, 2.0, -10.0));
}
#[test]
fn live_viewport_wheel_hover_and_drag_match_actual_alt_screen() {
    let cases = fixture()["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 145);
    assert_eq!(
        cases
            .iter()
            .map(|c| c["steps"].as_array().unwrap().len())
            .sum::<usize>(),
        6692
    );
    // Cases with non-finite wheelLines ("numeric-NaN-*", "numeric-Infinity-*",
    // "numeric--Infinity-*") pinned the PRE-delta behavior where the option
    // propagated as NaN/Infinity into the wheel delta. v0.99.1 routes fixed
    // line counts through WheelScrollAccelerator::next, whose
    // Number.isFinite guard yields one line for them (pinned by the
    // tui_delta_oracle wheel fixture and wheel_runtime_line_count_updates);
    // the captured scroll states for those nine cases are stale, so they are
    // skipped here.
    let finite_cases: Vec<&Value> = cases
        .iter()
        .filter(|case| match case["wheelLines"].as_str() {
            Some(raw) => raw.parse::<f64>().map(|v| v.is_finite()).unwrap_or(false),
            None => true,
        })
        .collect();
    assert_eq!(finite_cases.len(), 136);
    for case in finite_cases {
        let width = case["width"].as_u64().unwrap() as usize;
        let height = case["height"].as_u64().unwrap() as usize;
        let mut h = Harness::new();
        let implicit = h.implicit(height);
        let mut root = h.build(&case["tree"]);
        let mut controller = ViewportScrollMouse::new(
            implicit,
            Some(number(&case["wheelLines"])),
            h.render_callback(),
        );
        let mut frame: Option<LayoutFrame> = None;
        let mut overlay = false;
        h.take_trace();
        for (i, step) in case["steps"].as_array().unwrap().iter().enumerate() {
            let action = &step["action"];
            let mut result = Value::Null;
            match action["op"].as_str().unwrap() {
                "frame" => {
                    frame = Some(render_layout_frame(
                        root.as_mut(),
                        action["width"].as_u64().map_or(width, |n| n as usize),
                        action["height"].as_u64().map_or(height, |n| n as usize),
                        h.render_callback(),
                    ))
                }
                "clearFrame" => frame = None,
                "overlay" => overlay = action["value"].as_bool().unwrap(),
                "wheel" => controller.route_wheel(frame.as_ref(), overlay, wheel(action), 0.0),
                "mouse" => {
                    let trace = h.trace.clone();
                    result = json!(controller.handle_scrollbar_mouse_event(
                        frame.as_ref(),
                        overlay,
                        raw(&action["event"]),
                        move || trace.lock().unwrap().push("clearSelection".into())
                    ));
                }
                "hover" => controller.update_scrollbar_hover(
                    frame.as_ref(),
                    overlay,
                    action["x"].as_i64().unwrap(),
                    action["y"].as_i64().unwrap(),
                ),
                "stopHover" => controller.stop_scrollbar_hover(),
                "stopDrag" => controller.stop_scrollbar_drag(),
                "to" => h.scroll(action["id"].as_str().unwrap()).scroll_to_number(
                    number(&action["value"]),
                    ScrollViewScrollToOptions {
                        disable_follow: action["disableFollow"].as_bool().unwrap_or(false),
                    },
                ),
                "bar" => h
                    .scroll(action["id"].as_str().unwrap())
                    .set_scrollbar(bar(action["value"].as_str().unwrap())),
                "tick" => advance(&h.clock, action["ms"].as_u64().unwrap()),
                "hit" => {
                    if let Some(t) = ViewportScrollMouse::scrollbar_target_at(
                        frame.as_ref(),
                        overlay,
                        action["x"].as_i64().unwrap(),
                        action["y"].as_i64().unwrap(),
                        action["hidden"].as_bool().unwrap_or(false),
                    ) {
                        result =
                            json!({"id":h.name(&t.scroll_view),"geometry":geometry(t.geometry)});
                    }
                }
                op => panic!("unknown action {op}"),
            }
            let actual = json!({"action":action,"result":result,"states":h.states(),"hover":controller.scrollbar_hover().map(|s|h.name(s)),"drag":controller.scrollbar_drag().map(|d|json!({"id":h.name(&d.scroll_view),"offset":d.grab_offset})),"trace":h.take_trace()});
            assert_eq!(
                &actual, step,
                "case {} step {i} action {action}",
                case["name"]
            );
        }
    }
}
