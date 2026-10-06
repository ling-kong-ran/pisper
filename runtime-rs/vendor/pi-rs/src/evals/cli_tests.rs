//! Ports of the CLI argument parser behavior plus byte-level oracle
//! comparisons against the upstream module executed with node
//! (`tests/fixtures/evals_m6/oracle/cli.json`).

use super::{container_path, ensure_distinct_images, normalize_discovered_file, parse_eval_cli};
use crate::evals::docker::{BuiltImages, VariantImage};
use std::collections::BTreeMap;

fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

fn args(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| item.to_string()).collect()
}

#[test]
fn parses_provider_model_and_runs() {
    let parsed = parse_eval_cli(&args(&["--provider", "p1", "--model", "m1"]), &env(&[])).unwrap();
    assert_eq!(parsed.provider.as_deref(), Some("p1"));
    assert_eq!(parsed.model.as_deref(), Some("m1"));
    assert_eq!(parsed.runs_per_variant, 1);
    assert!(parsed.requested_files.is_empty());
    assert!(parsed.discovery_args.is_empty());

    let parsed = parse_eval_cli(
        &args(&["--provider=p1", "--model=m1", "--runs-per-variant=3"]),
        &env(&[]),
    )
    .unwrap();
    assert_eq!(parsed.runs_per_variant, 3);

    let parsed =
        parse_eval_cli(&args(&["--provider", " p1 ", "--model", " m1 "]), &env(&[])).unwrap();
    assert_eq!(parsed.provider.as_deref(), Some("p1"));
    assert_eq!(parsed.model.as_deref(), Some("m1"));
}

#[test]
fn requires_both_provider_and_model() {
    let error = parse_eval_cli(&args(&["--model", "m1"]), &env(&[])).unwrap_err();
    assert!(
        error.contains("CLI model selection requires both"),
        "{error}"
    );
    let error = parse_eval_cli(&args(&["--provider", "p1"]), &env(&[])).unwrap_err();
    assert!(
        error.contains("CLI model selection requires both"),
        "{error}"
    );
    let error = parse_eval_cli(&args(&["--provider", "p1", "--model"]), &env(&[])).unwrap_err();
    assert!(error.contains("Missing value for --model"), "{error}");
}

#[test]
fn falls_back_to_environment_defaults() {
    let parsed = parse_eval_cli(
        &args(&[]),
        &env(&[("PI_PROVIDER", "p2"), ("PI_MODEL", "m2")]),
    )
    .unwrap();
    assert_eq!(parsed.provider.as_deref(), Some("p2"));
    assert_eq!(parsed.model.as_deref(), Some("m2"));

    let error = parse_eval_cli(&args(&[]), &env(&[("PI_PROVIDER", "p2")])).unwrap_err();
    assert!(
        error.contains("Set both PI_PROVIDER and PI_MODEL"),
        "{error}"
    );

    let parsed = parse_eval_cli(
        &args(&[]),
        &env(&[("PI_PROVIDER", " p2 "), ("PI_MODEL", " m2 ")]),
    )
    .unwrap();
    assert_eq!(parsed.provider.as_deref(), Some("p2"));
    assert_eq!(parsed.model.as_deref(), Some("m2"));
}

#[test]
fn collects_filter_and_file_arguments() {
    let parsed = parse_eval_cli(
        &args(&[
            "evals/models.docs.eval.ts",
            "--provider",
            "p1",
            "--model",
            "m1",
            "-t",
            "foo bar",
        ]),
        &env(&[]),
    )
    .unwrap();
    assert_eq!(
        parsed.requested_files,
        vec!["evals/models.docs.eval.ts".to_string()]
    );
    assert_eq!(
        parsed.discovery_args,
        vec!["-t".to_string(), "foo bar".to_string()]
    );

    let parsed = parse_eval_cli(
        &args(&["--provider", "p1", "--model", "m1", "--testNamePattern=x"]),
        &env(&[]),
    )
    .unwrap();
    assert_eq!(
        parsed.discovery_args,
        vec!["--testNamePattern=x".to_string()]
    );

    for bad in [
        vec!["--provider", "p1", "--model", "m1", "--testNamePattern="],
        vec!["--provider", "p1", "--model", "m1", "-t"],
    ] {
        let error = parse_eval_cli(&args(&bad), &env(&[])).unwrap_err();
        assert!(error.contains("Missing value"), "{error}");
    }
}

#[test]
fn validates_runs_per_variant() {
    let parsed = parse_eval_cli(
        &args(&[
            "--runs-per-variant",
            "2",
            "--provider",
            "p1",
            "--model",
            "m1",
        ]),
        &env(&[]),
    )
    .unwrap();
    assert_eq!(parsed.runs_per_variant, 2);

    let error = parse_eval_cli(
        &args(&[
            "--runs-per-variant",
            "0",
            "--provider",
            "p1",
            "--model",
            "m1",
        ]),
        &env(&[]),
    )
    .unwrap_err();
    assert!(error.contains("positive integer"), "{error}");

    let error = parse_eval_cli(
        &args(&["--runs-per-variant=-1", "--provider", "p1", "--model", "m1"]),
        &env(&[]),
    )
    .unwrap_err();
    assert!(error.contains("positive integer"), "{error}");

    let parsed = parse_eval_cli(
        &args(&[]),
        &env(&[
            ("PI_PROVIDER", "p2"),
            ("PI_MODEL", "m2"),
            ("PI_EVAL_RUNS_PER_VARIANT", "4"),
        ]),
    )
    .unwrap();
    assert_eq!(parsed.runs_per_variant, 4);

    let error = parse_eval_cli(
        &args(&[]),
        &env(&[
            ("PI_PROVIDER", "p2"),
            ("PI_MODEL", "m2"),
            ("PI_EVAL_RUNS_PER_VARIANT", "x"),
        ]),
    )
    .unwrap_err();
    assert!(error.contains("positive integer"), "{error}");
}

#[test]
fn rejects_unsupported_arguments() {
    let error = parse_eval_cli(&args(&["--bogus"]), &env(&[])).unwrap_err();
    assert!(
        error.contains("Unsupported eval argument: --bogus"),
        "{error}"
    );
}

#[test]
fn normalizes_discovered_files_and_container_paths() {
    assert_eq!(
        normalize_discovered_file("/repo/packages/evals/evals/models.docs.eval.ts").unwrap(),
        "evals/models.docs.eval.ts"
    );
    let error = normalize_discovered_file("/repo/other/file.ts").unwrap_err();
    assert!(error.contains("outside the container package"), "{error}");

    let path = container_path("evals/models.docs.eval.ts").unwrap();
    assert_eq!(path, "evals/models.docs.eval.ts");
    let error = container_path("../outside.ts").unwrap_err();
    assert!(error.contains("Eval file must be inside"), "{error}");
}

#[test]
fn rejects_identical_variant_images() {
    let images = BuiltImages {
        without_docs: VariantImage {
            name: "a:local".to_string(),
            id: "sha256:1".to_string(),
        },
        with_docs: VariantImage {
            name: "b:local".to_string(),
            id: "sha256:1".to_string(),
        },
    };
    let error = ensure_distinct_images(&images).unwrap_err();
    assert!(error.contains("resolved to the same image"), "{error}");
    let images = BuiltImages {
        without_docs: VariantImage {
            id: "sha256:1".to_string(),
            ..images.without_docs
        },
        with_docs: VariantImage {
            id: "sha256:2".to_string(),
            ..images.with_docs
        },
    };
    ensure_distinct_images(&images).unwrap();
}

// ---------------------------------------------------------------------------
// Oracle comparisons (node, --experimental-strip-types)
// ---------------------------------------------------------------------------

#[test]
fn oracle_parse_eval_cli_matches_upstream() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/evals_m6/oracle/cli.json"
    );
    let oracle: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).expect("cli oracle")).expect("JSON");
    for case in oracle.as_array().expect("cases") {
        let cli_args: Vec<String> = case
            .get("args")
            .and_then(|value| value.as_array())
            .map(|items| {
                items
                    .iter()
                    .map(|value| value.as_str().expect("string").to_string())
                    .collect()
            })
            .unwrap_or_default();
        let environment: BTreeMap<String, String> = case
            .get("env")
            .and_then(|value| value.as_object())
            .map(|map| {
                map.iter()
                    .map(|(key, value)| (key.clone(), value.as_str().expect("string").to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let actual = match parse_eval_cli(&cli_args, &environment) {
            Ok(parsed) => {
                // JSON.stringify omits undefined fields, so the oracle "ok"
                // object carries only the defined options.
                let mut ok = serde_json::Map::new();
                if let Some(provider) = parsed.provider.clone() {
                    ok.insert("provider".to_string(), serde_json::json!(provider));
                }
                if let Some(model) = parsed.model.clone() {
                    ok.insert("model".to_string(), serde_json::json!(model));
                }
                ok.insert(
                    "runsPerVariant".to_string(),
                    serde_json::json!(parsed.runs_per_variant),
                );
                ok.insert(
                    "requestedFiles".to_string(),
                    serde_json::json!(parsed.requested_files),
                );
                ok.insert(
                    "discoveryArgs".to_string(),
                    serde_json::json!(parsed.discovery_args),
                );
                serde_json::json!({
                    "args": cli_args,
                    "env": case.get("env").cloned().unwrap_or(serde_json::Value::Null),
                    "ok": serde_json::Value::Object(ok),
                })
            }
            Err(error) => serde_json::json!({
                "args": cli_args,
                "env": case.get("env").cloned().unwrap_or(serde_json::Value::Null),
                "error": error,
            }),
        };
        assert_eq!(&actual, case, "cli oracle mismatch");
    }
}
