//! r9 oracle covers the actual typed ingress and bridge, not OrderedValue.
use crate::agent_core::types::AgentEvent;
use crate::coding_agent::agent_session::AgentSessionEvent;
use crate::coding_agent::modes::json_event::to_json_event_string;
use serde_json::Value;

fn check_group(group: &str, count: usize) {
    let oracle: Value = serde_json::from_str(include_str!("json_ingress_oracle.json")).unwrap();
    let rows: Vec<_> = oracle["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["group"] == group)
        .collect();
    assert_eq!(rows.len(), count);
    let mut differences = Vec::new();
    for row in rows {
        let name = row["name"].as_str().unwrap();
        let input = row["input"].as_str().unwrap();
        let expected = &row["projections"][0];
        let actual = serde_json::from_str::<AgentEvent>(input)
            .map_err(|e| e.to_string())
            .and_then(|event| to_json_event_string(&AgentSessionEvent::from(&event)));
        let expected = if expected["ok"] == true {
            Ok(expected["out"].as_str().unwrap().to_owned())
        } else {
            Err(expected["error"].as_str().unwrap().to_owned())
        };
        if actual != expected {
            differences.push(format!("{name}: actual={actual:?}\nexpected={expected:?}"));
        }
    }
    assert!(
        differences.is_empty(),
        "{} differences:\n{}",
        differences.len(),
        differences.join("\n")
    );
}
#[test]
fn json_ingress_ordinary_fields_and_order_cross_the_typed_bridge() {
    check_group("ordinary", 18);
}
#[test]
fn json_ingress_stream_extras_partial_usage_and_terminal_fields_survive() {
    check_group("updates", 11);
}
#[test]
fn json_ingress_upstream_start_partial_is_accepted_without_losing_extra_message() {
    check_group("start", 2);
}
#[test]
fn json_ingress_rejects_the_same_invalid_message_and_tool_partial() {
    check_group("errors", 2);
}
