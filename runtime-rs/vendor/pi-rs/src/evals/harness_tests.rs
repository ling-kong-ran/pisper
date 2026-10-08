//! Ports of `pi/packages/evals/test/harness.test.ts`, plus oracle
//! comparisons against the upstream pure helpers executed with node
//! (`tests/fixtures/evals_m6/oracle/harness.json`).

use super::{
    apply_isolated_environment, create_pi_documentation_eval_harness, exclude_pi_documentation,
    resolve_documentation_variant, resolve_model_selection, verify_system_prompt, HarnessOptions,
    ModelSelection, DOCUMENTATION_EVAL_TOOLS,
};
use crate::coding_agent::agent_session::system_prompt::build_system_prompt;
use crate::coding_agent::extensions::types::BuildSystemPromptOptions;
use std::collections::BTreeMap;

fn environment(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

#[test]
fn prefers_an_explicit_harness_model() {
    let selection = resolve_model_selection(
        Some(ModelSelection {
            provider: "anthropic".to_string(),
            id: "claude-opus-4-6".to_string(),
        }),
        &environment(&[("PI_PROVIDER", "openai-codex"), ("PI_MODEL", "gpt-5.6-sol")]),
    )
    .unwrap();
    assert_eq!(
        selection,
        ModelSelection {
            provider: "anthropic".to_string(),
            id: "claude-opus-4-6".to_string(),
        }
    );
}

#[test]
fn uses_trimmed_environment_defaults() {
    let selection = resolve_model_selection(
        None,
        &environment(&[
            ("PI_PROVIDER", " openai-codex "),
            ("PI_MODEL", " gpt-5.6-sol "),
        ]),
    )
    .unwrap();
    assert_eq!(
        selection,
        ModelSelection {
            provider: "openai-codex".to_string(),
            id: "gpt-5.6-sol".to_string(),
        }
    );
}

#[test]
fn rejects_incomplete_model_selection() {
    for env in [
        environment(&[]),
        environment(&[("PI_PROVIDER", "openai-codex")]),
        environment(&[("PI_MODEL", "gpt-5.6-sol")]),
    ] {
        let error = resolve_model_selection(None, &env).unwrap_err();
        assert!(
            error.contains("Select a harness model explicitly"),
            "{error}"
        );
    }
}

#[test]
fn removes_runner_metadata_and_restores_the_process_environment() {
    let _guard = super::ENV_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::env::set_var("PI_EVAL_VARIANT", "with_docs");
    std::env::set_var("PI_EVAL_ARTIFACT_DIR", "/tmp/artifacts");
    let old_home = std::env::var("HOME").ok();
    let restore = apply_isolated_environment("/tmp/eval-home", "/tmp/eval-agent");
    {
        assert_eq!(std::env::var("HOME").as_deref(), Ok("/tmp/eval-home"));
        assert_eq!(
            std::env::var("USERPROFILE").as_deref(),
            Ok("/tmp/eval-home")
        );
        assert_eq!(
            std::env::var("PI_CODING_AGENT_DIR").as_deref(),
            Ok("/tmp/eval-agent")
        );
        assert!(std::env::var("PI_EVAL_VARIANT").is_err());
        assert!(std::env::var("PI_EVAL_ARTIFACT_DIR").is_err());
    }
    restore();
    assert_eq!(std::env::var("HOME").ok().as_deref(), old_home.as_deref());
    assert_eq!(std::env::var("PI_EVAL_VARIANT").as_deref(), Ok("with_docs"));
    assert_eq!(
        std::env::var("PI_EVAL_ARTIFACT_DIR").as_deref(),
        Ok("/tmp/artifacts")
    );
    std::env::remove_var("PI_EVAL_VARIANT");
    std::env::remove_var("PI_EVAL_ARTIFACT_DIR");
}

#[test]
fn accepts_documentation_variants() {
    assert_eq!(
        resolve_documentation_variant(Some("without_docs")).unwrap(),
        super::super::plan::DocumentationVariant::WithoutDocs
    );
    assert_eq!(
        resolve_documentation_variant(Some("with_docs")).unwrap(),
        super::super::plan::DocumentationVariant::WithDocs
    );
}

#[test]
fn rejects_invalid_variants() {
    for variant in [None, Some(""), Some("other")] {
        let error = resolve_documentation_variant(variant).unwrap_err();
        assert!(error.contains("PI_EVAL_VARIANT"), "{error}");
    }
}

/// Upstream builds the default Pi prompt with `buildSystemPrompt`; the port
/// reuses the ported builder.
fn default_prompt() -> String {
    build_system_prompt(&BuildSystemPromptOptions {
        cwd: "/workspace".to_string(),
        selected_tools: Some(
            DOCUMENTATION_EVAL_TOOLS
                .iter()
                .map(|tool| tool.to_string())
                .collect::<Vec<String>>(),
        ),
        ..BuildSystemPromptOptions::with_cwd("/workspace")
    })
    .expect("system prompt")
}

#[test]
fn strips_only_the_documentation_routing_section_from_the_default_pi_prompt() {
    let prompt = default_prompt();
    assert!(
        prompt.contains("\n<docs>\nPi documentation (read only"),
        "{prompt}"
    );
    assert!(prompt.contains("\n<rules>\n"), "{prompt}");
    assert!(prompt.contains("\n<cwd>\n/workspace\n</cwd>"), "{prompt}");
    assert!(prompt.contains("docs/models.md"), "{prompt}");

    let stripped = exclude_pi_documentation(&prompt).unwrap();
    assert!(stripped.contains("\n<rules>\n"), "{stripped}");
    assert!(
        stripped.contains("\n<cwd>\n/workspace\n</cwd>"),
        "{stripped}"
    );
    assert!(!stripped.contains("<docs>"), "{stripped}");
    assert!(!stripped.contains("Pi documentation"), "{stripped}");
    assert!(!stripped.contains("docs/models.md"), "{stripped}");
    // Upstream asserts against getReadmePath()/getExamplesPath(); those
    // helpers are private in the ported system-prompt module, so the
    // assertions check the same path segments (divergence D2).
    assert!(!stripped.contains("README.md"), "{stripped}");
    assert!(
        !stripped.contains(&crate::coding_agent::core::auth_guidance::get_docs_path()),
        "{stripped}"
    );
    assert!(!stripped.contains("examples"), "{stripped}");
}

#[test]
fn verifies_the_prompt_that_was_sent() {
    let prompt = default_prompt();
    let stripped = exclude_pi_documentation(&prompt).unwrap();

    assert_eq!(
        verify_system_prompt(&stripped, Some("without_docs"), Some(false)).unwrap(),
        stripped
    );
    let error = verify_system_prompt(&prompt, Some("without_docs"), Some(false)).unwrap_err();
    assert!(error.contains("does not match"), "{error}");
}

#[test]
fn fails_closed_when_prompt_markers_are_missing() {
    let error = exclude_pi_documentation("Instructions").unwrap_err();
    assert!(error.contains("no Pi documentation section"), "{error}");
    let error = exclude_pi_documentation("\n<docs>\nPi documentation\n</docs>").unwrap_err();
    assert!(error.contains("no working-directory section"), "{error}");
}

#[test]
fn rejects_documentation_harnesses_outside_the_container_sandbox() {
    let error = create_pi_documentation_eval_harness(
        HarnessOptions::default(),
        std::sync::Arc::new(|_| Ok("unused".to_string())),
    )
    .err()
    .expect("harness construction fails outside the sandbox");
    assert!(error.contains("isolated container sandbox"), "{error}");
}

// ---------------------------------------------------------------------------
// Oracle comparisons (node, --experimental-strip-types)
// ---------------------------------------------------------------------------

fn oracle() -> serde_json::Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/evals_m6/oracle/harness.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).expect("harness oracle")).expect("JSON")
}

#[test]
fn oracle_model_selection_matches_upstream() {
    let oracle = oracle();
    let selection = resolve_model_selection(
        Some(ModelSelection {
            provider: "anthropic".to_string(),
            id: "claude-opus-4-6".to_string(),
        }),
        &environment(&[("PI_PROVIDER", "openai-codex"), ("PI_MODEL", "gpt-5.6-sol")]),
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(serde_json::json!({
            "provider": selection.provider,
            "id": selection.id,
        }))
        .unwrap(),
        oracle.get("sel1").unwrap().clone()
    );
    let selection = resolve_model_selection(
        None,
        &environment(&[
            ("PI_PROVIDER", " openai-codex "),
            ("PI_MODEL", " gpt-5.6-sol "),
        ]),
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(serde_json::json!({
            "provider": selection.provider,
            "id": selection.id,
        }))
        .unwrap(),
        oracle.get("sel2").unwrap().clone()
    );
    let errors: Vec<String> = [
        environment(&[]),
        environment(&[("PI_PROVIDER", "openai-codex")]),
        environment(&[("PI_MODEL", "gpt-5.6-sol")]),
    ]
    .into_iter()
    .map(|env| resolve_model_selection(None, &env).unwrap_err())
    .collect();
    let expected: Vec<String> = oracle
        .get("selErrs")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().expect("string").to_string())
        .collect();
    assert_eq!(errors, expected);
    let tools: Vec<&str> = DOCUMENTATION_EVAL_TOOLS.to_vec();
    assert_eq!(
        serde_json::to_value(tools).unwrap(),
        oracle.get("tools").unwrap().clone()
    );
}

#[test]
fn oracle_variants_and_prompt_helpers_match_upstream() {
    let oracle = oracle();
    let variants: Vec<String> = ["without_docs", "with_docs"]
        .iter()
        .map(|variant| {
            resolve_documentation_variant(Some(variant))
                .unwrap()
                .as_str()
                .to_string()
        })
        .collect();
    assert_eq!(
        serde_json::to_value(variants).unwrap(),
        oracle.get("variants").unwrap().clone()
    );
    let errors: Vec<String> = [None, Some(""), Some("other")]
        .into_iter()
        .map(|variant| resolve_documentation_variant(variant).unwrap_err())
        .collect();
    let expected: Vec<String> = oracle
        .get("variantErrs")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().expect("string").to_string())
        .collect();
    assert_eq!(errors, expected);

    // The oracle exercise prompt mirrors the default prompt's section layout
    // (docs section between rules and cwd); the ported builder itself is
    // verified structurally in the tests above.
    let prompt = "Preamble\n<rules>\nrule one\n</rules>\n<docs>\nPi documentation (read only)\ndocs/models.md\nmore docs lines\n</docs>\n<cwd>\n/workspace\n</cwd>\n";
    let stripped = exclude_pi_documentation(prompt).unwrap();
    assert_eq!(
        stripped,
        oracle.get("stripped").unwrap().as_str().unwrap(),
        "stripped prompt bytes"
    );
    assert_eq!(
        verify_system_prompt(&stripped, Some("without_docs"), Some(false)).unwrap(),
        oracle.get("verifyOk").unwrap().as_str().unwrap()
    );
    assert_eq!(
        verify_system_prompt(prompt, Some("with_docs"), Some(true)).unwrap(),
        oracle.get("verifyWith").unwrap().as_str().unwrap()
    );
    let error = verify_system_prompt(prompt, Some("without_docs"), Some(false)).unwrap_err();
    assert_eq!(error, oracle.get("verifyErr1").unwrap().as_str().unwrap());
    let error = verify_system_prompt(&stripped, Some("with_docs"), Some(true)).unwrap_err();
    assert_eq!(error, oracle.get("verifyErr2").unwrap().as_str().unwrap());
    let error = verify_system_prompt("no rules", Some("n"), Some(false)).unwrap_err();
    assert_eq!(error, oracle.get("verifyErr3").unwrap().as_str().unwrap());
    let error = exclude_pi_documentation("Instructions").unwrap_err();
    assert_eq!(error, oracle.get("excludeErr1").unwrap().as_str().unwrap());
    let error = exclude_pi_documentation("\n<docs>\nPi documentation\n</docs>").unwrap_err();
    assert_eq!(error, oracle.get("excludeErr2").unwrap().as_str().unwrap());
    assert_eq!(
        verify_system_prompt("anything", Some("n"), None).unwrap(),
        oracle.get("verifyPassthrough").unwrap().as_str().unwrap()
    );
}
