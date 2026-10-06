use super::*;
use serde_json::Value;

fn oracle() -> Value {
    serde_json::from_str(include_str!("jsonl_oracle.json")).unwrap()
}
fn units(value: &Value) -> Vec<u16> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|unit| unit.as_u64().unwrap() as u16)
        .collect()
}

#[test]
fn every_chunk_boundary_and_detach_matches_actual_upstream_jsonl() {
    let oracle = oracle();
    assert_eq!(oracle["cases"].as_array().unwrap().len(), 863);
    for case in oracle["cases"].as_array().unwrap() {
        let mut reader = JsonlLineReader::new();
        for (step, action) in case["actions"].as_array().unwrap().iter().enumerate() {
            let lines = if let Some(data) = action.get("bytes") {
                reader.feed_bytes(
                    &data
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|b| b.as_u64().unwrap() as u8)
                        .collect::<Vec<_>>(),
                )
            } else if let Some(text) = action.get("text") {
                reader.feed_utf16(&Utf16Text::from_units(units(text)))
            } else if action.get("detach").is_some() {
                reader.detach();
                Vec::new()
            } else {
                assert_eq!(action["end"], true);
                reader.finish()
            };
            let expected: Vec<_> = case["trace"][step]["lines"]
                .as_array()
                .unwrap()
                .iter()
                .map(units)
                .collect();
            assert_eq!(
                lines
                    .into_iter()
                    .map(Utf16Text::into_units)
                    .collect::<Vec<_>>(),
                expected,
                "{} step {step}",
                case["id"]
            );
            let count = if reader.is_attached() { 1 } else { 0 };
            assert_eq!(
                case["trace"][step]["listeners"],
                serde_json::json!([count, count]),
                "{} step {step}: detach",
                case["id"]
            );
        }
    }
}

#[test]
fn serializer_matches_actual_upstream_numbers_unicode_and_property_order() {
    let oracle = oracle();
    for case in oracle["serialization"].as_array().unwrap() {
        let value: Value = serde_json::from_str(case["input"].as_str().unwrap()).unwrap();
        assert_eq!(
            serialize_json_line(&value).unwrap(),
            case["line"].as_str().unwrap(),
            "{}",
            case["input"]
        );
    }
}

#[test]
fn byte_input_retains_malformed_incomplete_sequences_until_the_next_byte_or_eof() {
    let mut reader = JsonlLineReader::new();
    assert!(reader.feed_bytes(&[0xed, 0xa0]).is_empty());
    assert_eq!(reader.feed_text("text\n"), vec![Utf16Text::from("text")]);
    assert_eq!(reader.finish(), vec![Utf16Text::from("��")]);
    assert!(reader.finish().is_empty());
    assert_eq!(
        reader.feed_bytes(b"\r\r\n\n"),
        vec![Utf16Text::from("\r"), Utf16Text::new()]
    );
}

#[test]
fn serialize_preserves_null_and_rounds_js_numbers_without_changing_numeric_strings() {
    assert_eq!(serialize_json_line(&serde_json::json!({"value":null,"number":9007199254740993_u64,"string":"9007199254740993"})).unwrap(),"{\"value\":null,\"number\":9007199254740992,\"string\":\"9007199254740993\"}\n");
    assert_eq!(
        serialize_json_line(&[f64::NAN, f64::INFINITY, -0.0]).unwrap(),
        "[null,null,0]\n"
    );
}
