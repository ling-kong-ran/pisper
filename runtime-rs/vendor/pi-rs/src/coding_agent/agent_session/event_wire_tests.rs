//! Live handler/reducer integration in addition to the direct oracle bridge.
use super::*;
use crate::agent_core::types::{AgentEvent, AgentMessage};
use crate::coding_agent::extensions::types::HandlerResult;
use crate::coding_agent::modes::json_event::to_json_event_string;
use std::sync::atomic::Ordering;

type Capture = Arc<Mutex<Vec<(String, Result<String, String>)>>>;
fn oracle() -> Value {
    serde_json::from_str(include_str!("../modes/json_ingress_oracle.json")).unwrap()
}
fn row(name: &str) -> Value {
    oracle()["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == name)
        .unwrap()
        .clone()
}
fn event(name: &str) -> AgentEvent {
    serde_json::from_str(row(name)["input"].as_str().unwrap()).unwrap()
}
fn capture(session: &AgentSession) -> Capture {
    let events: Capture = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    session.subscribe(Arc::new(move |event| {
        let wire = serde_json::to_value(event).unwrap();
        sink.lock().unwrap().push((
            wire["type"].as_str().unwrap().into(),
            to_json_event_string(event),
        ));
    }));
    events
}

#[test]
fn json_ingress_session_bridge_matches_both_upstream_retry_overlays() {
    for row in oracle()["cases"].as_array().unwrap() {
        let event: AgentEvent = serde_json::from_str(row["input"].as_str().unwrap()).unwrap();
        let before = serde_json::to_string(&event).unwrap();
        for projection in row["projections"].as_array().unwrap() {
            let session = AgentSessionEvent::from_agent_event(
                &event,
                projection["willRetry"].as_bool().unwrap(),
            );
            let mut wire = serde_json::to_value(&session).unwrap();
            crate::serde_support::order_json_object_keys(&mut wire);
            let actual = crate::serde_support::to_json_string_with_js_numbers(&wire).unwrap();
            assert_eq!(
                actual,
                projection["sessionOut"].as_str().unwrap(),
                "{}",
                row["name"]
            );
            assert!(!matches!(session.kind(), AgentSessionEvent::Preserved(_)));
        }
        assert_eq!(serde_json::to_string(&event).unwrap(), before);
    }
}

#[tokio::test]
async fn json_ingress_live_session_dispatch_matches_all_oracle_cases() {
    let test = create_test_session(Vec::new(), Vec::new()).await;
    let captured = capture(&test.session);
    for row in oracle()["cases"].as_array().unwrap() {
        captured.lock().unwrap().clear();
        let event: AgentEvent = serde_json::from_str(row["input"].as_str().unwrap()).unwrap();
        let kind = serde_json::to_value(&event).unwrap()["type"]
            .as_str()
            .unwrap()
            .to_string();
        test.session.handle_agent_event(event).await;
        let values = captured.lock().unwrap();
        let emitted = &values.iter().find(|(ty, _)| ty == &kind).unwrap().1;
        let projection = &row["projections"][0];
        let expected = if projection["ok"] == true {
            Ok(projection["out"].as_str().unwrap().into())
        } else {
            Err(projection["error"].as_str().unwrap().into())
        };
        assert_eq!(emitted, &expected, "{}", row["name"]);
    }
}

#[tokio::test]
async fn json_ingress_live_session_updates_queues_before_listeners_and_flushes_turn_end() {
    let test = create_test_session(Vec::new(), Vec::new()).await;
    let session = &test.session;
    let captured = capture(session);
    session
        .steering_messages
        .lock()
        .unwrap()
        .push("queued".into());
    session
        .follow_up_messages
        .lock()
        .unwrap()
        .push("queued".into());
    session
        .overflow_recovery_attempted
        .store(true, Ordering::SeqCst);
    for remaining in [1, 0] {
        captured.lock().unwrap().clear();
        session
            .handle_agent_event(event("message_start_user"))
            .await;
        assert!(session.steering_messages.lock().unwrap().is_empty());
        assert_eq!(session.follow_up_messages.lock().unwrap().len(), remaining);
        assert!(!session.overflow_recovery_attempted.load(Ordering::SeqCst));
        let kinds: Vec<_> = captured
            .lock()
            .unwrap()
            .iter()
            .map(|(ty, _)| ty.clone())
            .collect();
        assert_eq!(kinds, ["queue_update", "message_start"]);
    }
    let custom: AgentMessage = serde_json::from_value(json!({"role":"custom","customType":"test","content":"after turn","display":false,"timestamp":0})).unwrap();
    session
        .pending_custom_messages
        .lock()
        .unwrap()
        .push(custom.clone());
    captured.lock().unwrap().clear();
    session.handle_agent_event(event("turn_end")).await;
    assert!(session.pending_custom_messages.lock().unwrap().is_empty());
    // Delta: the flushed message reaches agent state through the session
    // projection (append entry + `_refreshFinalizedContext`), so state holds
    // the projected custom message rather than the pushed object (its
    // timestamp comes from the entry).
    let last = session.agent.state().messages.last().cloned();
    let AgentMessage::Custom(last_custom) = last.expect("custom message in state") else {
        panic!("expected a custom message in agent state");
    };
    assert_eq!(last_custom.role, "custom");
    assert_eq!(
        last_custom.data.get("customType").and_then(Value::as_str),
        Some("test")
    );
    assert_eq!(
        last_custom.data.get("content").and_then(Value::as_str),
        Some("after turn")
    );
    assert_eq!(session.turn_index.load(Ordering::SeqCst), 1);
    let kinds: Vec<_> = captured
        .lock()
        .unwrap()
        .iter()
        .map(|(ty, _)| ty.clone())
        .collect();
    assert_eq!(kinds, ["turn_end", "message_start", "message_end"]);
}

#[tokio::test]
async fn json_ingress_live_retry_is_computed_without_losing_original_slots() {
    let test = create_test_session(Vec::new(), Vec::new()).await;
    let session = &test.session;
    let captured = capture(session);
    let mut raw = serde_json::to_value(event("agent_end_overwrite")).unwrap();
    raw["messages"][0]["stopReason"] = Value::from("error");
    raw["messages"][0]["errorMessage"] = Value::from("429 rate limit exceeded");
    for (attempt, expected) in [
        (0, true),
        (session.retry_settings().max_retries as u32, false),
    ] {
        session.retry_attempt.store(attempt, Ordering::SeqCst);
        captured.lock().unwrap().clear();
        session
            .handle_agent_event(serde_json::from_value(raw.clone()).unwrap())
            .await;
        let values = captured.lock().unwrap();
        let out = values
            .iter()
            .find(|(ty, _)| ty == "agent_end")
            .unwrap()
            .1
            .as_ref()
            .unwrap();
        assert!(out.starts_with(&format!("{{\"willRetry\":{expected},\"messages\":")));
        let output: Value = serde_json::from_str(out).unwrap();
        assert_eq!(output["extraFirst"], raw["extraFirst"]);
        assert_eq!(output["messages"], raw["messages"]);
    }
}

#[tokio::test]
async fn json_ingress_extension_message_replacement_updates_wire_state_and_persistence() {
    let row = oracle()["replacements"][0].clone();
    let replacement: Value = serde_json::from_str(row["replacement"].as_str().unwrap()).unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_factory = seen.clone();
    let replacement_for_handler = replacement.clone();
    let factory: ExtensionFactory = Arc::new(move |api: &ExtensionApi| {
        let seen = seen_factory.clone();
        let replacement = replacement_for_handler.clone();
        api.on(
            "message_end",
            crate::coding_agent::extensions::types::sync_handler(
                move |event: &mut Value, _ctx: &ExtensionContext| {
                    seen.lock().unwrap().push(event.clone());
                    Ok(Some(HandlerResult::Json(json!({"message":replacement}))))
                },
            ),
        )?;
        Ok(())
    });
    let test = create_test_session(vec![factory], Vec::new()).await;
    let session = &test.session;
    let captured = capture(session);
    let event: AgentEvent = serde_json::from_str(row["input"].as_str().unwrap()).unwrap();
    if let AgentEvent::MessageEnd { message } = event.kind() {
        session.agent.state().messages.push(message.clone());
    }
    session.handle_agent_event(event).await;
    let values = captured.lock().unwrap();
    assert_eq!(
        values
            .iter()
            .find(|(ty, _)| ty == "message_end")
            .unwrap()
            .1
            .as_ref()
            .unwrap(),
        row["out"].as_str().unwrap()
    );
    let original: Value = serde_json::from_str(row["input"].as_str().unwrap()).unwrap();
    assert_eq!(
        serde_json::to_string(&seen.lock().unwrap()[0]["message"]).unwrap(),
        serde_json::to_string(&original["message"]).unwrap()
    );
    let expected: AgentMessage = serde_json::from_value(replacement).unwrap();
    assert_eq!(session.agent.state().messages.last(), Some(&expected));
    let entries = session.session_manager.lock().unwrap().get_entries();
    let persisted = entries
        .iter()
        .find_map(|entry| match entry {
            SessionEntry::Message(entry) => Some(&entry.message),
            _ => None,
        })
        .unwrap();
    assert_eq!(persisted, &expected);
}

#[tokio::test]
async fn json_ingress_extensions_receive_original_nested_fields_and_partial() {
    let seen = Arc::new(Mutex::new(Vec::<Value>::new()));
    let slot = seen.clone();
    let factory: ExtensionFactory = Arc::new(move |api: &ExtensionApi| {
        for kind in [
            "agent_start",
            "agent_end",
            "turn_end",
            "message_start",
            "message_update",
            "tool_execution_start",
            "tool_execution_update",
            "tool_execution_end",
        ] {
            let seen = slot.clone();
            api.on(
                kind,
                crate::coding_agent::extensions::types::sync_handler(
                    move |event: &mut Value, _ctx: &ExtensionContext| {
                        seen.lock().unwrap().push(event.clone());
                        Ok(None)
                    },
                ),
            )?;
        }
        Ok(())
    });
    let test = create_test_session(vec![factory], Vec::new()).await;
    test.session.turn_index.store(5, Ordering::SeqCst);
    test.session.handle_agent_event(event("agent_start")).await;
    assert_eq!(test.session.turn_index.load(Ordering::SeqCst), 0);
    for name in [
        "agent_end_append",
        "turn_end",
        "message_start_user",
        "text_delta",
        "start_message_is_extension",
        "done_nested",
        "error_nested",
        "tool_start",
        "tool_update",
        "tool_end",
    ] {
        seen.lock().unwrap().clear();
        let event = event(name);
        let expected = serde_json::to_value(&event).unwrap();
        test.session.handle_agent_event(event).await;
        let observed = seen.lock().unwrap();
        let extension = observed
            .iter()
            .find(|event| event["type"] == expected["type"])
            .unwrap();
        for field in [
            "message",
            "messages",
            "toolResults",
            "assistantMessageEvent",
            "args",
            "partialResult",
            "result",
        ] {
            if let Some(expected) = expected.get(field) {
                assert_eq!(
                    serde_json::to_string(&extension[field]).unwrap(),
                    serde_json::to_string(expected).unwrap(),
                    "{name}.{field}"
                );
            }
        }
        assert!(
            extension.get("extraFirst").is_none(),
            "do not forward extra outer fields to extensions"
        );
    }
}

#[tokio::test]
async fn json_ingress_native_message_replacement_normalizes_content_without_reordering() {
    let replacement_slot = Arc::new(Mutex::new(Value::Null));
    let slot = replacement_slot.clone();
    let factory: ExtensionFactory = Arc::new(move |api: &ExtensionApi| {
        let slot = slot.clone();
        api.on(
            "message_end",
            crate::coding_agent::extensions::types::sync_handler(
                move |_event: &mut Value, _ctx: &ExtensionContext| {
                    Ok(Some(HandlerResult::Json(
                        json!({"message":slot.lock().unwrap().clone()}),
                    )))
                },
            ),
        )?;
        Ok(())
    });
    let test = create_test_session(vec![factory], Vec::new()).await;
    let captured = capture(&test.session);
    for (input, expected) in [
        (
            r#"{"tail":1,"role":"user","content":null,"timestamp":1,"newUnknown":true}"#,
            r#"{"type":"message_end","message":{"tail":1,"role":"user","content":[],"timestamp":1,"newUnknown":true}}"#,
        ),
        (
            r#"{"tail":1,"role":"user","timestamp":1,"newUnknown":true}"#,
            r#"{"type":"message_end","message":{"tail":1,"role":"user","timestamp":1,"newUnknown":true,"content":[]}}"#,
        ),
    ] {
        *replacement_slot.lock().unwrap() = serde_json::from_str(input).unwrap();
        let native = event("message_end_user").kind().clone();
        assert!(matches!(native, AgentEvent::MessageEnd { .. }));
        if let AgentEvent::MessageEnd { message } = &native {
            test.session.agent.state().messages.push(message.clone());
        }
        captured.lock().unwrap().clear();
        test.session.handle_agent_event(native).await;
        let values = captured.lock().unwrap();
        assert_eq!(
            values
                .iter()
                .find(|(ty, _)| ty == "message_end")
                .unwrap()
                .1
                .as_ref()
                .unwrap(),
            expected
        );
        let replacement =
            serde_json::to_value(test.session.agent.state().messages.last().unwrap()).unwrap();
        assert_eq!(replacement["content"], json!([]));
    }
}

#[tokio::test]
async fn json_ingress_invalid_or_wrong_role_replacements_leave_original_wire_intact() {
    let replacement_slot = Arc::new(Mutex::new(Value::Null));
    let slot = replacement_slot.clone();
    let factory: ExtensionFactory = Arc::new(move |api: &ExtensionApi| {
        let slot = slot.clone();
        api.on(
            "message_end",
            crate::coding_agent::extensions::types::sync_handler(
                move |_event: &mut Value, _ctx: &ExtensionContext| {
                    Ok(Some(HandlerResult::Json(
                        json!({"message":slot.lock().unwrap().clone()}),
                    )))
                },
            ),
        )?;
        Ok(())
    });
    let test = create_test_session(vec![factory], Vec::new()).await;
    let captured = capture(&test.session);
    for replacement in [
        json!({"role":"notification","text":"wrong role"}),
        json!({"role":"user","content":55,"timestamp":1}),
    ] {
        *replacement_slot.lock().unwrap() = replacement;
        captured.lock().unwrap().clear();
        test.session
            .handle_agent_event(event("message_end_user"))
            .await;
        let values = captured.lock().unwrap();
        assert_eq!(
            values
                .iter()
                .find(|(ty, _)| ty == "message_end")
                .unwrap()
                .1
                .as_ref()
                .unwrap(),
            row("message_end_user")["projections"][0]["out"]
                .as_str()
                .unwrap()
        );
    }
}
