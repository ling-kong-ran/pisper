use super::*;

fn oracle() -> Value {
    serde_json::from_str(include_str!(
        "../../coding_agent/modes/json_ingress_oracle.json"
    ))
    .unwrap()
}

#[test]
fn json_ingress_agent_event_roundtrip_keeps_all_original_fields_and_order() {
    for case in oracle()["cases"].as_array().unwrap() {
        let raw: Value = serde_json::from_str(case["input"].as_str().unwrap()).unwrap();
        let event: AgentEvent = serde_json::from_value(raw.clone()).unwrap();
        assert!(!matches!(event.kind(), AgentEvent::Preserved(_)));
        assert_eq!(
            serde_json::to_string(&event).unwrap(),
            serde_json::to_string(&raw).unwrap(),
            "{}",
            case["name"]
        );
        let second: AgentEvent =
            serde_json::from_value(serde_json::to_value(&event).unwrap()).unwrap();
        assert_eq!(event, second);
    }
}

#[test]
fn json_ingress_preservation_does_not_bypass_typed_validation() {
    for input in [
        r#"{"type":"vendor_event"}"#,
        r#"{"type":"message_end","message":{}}"#,
        r#"{"type":"tool_execution_end","toolCallId":"id"}"#,
        r#"{"type":"agent_end","messages":"bad"}"#,
    ] {
        assert!(
            serde_json::from_str::<AgentEvent>(input).is_err(),
            "{input}"
        );
    }
}

#[test]
fn json_ingress_message_replacement_is_atomic_and_cannot_keep_stale_fields() {
    for row in oracle()["replacements"].as_array().unwrap() {
        let mut event: AgentEvent = serde_json::from_str(row["input"].as_str().unwrap()).unwrap();
        let before = serde_json::to_string(&event).unwrap();
        assert!(event
            .replace_message_end(serde_json::json!({"role":"user","content":55}))
            .is_err());
        assert_eq!(serde_json::to_string(&event).unwrap(), before);
        let replacement: Value =
            serde_json::from_str(row["replacement"].as_str().unwrap()).unwrap();
        let message = event.replace_message_end(replacement.clone()).unwrap();
        assert!(
            matches!(event.kind(), AgentEvent::MessageEnd { message: typed } if typed == &message)
        );
        let wire = serde_json::to_value(&event).unwrap();
        assert_eq!(
            serde_json::to_string(&wire["message"]).unwrap(),
            serde_json::to_string(&replacement).unwrap()
        );
        assert!(wire["message"].get("vendor").is_none());
        let actual =
            crate::coding_agent::modes::json_event::to_json_event_string(&(&event).into()).unwrap();
        assert_eq!(actual, row["out"].as_str().unwrap());
    }
    let mut wrong_event: AgentEvent =
        serde_json::from_str(r#"{"extra":1,"type":"agent_start"}"#).unwrap();
    let before = serde_json::to_string(&wrong_event).unwrap();
    assert!(wrong_event
        .replace_message_end(serde_json::json!({"role":"notification"}))
        .is_err());
    assert_eq!(serde_json::to_string(&wrong_event).unwrap(), before);
}
