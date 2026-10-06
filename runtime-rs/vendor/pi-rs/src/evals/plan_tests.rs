//! Ports of `pi/packages/evals/test/plan.test.ts` plus byte-level oracle
//! comparisons against the upstream module executed with node
//! (`tests/fixtures/evals_m6/oracle/plan.json`).

use super::{create_task_plan, parse_discovered_cases, DocumentationVariant};
use crate::serde_support::to_json_string_with_js_numbers;

fn discovered() -> serde_json::Value {
    serde_json::json!([
        { "name": "Add model > adds the model", "file": "evals/models.docs.eval.ts" }
    ])
}

#[test]
fn derives_stable_case_identity_from_ordinary_vitest_names() {
    let cases = parse_discovered_cases(&discovered()).unwrap();
    assert_eq!(
        serde_json::to_value(&cases).unwrap(),
        serde_json::json!([
            {
                "file": "evals/models.docs.eval.ts",
                "fullName": "Add model > adds the model",
                "evalSet": "Add model",
                "caseId": "adds the model",
            }
        ])
    );
}

#[test]
fn rejects_ambiguous_and_duplicate_identities() {
    let error = parse_discovered_cases(&serde_json::json!([
        { "name": "adds the model", "file": "model.ts" }
    ]))
    .unwrap_err();
    assert!(error.contains("<eval set> > <case>"), "{error}");

    let doubled = serde_json::json!([
        { "name": "Add model > adds the model", "file": "evals/models.docs.eval.ts" },
        { "name": "Add model > adds the model", "file": "evals/models.docs.eval.ts" },
    ]);
    let error = parse_discovered_cases(&doubled).unwrap_err();
    assert!(error.contains("Duplicate eval case identity"), "{error}");
}

#[test]
fn creates_one_isolated_task_per_case_variant_model_and_repetition() {
    let cases = parse_discovered_cases(&discovered()).unwrap();
    let tasks = create_task_plan(&cases, "fixture/model", 2).unwrap();
    assert_eq!(tasks.len(), 4);
    let pairs: Vec<(&str, u32)> = tasks
        .iter()
        .map(|task| (task.variant.as_str(), task.run_number))
        .collect();
    assert_eq!(
        pairs,
        vec![
            ("without_docs", 1),
            ("with_docs", 1),
            ("with_docs", 2),
            ("without_docs", 2),
        ]
    );
}

#[test]
fn rejects_invalid_model_identities_and_repetitions() {
    let cases = parse_discovered_cases(&discovered()).unwrap();
    let error = create_task_plan(&cases, "model", 1).unwrap_err();
    assert!(error.contains("provider and model"), "{error}");
    let error = create_task_plan(&cases, "fixture/model", 0).unwrap_err();
    assert!(error.contains("positive integer"), "{error}");
    let error = create_task_plan(&cases, "/model", 1).unwrap_err();
    assert!(error.contains("provider and model"), "{error}");
    let error = create_task_plan(&cases, "model/", 1).unwrap_err();
    assert!(error.contains("provider and model"), "{error}");
}

fn oracle() -> serde_json::Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/evals_m6/oracle/plan.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).expect("plan oracle"))
        .expect("plan oracle JSON")
}

/// Serializes like the port and compares byte-for-byte against the compact
/// form of the upstream node output.
fn assert_oracle_bytes(key: &str, value: &impl serde::Serialize) {
    let oracle = oracle();
    let expected = oracle.get(key).expect("oracle key").to_string();
    let actual = to_json_string_with_js_numbers(value).expect("serialize");
    assert_eq!(actual, expected, "oracle mismatch for {key}");
}

#[test]
fn oracle_parse_discovered_cases_and_errors_match_upstream() {
    let oracle = oracle();
    assert_oracle_bytes("parseOk", &parse_discovered_cases(&discovered()).unwrap());
    for key in ["err1", "err2", "err3", "err4", "err5"] {
        let expected = oracle.get(key).unwrap().as_str().unwrap();
        let actual = match key {
            "err1" => parse_discovered_cases(&serde_json::json!([
                { "name": "adds the model", "file": "model.ts" }
            ]))
            .unwrap_err(),
            "err2" => parse_discovered_cases(&serde_json::json!([
                { "name": "Add model > adds the model", "file": "f.ts" },
                { "name": "Add model > adds the model", "file": "f.ts" },
            ]))
            .unwrap_err(),
            "err3" => parse_discovered_cases(&serde_json::json!([{}])).unwrap_err(),
            "err4" => parse_discovered_cases(&serde_json::json!("nope")).unwrap_err(),
            _ => parse_discovered_cases(&serde_json::json!([
                { "name": "A > B > C", "file": "f.ts" }
            ]))
            .unwrap_err(),
        };
        assert_eq!(actual, expected, "oracle mismatch for {key}");
    }
    let variants: Vec<&'static str> = super::DOCUMENTATION_VARIANTS
        .iter()
        .map(|variant| variant.as_str())
        .collect();
    assert_eq!(
        serde_json::to_value(variants).unwrap(),
        oracle.get("variants").cloned().unwrap()
    );
}

#[test]
fn oracle_task_plans_match_upstream_byte_for_byte() {
    let cases_a = parse_discovered_cases(&discovered()).unwrap();
    assert_oracle_bytes(
        "plan2",
        &create_task_plan(&cases_a, "fixture/model", 2).unwrap(),
    );
    let cases_b = parse_discovered_cases(&serde_json::json!([
        { "name": "A > one", "file": "a.ts" },
        { "name": "B > two", "file": "b.ts" },
    ]))
    .unwrap();
    assert_oracle_bytes("plan3", &create_task_plan(&cases_b, "p/m", 3).unwrap());
    let oracle = oracle();
    for (key, value) in [
        ("err6", serde_json::json!("model")),
        ("err7", serde_json::json!("fixture/model")),
    ] {
        let expected = oracle.get(key).unwrap().as_str().unwrap();
        let runs = if key == "err7" { 0 } else { 1 };
        let actual = create_task_plan(&cases_a, value.as_str().unwrap(), runs).unwrap_err();
        assert_eq!(actual, expected, "oracle mismatch for {key}");
    }
    assert_eq!(DocumentationVariant::WithoutDocs.as_str(), "without_docs");
}
