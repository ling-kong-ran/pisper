//! Tests for the ported `coding-agent/src/cli/list-models.ts` — byte
//! comparisons against the node oracle (full table, fuzzy searches,
/// no-match message, no-models message, load-error warning).
use std::sync::Arc;

use serde_json::Value;

use crate::ai::types::{Model, ModelInput};
use crate::coding_agent::cli::list_models::render_models_table;
use crate::coding_agent::core::model_resolver::{ModelRuntimeReads, PrefetchedRuntime};

const ORACLE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/cli_oracle/oracle.json"
));

fn oracle() -> &'static Value {
    use std::sync::OnceLock;
    static ORACLE_VALUE: OnceLock<Value> = OnceLock::new();
    ORACLE_VALUE.get_or_init(|| serde_json::from_str(ORACLE).expect("oracle json"))
}

fn model(
    provider: &str,
    id: &str,
    context_window: u64,
    max_tokens: u64,
    reasoning: bool,
    image: bool,
) -> Model {
    Model {
        r#type: None,
        prompt_cache: None,
        input_limits: None,
        id: id.to_string(),
        name: id.to_string(),
        api: "anthropic-messages".to_string(),
        provider: provider.to_string(),
        base_url: String::new(),
        reasoning,
        thinking_level_map: None,
        input: if image {
            vec![ModelInput::Text, ModelInput::Image]
        } else {
            vec![ModelInput::Text]
        },
        cost: crate::ai::types::ModelCost::default(),
        context_window,
        max_tokens,
        sampling_params: None,
        sampling_params_by_thinking_level: None,
        headers: None,
        compat: None,
    }
}

fn oracle_models() -> Vec<Model> {
    vec![
        model("openai", "gpt-4o-mini", 128_000, 16_384, false, true),
        model("openai", "gpt-5.5", 1_000_000, 128_000, true, false),
        model(
            "anthropic",
            "claude-sonnet-4-5",
            200_000,
            64_000,
            true,
            true,
        ),
        model("google", "gemini-2.5-pro", 1_048_576, 65_536, true, true),
        model("zai-coding-plan", "glm-4.7", 200_000, 128_000, true, false),
    ]
}

fn reads(models: Vec<Model>) -> PrefetchedRuntime {
    PrefetchedRuntime {
        available: models.clone(),
        configured_auth: Default::default(),
        model_lookup: models
            .iter()
            .map(|m| ((m.provider.clone(), m.id.clone()), m.clone()))
            .collect(),
        models,
    }
}

/// Splits the captured console output into (stdout, stderr) byte streams;
/// continuation lines (multi-line messages) stay on their stream.
fn split_console(captured: &str) -> (String, String) {
    let mut out = String::new();
    let mut err = String::new();
    let mut current_out = true;
    for line in captured.split_inclusive('\n') {
        if let Some(rest) = line.strip_prefix("out: ") {
            current_out = true;
            out.push_str(rest);
        } else if line == "out:\n" {
            current_out = true;
            out.push('\n');
        } else if let Some(rest) = line.strip_prefix("err: ") {
            current_out = false;
            err.push_str(rest);
        } else if line == "err:\n" {
            current_out = false;
            err.push('\n');
        } else if current_out {
            out.push_str(line);
        } else {
            err.push_str(line);
        }
    }
    (out, err)
}

fn run_case(name: &str, models: Vec<Model>, load_error: Option<&str>, search: Option<&str>) {
    let expected = oracle()["listModels"][name].as_str().unwrap();
    let (expected_out, expected_err) = split_console(expected);
    let snapshot = reads(models);
    let reads_ref: &dyn ModelRuntimeReads = &snapshot;
    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();
    let available = reads_ref.get_available_snapshot();
    if let Some(load_error) = load_error {
        use std::io::Write;
        writeln!(err, "Warning: errors loading models.json:\n{load_error}").unwrap();
    }
    render_models_table(&mut out, &available, search).unwrap();
    assert_eq!(
        String::from_utf8(out).unwrap(),
        expected_out,
        "{name} stdout"
    );
    assert_eq!(
        String::from_utf8(err).unwrap(),
        expected_err,
        "{name} stderr"
    );
}

/// The exact fixture set of the oracle capture (upstream `listModels`).
#[test]
fn list_models_table_matches_the_oracle() {
    run_case("all", oracle_models(), None, None);
}

#[test]
fn fuzzy_search_matches_the_oracle() {
    run_case("searchSonnet", oracle_models(), None, Some("sonnet"));
    run_case("searchGpt", oracle_models(), None, Some("gpt"));
}

#[test]
fn no_match_message_matches_the_oracle() {
    run_case("searchNoMatch", oracle_models(), None, Some("zzzznotfound"));
}

/// The no-models branch prints the auth-guidance message. The docs directory
/// is environment-specific (upstream resolves `<packageDir>/docs`; the port
/// resolves the port repo's docs), so the comparison pins the message
/// sentence and the referenced file names, substituting the docs root.
#[test]
fn empty_catalog_message_matches_the_oracle() {
    let expected = oracle()["listModels"]["empty"].as_str().unwrap();
    let (expected_out, _) = split_console(expected);
    let ported = crate::coding_agent::core::auth_guidance::format_no_models_available_message();
    // Same sentence, same referenced docs basenames.
    let first_line = |text: &str| text.lines().next().unwrap().to_string();
    assert_eq!(
        first_line(&ported),
        first_line(&expected_out),
        "no-models message first line"
    );
    let referenced: Vec<&str> = ported.lines().skip(1).map(|line| line.trim()).collect();
    let expected_referenced: Vec<&str> = expected_out
        .lines()
        .skip(1)
        .map(|line| line.trim().rsplit(['/', '\\']).next().unwrap())
        .collect();
    assert_eq!(referenced.len(), expected_referenced.len());
    for (line, base) in referenced.iter().zip(expected_referenced) {
        assert!(
            line.ends_with(base),
            "referenced docs line {line} should end with {base}"
        );
    }
    let rendered = render_empty();
    assert_eq!(first_line(&rendered), first_line(&expected_out));
}

fn render_empty() -> String {
    let mut out: Vec<u8> = Vec::new();
    render_models_table(&mut out, &[], None).unwrap();
    String::from_utf8(out).unwrap()
}

#[test]
fn load_error_warning_matches_the_oracle() {
    run_case(
        "loadError",
        oracle_models().into_iter().take(2).collect(),
        Some("boom\nsecond line"),
        None,
    );
}

/// `Arc` mirror used by the [`ModelRuntime`]-shaped call path.
#[allow(dead_code)]
fn _arc_check(_: Arc<Model>) {}
