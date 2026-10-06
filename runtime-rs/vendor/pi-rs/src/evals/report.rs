//! Port of `pi/packages/evals/src/report.ts` — task observation extraction,
//! paired control/treatment comparison and report formatting.
//!
//! Seam S1 (see module docs): upstream `@vitest-evals/core` /
//! `@vitest-evals/core/node` are external npm packages not present in the
//! upstream checkout. The port implements the observable contract directly:
//! `readVitestJsonReportFile` is the raw vitest jest-format JSON report and
//! `readReportWorkspace` lifts each assertion's `meta` (`eval`/`harness`
//! payloads) into a report case.

use crate::evals::plan::{DocumentationVariant, EvalTask};
use crate::serde_support::{js_number_string, to_json_string_with_js_numbers};
use serde::Serialize;
use sha2::{Digest, Sha256};

/// Upstream `PI_SESSION_SNAPSHOT_ARTIFACT`.
pub const PI_SESSION_SNAPSHOT_ARTIFACT: &str = "piSessionJsonl";

/// Upstream `EvalRunIdentity`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EvalRunIdentity {
    #[serde(rename = "evalSet")]
    pub eval_set: String,
    #[serde(rename = "caseId")]
    pub case_id: String,
    pub variant: DocumentationVariant,
    pub model: String,
    #[serde(rename = "runNumber")]
    pub run_number: u32,
}

/// Upstream `ExpectedEvalRun`.
pub type ExpectedEvalRun = EvalRunIdentity;

/// Upstream `EvalMetrics` (all fields optional; `JSON.stringify` drops
/// `undefined`).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct EvalMetrics {
    #[serde(rename = "inputTokens", skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<f64>,
    #[serde(rename = "outputTokens", skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<f64>,
    #[serde(rename = "cacheReadTokens", skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<f64>,
    #[serde(rename = "cacheWriteTokens", skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<f64>,
    #[serde(rename = "totalTokens", skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<f64>,
    #[serde(rename = "toolCalls", skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<f64>,
    #[serde(rename = "totalMs", skip_serializing_if = "Option::is_none")]
    pub total_ms: Option<f64>,
    #[serde(rename = "estimatedCostUsd", skip_serializing_if = "Option::is_none")]
    pub estimated_cost_usd: Option<f64>,
}

/// Upstream `EvalObservation.outcome` discriminator values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum EvalOutcome {
    #[serde(rename = "scored")]
    Scored,
    #[serde(rename = "unscored")]
    Unscored,
    #[serde(rename = "skipped")]
    Skipped,
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "errored")]
    Errored,
}

/// Upstream `EvalObservation`. Field order mirrors the upstream spreads:
/// identity, then metrics, then `outcome`, then `score` (scored only).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EvalObservation {
    #[serde(rename = "evalSet")]
    pub eval_set: String,
    #[serde(rename = "caseId")]
    pub case_id: String,
    pub variant: DocumentationVariant,
    pub model: String,
    #[serde(rename = "runNumber")]
    pub run_number: u32,
    #[serde(flatten)]
    pub metrics: EvalMetrics,
    pub outcome: EvalOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
}

/// Upstream `PairedMetricSummary`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PairedMetricSummary {
    #[serde(rename = "eligiblePairs")]
    pub eligible_pairs: u32,
    #[serde(rename = "controlMean")]
    pub control_mean: Option<f64>,
    #[serde(rename = "treatmentMean")]
    pub treatment_mean: Option<f64>,
    #[serde(rename = "meanDelta")]
    pub mean_delta: Option<f64>,
}

/// Upstream comparison flag union.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ComparisonFlag {
    #[serde(rename = "no-lift")]
    NoLift,
    #[serde(rename = "negative-delta")]
    NegativeDelta,
    #[serde(rename = "control-saturated")]
    ControlSaturated,
    #[serde(rename = "treatment-saturated")]
    TreatmentSaturated,
    #[serde(rename = "flaky")]
    Flaky,
}

/// Upstream `EvalSetComparison`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EvalSetComparison {
    #[serde(rename = "evalSet")]
    pub eval_set: String,
    #[serde(rename = "totalPairs")]
    pub total_pairs: u32,
    #[serde(rename = "eligiblePairs")]
    pub eligible_pairs: u32,
    #[serde(rename = "blockedPairs")]
    pub blocked_pairs: u32,
    #[serde(rename = "controlPassRate")]
    pub control_pass_rate: Option<f64>,
    #[serde(rename = "treatmentPassRate")]
    pub treatment_pass_rate: Option<f64>,
    pub lift: Option<f64>,
    pub flags: Vec<ComparisonFlag>,
    #[serde(rename = "totalTokens")]
    pub total_tokens: PairedMetricSummary,
    #[serde(rename = "toolCalls")]
    pub tool_calls: PairedMetricSummary,
    #[serde(rename = "totalMs")]
    pub total_ms: PairedMetricSummary,
    #[serde(rename = "estimatedCostUsd")]
    pub estimated_cost_usd: PairedMetricSummary,
}

/// Upstream `BlockedPair`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BlockedPair {
    #[serde(rename = "evalSet")]
    pub eval_set: String,
    #[serde(rename = "caseId")]
    pub case_id: String,
    pub model: String,
    #[serde(rename = "runNumber")]
    pub run_number: u32,
    pub reasons: Vec<String>,
}

/// Upstream `OperationalMetricTotal`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OperationalMetricTotal {
    #[serde(rename = "availableRuns")]
    pub available_runs: u32,
    pub total: Option<f64>,
}

/// Upstream `VariantTotals`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VariantTotals {
    pub variant: DocumentationVariant,
    pub runs: u32,
    #[serde(rename = "inputTokens")]
    pub input_tokens: OperationalMetricTotal,
    #[serde(rename = "outputTokens")]
    pub output_tokens: OperationalMetricTotal,
    #[serde(rename = "cacheReadTokens")]
    pub cache_read_tokens: OperationalMetricTotal,
    #[serde(rename = "cacheWriteTokens")]
    pub cache_write_tokens: OperationalMetricTotal,
    #[serde(rename = "totalTokens")]
    pub total_tokens: OperationalMetricTotal,
    #[serde(rename = "toolCalls")]
    pub tool_calls: OperationalMetricTotal,
    #[serde(rename = "totalMs")]
    pub total_ms: OperationalMetricTotal,
    #[serde(rename = "estimatedCostUsd")]
    pub estimated_cost_usd: OperationalMetricTotal,
}

/// Upstream `EvalComparisonReport`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EvalComparisonReport {
    #[serde(rename = "schemaVersion")]
    pub schema_version: u8,
    #[serde(rename = "protocolDigest")]
    pub protocol_digest: String,
    pub control: &'static str,
    pub treatment: &'static str,
    pub comparisons: Vec<EvalSetComparison>,
    #[serde(rename = "blockedPairs")]
    pub blocked_pairs: Vec<BlockedPair>,
    #[serde(rename = "operationalTotals")]
    pub operational_totals: Vec<VariantTotals>,
}

/// ECMAScript `Number.prototype.toFixed` for the digit counts this module
/// uses (0–4). Both JS `toFixed` and Rust's `{:.prec$}` round the exact
/// decimal expansion of the binary64 value, so the results agree.
fn js_to_fixed(value: f64, digits: usize) -> String {
    if value.is_nan() || value.is_infinite() {
        return js_number_string(value);
    }
    format!("{:.*}", digits, value)
}

/// Upstream `difference`: `Number((treatment - control).toPrecision(15))`.
fn difference(treatment: f64, control: f64) -> f64 {
    let raw = treatment - control;
    if !raw.is_finite() {
        return raw;
    }
    let formatted = format!("{:.*e}", 14, raw);
    formatted.parse::<f64>().unwrap_or(raw)
}

/// ECMAScript `Number::toString` (radix 10) via the shared port helper.
fn js_number(value: f64) -> String {
    js_number_string(value)
}

/// Upstream `optionalMetric`. `Err` carries the upstream `TypeError` message.
fn optional_metric(value: Option<f64>, name: &str) -> Result<Option<f64>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    if !value.is_finite() || value < 0.0 {
        return Err(format!("{name} must be a finite non-negative number."));
    }
    Ok(Some(value))
}

/// Upstream `validateScore`.
fn validate_score(value: Option<f64>) -> Result<Option<f64>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err("Eval score must be between 0 and 1.".to_string());
    }
    Ok(Some(value))
}

/// Upstream `classifyCaseStatus` over the vitest assertion status strings.
pub fn classify_case_status(status: &str) -> Option<EvalOutcome> {
    match status {
        "failed" => Some(EvalOutcome::Errored),
        "skipped" | "todo" | "disabled" => Some(EvalOutcome::Skipped),
        "pending" => Some(EvalOutcome::Pending),
        _ => None,
    }
}

/// Upstream `taskIdentity`.
fn task_identity(task: &EvalTask) -> EvalRunIdentity {
    EvalRunIdentity {
        eval_set: task.eval_set.clone(),
        case_id: task.case_id.clone(),
        variant: task.variant,
        model: task.model.clone(),
        run_number: task.run_number,
    }
}

/// Upstream `erroredObservation`.
pub fn errored_observation(task: &EvalTask) -> EvalObservation {
    let identity = task_identity(task);
    EvalObservation {
        eval_set: identity.eval_set,
        case_id: identity.case_id,
        variant: identity.variant,
        model: identity.model,
        run_number: identity.run_number,
        metrics: EvalMetrics::default(),
        outcome: EvalOutcome::Errored,
        score: None,
    }
}

/// Upstream `persistSession`: the session artifact lands under
/// `<artifactDirectory>/<variant>/sessions/<sha256(identity JSON)>/session.jsonl`.
fn persist_session(
    session: &str,
    identity: &EvalRunIdentity,
    artifact_directory: &std::path::Path,
) -> std::io::Result<()> {
    let identity_json = to_json_string_with_js_numbers(&serde_json::json!([
        identity.eval_set,
        identity.case_id,
        identity.variant.as_str(),
        identity.model,
        identity.run_number
    ]))
    .expect("array serializes");
    let digest = Sha256::digest(identity_json.as_bytes());
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    let directory = artifact_directory
        .join(identity.variant.as_str())
        .join("sessions")
        .join(hex);
    std::fs::create_dir_all(&directory)?;
    std::fs::write(directory.join("session.jsonl"), session)?;
    Ok(())
}

struct LoadedReport {
    cases: Vec<ReportCase>,
    assertions: Vec<RawAssertion>,
}

struct RawAssertion {
    full_name: String,
    status: String,
}

struct ReportCase {
    full_name: String,
    status: String,
    avg_score: Option<f64>,
    run: Option<HarnessRun>,
}

struct HarnessRun {
    usage: serde_json::Value,
    timings: serde_json::Value,
    errors: serde_json::Value,
    artifacts: serde_json::Value,
}

/// Seam S1: the vitest jest-format report plus the `meta`-lifted workspace
/// cases, read exactly as the upstream core helpers surface them.
fn read_report(report_path: &std::path::Path) -> Option<LoadedReport> {
    let text = std::fs::read_to_string(report_path).ok()?;
    let raw: serde_json::Value = serde_json::from_str(&text).ok()?;
    let test_results = raw.get("testResults")?.as_array()?;
    let mut assertions = Vec::new();
    let mut cases = Vec::new();
    for result in test_results {
        for assertion in result.get("assertionResults")?.as_array()? {
            let full_name = assertion.get("fullName")?.as_str()?.to_string();
            let status = assertion.get("status")?.as_str()?.to_string();
            let meta = assertion
                .get("meta")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            assertions.push(RawAssertion {
                full_name: full_name.clone(),
                status: status.clone(),
            });
            let run = meta.get("harness").and_then(|harness| harness.get("run"));
            cases.push(ReportCase {
                full_name,
                status,
                avg_score: meta
                    .get("eval")
                    .and_then(|eval| eval.get("avgScore"))
                    .and_then(|score| score.as_f64()),
                run: run.map(|run| HarnessRun {
                    usage: run.get("usage").cloned().unwrap_or(serde_json::Value::Null),
                    timings: run
                        .get("timings")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                    errors: run.get("errors").cloned().unwrap_or(serde_json::json!([])),
                    artifacts: run
                        .get("artifacts")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                }),
            });
        }
    }
    Some(LoadedReport { cases, assertions })
}

fn usage_number(usage: &serde_json::Value, key: &str) -> Option<f64> {
    usage.get(key).and_then(|value| value.as_f64())
}

/// Upstream `readTaskObservation`.
pub async fn read_task_observation(
    task: &EvalTask,
    report_path: &std::path::Path,
    artifact_directory: &std::path::Path,
) -> EvalObservation {
    let identity = task_identity(task);
    let observation =
        |outcome: EvalOutcome, metrics: EvalMetrics, score: Option<f64>| EvalObservation {
            eval_set: identity.eval_set.clone(),
            case_id: identity.case_id.clone(),
            variant: identity.variant,
            model: identity.model.clone(),
            run_number: identity.run_number,
            metrics,
            outcome,
            score,
        };
    let errored = |metrics: EvalMetrics| observation(EvalOutcome::Errored, metrics, None);
    let Some(loaded) = read_report(report_path) else {
        return errored(EvalMetrics::default());
    };
    let reported_full_name = format!("{} {}", task.eval_set, task.case_id);
    if loaded.assertions.len() != 1 {
        return errored(EvalMetrics::default());
    }
    let assertion = &loaded.assertions[0];
    if assertion.full_name != reported_full_name {
        return errored(EvalMetrics::default());
    }
    // Upstream `classifyCaseStatus` returns undefined for passed assertions
    // and execution falls through to the scored path; `None` mirrors that
    // undefined here.
    let status_outcome = classify_case_status(&assertion.status);
    if matches!(
        status_outcome,
        Some(EvalOutcome::Skipped) | Some(EvalOutcome::Pending)
    ) {
        return observation(status_outcome.unwrap(), EvalMetrics::default(), None);
    }
    if loaded.cases.len() != 1 {
        return errored(EvalMetrics::default());
    }
    let case_result = &loaded.cases[0];
    if case_result.full_name != reported_full_name {
        return errored(EvalMetrics::default());
    }
    if case_result.status != assertion.status {
        return errored(EvalMetrics::default());
    }
    let Some(run) = &case_result.run else {
        return errored(EvalMetrics::default());
    };
    if let Some(session) = run
        .artifacts
        .get(PI_SESSION_SNAPSHOT_ARTIFACT)
        .and_then(|value| value.as_str())
    {
        if persist_session(session, &identity, artifact_directory).is_err() {
            // Upstream awaits the write inside the happy path; a failing write
            // surfaces as a rejected `readTaskObservation` (the `.catch`
            // branch) and therefore an errored observation.
            return errored(EvalMetrics::default());
        }
    }
    let provider = run.usage.get("provider").and_then(|v| v.as_str());
    let model = run.usage.get("model").and_then(|v| v.as_str());
    let actual_model = match (provider, model) {
        (Some(provider), Some(model)) => Some(format!("{provider}/{model}")),
        _ => None,
    };
    if actual_model.as_deref() != Some(task.model.as_str()) {
        return errored(EvalMetrics::default());
    }
    let metrics = || -> Result<EvalMetrics, String> {
        Ok(EvalMetrics {
            input_tokens: optional_metric(usage_number(&run.usage, "inputTokens"), "inputTokens")?,
            output_tokens: optional_metric(
                usage_number(&run.usage, "outputTokens"),
                "outputTokens",
            )?,
            cache_read_tokens: optional_metric(
                run.usage
                    .get("metadata")
                    .and_then(|m| m.get("cacheReadTokens"))
                    .and_then(|v| v.as_f64()),
                "cacheReadTokens",
            )?,
            cache_write_tokens: optional_metric(
                run.usage
                    .get("metadata")
                    .and_then(|m| m.get("cacheWriteTokens"))
                    .and_then(|v| v.as_f64()),
                "cacheWriteTokens",
            )?,
            total_tokens: optional_metric(usage_number(&run.usage, "totalTokens"), "totalTokens")?,
            tool_calls: optional_metric(usage_number(&run.usage, "toolCalls"), "toolCalls")?,
            total_ms: optional_metric(
                run.timings.get("totalMs").and_then(|v| v.as_f64()),
                "totalMs",
            )?,
            estimated_cost_usd: optional_metric(
                run.usage
                    .get("metadata")
                    .and_then(|m| m.get("estimatedCostUsd"))
                    .and_then(|v| v.as_f64()),
                "estimatedCostUsd",
            )?,
        })
    }();
    let Ok(metrics) = metrics else {
        return errored(EvalMetrics::default());
    };
    let run_errors = run.errors.as_array().map(Vec::as_slice).unwrap_or(&[]);
    if status_outcome == Some(EvalOutcome::Errored) || !run_errors.is_empty() {
        return errored(metrics);
    }
    let score = match validate_score(case_result.avg_score) {
        Ok(score) => score,
        Err(_) => return observation(EvalOutcome::Errored, metrics, None),
    };
    match score {
        None => observation(EvalOutcome::Unscored, metrics, None),
        Some(score) => observation(EvalOutcome::Scored, metrics, Some(score)),
    }
}

const CONTROL: DocumentationVariant = DocumentationVariant::WithoutDocs;
const TREATMENT: DocumentationVariant = DocumentationVariant::WithDocs;

struct Pair {
    control: EvalObservation,
    treatment: EvalObservation,
}

struct PairGroup {
    eval_set: String,
    case_id: String,
    model: String,
    run_number: u32,
    expected: std::collections::HashMap<DocumentationVariant, u32>,
    observations: std::collections::HashMap<DocumentationVariant, Vec<EvalObservation>>,
}

fn mean(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    Some(values.iter().sum::<f64>() / values.len() as f64)
}

/// Upstream `pairKey`: JSON of `[evalSet, caseId, model, runNumber]`.
fn pair_key(eval_set: &str, case_id: &str, model: &str, run_number: u32) -> String {
    to_json_string_with_js_numbers(&serde_json::json!([eval_set, case_id, model, run_number]))
        .expect("array serializes")
}

fn group_pairs(
    expected_runs: &[ExpectedEvalRun],
    observations: &[EvalObservation],
) -> Vec<PairGroup> {
    fn identity_key(eval_set: &str, case_id: &str, model: &str, run_number: u32) -> String {
        pair_key(eval_set, case_id, model, run_number)
    }
    fn get_group<'a>(
        groups: &'a mut std::collections::HashMap<String, PairGroup>,
        identity: &EvalRunIdentity,
    ) -> &'a mut PairGroup {
        let key = identity_key(
            &identity.eval_set,
            &identity.case_id,
            &identity.model,
            identity.run_number,
        );
        groups.entry(key).or_insert_with(|| PairGroup {
            eval_set: identity.eval_set.clone(),
            case_id: identity.case_id.clone(),
            model: identity.model.clone(),
            run_number: identity.run_number,
            expected: std::collections::HashMap::new(),
            observations: std::collections::HashMap::new(),
        })
    }
    let mut groups: std::collections::HashMap<String, PairGroup> = std::collections::HashMap::new();
    for expected in expected_runs {
        let group = get_group(&mut groups, expected);
        *group.expected.entry(expected.variant).or_insert(0) += 1;
    }
    for observation in observations {
        let identity = EvalRunIdentity {
            eval_set: observation.eval_set.clone(),
            case_id: observation.case_id.clone(),
            variant: observation.variant,
            model: observation.model.clone(),
            run_number: observation.run_number,
        };
        get_group(&mut groups, &identity)
            .observations
            .entry(observation.variant)
            .or_default()
            .push(observation.clone());
    }
    let mut sorted: Vec<PairGroup> = groups.into_values().collect();
    sorted.sort_by(|left, right| {
        left.eval_set
            .cmp(&right.eval_set)
            .then(left.case_id.cmp(&right.case_id))
            .then(left.model.cmp(&right.model))
            .then(left.run_number.cmp(&right.run_number))
    });
    sorted
}

/// Upstream `groupPairs` sorts with `localeCompare`; the port uses plain
/// byte-wise `cmp`, which matches for the ASCII identifiers eval sets use.
fn resolve_pair(group: &PairGroup) -> (Option<Pair>, Option<BlockedPair>) {
    let mut reasons: Vec<String> = Vec::new();
    for variant in [CONTROL, TREATMENT] {
        let expected = group.expected.get(&variant).copied().unwrap_or(0);
        let observed = group
            .observations
            .get(&variant)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        if expected != 1 {
            reasons.push(format!(
                "{}: design expected 1 run, found {expected}",
                variant.as_str()
            ));
        }
        if observed.len() as u32 != expected {
            reasons.push(format!(
                "{}: expected {expected} observation{}, found {}",
                variant.as_str(),
                if expected == 1 { "" } else { "s" },
                observed.len()
            ));
        }
        if expected == 1 && observed.len() == 1 {
            let outcome = match observed[0].outcome {
                EvalOutcome::Scored => None,
                EvalOutcome::Unscored => Some("unscored"),
                EvalOutcome::Skipped => Some("skipped"),
                EvalOutcome::Pending => Some("pending"),
                EvalOutcome::Errored => Some("errored"),
            };
            if let Some(outcome) = outcome {
                reasons.push(format!("{}: {outcome}", variant.as_str()));
            }
        }
    }
    if !reasons.is_empty() {
        return (
            None,
            Some(BlockedPair {
                eval_set: group.eval_set.clone(),
                case_id: group.case_id.clone(),
                model: group.model.clone(),
                run_number: group.run_number,
                reasons,
            }),
        );
    }
    let control = group.observations.get(&CONTROL).expect("expected 1")[0].clone();
    let treatment = group.observations.get(&TREATMENT).expect("expected 1")[0].clone();
    (Some(Pair { control, treatment }), None)
}

fn summarize_metric(
    pairs: &[Pair],
    select: impl Fn(&EvalObservation) -> Option<f64>,
) -> PairedMetricSummary {
    let mut control = Vec::new();
    let mut treatment = Vec::new();
    for pair in pairs {
        let (Some(control_value), Some(treatment_value)) =
            (select(&pair.control), select(&pair.treatment))
        else {
            continue;
        };
        control.push(control_value);
        treatment.push(treatment_value);
    }
    let control_mean = mean(&control);
    let treatment_mean = mean(&treatment);
    PairedMetricSummary {
        eligible_pairs: control.len() as u32,
        control_mean,
        treatment_mean,
        mean_delta: match (control_mean, treatment_mean) {
            (Some(control_mean), Some(treatment_mean)) => {
                Some(difference(treatment_mean, control_mean))
            }
            _ => None,
        },
    }
}

fn operational_total(
    runs: &[EvalObservation],
    select: impl Fn(&EvalObservation) -> Option<f64>,
) -> OperationalMetricTotal {
    let values: Vec<f64> = runs.iter().filter_map(select).collect();
    OperationalMetricTotal {
        available_runs: values.len() as u32,
        total: if values.is_empty() {
            None
        } else {
            Some(values.iter().sum())
        },
    }
}

fn variant_totals(
    observations: &[EvalObservation],
    variant: DocumentationVariant,
) -> VariantTotals {
    let runs: Vec<EvalObservation> = observations
        .iter()
        .filter(|observation| observation.variant == variant)
        .cloned()
        .collect();
    let select_all = |observation: &EvalObservation| observation.metrics.clone();
    let field = |metrics: &EvalMetrics, pick: fn(&EvalMetrics) -> Option<f64>| pick(metrics);
    let metric_total = |pick: fn(&EvalMetrics) -> Option<f64>| {
        operational_total(&runs, |observation| field(&select_all(observation), pick))
    };
    VariantTotals {
        variant,
        runs: runs.len() as u32,
        input_tokens: metric_total(|m| m.input_tokens),
        output_tokens: metric_total(|m| m.output_tokens),
        cache_read_tokens: metric_total(|m| m.cache_read_tokens),
        cache_write_tokens: metric_total(|m| m.cache_write_tokens),
        total_tokens: metric_total(|m| m.total_tokens),
        tool_calls: metric_total(|m| m.tool_calls),
        total_ms: metric_total(|m| m.total_ms),
        estimated_cost_usd: metric_total(|m| m.estimated_cost_usd),
    }
}

fn comparison_flags(
    pairs: &[Pair],
    control_pass_rate: Option<f64>,
    treatment_pass_rate: Option<f64>,
) -> Vec<ComparisonFlag> {
    let mut flags = Vec::new();
    if let (Some(control), Some(treatment)) = (control_pass_rate, treatment_pass_rate) {
        if control == treatment {
            flags.push(ComparisonFlag::NoLift);
        }
        if treatment < control {
            flags.push(ComparisonFlag::NegativeDelta);
        }
        if control == 1.0 {
            flags.push(ComparisonFlag::ControlSaturated);
        }
        if treatment == 1.0 {
            flags.push(ComparisonFlag::TreatmentSaturated);
        }
    }
    let mut outcomes: std::collections::HashMap<String, std::collections::BTreeSet<String>> =
        std::collections::HashMap::new();
    for pair in pairs {
        let mut record = |case_id: &str, variant: DocumentationVariant, score: f64| {
            let key =
                to_json_string_with_js_numbers(&serde_json::json!([case_id, variant.as_str()]))
                    .expect("array serializes");
            // Upstream records `String(score >= 1)`, i.e. "true"/"false".
            outcomes
                .entry(key)
                .or_default()
                .insert((score >= 1.0).to_string());
        };
        record(
            &pair.control.case_id,
            pair.control.variant,
            pair.control.score.unwrap_or(f64::NEG_INFINITY),
        );
        record(
            &pair.treatment.case_id,
            pair.treatment.variant,
            pair.treatment.score.unwrap_or(f64::NEG_INFINITY),
        );
    }
    if outcomes.values().any(|values| values.len() > 1) {
        flags.push(ComparisonFlag::Flaky);
    }
    flags
}

/// Upstream `summarizeEvalObservations`.
pub fn summarize_eval_observations(
    protocol_digest: &str,
    expected_runs: &[ExpectedEvalRun],
    observations: &[EvalObservation],
) -> EvalComparisonReport {
    let groups = group_pairs(expected_runs, observations);
    let mut blocked_pairs: Vec<BlockedPair> = Vec::new();
    let mut pairs_by_eval_set: std::collections::HashMap<String, Vec<Pair>> =
        std::collections::HashMap::new();
    // Upstream relies on Map insertion order and then sorts by eval set; the
    // port sorts the collected set directly, which yields the same order.
    let mut totals_by_eval_set: std::collections::BTreeMap<String, u32> =
        std::collections::BTreeMap::new();
    for group in &groups {
        *totals_by_eval_set
            .entry(group.eval_set.clone())
            .or_insert(0) += 1;
        let (pair, blocked) = resolve_pair(group);
        if let Some(blocked) = blocked {
            blocked_pairs.push(blocked);
        }
        if let Some(pair) = pair {
            pairs_by_eval_set
                .entry(group.eval_set.clone())
                .or_default()
                .push(pair);
        }
    }
    let comparisons = totals_by_eval_set
        .iter()
        .map(|(eval_set, total_pairs)| {
            let pairs: &[Pair] = pairs_by_eval_set
                .get(eval_set)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let blocked_pair_count = *total_pairs - pairs.len() as u32;
            let publish_headline = blocked_pair_count == 0 && !pairs.is_empty();
            let pass_rate = |side: fn(&Pair) -> &EvalObservation| -> Option<f64> {
                if !publish_headline {
                    return None;
                }
                let passes = pairs
                    .iter()
                    .filter(|pair| side(pair).score.unwrap_or(0.0) >= 1.0)
                    .count();
                Some(passes as f64 / pairs.len() as f64)
            };
            let control_pass_rate = pass_rate(|pair| &pair.control);
            let treatment_pass_rate = pass_rate(|pair| &pair.treatment);
            EvalSetComparison {
                eval_set: eval_set.clone(),
                total_pairs: *total_pairs,
                eligible_pairs: pairs.len() as u32,
                blocked_pairs: blocked_pair_count,
                control_pass_rate,
                treatment_pass_rate,
                lift: match (control_pass_rate, treatment_pass_rate) {
                    (Some(control), Some(treatment)) => Some(difference(treatment, control)),
                    _ => None,
                },
                flags: comparison_flags(pairs, control_pass_rate, treatment_pass_rate),
                total_tokens: summarize_metric(pairs, |observation| {
                    observation.metrics.total_tokens
                }),
                tool_calls: summarize_metric(pairs, |observation| observation.metrics.tool_calls),
                total_ms: summarize_metric(pairs, |observation| observation.metrics.total_ms),
                estimated_cost_usd: summarize_metric(pairs, |observation| {
                    observation.metrics.estimated_cost_usd
                }),
            }
        })
        .collect();
    EvalComparisonReport {
        schema_version: 3,
        protocol_digest: protocol_digest.to_string(),
        control: CONTROL.as_str(),
        treatment: TREATMENT.as_str(),
        comparisons,
        blocked_pairs,
        operational_totals: vec![
            variant_totals(observations, CONTROL),
            variant_totals(observations, TREATMENT),
        ],
    }
}

/// Upstream `percentage`.
fn percentage(value: Option<f64>) -> String {
    match value {
        None => "unavailable".to_string(),
        Some(value) => format!("{:.1}%", value * 100.0),
    }
}

/// Upstream `signed`.
fn signed(value: f64, digits: usize) -> String {
    format!(
        "{}{:.*}",
        if value >= 0.0 { "+" } else { "" },
        digits,
        value
    )
}

/// Upstream `pairedMetric`.
fn paired_metric(label: &str, metric: &PairedMetricSummary, unit: &str) -> String {
    if let (Some(mean_delta), Some(control_mean), Some(treatment_mean)) = (
        metric.mean_delta,
        metric.control_mean,
        metric.treatment_mean,
    ) {
        format!(
            "    {:>10}  {}{} (with {}{}, without {}{}, {} pairs)",
            label,
            signed(mean_delta, 1),
            unit,
            js_to_fixed(treatment_mean, 1),
            unit,
            js_to_fixed(control_mean, 1),
            unit,
            metric.eligible_pairs
        )
    } else {
        format!("    {:>10}  unavailable", label)
    }
}

/// Upstream `operationalMetric`.
fn operational_metric(
    metric: &OperationalMetricTotal,
    runs: u32,
    format_total: impl Fn(f64) -> String,
) -> String {
    let Some(total) = metric.total else {
        return format!("unavailable (0/{runs} measured)");
    };
    let coverage = if metric.available_runs == runs {
        String::new()
    } else {
        format!(" ({}/{runs} measured)", metric.available_runs)
    };
    format!("{0}{coverage}", format_total(total))
}

/// Upstream `formatEvalComparisonReport` (`styleText("bold", ...)` renders as
/// the SGR bold sequence `\x1b[1m...\x1b[22m`).
pub fn format_eval_comparison_report(report: &EvalComparisonReport) -> String {
    if report.comparisons.is_empty() {
        return String::new();
    }
    // Upstream `styleText("bold", ...)` emits SGR only when the target
    // stream supports color (node disables styling for non-TTY output).
    let heading = "Documentation Eval Comparisons";
    let bold = if std::io::IsTerminal::is_terminal(&std::io::stdout()) {
        format!("[1m{heading}[22m")
    } else {
        heading.to_string()
    };
    let mut lines = vec![bold];
    for comparison in &report.comparisons {
        lines.push(format!("  {}", comparison.eval_set));
        lines.push(format!(
            "         Pairs  {}/{} eligible",
            comparison.eligible_pairs, comparison.total_pairs
        ));
        if comparison.lift.is_none() {
            lines.push(
                if comparison.blocked_pairs > 0 {
                    "     Pass rate  withheld because pairs are blocked"
                } else {
                    "     Pass rate  unavailable"
                }
                .to_string(),
            );
        } else {
            lines.push(format!(
                "     Pass rate  {} pp (with {}, without {})",
                signed(comparison.lift.unwrap_or(0.0) * 100.0, 1),
                percentage(comparison.treatment_pass_rate),
                percentage(comparison.control_pass_rate),
            ));
        }
        if !comparison.flags.is_empty() {
            let flags: Vec<&'static str> = comparison
                .flags
                .iter()
                .map(|flag| match flag {
                    ComparisonFlag::NoLift => "no-lift",
                    ComparisonFlag::NegativeDelta => "negative-delta",
                    ComparisonFlag::ControlSaturated => "control-saturated",
                    ComparisonFlag::TreatmentSaturated => "treatment-saturated",
                    ComparisonFlag::Flaky => "flaky",
                })
                .collect();
            lines.push(format!("         Flags  {}", flags.join(", ")));
        }
        lines.push(paired_metric("Tokens", &comparison.total_tokens, ""));
        lines.push(paired_metric("Tools", &comparison.tool_calls, ""));
        lines.push(paired_metric("Latency", &comparison.total_ms, "ms"));
        let cost = &comparison.estimated_cost_usd;
        if let (Some(mean_delta), Some(control_mean), Some(treatment_mean)) =
            (cost.mean_delta, cost.control_mean, cost.treatment_mean)
        {
            lines.push(format!(
                "     Est. cost  {}${:.4} (with ${:.4}, without ${:.4}, {} pairs)",
                if mean_delta >= 0.0 { "+" } else { "-" },
                mean_delta.abs(),
                treatment_mean,
                control_mean,
                cost.eligible_pairs,
            ));
        } else {
            lines.push("     Est. cost  unavailable".to_string());
        }
    }
    lines.push("  Operational totals".to_string());
    for totals in &report.operational_totals {
        let tokens = operational_metric(&totals.total_tokens, totals.runs, |total| {
            format!("{} tokens", js_number(total))
        });
        let tools = operational_metric(&totals.tool_calls, totals.runs, |total| {
            format!("{} tools", js_number(total))
        });
        let latency = operational_metric(&totals.total_ms, totals.runs, |total| {
            format!("{:.2}s", total / 1000.0)
        });
        let cost = operational_metric(&totals.estimated_cost_usd, totals.runs, |total| {
            format!("${:.4} cost", total)
        });
        lines.push(format!(
            "    {}: {} runs, {}, {}, {}, {}",
            totals.variant.as_str(),
            totals.runs,
            tokens,
            tools,
            latency,
            cost
        ));
    }
    if !report.blocked_pairs.is_empty() {
        lines.push("  Blocked pairs".to_string());
        for blocked in &report.blocked_pairs {
            lines.push(format!(
                "    {}/{}/{}/run-{}: {}",
                blocked.eval_set,
                blocked.case_id,
                blocked.model,
                blocked.run_number,
                blocked.reasons.join("; ")
            ));
        }
    }
    lines.join("\n")
}

#[cfg(test)]
#[path = "report_tests.rs"]
mod tests;
