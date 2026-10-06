use crate::tui::component::{
    Component, TuiMouseButton, TuiMouseEvent, TuiMouseEventResult, TuiMouseEventType,
};
use crate::tui::components::input::{Input, InputOptions};
use crate::tui::components::select_list::{
    SelectItem, SelectList, SelectListCallbacks, SelectListLayoutOptions, SelectListTheme,
};
use crate::tui::mouse_dispatch::*;
use crate::tui::viewport_mouse::SgrMouseEvent;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

fn fixture() -> Value {
    serde_json::from_str(include_str!("../mouse_dispatch/fixtures.json")).unwrap()
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
fn target(v: &Value) -> MouseDispatchTarget<String> {
    MouseDispatchTarget {
        component: v["id"].as_str().unwrap().into(),
        origin_x: v["originX"].as_i64().unwrap(),
        origin_y: v["originY"].as_i64().unwrap(),
        width: v["width"].as_u64().unwrap() as usize,
        height: v["height"].as_u64().unwrap() as usize,
    }
}
fn result(v: &Value) -> MouseDispatchResult<String> {
    MouseDispatchResult {
        result: flags(v),
        target: target(&v["target"]),
        focus_target: v["focusTarget"].as_str().map(String::from),
    }
}
fn result_value(r: Option<MouseDispatchResult<String>>) -> Value {
    r.map_or(Value::Null,|r|{let mut v=flags_value(Some(r.result));v["target"]=json!({"id":r.target.component,"originX":r.target.origin_x,"originY":r.target.origin_y,"width":r.target.width,"height":r.target.height});v["focusTarget"]=json!(r.focus_target);v})
}

#[test]
fn normalized_mouse_events_match_actual_tui_alt_screen() {
    let f = fixture();
    for (i, c) in f["create"].as_array().unwrap().iter().enumerate() {
        let mut e = create_mouse_event(
            event_type(&c["type"]),
            c["button"].as_i64().unwrap(),
            c["x"].as_i64().unwrap(),
            c["y"].as_i64().unwrap(),
            c["cols"].as_u64().unwrap() as usize,
            c["rows"].as_u64().unwrap() as usize,
        );
        e.wheel_delta = c["extra"].get("wheelDelta").map(number);
        e.click_count = c["extra"]
            .get("clickCount")
            .map(|v| v.as_u64().unwrap() as u32);
        assert_eq!(event_value(&e), c["expected"], "create {i}");
    }
    for (i, c) in f["raw"].as_array().unwrap().iter().enumerate() {
        let v = &c["event"];
        let raw = SgrMouseEvent {
            button: v["button"].as_i64().unwrap(),
            x: v["x"].as_i64().unwrap(),
            y: v["y"].as_i64().unwrap(),
            release: v["release"].as_bool().unwrap(),
        };
        let e = create_mouse_event(mouse_event_type(raw), raw.button, raw.x, raw.y, 12, 5);
        assert_eq!(event_value(&e), c["expected"], "raw {i}");
    }
}

#[test]
fn dispatch_preserves_flags_nested_targets_focus_and_coordinates() {
    let f = fixture();
    for (i, c) in f["dispatch"].as_array().unwrap().iter().enumerate() {
        let e = event(&c["event"]);
        let mut calls = 0;
        let r = dispatch_mouse_event("a".to_owned(), &e, |actual| {
            calls += 1;
            assert_eq!(actual, &e);
            if c["response"].is_null() {
                None
            } else if c["forwarded"] == true {
                Some(MouseHandlerResult::Dispatched(result(&c["response"])))
            } else {
                Some(MouseHandlerResult::Direct(flags(&c["response"])))
            }
        });
        assert_eq!(calls, 1);
        assert_eq!(result_value(r), c["expected"], "dispatch {i}");
    }
}

#[test]
fn captured_retarget_uses_saved_origin_bounds_and_keeps_other_fields() {
    let f = fixture();
    for (i, c) in f["retarget"].as_array().unwrap().iter().enumerate() {
        let e = event(&c["event"]);
        let t = target(&c["target"]);
        let before = t.clone();
        assert_eq!(
            event_value(&retarget_mouse_event(&e, &t)),
            c["expected"],
            "retarget {i}"
        );
        assert_eq!(t, before);
    }
}

#[test]
fn explicit_render_overrides_focus_change_and_event_default() {
    let f = fixture();
    for (i, c) in f["render"].as_array().unwrap().iter().enumerate() {
        let focus = c["focus"].as_bool().unwrap();
        let r = MouseDispatchResult {
            result: TuiMouseEventResult {
                handled: true,
                focus,
                render: c["render"].as_bool(),
                ..Default::default()
            },
            target: MouseDispatchTarget {
                component: (),
                origin_x: 0,
                origin_y: 0,
                width: 1,
                height: 1,
            },
            focus_target: None,
        };
        assert_eq!(
            r.wants_render(
                event_type(&c["type"]),
                focus && c["changed"].as_bool().unwrap()
            ),
            c["expected"].as_bool().unwrap(),
            "render {i}"
        );
    }
}

#[test]
fn click_cycle_includes_identity_cell_timeout_clear_and_backward_clock() {
    let f = fixture();
    let mut tracker = ComponentClickTracker::<String>::default();
    for (i, c) in f["clicks"].as_array().unwrap().iter().enumerate() {
        let a = &c["action"];
        if a["clear"] == true {
            tracker.clear();
            assert!(tracker.previous().is_none());
            continue;
        }
        let n = tracker.count(
            a["id"].as_str().unwrap().into(),
            a["x"].as_i64().unwrap(),
            a["y"].as_i64().unwrap(),
            a["ms"].as_i64().unwrap(),
        );
        assert_eq!(json!(n), c["expected"], "click {i}");
        assert_eq!(tracker.previous().unwrap().count, n);
    }
}

#[test]
fn input_consumes_signed_columns_like_actual_component() {
    let f = fixture();
    for (i, c) in f["inputs"].as_array().unwrap().iter().enumerate() {
        let mut input = Input::new(InputOptions::default());
        input.set_value(c["value"].as_str().unwrap());
        input.set_focused(true);
        if c["end"] == true {
            input.handle_input("\x05");
        }
        input.render(c["width"].as_u64().unwrap() as usize);
        let r = dispatch_component_mouse(&mut input, "input".to_owned(), &event(&c["event"]));
        assert_eq!(
            flags_value(r.map(|r| r.result)),
            c["expected"],
            "input flags {i}"
        );
        assert_eq!(
            json!(&input.value()[..input.cursor()]),
            c["prefix"],
            "input prefix {i}"
        );
    }
}

#[test]
fn select_list_wheel_truthiness_and_numeric_direction_match_actual_component() {
    let f = fixture();
    for (i, c) in f["selects"].as_array().unwrap().iter().enumerate() {
        let trace = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink = trace.clone();
        let items = (0..c["count"].as_u64().unwrap())
            .map(|n| SelectItem {
                value: n.to_string(),
                label: format!("item{n}"),
                description: None,
            })
            .collect();
        let mut list = SelectList::new(
            items,
            3,
            SelectListTheme::default(),
            SelectListLayoutOptions::default(),
        )
        .with_callbacks(SelectListCallbacks {
            on_selection_change: Some(Box::new(move |item| {
                sink.lock().unwrap().push(item.value.clone())
            })),
            ..Default::default()
        });
        list.set_selected_index(c["start"].as_u64().unwrap() as usize);
        let r = list.handle_mouse(&event(&c["event"]));
        assert_eq!(flags_value(r), c["expected"], "select flags {i}");
        let selected = if list.filtered_len() == 0 {
            Value::Null
        } else {
            json!(list.selected_index().to_string())
        };
        assert_eq!(selected, c["selected"], "select value {i}");
        assert_eq!(
            json!(*trace.lock().unwrap()),
            c["trace"],
            "select trace {i}"
        );
    }
}

#[test]
fn owning_target_retains_component_and_integer_extremes_do_not_overflow() {
    // Rust ownership/domain test, not an upstream JS behavior assertion.
    let owner = Arc::new(String::from("stable-component"));
    let weak = Arc::downgrade(&owner);
    let mut e = create_mouse_event(TuiMouseEventType::Drag, 32, i64::MIN, i64::MAX, 0, 0);
    e.screen_x = i64::MAX;
    e.screen_y = i64::MIN;
    let dispatched = dispatch_mouse_event(owner, &e, |_| {
        Some(MouseHandlerResult::Direct(TuiMouseEventResult {
            capture: true,
            ..Default::default()
        }))
    })
    .unwrap();
    assert!(weak.upgrade().is_some());
    assert_eq!(dispatched.target.origin_x, i64::MAX);
    assert_eq!(dispatched.target.origin_y, i64::MIN);
    let mut opposite = e.clone();
    opposite.screen_x = i64::MIN;
    opposite.screen_y = i64::MAX;
    let retargeted = retarget_mouse_event(&opposite, &dispatched.target);
    assert_eq!(retargeted.x, i64::MIN);
    assert_eq!(retargeted.y, i64::MAX);
    drop(dispatched);
    assert!(weak.upgrade().is_none());
}
