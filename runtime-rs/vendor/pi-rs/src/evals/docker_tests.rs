//! Deterministic tests for the docker runner port (upstream has no direct
//! test file for `docker.ts`; the identity hashing, escaping and auth-file
//! validation are exercised here — see seam S3 for the spawn surfaces).

use super::{regex_escape, require_eval_auth_file, task_directory_name};
use crate::evals::plan::{DocumentationVariant, EvalTask};

fn task() -> EvalTask {
    EvalTask {
        file: "evals/models.docs.eval.ts".to_string(),
        full_name: "Add model > adds the model".to_string(),
        eval_set: "Add model".to_string(),
        case_id: "adds the model".to_string(),
        variant: DocumentationVariant::WithDocs,
        model: "fixture/model".to_string(),
        run_number: 2,
    }
}

#[test]
fn task_directory_is_the_sha256_of_the_identity_json() {
    // The identity JSON is `[evalSet, caseId, variant, model, runNumber]`;
    // verified against an independent digest of that exact string.
    let name = task_directory_name(&task());
    let expected_input = r#"["Add model","adds the model","with_docs","fixture/model",2]"#;
    assert_eq!(name.len(), 64);
    assert_eq!(name, {
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(expected_input.as_bytes());
        digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    });
}

#[test]
fn regex_escape_matches_the_upstream_character_class() {
    assert_eq!(
        regex_escape("Add model > adds the model"),
        "Add model > adds the model"
    );
    assert_eq!(regex_escape("a.b*c"), r"a\.b\*c");
    assert_eq!(regex_escape("x(y)[z]{w}"), r"x\(y\)\[z\]\{w\}");
    assert_eq!(regex_escape("p^$|?\\"), r"p\^\$\|\?\\");
}

#[test]
fn require_eval_auth_file_rejects_missing_files_and_unknown_providers() {
    let _guard = crate::evals::harness::ENV_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let scratch = std::env::temp_dir().join(format!("pi-eval-auth-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let agent_dir = scratch.join("agent");
    std::fs::create_dir_all(&agent_dir).expect("mkdir");
    let previous = std::env::var("PI_CODING_AGENT_DIR").ok();
    std::env::set_var("PI_CODING_AGENT_DIR", &agent_dir);

    let error = require_eval_auth_file("acme").unwrap_err();
    assert!(error.contains("does not exist"), "{error}");

    std::fs::write(agent_dir.join("auth.json"), "{").expect("write");
    let error = require_eval_auth_file("acme").unwrap_err();
    assert!(error.contains("is invalid"), "{error}");

    std::fs::write(agent_dir.join("auth.json"), r#"{"other": {"type": "api"}}"#).expect("write");
    let error = require_eval_auth_file("acme").unwrap_err();
    assert!(error.contains("no credential for provider acme"), "{error}");

    std::fs::write(agent_dir.join("auth.json"), r#"{"acme": {"type": "api"}}"#).expect("write");
    let path = require_eval_auth_file("acme").unwrap();
    assert!(path.ends_with("auth.json"));

    match previous {
        Some(value) => std::env::set_var("PI_CODING_AGENT_DIR", value),
        None => std::env::remove_var("PI_CODING_AGENT_DIR"),
    }
    let _ = std::fs::remove_dir_all(&scratch);
}
