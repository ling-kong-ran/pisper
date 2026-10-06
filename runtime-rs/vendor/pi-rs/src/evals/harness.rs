//! Port of `pi/packages/evals/src/harness.ts`.
//!
//! Deterministic helpers (model selection, environment isolation, sandbox
//! identity, prompt verification and documentation stripping) are ported
//! verbatim. The live agent run (`runPiCodingAgent`: `AgentSession`,
//! `createAgentSessionServices`, inline extensions, POSIX sandbox entry) is
//! seam S2: it is represented by the [`AgentRunner`] closure so the eval
//! driver can inject either the coding-agent surface or a fixture.

use crate::evals::plan::{parse_documentation_variant, DocumentationVariant};
use std::collections::BTreeMap;

/// Upstream `PiCodingAgentModelSelection`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSelection {
    pub provider: String,
    pub id: String,
}

/// Upstream `PiCodingAgentHarnessOptions` (the deterministic surface).
#[derive(Debug, Clone, Default)]
pub struct HarnessOptions {
    pub name: Option<String>,
    pub model: Option<ModelSelection>,
    pub workspace_files: BTreeMap<String, String>,
    pub expected_pi_documentation: Option<bool>,
}

/// Upstream `resolveModelSelection` over an environment snapshot.
pub fn resolve_model_selection(
    explicit_model: Option<ModelSelection>,
    environment: &BTreeMap<String, String>,
) -> Result<ModelSelection, String> {
    let provider = explicit_model
        .as_ref()
        .map(|model| model.provider.clone())
        .or_else(|| environment.get("PI_PROVIDER").cloned())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let id = explicit_model
        .map(|model| model.id)
        .or_else(|| environment.get("PI_MODEL").cloned())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    match (provider, id) {
        (Some(provider), Some(id)) => Ok(ModelSelection { provider, id }),
        _ => Err(
            "Select a harness model explicitly or set both PI_PROVIDER and PI_MODEL as defaults."
                .to_string(),
        ),
    }
}

/// Serializes tests that mutate the process environment (env vars are
/// process-global; upstream vitest isolates per-worker).
#[cfg(test)]
pub(crate) static ENV_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Reads the current process environment into the snapshot shape
/// [`resolve_model_selection`] and [`apply_isolated_environment`] consume.
pub fn process_environment() -> BTreeMap<String, String> {
    std::env::vars().collect()
}

/// Upstream `applyIsolatedEnvironment`: removes `PI_EVAL_*` variables, sets
/// the isolated `HOME`/`USERPROFILE`/`PI_CODING_AGENT_DIR`, and returns the
/// restore closure.
pub fn apply_isolated_environment(home: &str, agent_dir: &str) -> impl FnOnce() + Send {
    let overrides = [
        ("HOME".to_string(), home.to_string()),
        ("USERPROFILE".to_string(), home.to_string()),
        ("PI_CODING_AGENT_DIR".to_string(), agent_dir.to_string()),
    ];
    let mut previous: Vec<(String, Option<String>)> = Vec::new();
    for (name, value) in std::env::vars() {
        if name.starts_with("PI_EVAL_") {
            previous.push((name, Some(value)));
        }
    }
    for name in std::env::vars().map(|(name, _)| name).collect::<Vec<_>>() {
        if name.starts_with("PI_EVAL_") {
            std::env::remove_var(&name);
        }
    }
    for (name, value) in overrides {
        let already_recorded = previous.iter().any(|(recorded, _)| recorded == &name);
        if !already_recorded {
            previous.push((name.clone(), std::env::var(&name).ok()));
        }
        std::env::set_var(&name, &value);
    }
    move || {
        for (name, value) in previous {
            match value {
                Some(value) => std::env::set_var(&name, value),
                None => std::env::remove_var(&name),
            }
        }
    }
}

/// Upstream `parseSandboxId`.
fn parse_sandbox_id(
    environment: &BTreeMap<String, String>,
    name: &str,
) -> Result<Option<u32>, String> {
    let Some(value) = environment.get(name) else {
        return Ok(None);
    };
    let parsed: Option<i64> = value.trim().parse().ok();
    match parsed {
        Some(id) if id >= 1 && id <= u32::MAX as i64 => Ok(Some(id as u32)),
        _ => Err(format!("{name} must be a positive integer.")),
    }
}

/// Upstream `SandboxIdentity`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SandboxIdentity {
    pub uid: u32,
    pub gid: u32,
}

/// Upstream `resolveSandboxIdentity`.
pub fn resolve_sandbox_identity(
    environment: &BTreeMap<String, String>,
) -> Result<Option<SandboxIdentity>, String> {
    let uid = parse_sandbox_id(environment, "PI_EVAL_SANDBOX_UID")?;
    let gid = parse_sandbox_id(environment, "PI_EVAL_SANDBOX_GID")?;
    match (uid, gid) {
        (None, None) => Ok(None),
        (Some(uid), Some(gid)) => Ok(Some(SandboxIdentity { uid, gid })),
        _ => Err("Set both PI_EVAL_SANDBOX_UID and PI_EVAL_SANDBOX_GID, or neither.".to_string()),
    }
}

/// Upstream `DOCUMENTATION_EVAL_TOOLS`.
pub const DOCUMENTATION_EVAL_TOOLS: [&str; 6] = ["read", "write", "edit", "grep", "find", "ls"];

/// Upstream `verifySystemPrompt`.
pub fn verify_system_prompt(
    system_prompt: &str,
    name: Option<&str>,
    expected_pi_documentation: Option<bool>,
) -> Result<String, String> {
    let Some(expected) = expected_pi_documentation else {
        return Ok(system_prompt.to_string());
    };
    let name = name.unwrap_or("");
    if !system_prompt.contains("\n<rules>\n") {
        return Err(format!(
            "Pi system prompt lost its rules in the {name} eval variant."
        ));
    }
    let has_documentation = system_prompt.contains("\n<docs>\nPi documentation (read only");
    if has_documentation != expected {
        return Err(format!(
            "Pi system prompt does not match the {name} eval variant."
        ));
    }
    Ok(system_prompt.to_string())
}

/// Upstream `resolveDocumentationVariant` over the `PI_EVAL_VARIANT` value.
pub fn resolve_documentation_variant(value: Option<&str>) -> Result<DocumentationVariant, String> {
    match value.and_then(parse_documentation_variant) {
        Some(variant) => Ok(variant),
        None => Err("PI_EVAL_VARIANT must be \"without_docs\" or \"with_docs\".".to_string()),
    }
}

/// Upstream `excludePiDocumentation`.
pub fn exclude_pi_documentation(default_prompt: &str) -> Result<String, String> {
    let documentation_start_marker = "\n<docs>\n";
    let documentation_end_marker = "\n</docs>";
    let documentation_start = default_prompt
        .find(documentation_start_marker)
        .ok_or("Default Pi system prompt has no Pi documentation section.")?;
    let documentation_end = default_prompt[documentation_start..]
        .find(documentation_end_marker)
        .map(|index| documentation_start + index)
        .ok_or("Default Pi system prompt has no complete Pi documentation section.")?;
    let cwd_start = default_prompt
        .rfind("\n<cwd>\n")
        .ok_or("Default Pi system prompt has no working-directory section.")?;
    if cwd_start < documentation_end {
        return Err("Default Pi system prompt has no working-directory section.".to_string());
    }
    Ok(format!(
        "{}{}",
        &default_prompt[..documentation_start],
        &default_prompt[documentation_end + documentation_end_marker.len()..]
    ))
}

/// Seam S2: the live coding-agent run surface. Implementations receive the
/// prompt input and return the assistant response text; everything around it
/// (isolation, sandboxing, session artifacts, diagnostics aggregation) is the
/// harness' responsibility, mirroring upstream `runPiCodingAgent`.
pub type AgentRunner = dyn Fn(&str) -> Result<String, String> + Send + Sync;

/// Upstream `createPiCodingAgentHarness` (the harness record the runner
/// surface carries).
pub struct PiCodingAgentHarness {
    pub name: String,
    pub options: HarnessOptions,
    pub runner: std::sync::Arc<AgentRunner>,
}

/// Upstream `createPiCodingAgentHarness`.
pub fn create_pi_coding_agent_harness(
    options: HarnessOptions,
    runner: std::sync::Arc<AgentRunner>,
) -> PiCodingAgentHarness {
    PiCodingAgentHarness {
        name: options
            .name
            .clone()
            .unwrap_or_else(|| "pi-coding-agent".to_string()),
        options,
        runner,
    }
}

/// Upstream `createPiDocumentationEvalHarness` guard: the harness refuses to
/// construct outside the isolated container sandbox (S2: the variant-specific
/// tools/transform wiring is applied by the caller-supplied runner).
pub fn create_pi_documentation_eval_harness(
    options: HarnessOptions,
    runner: std::sync::Arc<AgentRunner>,
) -> Result<PiCodingAgentHarness, String> {
    let environment = process_environment();
    if environment.get("PI_EVAL_CONTAINER").map(String::as_str) != Some("1")
        || resolve_sandbox_identity(&environment)?.is_none()
    {
        return Err("Documentation evals must run in the isolated container sandbox.".to_string());
    }
    let variant =
        resolve_documentation_variant(environment.get("PI_EVAL_VARIANT").map(String::as_str))?;
    let mut options = options;
    options.name = Some(variant.as_str().to_string());
    options.expected_pi_documentation = Some(variant == DocumentationVariant::WithDocs);
    Ok(create_pi_coding_agent_harness(options, runner))
}

#[cfg(test)]
#[path = "harness_tests.rs"]
mod tests;
