//! Port of `pi/packages/evals/src/cli.ts` — the documentation-eval runner
//! entry point (argument parsing, discovery comparison, protocol artifact).
//!
//! The upstream file is a top-level script; the port moves the orchestration
//! into [`run_documentation_eval`] with identical sequencing. Live docker and
//! discovery surfaces delegate to [`crate::evals::docker`].

use crate::evals::docker;
use crate::evals::plan::{
    create_task_plan, parse_discovered_cases, DiscoveredEvalCase, EvalTask, DOCUMENTATION_VARIANTS,
};
use crate::evals::report::{
    errored_observation, format_eval_comparison_report, read_task_observation,
    summarize_eval_observations, EvalObservation,
};
use crate::serde_support::to_json_string_with_js_numbers;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Upstream `EvalCliOptions`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EvalCliOptions {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub runs_per_variant: u32,
    pub requested_files: Vec<String>,
    pub discovery_args: Vec<String>,
}

const RUNNER_OPTIONS: [&str; 3] = ["--provider", "--model", "--runs-per-variant"];
const FILTER_OPTIONS: [&str; 2] = ["-t", "--testNamePattern"];

fn is_safe_integer_runs(value: f64) -> bool {
    value.fract() == 0.0 && value.abs() <= 9007199254740991.0
}

/// Upstream `parseEvalCli`.
pub fn parse_eval_cli(
    args: &[String],
    environment: &BTreeMap<String, String>,
) -> Result<EvalCliOptions, String> {
    let mut provider: Option<String> = None;
    let mut model: Option<String> = None;
    let mut runs_text: Option<String> = None;
    let mut cli_selected_model = false;
    let mut requested_files: Vec<String> = Vec::new();
    let mut discovery_args: Vec<String> = Vec::new();

    let mut index = 0;
    while index < args.len() {
        let argument = &args[index];
        if argument.ends_with(".docs.eval.ts") {
            requested_files.push(argument.clone());
            index += 1;
            continue;
        }
        if FILTER_OPTIONS.contains(&argument.as_str()) {
            let value = args.get(index + 1);
            let Some(value) = value.filter(|value| !value.is_empty()) else {
                return Err(format!("Missing value for {argument}."));
            };
            discovery_args.push(argument.clone());
            discovery_args.push(value.clone());
            index += 2;
            continue;
        }
        if let Some(stripped) = argument.strip_prefix("--testNamePattern=") {
            if stripped.is_empty() {
                return Err("Missing value for --testNamePattern.".to_string());
            }
            discovery_args.push(argument.clone());
            index += 1;
            continue;
        }

        let equals = argument.find('=');
        let (name, inline_value) = match equals {
            Some(equals) => (
                &argument[..equals],
                Some(argument[equals + 1..].to_string()),
            ),
            None => (argument.as_str(), None),
        };
        if RUNNER_OPTIONS.contains(&name) {
            let had_inline = inline_value.is_some();
            let value = match inline_value {
                Some(value) => Some(value),
                None => args.get(index + 1).cloned(),
            };
            let Some(value) = value.filter(|value| !value.is_empty()) else {
                return Err(format!("Missing value for {name}."));
            };
            if !had_inline && value.starts_with('-') {
                return Err(format!("Missing value for {name}."));
            }
            match name {
                "--provider" => provider = Some(value),
                "--model" => model = Some(value),
                _ => runs_text = Some(value),
            }
            if name != "--runs-per-variant" {
                cli_selected_model = true;
            }
            if !had_inline {
                index += 1;
            }
            index += 1;
            continue;
        }
        return Err(format!("Unsupported eval argument: {argument}"));
    }

    provider = provider
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    model = model
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    if cli_selected_model {
        if provider.is_none() || model.is_none() {
            return Err("CLI model selection requires both --provider and --model.".to_string());
        }
    } else {
        provider = environment
            .get("PI_PROVIDER")
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        model = environment
            .get("PI_MODEL")
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        if provider.is_some() != model.is_some() {
            return Err("Set both PI_PROVIDER and PI_MODEL, or neither.".to_string());
        }
    }

    let configured_runs = runs_text
        .or_else(|| environment.get("PI_EVAL_RUNS_PER_VARIANT").cloned())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let runs_per_variant = match configured_runs {
        Some(text) => {
            let parsed: f64 = text
                .parse()
                .map_err(|_| "Runs per variant must be a positive integer.".to_string())?;
            if !is_safe_integer_runs(parsed) || parsed < 1.0 {
                return Err("Runs per variant must be a positive integer.".to_string());
            }
            parsed as u32
        }
        None => 1,
    };

    Ok(EvalCliOptions {
        provider,
        model,
        runs_per_variant,
        requested_files,
        discovery_args,
    })
}

/// Upstream `containerPath`: resolves against the package root and rejects
/// escapes, returning forward-slash package-relative paths.
pub fn container_path(path: &str) -> Result<String, String> {
    let package_root = docker::package_root();
    let absolute = if std::path::Path::new(path).is_absolute() {
        std::path::PathBuf::from(path)
    } else {
        package_root.join(path)
    };
    let absolute = absolute.canonicalize().unwrap_or(absolute);
    let package_relative = absolute
        .strip_prefix(&package_root)
        .map_err(|_| {
            format!(
                "Eval file must be inside {}: {path}",
                package_root.to_string_lossy()
            )
        })?
        .to_string_lossy()
        .replace('\\', "/");
    if package_relative.starts_with("..") {
        return Err(format!(
            "Eval file must be inside {}: {path}",
            package_root.to_string_lossy()
        ));
    }
    Ok(package_relative)
}

/// Upstream `normalizeDiscoveredFile`.
pub fn normalize_discovered_file(path: &str) -> Result<String, String> {
    let prefix = "/repo/packages/evals/";
    let Some(stripped) = path.strip_prefix(prefix) else {
        return Err(format!(
            "Discovered eval path is outside the container package: {path}"
        ));
    };
    Ok(stripped.to_string())
}

/// Upstream `compareDiscovery`.
pub fn compare_discovery(
    left: &[DiscoveredEvalCase],
    right: &[DiscoveredEvalCase],
) -> Result<(), String> {
    let identities = |cases: &[DiscoveredEvalCase]| -> Vec<String> {
        let mut items: Vec<String> = cases
            .iter()
            .map(|eval_case| {
                to_json_string_with_js_numbers(&serde_json::json!([
                    eval_case.full_name,
                    normalize_discovered_file(&eval_case.file).expect("normalized file")
                ]))
                .expect("array serializes")
            })
            .collect();
        items.sort();
        items
    };
    if serde_json::to_string(&identities(left)).unwrap()
        != serde_json::to_string(&identities(right)).unwrap()
    {
        return Err("Documentation variants discovered different eval cases.".to_string());
    }
    Ok(())
}

/// Upstream `artifactRunId` (`ISO timestamp with ':' → '-'` + UUID v4). The
/// UUID is synthesized from the clock and process id (no `rand` dependency on
/// this surface).
pub fn artifact_run_id(now: std::time::SystemTime) -> String {
    let mut stamp = humantime_timestamp(now);
    stamp = stamp.replace(':', "-");
    let mut bytes = [0u8; 16];
    let nanos = now
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    bytes[..8].copy_from_slice(&nanos.to_be_bytes());
    bytes[8..12].copy_from_slice(&(std::process::id()).to_be_bytes());
    let pid_nanos = nanos ^ ((std::process::id() as u128) << 64);
    bytes[12..].copy_from_slice(&(pid_nanos as u32).to_be_bytes());
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}_{}-{}-{}-{}-{}",
        stamp,
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

pub fn humantime_timestamp(now: std::time::SystemTime) -> String {
    // Minimal ISO-8601 UTC formatting (upstream `new Date().toISOString()`).
    let seconds = now
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs();
    let days = seconds / 86_400;
    let time = seconds % 86_400;
    let (hour, minute, second) = (time / 3600, (time % 3600) / 60, time % 60);
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{:02}-{:02}T{hour:02}:{minute:02}:{second:02}.000Z",
        m, d
    )
}

/// Upstream protocol JSON document (written `protocol.json` with the digest).
pub struct ProtocolPlan {
    pub model_identity: String,
    pub files: Vec<String>,
    pub cases: Vec<DiscoveredEvalCase>,
    pub tasks: Vec<EvalTask>,
}

/// Upstream protocol assembly + digest: compact `JSON.stringify` of the
/// protocol object (without the digest), sha256-hex.
pub fn protocol_digest(
    plan: &ProtocolPlan,
    runs_per_variant: u32,
    images: &docker::BuiltImages,
) -> String {
    let protocol = serde_json::json!({
        "schemaVersion": 1,
        "model": plan.model_identity,
        "runsPerVariant": runs_per_variant,
        "images": {
            "without_docs": { "name": images.without_docs.name, "id": images.without_docs.id },
            "with_docs": { "name": images.with_docs.name, "id": images.with_docs.id },
        },
        "files": plan.files,
        "cases": plan.cases.iter().map(|eval_case| serde_json::json!({
            "evalSet": eval_case.eval_set,
            "caseId": eval_case.case_id,
            "file": eval_case.file,
        })).collect::<Vec<_>>(),
        "tasks": plan.tasks,
    });
    let text = to_json_string_with_js_numbers(&protocol).expect("protocol serializes");
    let digest = Sha256::digest(text.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Upstream the driver loop after discovery: runs every planned task through
/// docker, records observations and writes the comparison report. Returns the
/// rendered report text (and whether any pairs were blocked).
pub async fn run_documentation_eval(
    cli: &EvalCliOptions,
    cases: &[DiscoveredEvalCase],
    context: &docker::DockerContext,
    artifact_directory: &std::path::Path,
) -> Result<(String, bool), String> {
    let model_identity = format!(
        "{}/{}",
        cli.provider.clone().unwrap_or_default(),
        cli.model.clone().unwrap_or_default()
    );
    let mut normalized: Vec<DiscoveredEvalCase> = Vec::with_capacity(cases.len());
    for eval_case in cases {
        normalized.push(DiscoveredEvalCase {
            file: normalize_discovered_file(&eval_case.file)?,
            ..eval_case.clone()
        });
    }
    if normalized.is_empty() {
        return Err("No documentation eval cases matched the selection.".to_string());
    }
    let tasks = create_task_plan(&normalized, &model_identity, cli.runs_per_variant)?;

    let mut observations: Vec<EvalObservation> = Vec::new();
    for task in &tasks {
        let observation = match docker::run_task(context, task)? {
            Some(report_path) => {
                read_task_observation(task, &report_path, artifact_directory).await
            }
            None => errored_observation(task),
        };
        observations.push(observation);
    }

    let report = summarize_eval_observations(
        &protocol_digest(
            &ProtocolPlan {
                model_identity,
                files: Vec::new(),
                cases: normalized,
                tasks: tasks.clone(),
            },
            cli.runs_per_variant,
            &context.images,
        ),
        &tasks
            .iter()
            .map(|task| crate::evals::report::EvalRunIdentity {
                eval_set: task.eval_set.clone(),
                case_id: task.case_id.clone(),
                variant: task.variant,
                model: task.model.clone(),
                run_number: task.run_number,
            })
            .collect::<Vec<_>>(),
        &observations,
    );
    let report_text = format_eval_comparison_report(&report);
    Ok((report_text, !report.blocked_pairs.is_empty()))
}

/// Upstream guard: both discovery passes must select the same images set.
pub fn ensure_distinct_images(images: &docker::BuiltImages) -> Result<(), String> {
    if images.with_docs.id == images.without_docs.id {
        return Err("Documentation variants resolved to the same image.".to_string());
    }
    Ok(())
}

/// Upstream file selection: requested `.docs.eval.ts` files or the default
/// glob, normalized and sorted; empty selections are rejected and
/// non-documentation evals never reach the runner.
pub fn select_eval_files(cli: &EvalCliOptions) -> Result<Vec<String>, String> {
    let mut files: Vec<String> = if cli.requested_files.is_empty() {
        glob_docs_evals()?
    } else {
        cli.requested_files.clone()
    }
    .into_iter()
    .map(|file| container_path(&file))
    .collect::<Result<Vec<_>, _>>()?;
    files.sort();
    if files.is_empty() {
        return Err("No documentation eval files were selected.".to_string());
    }
    for file in &files {
        if !file.ends_with(".docs.eval.ts") {
            return Err(format!(
                "Documentation runner cannot execute non-doc eval: {file}"
            ));
        }
    }
    Ok(files)
}

fn glob_docs_evals() -> Result<Vec<String>, String> {
    let root = docker::package_root().join("evals");
    let mut found = Vec::new();
    let mut stack = vec![root];
    while let Some(directory) = stack.pop() {
        let entries = std::fs::read_dir(&directory).map_err(|error| error.to_string())?;
        for entry in entries {
            let entry = entry.map_err(|error| error.to_string())?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path
                .file_name()
                .map(|name| name.to_string_lossy().ends_with(".docs.eval.ts"))
                .unwrap_or(false)
            {
                found.push(path.to_string_lossy().to_string());
            }
        }
    }
    Ok(found)
}

/// Upstream discovery comparison across the two documentation variants.
pub async fn discover_both_variants(
    context: &docker::DockerContext,
    files: &[String],
    discovery_args: &[String],
) -> Result<Vec<DiscoveredEvalCase>, String> {
    let mut discoveries: Vec<Vec<DiscoveredEvalCase>> = Vec::new();
    for variant in DOCUMENTATION_VARIANTS {
        let path = docker::discover_cases(context, variant, files, discovery_args)?;
        let text = std::fs::read_to_string(&path).map_err(|error| error.to_string())?;
        let parsed: serde_json::Value =
            serde_json::from_str(&text).map_err(|error| error.to_string())?;
        discoveries.push(parse_discovered_cases(&parsed)?);
    }
    compare_discovery(&discoveries[0], &discoveries[1])?;
    Ok(discoveries.remove(0))
}

#[cfg(test)]
#[path = "cli_tests.rs"]
mod tests;
