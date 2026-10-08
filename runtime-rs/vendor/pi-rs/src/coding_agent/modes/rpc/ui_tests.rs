use super::*;
use crate::coding_agent::extensions::types::{AbortSignal, WidgetPlacement};
use futures::poll;
use std::task::Poll;

fn fixture() -> (RpcExtensionUi, Arc<Mutex<Vec<Value>>>) {
    let output = Arc::new(Mutex::new(Vec::new()));
    let sink = output.clone();
    (
        RpcExtensionUi::with_theme(
            Arc::new(move |value| sink.lock().unwrap().push(value)),
            Arc::new(|| json!({"name":"live"})),
        ),
        output,
    )
}
fn normalized(output: &Arc<Mutex<Vec<Value>>>) -> Value {
    let mut values = output.lock().unwrap().clone();
    let mut ids = std::collections::BTreeSet::new();
    for (index, value) in values.iter_mut().enumerate() {
        let id = value["id"].as_str().unwrap();
        assert_eq!(id.len(), 36);
        assert_eq!(&id[14..15], "4");
        assert!(matches!(&id[19..20], "8" | "9" | "a" | "b"));
        assert!(ids.insert(id.to_owned()));
        value["id"] = json!(format!("request-{}", index + 1));
    }
    json!(values)
}
fn reply(ui: &RpcExtensionUi, output: &Arc<Mutex<Vec<Value>>>, fields: &Value) -> bool {
    let mut response =
        json!({"type":"extension_ui_response","id":output.lock().unwrap().last().unwrap()["id"]});
    for (key, value) in fields.as_object().unwrap() {
        response[key] = value.clone();
    }
    ui.respond(response)
}
fn oracle() -> Value {
    serde_json::from_str(include_str!("ui_oracle.json")).unwrap()
}

#[tokio::test]
async fn dialog_wire_and_results_match_actual_upstream_ui_oracle() {
    for case in oracle()["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| case.get("method").is_some())
    {
        let (ui, output) = fixture();
        let args = case["args"].as_array().unwrap();
        let opts = ExtensionUiDialogOptions::default();
        let choices: Vec<String> = args
            .get(1)
            .and_then(Value::as_array)
            .map(|a| a.iter().map(|v| v.as_str().unwrap().into()).collect())
            .unwrap_or_default();
        let mut pending: UiFuture<'_, Value> = Box::pin(async {
            let title = args[0].as_str().unwrap();
            let argument = args.get(1).and_then(Value::as_str);
            Ok(match case["method"].as_str().unwrap() {
                "select" => json!(ui.select(title, &choices, &opts).await?),
                "confirm" => json!(ui.confirm(title, argument.unwrap(), &opts).await?),
                "input" => json!(ui.input(title, argument, &opts).await?),
                "editor" => json!(ui.editor(title, argument).await?),
                _ => unreachable!(),
            })
        });
        assert!(poll!(&mut pending).is_pending());
        assert_eq!(ui.pending_count(), 1);
        assert!(!ui.respond(json!({"type":"extension_ui_response","id":"unknown","value":"bad"})));
        assert!(reply(&ui, &output, &case["fields"]));
        assert!(!reply(&ui, &output, &json!({"value":"duplicate"})));
        assert_eq!(pending.await.unwrap(), case["result"], "{}", case["id"]);
        assert_eq!(normalized(&output), case["outputs"], "{}", case["id"]);
        assert_eq!(ui.pending_count(), 0);
    }
}

#[tokio::test(start_paused = true)]
async fn cancellation_and_response_first_winner_match_upstream_oracle() {
    for case in oracle()["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| case.get("timers").is_some())
    {
        let (ui, output) = fixture();
        let signal = Arc::new(AbortSignal::new());
        let id = case["id"].as_str().unwrap();
        if id == "preabort" {
            signal.abort();
        }
        let opts = ExtensionUiDialogOptions {
            signal: Some(signal.clone()),
            timeout: Some(25.5),
        };
        let mut pending = ui.input("Wait", None, &opts);
        let initial = poll!(&mut pending);
        if id == "preabort" {
            assert_eq!(initial, Poll::Ready(Ok(None)));
        } else {
            assert!(initial.is_pending());
            if id == "abort-response" {
                signal.abort();
                assert!(!reply(&ui, &output, &json!({"value":"late"})));
            }
            if id == "response-abort" {
                assert!(reply(&ui, &output, &json!({"value":"first"})));
                signal.abort();
            }
            if id == "timeout-response" {
                tokio::time::advance(Duration::from_millis(26)).await;
                tokio::task::yield_now().await;
                assert!(!reply(&ui, &output, &json!({"value":"late"})));
            }
            if id == "response-timeout" {
                assert!(reply(&ui, &output, &json!({"value":"first"})));
                tokio::time::advance(Duration::from_millis(26)).await;
            }
            assert_eq!(json!(pending.await.unwrap()), case["result"], "{id}");
        }
        assert_eq!(normalized(&output), case["outputs"], "{id}");
        assert_eq!(ui.pending_count(), 0);
    }
}

#[test]
fn notification_widget_and_editor_wire_matches_upstream_oracle() {
    let (ui, output) = fixture();
    ui.notify("hello", None);
    ui.notify("warn", Some("warning"));
    ui.set_status("s", None);
    ui.set_status("s", Some(""));
    ui.set_widget("w", None, &ExtensionWidgetOptions::default());
    ui.set_widget(
        "w",
        Some(&json!(["line", 4, null])),
        &ExtensionWidgetOptions {
            placement: Some(WidgetPlacement::BelowEditor),
        },
    );
    ui.set_widget(
        "ignore",
        Some(&Value::Null),
        &ExtensionWidgetOptions::default(),
    );
    ui.set_widget(
        "ignore",
        Some(&json!({"factory":true})),
        &ExtensionWidgetOptions::default(),
    );
    ui.set_title("title");
    ui.set_editor_text("direct");
    ui.paste_to_editor("paste");
    let expected = oracle()["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == "fire-and-forget")
        .unwrap()
        .clone();
    assert_eq!(normalized(&output), expected["outputs"]);
    assert_eq!(ui.pending_count(), 0);
}

#[tokio::test]
async fn unsupported_ui_is_explicitly_noop_and_theme_is_live() {
    let (ui, output) = fixture();
    ui.on_terminal_input(Value::Null).unsubscribe();
    ui.set_working_message(Some("busy"));
    ui.set_working_visible(true);
    ui.set_working_indicator(None);
    ui.set_hidden_thinking_label(Some("h"));
    ui.set_footer(Some(&json!({})));
    ui.set_header(Some(&json!({})));
    ui.add_autocomplete_provider(&json!({}));
    ui.set_editor_component(Some(&json!({})));
    ui.set_tools_expanded(true);
    let set = ui.set_theme(&json!("dark"));
    let actual = json!({"id":"unsupported-defaults","outputs":normalized(&output),
        "custom":ui.custom(&Value::Null,&ExtensionUiDialogOptions::default()).await.unwrap(),
        "editorText":ui.get_editor_text(),"editorComponent":ui.get_editor_component(),"allThemes":ui.get_all_themes(),
        "missingTheme":ui.get_theme("none"),"setTheme":{"success":set.success,"error":set.error},
        "toolsExpanded":ui.get_tools_expanded(),"theme":ui.theme()});
    let oracle = oracle();
    let expected = oracle["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == "unsupported-defaults")
        .unwrap();
    assert_eq!(&actual, expected);
    let theme = Arc::new(Mutex::new(json!({"name":"one"})));
    let handle = theme.clone();
    let ui = RpcExtensionUi::with_theme(
        Arc::new(|_| {}),
        Arc::new(move || handle.lock().unwrap().clone()),
    );
    assert_eq!(ui.theme()["name"], "one");
    *theme.lock().unwrap() = json!({"name":"two"});
    assert_eq!(ui.theme()["name"], "two");
}

#[tokio::test(start_paused = true)]
async fn dropping_or_rejecting_dialog_cleans_pending_without_leaking_timers() {
    let (ui, output) = fixture();
    let signal = Arc::new(AbortSignal::new());
    let opts = ExtensionUiDialogOptions {
        signal: Some(signal.clone()),
        timeout: Some(1000.0),
    };
    let mut dialog = ui.input("drop", None, &opts);
    assert!(poll!(&mut dialog).is_pending());
    assert_eq!(ui.pending_count(), 1);
    drop(dialog);
    assert_eq!(ui.pending_count(), 0);
    signal.abort();
    tokio::time::advance(Duration::from_secs(2)).await;
    assert!(!reply(&ui, &output, &json!({"value":"too late"})));
    let mut first = ui.editor("editor", None);
    let opts = ExtensionUiDialogOptions::default();
    let mut second = ui.confirm("confirm", "message", &opts);
    assert!(poll!(&mut first).is_pending());
    assert!(poll!(&mut second).is_pending());
    ui.reject_pending("host gone");
    assert_eq!(first.await.unwrap_err(), "host gone");
    assert_eq!(second.await.unwrap_err(), "host gone");
    assert_eq!(ui.pending_count(), 0);
}

#[test]
fn timer_coercion_preserves_node_truthiness_and_range() {
    for timeout in [0.0, -0.0, f64::NAN] {
        assert_eq!(node_timeout(timeout), None);
    }
    for timeout in [-8.0, 0.5, f64::INFINITY, f64::NEG_INFINITY, 2147483648.0] {
        assert_eq!(node_timeout(timeout), Some(Duration::from_millis(1)));
    }
    assert_eq!(node_timeout(25.9), Some(Duration::from_millis(25)));
    assert_eq!(
        node_timeout(2147483647.0),
        Some(Duration::from_millis(2147483647))
    );
}

#[tokio::test]
async fn output_may_respond_reentrantly_without_deadlock() {
    let slot: Arc<Mutex<Option<RpcExtensionUi>>> = Arc::new(Mutex::new(None));
    let target = slot.clone();
    let ui = RpcExtensionUi::new(Arc::new(move |request| {
        let ui = target.lock().unwrap().as_ref().unwrap().clone();
        assert!(ui.respond(
            json!({"type":"extension_ui_response","id":request["id"],"value":"immediate"})
        ));
    }));
    *slot.lock().unwrap() = Some(ui.clone());
    assert_eq!(
        ui.input("input", None, &ExtensionUiDialogOptions::default())
            .await
            .unwrap()
            .as_deref(),
        Some("immediate")
    );
    assert_eq!(ui.pending_count(), 0);
    slot.lock().unwrap().take();
}

#[tokio::test]
async fn native_abort_listeners_are_idempotent_wakeable_and_reentrant() {
    let signal = Arc::new(AbortSignal::new());
    let calls = Arc::new(Mutex::new(Vec::new()));
    let values = calls.clone();
    let removed = signal.on_abort(Arc::new(move || values.lock().unwrap().push(0)));
    drop(removed);
    let weak = Arc::downgrade(&signal);
    let values = calls.clone();
    let _one = signal.on_abort(Arc::new(move || {
        values.lock().unwrap().push(1);
        weak.upgrade().unwrap().abort();
    }));
    let values = calls.clone();
    let _two = signal.on_abort(Arc::new(move || values.lock().unwrap().push(2)));
    let mut waiter = Box::pin(signal.cancelled());
    assert!(poll!(&mut waiter).is_pending());
    signal.abort();
    signal.abort();
    assert!(poll!(&mut waiter).is_ready());
    let values = calls.clone();
    let _late = signal.on_abort(Arc::new(move || values.lock().unwrap().push(3)));
    assert_eq!(*calls.lock().unwrap(), vec![1, 2, 3]);
}

#[test]
fn abort_listener_can_remove_a_later_listener_during_dispatch() {
    let signal = Arc::new(AbortSignal::new());
    let later = Arc::new(Mutex::new(None));
    let handle = later.clone();
    let _first = signal.on_abort(Arc::new(move || {
        handle.lock().unwrap().take();
    }));
    *later.lock().unwrap() =
        Some(signal.on_abort(Arc::new(|| panic!("removed listener must not run"))));
    signal.abort();
    assert!(signal.is_aborted());
}
