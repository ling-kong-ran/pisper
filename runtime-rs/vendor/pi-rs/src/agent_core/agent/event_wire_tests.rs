use super::*;
use serde_json::Value;

fn event(name: &str) -> AgentEvent {
    let oracle: Value = serde_json::from_str(include_str!(
        "../../coding_agent/modes/json_ingress_oracle.json"
    ))
    .unwrap();
    let row = oracle["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == name)
        .unwrap();
    serde_json::from_str(row["input"].as_str().unwrap()).unwrap()
}

#[test]
fn json_ingress_agent_reducer_observes_stream_messages_tools_and_agent_end() {
    let state = Mutex::new(AgentState::default());
    for name in ["message_start_user", "text_delta"] {
        let event = event(name);
        let before = serde_json::to_string(&event).unwrap();
        reduce_state(&state, &event);
        let expected = match event.kind() {
            AgentEvent::MessageStart { message } | AgentEvent::MessageUpdate { message, .. } => {
                message
            }
            _ => panic!("fixture is a message"),
        };
        assert_eq!(
            state.lock().unwrap().streaming_message.as_ref(),
            Some(expected)
        );
        assert_eq!(serde_json::to_string(&event).unwrap(), before);
    }
    reduce_state(&state, &event("tool_start"));
    assert!(state
        .lock()
        .unwrap()
        .pending_tool_calls
        .contains("call-own"));
    reduce_state(&state, &event("tool_end"));
    assert!(state.lock().unwrap().pending_tool_calls.is_empty());
    reduce_state(&state, &event("message_end_user"));
    assert!(state.lock().unwrap().streaming_message.is_none());
    assert_eq!(state.lock().unwrap().messages.len(), 1);
    reduce_state(&state, &event("message_start_assistant"));
    reduce_state(&state, &event("agent_end_append"));
    assert!(state.lock().unwrap().streaming_message.is_none());
}

#[test]
fn json_ingress_agent_reducer_reads_turn_end_errors_and_passive_events() {
    let state = Mutex::new(AgentState::default());
    let mut raw = serde_json::to_value(event("turn_end")).unwrap();
    raw["message"]["errorMessage"] = Value::from("a turn failure");
    raw["message"]["stopReason"] = Value::from("error");
    reduce_state(&state, &serde_json::from_value(raw).unwrap());
    assert_eq!(
        state.lock().unwrap().error_message.as_deref(),
        Some("a turn failure")
    );
    for name in ["agent_start", "turn_start", "tool_update"] {
        reduce_state(&state, &event(name));
    }
    assert_eq!(
        state.lock().unwrap().error_message.as_deref(),
        Some("a turn failure")
    );
}
