use super::*;
use crate::tui::component::{is_focusable, CURSOR_MARKER};
use crate::tui::component_mouse::ComponentHandle;
use crate::tui::keybindings::{tui_keybindings, KeybindingsManager};
use crate::tui::utils::strip_terminal_sequences;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::OnceLock;

fn fixture() -> &'static Value {
    static FIXTURE: OnceLock<Value> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        serde_json::from_str(include_str!("../alt_screen_search_component/fixtures.json")).unwrap()
    })
}
fn direction(value: &Value) -> Option<SearchNavigationDirection> {
    match value.as_i64() {
        Some(-1) => Some(SearchNavigationDirection::Previous),
        Some(1) => Some(SearchNavigationDirection::Next),
        None => None,
        _ => panic!("unexpected direction {value}"),
    }
}
fn direction_value(direction: Option<SearchNavigationDirection>) -> Value {
    match direction {
        Some(SearchNavigationDirection::Previous) => json!(-1),
        Some(SearchNavigationDirection::Next) => json!(1),
        None => Value::Null,
    }
}
fn state(component: &AltScreenSearchComponent) -> Value {
    let range = |value: &Option<std::ops::Range<usize>>| match value {
        Some(value) => [value.start as i64, value.end as i64],
        None => [-1, -1],
    };
    let previous = range(&component.previous_button);
    let next = range(&component.next_button);
    json!({
        "value":component.input.value(),
        // Observational conversion only; real Input retains its own byte cursor.
        "cursor":component.input.value()[..component.input.cursor()].encode_utf16().count(),
        "focused":component.focused(),"inputFocused":component.input.focused(),
        "rect":[previous[0],previous[1],next[0],next[1]],
        "hover":direction_value(component.hovered_navigation_direction),
    })
}
fn replay(case: &Value) -> Result<(), String> {
    let events = Rc::new(RefCell::new(Vec::<Value>::new()));
    let queries = events.clone();
    let styles = events.clone();
    let style = case["style"].as_str().unwrap().to_owned();
    let manager = Rc::new(RefCell::new(KeybindingsManager::new(
        &tui_keybindings(),
        &[],
    )));
    let environment_manager = manager.clone();
    let host = Rc::new(RefCell::new(case["host"].as_str().unwrap().to_owned()));
    let environment_host = host.clone();
    let mut component = AltScreenSearchComponent::with_environment(
        move |query| {
            queries
                .borrow_mut()
                .push(json!({"type":"query","query":query}))
        },
        move |text, hovered| {
            if style != "default" {
                styles
                    .borrow_mut()
                    .push(json!({"type":"style","text":text,"hovered":hovered}));
            }
            match style.as_str() {
                "ansi" => format!(
                    "{}{text}\x1b[49m",
                    if hovered { "\x1b[45m" } else { "\x1b[44m" }
                ),
                "empty" => String::new(),
                "expand" => format!(
                    "{}{text}{}",
                    if hovered { '[' } else { '{' },
                    if hovered { ']' } else { '}' }
                ),
                "osc" => format!("\x1b]8;;https://example.invalid/\x07{text}\x1b]8;;\x07"),
                "asymmetric" if text.starts_with('↑') => String::new(),
                "asymmetric" => format!("\x1b[2m{}\x1b[22m", text.chars().next().unwrap()),
                _ => text.to_owned(),
            }
        },
        move || {
            let manager = environment_manager.borrow();
            SearchNavigationEnvironment {
                macos: *environment_host.borrow() == "darwin",
                previous_keys: manager.get_keys("tui.altScreen.searchPrevious"),
                next_keys: manager.get_keys("tui.altScreen.searchNext"),
            }
        },
    );
    let ops = case["ops"].as_array().unwrap();
    assert_eq!(ops.len(), case["expected"].as_array().unwrap().len());
    for (index, op) in ops.iter().enumerate() {
        events.borrow_mut().clear();
        let output = match op["op"].as_str().unwrap() {
            "input" => {
                component.handle_input(op["data"].as_str().unwrap());
                Value::Null
            }
            "focus" => {
                component.set_focused(op["value"].as_bool().unwrap());
                Value::Null
            }
            "result" => {
                component.set_result(op["index"].as_i64().unwrap(), op["count"].as_i64().unwrap());
                Value::Null
            }
            "hover" => {
                json!(component.set_hovered_navigation_direction(direction(&op["direction"])))
            }
            "invalidate" => {
                component.invalidate();
                Value::Null
            }
            "keys" => {
                let bindings: Vec<_> = op["bindings"]
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(id, keys)| {
                        (
                            id.as_str(),
                            keys.as_array()
                                .unwrap()
                                .iter()
                                .map(|v| v.as_str().unwrap().to_owned())
                                .collect(),
                        )
                    })
                    .collect();
                manager.borrow_mut().set_user_bindings(&bindings);
                Value::Null
            }
            "platform" => {
                *host.borrow_mut() = op["value"].as_str().unwrap().to_owned();
                Value::Null
            }
            "probe" => Value::Array(
                op["points"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|point| {
                        direction_value(component.navigation_direction_at(
                            point[0].as_i64().unwrap(),
                            point[1].as_i64().unwrap(),
                        ))
                    })
                    .collect(),
            ),
            "render" => {
                let width = op["width"].as_u64().unwrap() as usize;
                let lines = component.render(width);
                let widths: Vec<_> = lines.iter().map(|line| visible_width(line)).collect();
                let hits: Vec<_> = (-2..width as i64 + 3)
                    .map(|column| direction_value(component.navigation_direction_at(2, column)))
                    .collect();
                json!({"lines":lines,"widths":widths,"hits":hits})
            }
            other => panic!("unknown operation {other}"),
        };
        let actual = json!({"output":output,"events":*events.borrow(),"state":state(&component)});
        if actual != case["expected"][index] {
            return Err(format!(
                "{} step {index} op={op}\nACTUAL {actual}\nEXPECTED {}",
                case["name"], case["expected"][index]
            ));
        }
    }
    Ok(())
}
fn check_group(name: &str, count: usize) {
    let cases = fixture()[name].as_array().unwrap();
    assert_eq!(cases.len(), count);
    let mut failures = Vec::new();
    for case in cases {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| replay(case))) {
            Ok(Ok(())) => {}
            Ok(Err(error)) => failures.push(error),
            Err(_) => failures.push(format!("{} PANIC", case["name"])),
        }
    }
    assert!(
        failures.is_empty(),
        "{name}: {} of {count} cases failed\n{}",
        failures.len(),
        failures.join("\n")
    );
}
macro_rules! oracle_group {
    ($name:ident,$key:literal,$count:literal) => {
        #[test]
        fn $name() {
            check_group($key, $count);
        }
    };
}
oracle_group!(search_component_oracle_render, "render", 953);
oracle_group!(search_component_oracle_styles, "styles", 84);
oracle_group!(search_component_oracle_keys, "keys", 169);
oracle_group!(search_component_oracle_editing, "editing", 35);
oracle_group!(search_component_oracle_unicode, "unicode", 198);
oracle_group!(search_component_oracle_paste, "paste", 24);
oracle_group!(search_component_oracle_sequences, "sequences", 50);

#[test]
fn search_component_upstream_placeholder_and_right_aligned_controls() {
    let mut component = AltScreenSearchComponent::new(|_| {});
    let rendered = component.render(48);
    let lines: Vec<_> = rendered
        .iter()
        .map(|line| strip_terminal_sequences(line))
        .collect();
    assert_eq!(lines.len(), 3);
    assert!(lines.iter().all(|line| visible_width(line) == 48));
    assert!(lines[0].starts_with("┌─") && lines[0].ends_with('┐'));
    assert!(lines[1].starts_with("│ Find in transcript ") && lines[1].ends_with('│'));
    assert!(rendered[1].contains("\x1b[2m"));
    assert!(lines[2].ends_with(" ↑ Shift+Enter · ↓ Enter ─┘"));
    let column = |needle: &str| lines[2][..lines[2].find(needle).unwrap()].chars().count() as i64;
    assert_eq!(
        component.navigation_direction_at(2, column("↑")),
        Some(SearchNavigationDirection::Previous)
    );
    assert_eq!(
        component.navigation_direction_at(2, column("Shift+Enter") + 5),
        Some(SearchNavigationDirection::Previous)
    );
    assert_eq!(component.navigation_direction_at(2, column("·")), None);
    assert_eq!(
        component.navigation_direction_at(2, column("↓")),
        Some(SearchNavigationDirection::Next)
    );
    component.handle_input("n");
    component.set_result(0, 2);
    let populated = component.render(48);
    assert!(populated[1].contains("\x1b[2m 1/2 \x1b[22m"));
    assert!(!populated
        .iter()
        .any(|line| line.contains("Find in transcript")));
}

#[test]
fn search_component_owning_component_focus_and_input_are_real() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let output = calls.clone();
    let (handle, typed) = ComponentHandle::with_shared(AltScreenSearchComponent::new(move |q| {
        output.borrow_mut().push(q.to_owned())
    }));
    handle.with_mut(|component| {
        assert!(is_focusable(component));
        assert!(!component.focused());
        component.set_focused(true);
        component.handle_input("界🙂");
        assert!(component.render(48)[1].contains(CURSOR_MARKER));
        component.invalidate();
    });
    assert_eq!(&*calls.borrow(), &["界🙂"]);
    assert_eq!(typed.borrow().input.value(), "界🙂");
    assert!(typed.borrow().input.focused());
    handle.with_mut(|component| component.set_focused(false));
    assert!(!typed.borrow_mut().render(48)[1].contains(CURSOR_MARKER));
    assert!(!typed.borrow().input.focused());
}

#[test]
fn search_component_query_callback_once_after_complete_input_not_cursor_moves() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let output = calls.clone();
    let mut component =
        AltScreenSearchComponent::new(move |q| output.borrow_mut().push(q.to_owned()));
    for data in ["", "\x1b[D", "\x1b", "\r", "\x03", "\x1b[31m"] {
        component.handle_input(data);
    }
    assert!(calls.borrow().is_empty());
    component.handle_input("\x1b[200~a\nb\x1b[201~c");
    assert_eq!(&*calls.borrow(), &["abc"]);
    component.handle_input("\x1b[D");
    component.handle_input("\x1b[C");
    component.set_focused(true);
    component.invalidate();
    component.render(48);
    assert_eq!(calls.borrow().len(), 1);
    component.handle_input("\x7f");
    assert_eq!(&*calls.borrow(), &["abc", "ab"]);
}

#[test]
fn search_component_rect_stays_stale_until_render_and_is_half_open() {
    let mut component = AltScreenSearchComponent::new(|_| {});
    assert_eq!(component.navigation_direction_at(2, 3), None);
    component.render(8);
    assert_eq!(component.previous_button, Some(2..3));
    assert_eq!(component.next_button, Some(4..5));
    component.handle_input("query");
    component.set_result(8, 10);
    component.set_hovered_navigation_direction(Some(SearchNavigationDirection::Next));
    component.invalidate();
    assert_eq!(
        component.navigation_direction_at(2, 2),
        Some(SearchNavigationDirection::Previous)
    );
    assert_eq!(component.navigation_direction_at(2, 3), None);
    assert_eq!(
        component.navigation_direction_at(2, 4),
        Some(SearchNavigationDirection::Next)
    );
    for (row, column) in [(1, 3), (3, 5), (-1, 3), (2, -1), (2, 5)] {
        assert_eq!(component.navigation_direction_at(row, column), None);
    }
    component.render(1);
    assert_eq!(component.previous_button, None);
    assert_eq!(component.next_button, None);
}

#[test]
fn search_component_styles_are_ordered_only_when_shown_and_rects_are_unstyled() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let output = calls.clone();
    let mut component = AltScreenSearchComponent::with_navigation_button_style(
        |_| {},
        move |text, hovered| {
            output.borrow_mut().push((text.to_owned(), hovered));
            format!("[{text}]")
        },
    );
    assert!(component.set_hovered_navigation_direction(Some(SearchNavigationDirection::Previous)));
    assert!(!component.set_hovered_navigation_direction(Some(SearchNavigationDirection::Previous)));
    assert_eq!(component.render(0), ["┌", "│", "└"]);
    component.render(7);
    assert!(calls.borrow().is_empty());
    let lines = component.render(8);
    assert_eq!(
        &*calls.borrow(),
        &[("↑".to_owned(), true), ("↓".to_owned(), false)]
    );
    assert_eq!(visible_width(&lines[2]), 12); // Width-changing painters are not silently clipped.
    assert_eq!(component.previous_button, Some(2..3));
    assert_eq!(component.next_button, Some(4..5));
}

#[test]
fn search_component_format_key_preserves_js_first_utf16_unit_and_option() {
    for (key, macos, expected) in [
        ("alt+enter", true, "Option+Enter"),
        ("aLt+enter", true, "Option+Enter"),
        ("alt+enter", false, "Alt+Enter"),
        ("ctrl+ß", false, "Ctrl+SS"),
        ("𐐨+ß", false, "𐐨+SS"),
        ("+", false, "+"),
        ("", false, "Unbound"),
    ] {
        assert_eq!(format_key(Some(&key.to_owned()), macos), expected);
    }
    assert_eq!(format_key(None, false), "Unbound");
}

#[test]
fn search_component_environment_read_each_render_even_tiny_no_fake_input() {
    let reads = Rc::new(RefCell::new(0));
    let observed = reads.clone();
    let mut component = AltScreenSearchComponent::with_environment(
        |_| {},
        |text, _| text.to_owned(),
        move || {
            *observed.borrow_mut() += 1;
            SearchNavigationEnvironment {
                macos: true,
                previous_keys: vec!["alt+p".to_owned(), "unused".to_owned()],
                next_keys: vec![],
            }
        },
    );
    component.render(0);
    component.render(1);
    assert_eq!(*reads.borrow(), 2);
    let lines = component.render(48);
    assert!(lines[2].contains("↑ Option+P · ↓ Unbound"));
    assert!(!lines[2].contains("unused"));
    assert!(strip_terminal_sequences(&lines[1]).contains("Find in transcript"));
    assert_eq!(*reads.borrow(), 3);
}

#[test]
fn search_component_vs16_reference_membership_width_truncate_and_slice() {
    use crate::tui::utils::{is_rgi_emoji_probe, slice_by_column};
    let fixture: Value = serde_json::from_str(include_str!(
        "../alt_screen_search_component/width_vs16_fixtures.json"
    ))
    .unwrap();
    assert_eq!(fixture["scannedScalars"], 1_112_064);
    assert_eq!(fixture["acceptedBases"].as_array().unwrap().len(), 207);
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 1606);
    let mut failures = Vec::new();
    for case in cases {
        let text = case["text"].as_str().unwrap();
        let actual = json!({
            "text":text,"width":visible_width(text),"rgi":is_rgi_emoji_probe(text),
            "truncate":(0..=5).map(|width|truncate_to_width(text,width,"",false)).collect::<Vec<_>>(),
            "slices":(0..=2).map(|start|slice_by_column(text,start,1,true)).collect::<Vec<_>>(),
        });
        if &actual != case {
            failures.push(format!("ACTUAL {actual}\nEXPECTED {case}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} cases failed\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn search_component_vs16_correction_is_shared_with_raw_width_and_search_index() {
    use crate::tui::alt_screen_search_index::find_alt_screen_search_matches;
    use crate::tui::utils::visible_width_utf16;
    assert_eq!(visible_width("©️"), 2);
    assert_eq!(visible_width("©"), 1);
    assert_eq!(
        visible_width_utf16(&"©️".encode_utf16().collect::<Vec<_>>()),
        2
    );
    let matches = find_alt_screen_search_matches(&["©️x"], "x");
    let matches = matches.borrow();
    let matched = matches[0].borrow();
    let segments = matched.segments.borrow();
    let segment = segments[0].borrow();
    assert_eq!((segment.row, segment.start_col, segment.end_col), (0, 2, 3));
}
