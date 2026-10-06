//! Direct actual-source differential coverage; not viewport layout integration.
use crate::tui::component::Component;
use crate::tui::components::stack::{
    allocate_stack_sizes, HStack, Stack, StackAlign, StackBasis, StackEntry, StackEntryOptions,
    StackKind, StackLayoutNode, StackOptions, VStack,
};
use crate::tui::overlay::composite_tui_line;
use crate::tui::utils::{visible_width, wrap_text_with_ansi};
use serde_json::{json, Map, Value};
use std::sync::{Arc, Mutex};

fn corpus() -> Value {
    serde_json::from_str(include_str!("../components/stack/fixtures.json")).unwrap()
}

#[test]
fn stack_compositor_matches_actual_upstream_bytes() {
    let data = corpus();
    let cases = data["composites"].as_array().unwrap();
    assert_eq!(cases.len(), 2691);
    let mut failures = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        let actual = std::panic::catch_unwind(|| {
            composite_tui_line(
                case["base"].as_str().unwrap(),
                case["overlay"].as_str().unwrap(),
                case["start"].as_u64().unwrap() as usize,
                case["width"].as_u64().unwrap() as usize,
                case["total"].as_u64().unwrap() as usize,
            )
        });
        let expected = case["output"].as_str().unwrap();
        match actual {
            Ok(output) if output == expected => {
                assert_eq!(
                    visible_width(&output),
                    case["visibleWidth"].as_u64().unwrap() as usize,
                    "width case {index}"
                );
            }
            actual => {
                if failures.len() < 8 {
                    eprintln!("COMPOSITE FAILURE {index}: input={case} actual={actual:?}");
                }
                failures.push(index);
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} composite differences: {failures:?}",
        failures.len()
    );
}

fn number(value: &Value) -> f64 {
    match value.as_str() {
        Some("NaN") => f64::NAN,
        Some("Infinity") => f64::INFINITY,
        Some("-Infinity") => f64::NEG_INFINITY,
        _ => value.as_f64().expect("numeric oracle value"),
    }
}

fn number_value(value: f64) -> Value {
    if value.is_nan() {
        json!("NaN")
    } else if value == f64::INFINITY {
        json!("Infinity")
    } else if value == f64::NEG_INFINITY {
        json!("-Infinity")
    } else if value.fract() == 0.0 && value.abs() <= 9_007_199_254_740_991.0 {
        json!(value as i64)
    } else {
        json!(value)
    }
}

fn entry_options(spec: &Value) -> StackEntryOptions {
    StackEntryOptions {
        basis: spec.get("basis").map(|value| {
            if value == "auto" {
                StackBasis::Auto
            } else {
                StackBasis::Size(number(value))
            }
        }),
        grow: spec.get("grow").map(number),
        shrink: spec.get("shrink").map(number),
        min_size: spec.get("minSize").map(number),
        max_size: spec.get("maxSize").map(number),
        visible: None,
    }
}

fn align(value: &str) -> StackAlign {
    match value {
        "stretch" => StackAlign::Stretch,
        "start" => StackAlign::Start,
        "center" => StackAlign::Center,
        "end" => StackAlign::End,
        _ => panic!("unknown oracle alignment: {value}"),
    }
}

fn node_value(node: StackLayoutNode<'_>, ids: Option<&[String]>) -> Value {
    let entries: Vec<_> = node
        .entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let mut value = Map::new();
            if let Some(ids) = ids {
                value.insert("id".into(), json!(ids[index]));
            }
            if let Some(basis) = entry.options.basis {
                value.insert(
                    "basis".into(),
                    match basis {
                        StackBasis::Auto => json!("auto"),
                        StackBasis::Size(size) => number_value(size),
                    },
                );
            }
            for (name, field) in [
                ("grow", entry.options.grow),
                ("shrink", entry.options.shrink),
                ("minSize", entry.options.min_size),
                ("maxSize", entry.options.max_size),
            ] {
                if let Some(field) = field {
                    value.insert(name.into(), number_value(field));
                }
            }
            if entry.options.visible.is_some() {
                value.insert("visible".into(), json!(true));
            }
            Value::Object(value)
        })
        .collect();
    json!({
        "type": match node.kind { StackKind::Horizontal => "hstack", StackKind::Vertical => "vstack" },
        "align": match node.align {
            StackAlign::Stretch => "stretch", StackAlign::Start => "start",
            StackAlign::Center => "center", StackAlign::End => "end",
        },
        "gap": number_value(node.gap), "entries": entries,
    })
}

#[test]
fn stack_allocator_matches_actual_upstream_numbers() {
    let data = corpus();
    let cases = data["allocations"].as_array().unwrap();
    assert_eq!(cases.len(), 4169);
    let mut failures = Vec::new();
    for case in cases {
        let entries: Vec<_> = case["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(entry_options)
            .collect();
        let intrinsic: Vec<_> = case["intrinsic"]
            .as_array()
            .unwrap()
            .iter()
            .map(number)
            .collect();
        let available = (!case["available"].is_null()).then(|| number(&case["available"]));
        let actual = allocate_stack_sizes(&entries, &intrinsic, available, number(&case["gap"]));
        let expected: Vec<_> = case["sizes"]
            .as_array()
            .unwrap()
            .iter()
            .map(number)
            .collect();
        if actual.len() != expected.len()
            || !actual
                .iter()
                .zip(&expected)
                .all(|(a, b)| a == b || (a.is_nan() && b.is_nan()))
        {
            if failures.len() < 8 {
                eprintln!("ALLOCATION FAILURE: {case}, actual={actual:?}");
            }
            failures.push(case["name"].as_str().unwrap());
        }
    }
    assert!(
        failures.is_empty(),
        "{} allocation differences: {failures:?}",
        failures.len()
    );
}

type Trace = Arc<Mutex<Vec<Value>>>;

struct Probe {
    id: String,
    lines: Vec<String>,
    mode: String,
    count: usize,
    trace: Trace,
}

impl Component for Probe {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.trace
            .lock()
            .unwrap()
            .push(json!(["render", self.id, width]));
        self.count += 1;
        match self.mode.as_str() {
            "width" => vec![format!("{width}:{}", self.id)],
            "stateful" => std::iter::once(self.count.to_string())
                .chain(self.lines.clone())
                .collect(),
            "wrap" => self
                .lines
                .iter()
                .flat_map(|s| wrap_text_with_ansi(s, width))
                .collect(),
            _ => self.lines.clone(),
        }
    }

    fn invalidate(&mut self) {
        self.trace
            .lock()
            .unwrap()
            .push(json!(["invalidate", self.id]));
    }
}

fn child(spec: &Value, trace: &Trace, id: &str) -> StackEntry {
    let component = if let Some(direction) = spec["stack"].as_str() {
        build(
            direction,
            spec,
            trace,
            id,
            spec["align"].as_str().unwrap_or("stretch"),
        )
    } else {
        Box::new(Probe {
            id: id.into(),
            lines: spec["lines"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s.as_str().unwrap().to_string())
                .collect(),
            mode: spec["mode"].as_str().unwrap_or_default().into(),
            count: 0,
            trace: trace.clone(),
        }) as Box<dyn Component>
    };
    if spec["bare"] == true {
        return StackEntry::from(component);
    }
    let mut options = entry_options(spec);
    if let Some(visibility) = spec["visible"].as_str() {
        let trace = trace.clone();
        let id = id.to_string();
        let never = visibility == "never";
        options.visible = Some(Arc::new(move |viewport| {
            trace
                .lock()
                .unwrap()
                .push(json!(["visible", id, viewport.width, viewport.height]));
            !never && viewport.width >= 4
        }));
    }
    StackEntry::new(component, options)
}

fn build(
    direction: &str,
    spec: &Value,
    trace: &Trace,
    path: &str,
    alignment: &str,
) -> Box<dyn Component> {
    let children = spec["children"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(index, spec)| child(spec, trace, &format!("{path}.{index}")))
        .collect();
    let options = StackOptions {
        gap: spec.get("gap").map(number),
        align: align(alignment),
    };
    match direction {
        "h" => Box::new(HStack::new(children, options)),
        "v" => Box::new(VStack::new(children, options)),
        _ => panic!("unknown oracle stack direction: {direction}"),
    }
}

#[test]
fn stack_constructor_normalization_matches_actual_upstream() {
    let data = corpus();
    let cases = data["normalizations"].as_array().unwrap();
    assert_eq!(cases.len(), 49);
    for (index, case) in cases.iter().enumerate() {
        let value = &case["value"];
        let spec = if value.is_null() {
            json!({"lines":[]})
        } else {
            json!({"lines":[], "basis":value, "grow":value, "shrink":value, "minSize":value, "maxSize":value})
        };
        let trace = Trace::default();
        let entry = child(&spec, &trace, "normalization");
        let stack = VStack::new(
            vec![entry],
            StackOptions {
                gap: (!case["gap"].is_null()).then(|| number(&case["gap"])),
                ..Default::default()
            },
        );
        assert_eq!(
            node_value(stack.layout_node(), None),
            case["node"],
            "normalization {index}: {case}"
        );
        assert!(
            trace.lock().unwrap().is_empty(),
            "constructor must not render children"
        );
    }
}

#[test]
fn stacks_direct_render_and_lifecycle_trace_match_actual_upstream() {
    let data = corpus();
    let cases = data["renders"].as_array().unwrap();
    assert_eq!(cases.len(), 952);
    let mut failures = Vec::new();
    for case in cases {
        let trace = Trace::default();
        let mut stack = build(
            case["direction"].as_str().unwrap(),
            &case["scenario"],
            &trace,
            "root",
            case["align"].as_str().unwrap(),
        );
        let width = case["width"].as_u64().unwrap() as usize;
        let first = stack.render(width);
        let second = stack.render(width);
        stack.invalidate();
        let invalidated = stack.render(width);
        let actual = json!({"first":first, "second":second, "invalidated":invalidated, "trace": *trace.lock().unwrap()});
        let expected = json!({"first":case["first"], "second":case["second"], "invalidated":case["invalidated"], "trace":case["trace"]});
        if actual != expected {
            if failures.len() < 8 {
                eprintln!(
                    "RENDER FAILURE {}: expected={expected}, actual={actual}",
                    case["name"]
                );
            }
            failures.push(case["name"].as_str().unwrap());
        }
    }
    assert!(
        failures.is_empty(),
        "{} direct-render differences: {failures:?}",
        failures.len()
    );
}

fn lifecycle<const H: bool>(case: &Value) {
    let trace = Trace::default();
    let initial = case["initial"].as_array().unwrap();
    let mut ids: Vec<String> = initial
        .iter()
        .map(|spec| spec["id"].as_str().unwrap().into())
        .collect();
    let children = initial
        .iter()
        .zip(&ids)
        .map(|(spec, id)| child(spec, &trace, id))
        .collect();
    let mut stack = Stack::<H>::new(
        children,
        StackOptions {
            gap: Some(number(&case["gap"])),
            align: align(case["align"].as_str().unwrap()),
        },
    );
    let actions = case["actions"].as_array().unwrap();
    let steps = case["steps"].as_array().unwrap();
    assert_eq!(actions.len(), steps.len());
    assert_eq!(actions.len(), 20);
    for (index, (action, expected)) in actions.iter().zip(steps).enumerate() {
        let mut lines = Value::Null;
        match action["op"].as_str().unwrap() {
            "render" => lines = json!(stack.render(case["width"].as_u64().unwrap() as usize)),
            "add" => {
                let spec = &action["child"];
                let id = spec["id"].as_str().unwrap();
                let entry = child(spec, &trace, id);
                assert_eq!(stack.add_child(entry.component, entry.options), ids.len());
                ids.push(id.into());
            }
            "remove" => {
                let index = action["index"].as_u64().unwrap() as usize;
                let removed = stack.remove_child(index);
                assert_eq!(removed.is_some(), index < ids.len());
                if index < ids.len() {
                    ids.remove(index);
                }
            }
            "invalidate" => stack.invalidate(),
            "clear" => {
                stack.clear();
                ids.clear();
            }
            op => panic!("unknown lifecycle action: {op}"),
        }
        assert_eq!(stack.len(), ids.len());
        assert_eq!(stack.is_empty(), ids.is_empty());
        let actual = json!({"lines":lines, "node":node_value(stack.layout_node(), Some(&ids)), "children":ids, "trace":*trace.lock().unwrap()});
        assert_eq!(
            &actual, expected,
            "lifecycle {} step {index}: {action}",
            case["name"]
        );
    }
}

#[test]
fn stacks_add_remove_clear_match_actual_upstream() {
    let data = corpus();
    let cases = data["lifecycles"].as_array().unwrap();
    assert_eq!(cases.len(), 48);
    for case in cases {
        match case["direction"].as_str().unwrap() {
            "h" => lifecycle::<true>(case),
            "v" => lifecycle::<false>(case),
            _ => unreachable!(),
        }
    }
    assert!(HStack::default().is_empty());
    assert!(VStack::default().is_empty());
}
