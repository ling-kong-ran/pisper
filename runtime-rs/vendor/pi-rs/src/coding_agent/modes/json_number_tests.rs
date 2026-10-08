//! Actual-source Number contract, including the real AgentEvent/session bridge.
//! The oracle's rawOverflowAudit is an explicit remaining ingress limitation,
//! not a passing fixture: serde_json::Value cannot represent Infinity.
use super::*;
use crate::agent_core::types::AgentEvent;
use crate::coding_agent::core::model_config::OrderedValue;

fn number_oracle() -> &'static Value {
    static ORACLE: OnceLock<Value> = OnceLock::new();
    ORACLE.get_or_init(|| serde_json::from_str(include_str!("json_number_oracle.json")).unwrap())
}

#[derive(Default)]
struct Differences {
    count: usize,
    examples: Vec<String>,
}

impl Differences {
    fn check(&mut self, label: &str, actual: &str, expected: &str) {
        if actual != expected {
            self.count += 1;
            if self.examples.len() < 8 {
                self.examples
                    .push(format!("{label}\nactual: {actual}\nexpected: {expected}"));
            }
        }
    }
    fn finish(self) {
        assert_eq!(
            self.count,
            0,
            "{} mismatches; first examples:\n{}",
            self.count,
            self.examples.join("\n\n")
        );
    }
}

fn tool_event(value: f64, label: &str) -> AgentSessionEvent {
    let raw = serde_json::json!({"type":"tool_execution_start","toolCallId":"number-probe","toolName":"test",
        "args":{"n":value,"nested":[value,{"n":value}],"text":label}});
    let event: AgentEvent = serde_json::from_value(raw).unwrap();
    AgentSessionEvent::from(&event)
}

#[test]
fn json_number_raw_literals_cross_the_actual_agent_session_bridge() {
    let raw = number_oracle()["raw"].as_array().unwrap();
    assert_eq!(raw.len(), 61);
    let mut differences = Differences::default();
    for case in raw {
        let input = case["input"].as_str().unwrap();
        let label = case["literal"].as_str().unwrap();
        let actual = serde_json::from_str::<AgentEvent>(input)
            .map_err(|error| error.to_string())
            .and_then(|event| to_json_event_string(&AgentSessionEvent::from(&event)))
            .unwrap_or_else(|error| format!("ERROR: {error}"));
        differences.check(label, &actual, case["out"].as_str().unwrap());
    }
    differences.finish();
}

#[test]
fn json_number_ieee_patterns_cross_tool_and_typed_usage_paths() {
    let rows = number_oracle()["bits"].as_array().unwrap();
    assert_eq!(rows.len(), 6378);
    assert_eq!(rows.iter().filter(|row| row["finite"] == false).count(), 8);
    let mut differences = Differences::default();
    for row in rows {
        let bits = row["bits"].as_str().unwrap();
        let value = f64::from_bits(u64::from_str_radix(bits, 16).unwrap());
        let event = tool_event(value, row["label"].as_str().unwrap());
        let before = serde_json::to_string(&event).unwrap();
        differences.check(
            &format!("tool {bits}"),
            &to_json_event_string(&event).unwrap(),
            row["toolOut"].as_str().unwrap(),
        );
        assert_eq!(serde_json::to_string(&event).unwrap(), before);
        // This is a typed f64 path, so NaN and infinities reach the actual
        // serde/session projection rather than being preconverted by a fixture.
        let mut usage = usage();
        usage.cost.input = value;
        usage.cost.output = value;
        usage.cost.total = value;
        let event = AgentSessionEvent::MessageUpdate {
            message: assistant_agent_message(usage),
            assistant_message_event: serde_json::json!({"type":"text_delta","contentIndex":0,"delta":"numbers"}),
        };
        differences.check(
            &format!("usage {bits}"),
            &to_json_event_string(&event).unwrap(),
            row["usageOut"].as_str().unwrap(),
        );
    }
    differences.finish();
}

#[test]
fn json_number_ordered_projection_matches_raw_numeric_inputs() {
    let mut differences = Differences::default();
    for case in number_oracle()["raw"].as_array().unwrap() {
        let actual = serde_json::from_str::<OrderedValue>(case["input"].as_str().unwrap())
            .map_err(|error| error.to_string())
            .and_then(|event| super::super::json_event::project_ordered_event(&event))
            .map(|value| value.to_json_string())
            .unwrap_or_else(|error| format!("ERROR: {error}"));
        differences.check(
            case["literal"].as_str().unwrap(),
            &actual,
            case["out"].as_str().unwrap(),
        );
    }
    differences.finish();
}

#[test]
fn json_number_decimal_parse_recovers_v8_ieee_bits() {
    let mut differences = Differences::default();
    for row in number_oracle()["bits"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["finite"] == true)
    {
        let input = row["json"].as_str().unwrap();
        let actual = serde_json::from_str::<Value>(input)
            .map_err(|error| error.to_string())
            .and_then(|value| value.as_f64().ok_or_else(|| "not numeric".into()))
            .map(|value| format!("{:016x}", value.to_bits()))
            .unwrap_or_else(|error| format!("ERROR: {error}"));
        differences.check(input, &actual, row["parsedBits"].as_str().unwrap());
    }
    for row in number_oracle()["raw"].as_array().unwrap() {
        let input = row["literal"].as_str().unwrap();
        let actual = serde_json::from_str::<Value>(input)
            .map_err(|error| error.to_string())
            .and_then(|value| value.as_f64().ok_or_else(|| "not numeric".into()))
            .map(|value| format!("{:016x}", value.to_bits()))
            .unwrap_or_else(|error| format!("ERROR: {error}"));
        differences.check(input, &actual, row["parsedBits"].as_str().unwrap());
    }
    differences.finish();
}

#[test]
fn json_number_tool_index_error_labels_follow_js_number_strings() {
    let rows = number_oracle()["errors"].as_array().unwrap();
    assert_eq!(rows.len(), 14);
    let mut differences = Differences::default();
    for case in rows {
        let input = case["input"].as_str().unwrap();
        let raw: Value = serde_json::from_str(input).unwrap();
        let event = AgentSessionEvent::MessageUpdate {
            message: serde_json::from_value(raw["message"].clone()).unwrap(),
            assistant_message_event: raw["assistantMessageEvent"].clone(),
        };
        let expected = case["error"].as_str().unwrap();
        let label = case["literal"].as_str().unwrap();
        differences.check(label, &to_json_event_string(&event).unwrap_err(), expected);
        let ordered: OrderedValue = serde_json::from_str(input).unwrap();
        differences.check(
            &format!("ordered {label}"),
            &super::super::json_event::project_ordered_event(&ordered).unwrap_err(),
            expected,
        );
    }
    differences.finish();
}

#[test]
fn json_number_formatter_labels_match_all_oracle_bits() {
    let mut differences = Differences::default();
    for row in number_oracle()["bits"].as_array().unwrap() {
        let bits = row["bits"].as_str().unwrap();
        let value = f64::from_bits(u64::from_str_radix(bits, 16).unwrap());
        differences.check(
            bits,
            &crate::serde_support::js_number_string(value),
            row["label"].as_str().unwrap(),
        );
    }
    differences.finish();
}
