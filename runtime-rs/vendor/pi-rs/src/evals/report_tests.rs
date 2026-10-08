//! Ports of `pi/packages/evals/test/report.test.ts` and
//! `test/comparison.test.ts`, plus oracle comparisons against the upstream
//! module executed with node (`tests/fixtures/evals_m6/oracle/report.json`,
//! `report_formatted.txt`).

use super::{
    classify_case_status, errored_observation, format_eval_comparison_report,
    read_task_observation, summarize_eval_observations, ComparisonFlag, EvalMetrics,
    EvalObservation, EvalOutcome, ExpectedEvalRun, PI_SESSION_SNAPSHOT_ARTIFACT,
};
use crate::evals::plan::{DocumentationVariant, EvalTask};

fn task() -> EvalTask {
    EvalTask {
        file: "evals/example.docs.eval.ts".to_string(),
        full_name: "Example workflow > handles the case".to_string(),
        eval_set: "Example workflow".to_string(),
        case_id: "handles the case".to_string(),
        variant: DocumentationVariant::WithoutDocs,
        model: "fixture/model".to_string(),
        run_number: 1,
    }
}

const SESSION: &str = "{\"type\":\"session\"}\n";

struct TempDir(std::path::PathBuf);
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp_dir(tag: &str) -> TempDir {
    let base = std::env::temp_dir();
    let path = base.join(format!(
        "pi-eval-report-test-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).expect("mkdir");
    TempDir(path)
}

/// Upstream `writeTaskReport`.
fn write_task_report(
    directory: &std::path::Path,
    status: &str,
    meta: Option<serde_json::Value>,
) -> std::path::PathBuf {
    let report_path = directory.join("vitest.json");
    let task = task();
    let report = serde_json::json!({
        "numFailedTests": 0,
        "numPassedTests": if status == "passed" { 1 } else { 0 },
        "numPendingTests": if status == "pending" || status == "skipped" { 1 } else { 0 },
        "numTodoTests": 0,
        "numTotalTests": 1,
        "startTime": 0,
        "success": true,
        "testResults": [
            {
                "message": "",
                "name": "/repo/packages/evals/evals/example.docs.eval.ts",
                "status": "passed",
                "assertionResults": [
                    {
                        "ancestorTitles": [task.eval_set],
                        "fullName": format!("{} {}", task.eval_set, task.case_id),
                        "status": status,
                        "title": task.case_id,
                        "failureMessages": [],
                        "meta": meta.unwrap_or(serde_json::json!({})),
                    }
                ],
            }
        ],
    });
    std::fs::write(&report_path, serde_json::to_string(&report).unwrap()).expect("write report");
    report_path
}

/// Upstream `scoredMeta`.
fn scored_meta(overrides: serde_json::Value) -> serde_json::Value {
    let avg_score = overrides
        .get("avgScore")
        .cloned()
        .unwrap_or(serde_json::json!(0.5));
    let model = overrides
        .get("model")
        .cloned()
        .unwrap_or(serde_json::json!("model"));
    let errors = overrides
        .get("errors")
        .cloned()
        .unwrap_or(serde_json::json!([]));
    let artifacts = overrides
        .get("artifacts")
        .cloned()
        .unwrap_or(serde_json::json!({
            "runId": "run-1",
            PI_SESSION_SNAPSHOT_ARTIFACT: SESSION,
        }));
    serde_json::json!({
        "eval": {
            "avgScore": avg_score,
            "scores": [{ "name": "StructuredOutputJudge", "score": 0.5 }],
            "thresholdFailed": false,
        },
        "harness": {
            "name": "without_docs",
            "run": {
                "output": { "ok": true },
                "session": { "events": [{ "type": "message", "role": "user", "content": "prompt" }] },
                "usage": {
                    "provider": "fixture",
                    "model": model,
                    "inputTokens": 10,
                    "outputTokens": 5,
                    "totalTokens": 15,
                    "toolCalls": 1,
                    "metadata": { "cacheReadTokens": 2, "cacheWriteTokens": 3, "estimatedCostUsd": 0.01 },
                },
                "timings": { "totalMs": 1234 },
                "artifacts": artifacts,
                "errors": errors,
            },
        },
    })
}

async fn read_observation(
    directory: &std::path::Path,
    status: &str,
    meta: Option<serde_json::Value>,
) -> EvalObservation {
    let report_path = write_task_report(directory, status, meta);
    read_task_observation(&task(), &report_path, directory).await
}

#[test]
fn classify_maps_skip_statuses_to_skipped() {
    for status in ["skipped", "todo", "disabled"] {
        assert_eq!(classify_case_status(status), Some(EvalOutcome::Skipped));
    }
}

#[test]
fn classify_maps_failed_infrastructure_to_errored() {
    assert_eq!(classify_case_status("failed"), Some(EvalOutcome::Errored));
    assert_eq!(classify_case_status("passed"), None);
}

#[tokio::test]
async fn preserves_skipped_outcome_when_no_harness_run_exists() {
    let directory = temp_dir("skipped");
    let observation = read_observation(&directory.0, "skipped", None).await;
    assert_eq!(observation.outcome, EvalOutcome::Skipped);
}

#[tokio::test]
async fn preserves_pending_outcome_when_no_harness_run_exists() {
    let directory = temp_dir("pending");
    let observation = read_observation(&directory.0, "pending", None).await;
    assert_eq!(observation.outcome, EvalOutcome::Pending);
}

#[tokio::test]
async fn preserves_metrics_from_failed_eval_with_partial_harness_run() {
    let directory = temp_dir("failed-partial");
    let observation = read_observation(
        &directory.0,
        "failed",
        Some(scored_meta(serde_json::json!({
            "errors": [{ "message": "Prompt verification failed" }]
        }))),
    )
    .await;
    let task = task();
    assert_eq!(
        observation,
        EvalObservation {
            eval_set: task.eval_set.clone(),
            case_id: task.case_id.clone(),
            variant: task.variant,
            model: task.model.clone(),
            run_number: task.run_number,
            metrics: EvalMetrics {
                input_tokens: Some(10.0),
                output_tokens: Some(5.0),
                cache_read_tokens: Some(2.0),
                cache_write_tokens: Some(3.0),
                total_tokens: Some(15.0),
                tool_calls: Some(1.0),
                total_ms: Some(1234.0),
                estimated_cost_usd: Some(0.01),
            },
            outcome: EvalOutcome::Errored,
            score: None,
        }
    );
    let sessions = directory.0.join(task.variant.as_str()).join("sessions");
    let hash = std::fs::read_dir(&sessions)
        .expect("sessions dir")
        .next()
        .expect("one hash dir")
        .expect("entry")
        .file_name();
    let session =
        std::fs::read_to_string(sessions.join(hash).join("session.jsonl")).expect("session");
    assert_eq!(session, SESSION);
}

#[tokio::test]
async fn records_errored_outcome_when_passed_eval_has_no_harness_run() {
    let directory = temp_dir("no-run");
    let observation = read_observation(&directory.0, "passed", None).await;
    assert_eq!(observation.outcome, EvalOutcome::Errored);
}

#[tokio::test]
async fn reads_scored_harness_run_and_persists_session_artifact() {
    let directory = temp_dir("scored");
    let observation = read_observation(
        &directory.0,
        "passed",
        Some(scored_meta(serde_json::json!({}))),
    )
    .await;
    let task = task();
    assert_eq!(observation.outcome, EvalOutcome::Scored);
    assert_eq!(observation.score, Some(0.5));
    assert_eq!(
        observation.metrics,
        EvalMetrics {
            input_tokens: Some(10.0),
            output_tokens: Some(5.0),
            cache_read_tokens: Some(2.0),
            cache_write_tokens: Some(3.0),
            total_tokens: Some(15.0),
            tool_calls: Some(1.0),
            total_ms: Some(1234.0),
            estimated_cost_usd: Some(0.01),
        }
    );
    let sessions = directory.0.join(task.variant.as_str()).join("sessions");
    let hashes: Vec<_> = std::fs::read_dir(&sessions)
        .expect("sessions dir")
        .collect();
    assert_eq!(hashes.len(), 1);
    let session = std::fs::read_to_string(
        sessions
            .join(hashes[0].as_ref().expect("entry").file_name())
            .join("session.jsonl"),
    )
    .expect("session");
    assert_eq!(session, SESSION);
}

#[tokio::test]
async fn treats_zero_score_as_scored_data() {
    let directory = temp_dir("zero");
    let observation = read_observation(
        &directory.0,
        "passed",
        Some(scored_meta(serde_json::json!({ "avgScore": 0 }))),
    )
    .await;
    assert_eq!(observation.outcome, EvalOutcome::Scored);
    assert_eq!(observation.score, Some(0.0));
}

#[tokio::test]
async fn records_unscored_outcome_when_completed_eval_has_no_score() {
    let directory = temp_dir("unscored");
    let observation = read_observation(
        &directory.0,
        "passed",
        Some(scored_meta(serde_json::json!({ "avgScore": null }))),
    )
    .await;
    assert_eq!(observation.outcome, EvalOutcome::Unscored);
}

#[tokio::test]
async fn records_errored_outcome_when_reported_model_mismatches_task() {
    let directory = temp_dir("model-mismatch");
    let observation = read_observation(
        &directory.0,
        "passed",
        Some(scored_meta(serde_json::json!({ "model": "other" }))),
    )
    .await;
    assert_eq!(observation.outcome, EvalOutcome::Errored);
}

#[tokio::test]
async fn records_errored_outcome_when_completed_harness_run_contains_errors() {
    let directory = temp_dir("run-errors");
    let observation = read_observation(
        &directory.0,
        "passed",
        Some(scored_meta(
            serde_json::json!({ "errors": [{ "message": "boom" }] }),
        )),
    )
    .await;
    assert_eq!(observation.outcome, EvalOutcome::Errored);
}

#[tokio::test]
async fn still_scores_completed_eval_when_session_artifact_is_missing() {
    let directory = temp_dir("no-artifact");
    let observation = read_observation(
        &directory.0,
        "passed",
        Some(scored_meta(
            serde_json::json!({ "artifacts": { "runId": "run-1" } }),
        )),
    )
    .await;
    assert_eq!(observation.outcome, EvalOutcome::Scored);
    assert_eq!(observation.score, Some(0.5));
}

// ---------------------------------------------------------------------------
// comparison.test.ts
// ---------------------------------------------------------------------------

fn scored(
    variant: DocumentationVariant,
    run_number: u32,
    score: f64,
    metrics: EvalMetrics,
) -> EvalObservation {
    // Upstream spread semantics: explicit defaults first, overrides win.
    let merge = |overridden: Option<f64>, default: f64| overridden.or(Some(default));
    EvalObservation {
        eval_set: "tool access".to_string(),
        case_id: "create".to_string(),
        variant,
        model: "fixture/model".to_string(),
        run_number,
        metrics: EvalMetrics {
            total_tokens: merge(metrics.total_tokens, 100.0),
            tool_calls: merge(metrics.tool_calls, 2.0),
            total_ms: merge(metrics.total_ms, 1000.0),
            estimated_cost_usd: merge(metrics.estimated_cost_usd, 0.01),
            ..EvalMetrics::default()
        },
        outcome: EvalOutcome::Scored,
        score: Some(score),
    }
}

fn errored(
    variant: DocumentationVariant,
    run_number: u32,
    total_tokens: Option<f64>,
) -> EvalObservation {
    EvalObservation {
        eval_set: "tool access".to_string(),
        case_id: "create".to_string(),
        variant,
        model: "fixture/model".to_string(),
        run_number,
        metrics: EvalMetrics {
            total_tokens,
            ..EvalMetrics::default()
        },
        outcome: EvalOutcome::Errored,
        score: None,
    }
}

fn expected_for(run_numbers: &[u32]) -> Vec<ExpectedEvalRun> {
    run_numbers
        .iter()
        .flat_map(|run_number| {
            [
                DocumentationVariant::WithoutDocs,
                DocumentationVariant::WithDocs,
            ]
            .into_iter()
            .map(move |variant| ExpectedEvalRun {
                eval_set: "tool access".to_string(),
                case_id: "create".to_string(),
                variant,
                model: "fixture/model".to_string(),
                run_number: *run_number,
            })
        })
        .collect()
}

fn strip_vt(text: &str) -> String {
    // Upstream `stripVTControlCharacters`.
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(character);
        }
    }
    out
}

#[test]
fn computes_paired_lift_and_efficiency_deltas() {
    let observations = vec![
        scored(
            DocumentationVariant::WithoutDocs,
            1,
            0.0,
            EvalMetrics {
                total_tokens: Some(100.0),
                tool_calls: Some(3.0),
                total_ms: Some(1000.0),
                ..EvalMetrics::default()
            },
        ),
        scored(
            DocumentationVariant::WithDocs,
            1,
            1.0,
            EvalMetrics {
                total_tokens: Some(120.0),
                tool_calls: Some(2.0),
                total_ms: Some(800.0),
                ..EvalMetrics::default()
            },
        ),
        scored(
            DocumentationVariant::WithoutDocs,
            2,
            1.0,
            EvalMetrics {
                total_tokens: Some(200.0),
                ..EvalMetrics::default()
            },
        ),
        scored(
            DocumentationVariant::WithDocs,
            2,
            1.0,
            EvalMetrics {
                total_tokens: Some(180.0),
                ..EvalMetrics::default()
            },
        ),
    ];
    let report = summarize_eval_observations("digest", &expected_for(&[1, 2]), &observations);
    assert_eq!(report.comparisons.len(), 1);
    let comparison = &report.comparisons[0];
    assert_eq!(comparison.eval_set, "tool access");
    assert_eq!(comparison.total_pairs, 2);
    assert_eq!(comparison.eligible_pairs, 2);
    assert_eq!(comparison.blocked_pairs, 0);
    assert_eq!(comparison.control_pass_rate, Some(0.5));
    assert_eq!(comparison.treatment_pass_rate, Some(1.0));
    assert_eq!(comparison.lift, Some(0.5));
    assert_eq!(
        comparison.total_tokens,
        super::PairedMetricSummary {
            eligible_pairs: 2,
            control_mean: Some(150.0),
            treatment_mean: Some(150.0),
            mean_delta: Some(0.0)
        }
    );
    assert_eq!(
        comparison.tool_calls,
        super::PairedMetricSummary {
            eligible_pairs: 2,
            control_mean: Some(2.5),
            treatment_mean: Some(2.0),
            mean_delta: Some(-0.5)
        }
    );
}

#[test]
fn fails_closed_for_incomplete_and_errored_pairs_while_retaining_totals() {
    let observations = vec![
        scored(
            DocumentationVariant::WithoutDocs,
            1,
            0.0,
            EvalMetrics::default(),
        ),
        errored(DocumentationVariant::WithDocs, 1, Some(120.0)),
        scored(
            DocumentationVariant::WithoutDocs,
            2,
            1.0,
            EvalMetrics {
                total_tokens: Some(200.0),
                ..EvalMetrics::default()
            },
        ),
    ];
    let report = summarize_eval_observations("digest", &expected_for(&[1, 2]), &observations);
    let comparison = &report.comparisons[0];
    assert_eq!(comparison.total_pairs, 2);
    assert_eq!(comparison.eligible_pairs, 0);
    assert_eq!(comparison.blocked_pairs, 2);
    assert_eq!(comparison.lift, None);
    assert_eq!(report.blocked_pairs[0].run_number, 1);
    assert_eq!(report.blocked_pairs[0].reasons, vec!["with_docs: errored"]);
    assert_eq!(report.blocked_pairs[1].run_number, 2);
    assert_eq!(
        report.blocked_pairs[1].reasons,
        vec!["with_docs: expected 1 observation, found 0"]
    );
    assert_eq!(
        report.operational_totals[0].total_tokens,
        super::OperationalMetricTotal {
            available_runs: 2,
            total: Some(300.0)
        }
    );
}

#[test]
fn blocks_duplicate_observations_and_keeps_missing_metrics_distinct_from_zero() {
    // Upstream `scored(...)` then `delete withoutTokens.totalTokens`.
    let mut without_tokens = scored(
        DocumentationVariant::WithoutDocs,
        1,
        1.0,
        EvalMetrics::default(),
    );
    without_tokens.metrics.total_tokens = None;
    let report = summarize_eval_observations(
        "digest",
        &expected_for(&[1]),
        &[
            without_tokens.clone(),
            without_tokens,
            scored(
                DocumentationVariant::WithDocs,
                1,
                1.0,
                EvalMetrics {
                    total_tokens: Some(0.0),
                    ..EvalMetrics::default()
                },
            ),
        ],
    );
    assert_eq!(
        report.blocked_pairs[0].reasons,
        vec!["without_docs: expected 1 observation, found 2"]
    );
    assert_eq!(
        report.operational_totals[0].total_tokens,
        super::OperationalMetricTotal {
            available_runs: 0,
            total: None
        }
    );
    assert_eq!(
        report.operational_totals[1].total_tokens,
        super::OperationalMetricTotal {
            available_runs: 1,
            total: Some(0.0)
        }
    );
}

#[test]
fn formats_blocked_comparisons_and_operational_totals() {
    let report = summarize_eval_observations(
        "digest",
        &expected_for(&[1, 2]),
        &[
            scored(
                DocumentationVariant::WithoutDocs,
                1,
                1.0,
                EvalMetrics::default(),
            ),
            scored(
                DocumentationVariant::WithDocs,
                1,
                1.0,
                EvalMetrics::default(),
            ),
        ],
    );
    let formatted = strip_vt(&format_eval_comparison_report(&report));
    assert!(
        formatted.contains("Documentation Eval Comparisons"),
        "{formatted}"
    );
    assert!(
        formatted.contains("Pass rate  withheld because pairs are blocked"),
        "{formatted}"
    );
    assert!(formatted.contains("without_docs: 1 runs"), "{formatted}");
}

#[test]
fn errored_observation_carries_only_the_task_identity() {
    let task = EvalTask {
        file: "f".to_string(),
        full_name: "A > b".to_string(),
        eval_set: "A".to_string(),
        case_id: "b".to_string(),
        variant: DocumentationVariant::WithDocs,
        model: "m/n".to_string(),
        run_number: 7,
    };
    let observation = errored_observation(&task);
    assert_eq!(observation.outcome, EvalOutcome::Errored);
    assert_eq!(observation.metrics, EvalMetrics::default());
    assert_eq!(observation.run_number, 7);
    assert_eq!(observation.variant, DocumentationVariant::WithDocs);
    assert_eq!(observation.case_id, "b");
    assert_eq!(observation.eval_set, "A");
    assert_eq!(observation.model, "m/n");
}

// ---------------------------------------------------------------------------
// Oracle comparisons (node, --experimental-strip-types)
// ---------------------------------------------------------------------------

fn oracle() -> serde_json::Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/evals_m6/oracle/report.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).expect("report oracle")).expect("JSON")
}

fn oracle_observation(
    variant: DocumentationVariant,
    run_number: u32,
    score: f64,
    metrics: EvalMetrics,
) -> EvalObservation {
    scored(variant, run_number, score, metrics)
}

fn oracle_errored(
    variant: DocumentationVariant,
    run_number: u32,
    total_tokens: Option<f64>,
) -> EvalObservation {
    errored(variant, run_number, total_tokens)
}

fn oracle_expected(run_numbers: &[u32]) -> Vec<ExpectedEvalRun> {
    expected_for(run_numbers)
}

fn scenario_observations() -> Vec<EvalObservation> {
    vec![
        oracle_observation(
            DocumentationVariant::WithoutDocs,
            1,
            0.0,
            EvalMetrics {
                total_tokens: Some(100.0),
                tool_calls: Some(3.0),
                total_ms: Some(1000.0),
                ..EvalMetrics::default()
            },
        ),
        oracle_observation(
            DocumentationVariant::WithDocs,
            1,
            1.0,
            EvalMetrics {
                total_tokens: Some(120.0),
                tool_calls: Some(2.0),
                total_ms: Some(800.0),
                ..EvalMetrics::default()
            },
        ),
        oracle_observation(
            DocumentationVariant::WithoutDocs,
            2,
            1.0,
            EvalMetrics {
                total_tokens: Some(200.0),
                ..EvalMetrics::default()
            },
        ),
        oracle_observation(
            DocumentationVariant::WithDocs,
            2,
            1.0,
            EvalMetrics {
                total_tokens: Some(180.0),
                ..EvalMetrics::default()
            },
        ),
    ]
}

fn assert_report_matches_oracle(key: &str, report: &super::EvalComparisonReport) {
    let oracle = oracle();
    let expected = oracle.get(key).expect("oracle key");
    // Byte-level check of the JSON.stringify surface (compact, JS numbers).
    let actual_bytes =
        crate::serde_support::to_json_string_with_js_numbers(report).expect("serialize");
    assert_eq!(
        actual_bytes,
        expected.to_string(),
        "oracle bytes mismatch for {key}"
    );
    // Structural check: reparse the byte-exact form (integral JS numbers
    // parse as integers on both sides, keeping Value equality meaningful).
    let actual: serde_json::Value = serde_json::from_str(&actual_bytes).expect("reparse");
    assert_eq!(&actual, expected, "oracle report mismatch for {key}");
}

#[test]
fn oracle_report1_matches_upstream_bytes() {
    let report = summarize_eval_observations(
        "digest",
        &oracle_expected(&[1, 2]),
        &scenario_observations(),
    );
    assert_report_matches_oracle("report1", &report);
}

#[test]
fn oracle_report2_and_report3_match_upstream_bytes() {
    let observations = vec![
        oracle_observation(
            DocumentationVariant::WithoutDocs,
            1,
            0.0,
            EvalMetrics::default(),
        ),
        oracle_errored(DocumentationVariant::WithDocs, 1, Some(120.0)),
        oracle_observation(
            DocumentationVariant::WithoutDocs,
            2,
            1.0,
            EvalMetrics {
                total_tokens: Some(200.0),
                ..EvalMetrics::default()
            },
        ),
    ];
    let report = summarize_eval_observations("digest", &oracle_expected(&[1, 2]), &observations);
    assert_report_matches_oracle("report2", &report);

    // Upstream `scored(...)` then `delete withoutTokens.totalTokens`.
    let mut without_tokens = oracle_observation(
        DocumentationVariant::WithoutDocs,
        1,
        1.0,
        EvalMetrics::default(),
    );
    without_tokens.metrics.total_tokens = None;
    let report = summarize_eval_observations(
        "digest",
        &oracle_expected(&[1]),
        &[
            without_tokens.clone(),
            without_tokens,
            oracle_observation(
                DocumentationVariant::WithDocs,
                1,
                1.0,
                EvalMetrics {
                    total_tokens: Some(0.0),
                    ..EvalMetrics::default()
                },
            ),
        ],
    );
    assert_report_matches_oracle("report3", &report);
}

#[test]
fn oracle_report4_and_empty_match_upstream() {
    let report = summarize_eval_observations(
        "digest",
        &oracle_expected(&[1, 2]),
        &[
            oracle_observation(
                DocumentationVariant::WithoutDocs,
                1,
                1.0,
                EvalMetrics::default(),
            ),
            oracle_observation(
                DocumentationVariant::WithDocs,
                1,
                1.0,
                EvalMetrics::default(),
            ),
        ],
    );
    assert_report_matches_oracle("report4", &report);
    let oracle = oracle();
    let empty = summarize_eval_observations("d", &[], &[]);
    assert_eq!(format_eval_comparison_report(&empty), "");
    let expected = oracle.get("formattedEmpty").unwrap().as_str().unwrap();
    assert_eq!(
        serde_json::to_string(&format_eval_comparison_report(&empty)).unwrap(),
        expected
    );
}

fn oracle_formatted() -> String {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/evals_m6/oracle/report_formatted.txt"
    );
    std::fs::read_to_string(path).expect("formatted oracle")
}

#[test]
fn oracle_formatted_text_matches_upstream_byte_for_byte() {
    let report1 = summarize_eval_observations(
        "digest",
        &oracle_expected(&[1, 2]),
        &scenario_observations(),
    );
    let observations = vec![
        oracle_observation(
            DocumentationVariant::WithoutDocs,
            1,
            0.0,
            EvalMetrics::default(),
        ),
        oracle_errored(DocumentationVariant::WithDocs, 1, Some(120.0)),
        oracle_observation(
            DocumentationVariant::WithoutDocs,
            2,
            1.0,
            EvalMetrics {
                total_tokens: Some(200.0),
                ..EvalMetrics::default()
            },
        ),
    ];
    let report2 = summarize_eval_observations("digest", &oracle_expected(&[1, 2]), &observations);
    // Upstream `scored(...)` then `delete withoutTokens.totalTokens`.
    let mut without_tokens = oracle_observation(
        DocumentationVariant::WithoutDocs,
        1,
        1.0,
        EvalMetrics::default(),
    );
    without_tokens.metrics.total_tokens = None;
    let report3 = summarize_eval_observations(
        "digest",
        &oracle_expected(&[1]),
        &[
            without_tokens.clone(),
            without_tokens,
            oracle_observation(
                DocumentationVariant::WithDocs,
                1,
                1.0,
                EvalMetrics {
                    total_tokens: Some(0.0),
                    ..EvalMetrics::default()
                },
            ),
        ],
    );
    let report5 = summarize_eval_observations(
        "digest",
        &oracle_expected(&[1]),
        &[
            oracle_observation(
                DocumentationVariant::WithoutDocs,
                1,
                0.1234567890123456,
                EvalMetrics {
                    total_tokens: Some(1.0),
                    tool_calls: Some(0.0),
                    total_ms: Some(333.3),
                    estimated_cost_usd: Some(0.00015),
                    ..EvalMetrics::default()
                },
            ),
            oracle_observation(
                DocumentationVariant::WithDocs,
                1,
                1.0,
                EvalMetrics {
                    total_tokens: Some(2.0),
                    tool_calls: Some(1.0),
                    total_ms: Some(666.7),
                    estimated_cost_usd: Some(0.00025),
                    ..EvalMetrics::default()
                },
            ),
        ],
    );
    let actual = format!(
        "{}\n---\n{}\n---\n{}\n---\n{}",
        format_eval_comparison_report(&report1),
        format_eval_comparison_report(&report2),
        format_eval_comparison_report(&report3),
        format_eval_comparison_report(&report5),
    );
    assert_eq!(actual, oracle_formatted());
}

#[test]
fn oracle_classify_and_errored_observation_match_upstream() {
    let oracle = oracle();
    let expected: Vec<Option<&str>> =
        ["failed", "skipped", "todo", "disabled", "pending", "passed"]
            .iter()
            .map(|status| {
                classify_case_status(status).map(|outcome| match outcome {
                    EvalOutcome::Errored => "errored",
                    EvalOutcome::Skipped => "skipped",
                    EvalOutcome::Pending => "pending",
                    _ => unreachable!(),
                })
            })
            .collect();
    let oracle_classify: Vec<Option<&str>> = oracle
        .get("classify")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str())
        .collect();
    assert_eq!(expected, oracle_classify);

    let task = EvalTask {
        file: "f".to_string(),
        full_name: "A > b".to_string(),
        eval_set: "A".to_string(),
        case_id: "b".to_string(),
        variant: DocumentationVariant::WithDocs,
        model: "m/n".to_string(),
        run_number: 7,
    };
    let actual = crate::serde_support::to_json_string_with_js_numbers(&errored_observation(&task))
        .expect("ser");
    assert_eq!(actual, oracle.get("erroredObs").unwrap().to_string());
    assert_eq!(
        ComparisonFlag::Flaky,
        ComparisonFlag::Flaky,
        "flags carry the upstream names"
    );
}
