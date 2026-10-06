//! Port of `pi/packages/evals/src/docker.ts` — docker-backed eval execution
//! (image builds, per-variant discovery and task runs).

use crate::evals::plan::{DocumentationVariant, EvalTask};
use crate::serde_support::to_json_string_with_js_numbers;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Upstream `packageRoot` (`src/../`): the evals package root.
pub fn package_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("packages/evals")
}

/// Upstream `repositoryRoot`.
pub fn repository_root() -> PathBuf {
    package_root()
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .expect("package root sits two levels below the repository root")
}

/// Upstream `BuiltImages`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltImages {
    pub without_docs: VariantImage,
    pub with_docs: VariantImage,
}

/// Upstream per-variant image record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariantImage {
    pub name: String,
    pub id: String,
}

/// Upstream `DockerContext`.
#[derive(Debug, Clone)]
pub struct DockerContext {
    pub images: BuiltImages,
    pub artifact_directory: PathBuf,
    pub auth_path: PathBuf,
    pub provider: String,
    pub model: String,
    pub runs_per_variant: u32,
}

/// Upstream `execute` (spawn with inherited stdio; `capture` pipes stdout).
fn execute(
    cwd: &Path,
    command: &str,
    args: &[String],
    capture: bool,
) -> Result<(i32, String), String> {
    let mut cmd = Command::new(command);
    cmd.args(args).current_dir(cwd);
    let output = if capture {
        cmd.output()
    } else {
        cmd.status().map(|status| std::process::Output {
            status,
            stdout: Vec::new(),
            stderr: Vec::new(),
        })
    };
    match output {
        Ok(output) => {
            let code = output.status.code().unwrap_or(1);
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            Ok((code, stdout))
        }
        Err(error) => Err(error.to_string()),
    }
}

fn require_success(cwd: &Path, command: &str, args: &[String]) -> Result<(), String> {
    let (status, _) = execute(cwd, command, args, false)?;
    if status != 0 {
        return Err(format!("{command} exited with status {status}."));
    }
    Ok(())
}

fn hex_digest(text: &str, take: Option<usize>) -> String {
    let digest = Sha256::digest(text.as_bytes());
    let mut hex = String::new();
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    match take {
        Some(take) => hex[..take].to_string(),
        None => hex,
    }
}

/// Upstream `buildImages`.
pub fn build_images() -> Result<BuiltImages, String> {
    let repository = repository_root();
    let key = repository.to_string_lossy();
    let prefix = format!("pi-evals-{}", hex_digest(&key, Some(12)));
    let mut images = BuiltImages {
        without_docs: VariantImage {
            name: format!("{prefix}-without-docs:local"),
            id: String::new(),
        },
        with_docs: VariantImage {
            name: format!("{prefix}-with-docs:local"),
            id: String::new(),
        },
    };
    let package_root = package_root();
    for (variant, image) in [
        (
            DocumentationVariant::WithoutDocs,
            (&mut images.without_docs) as &mut VariantImage,
        ),
        (DocumentationVariant::WithDocs, &mut images.with_docs),
    ] {
        let args = vec![
            "build".to_string(),
            "--target".to_string(),
            variant.as_str().to_string(),
            "--tag".to_string(),
            image.name.clone(),
            "--file".to_string(),
            package_root
                .join("docker")
                .join("Dockerfile")
                .to_string_lossy()
                .to_string(),
            repository.to_string_lossy().to_string(),
        ];
        require_success(&package_root, "docker", &args)?;
        let inspect_args = vec![
            "image".to_string(),
            "inspect".to_string(),
            "--format".to_string(),
            "{{.Id}}".to_string(),
            image.name.clone(),
        ];
        let (status, stdout) = execute(&package_root, "docker", &inspect_args, true)?;
        let trimmed = stdout.trim().to_string();
        if status != 0 || trimmed.is_empty() {
            return Err(format!("Cannot inspect {}.", image.name));
        }
        image.id = trimmed;
    }
    Ok(images)
}

/// Upstream `requireEvalAuthFile`.
pub fn require_eval_auth_file(provider: &str) -> Result<PathBuf, String> {
    let base = match std::env::var("PI_CODING_AGENT_DIR") {
        Ok(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => dirs::home_dir()
            .unwrap_or_default()
            .join(".pi")
            .join("agent"),
    };
    let path = base.join("auth.json");
    let metadata = std::fs::metadata(&path).map_err(|_| {
        format!(
            "Eval authentication file does not exist: {}",
            path.to_string_lossy()
        )
    })?;
    if !metadata.is_file() {
        return Err(format!(
            "Eval authentication file does not exist: {}",
            path.to_string_lossy()
        ));
    }
    let text = std::fs::read_to_string(&path).map_err(|error| {
        format!(
            "Eval authentication file is invalid: {} ({error})",
            path.to_string_lossy()
        )
    })?;
    let credentials: serde_json::Value = serde_json::from_str(&text).map_err(|error| {
        format!(
            "Eval authentication file is invalid: {} ({error})",
            path.to_string_lossy()
        )
    })?;
    match credentials {
        serde_json::Value::Object(map) if map.get(provider).is_some() => Ok(path),
        _ => Err(format!(
            "Eval authentication file has no credential for provider {provider}."
        )),
    }
}

/// Upstream `dockerArgs`. Returns the full `docker run` argument vector.
pub fn docker_args(
    context: &DockerContext,
    variant: DocumentationVariant,
    output_directory: &Path,
    entrypoint_args: &[String],
) -> Result<Vec<String>, String> {
    std::fs::create_dir_all(output_directory).map_err(|error| error.to_string())?;
    let environment =
        |name: &str, value: String| vec!["--env".to_string(), format!("{name}={value}")];
    let mut args = vec![
        "run".to_string(),
        "--rm".to_string(),
        "--read-only".to_string(),
        "--tmpfs".to_string(),
        "/tmp:rw,exec,mode=1777".to_string(),
        "--tmpfs".to_string(),
        "/repo/node_modules/.vite-temp:rw,exec,mode=1777".to_string(),
        "--mount".to_string(),
        format!(
            "type=bind,source={},target=/artifacts",
            output_directory.to_string_lossy()
        ),
    ];
    args.extend(environment(
        "PI_EVAL_ARTIFACT_DIR",
        "/artifacts".to_string(),
    ));
    args.extend(environment(
        "PI_EVAL_RUNS_PER_VARIANT",
        context.runs_per_variant.to_string(),
    ));
    args.extend(environment("PI_EVAL_SANDBOX_UID", "65532".to_string()));
    args.extend(environment("PI_EVAL_SANDBOX_GID", "65532".to_string()));
    args.extend(environment("PI_PROVIDER", context.provider.clone()));
    args.extend(environment("PI_MODEL", context.model.clone()));
    // Upstream appends PI_EVAL_ARTIFACT_UID/GID when the process exposes
    // getuid/getgid — a POSIX-only detail this port omits (divergence D1: the
    // no-unsafe constraint forbids the raw libc externs and `libc` is not an
    // existing dependency). The docker CLI consumes the same arguments
    // otherwise.
    args.push("--mount".to_string());
    args.push(format!(
        "type=bind,source={},target=/run/pi-eval-secrets/auth.json,readonly",
        context.auth_path.to_string_lossy()
    ));
    args.push(context.image_for(variant).name.clone());
    args.extend(entrypoint_args.to_vec());
    Ok(args)
}

impl DockerContext {
    /// Upstream `context.images[variant]`.
    pub fn image_for(&self, variant: DocumentationVariant) -> &VariantImage {
        match variant {
            DocumentationVariant::WithoutDocs => &self.images.without_docs,
            DocumentationVariant::WithDocs => &self.images.with_docs,
        }
    }
}

/// Upstream `createDockerContext`.
pub fn create_docker_context(
    images: BuiltImages,
    artifact_directory: PathBuf,
    auth_path: PathBuf,
    provider: &str,
    model: &str,
    runs_per_variant: u32,
) -> DockerContext {
    DockerContext {
        images,
        artifact_directory,
        auth_path,
        provider: provider.to_string(),
        model: model.to_string(),
        runs_per_variant,
    }
}

/// Upstream `discoverCases`.
pub fn discover_cases(
    context: &DockerContext,
    variant: DocumentationVariant,
    files: &[String],
    vitest_args: &[String],
) -> Result<PathBuf, String> {
    let output_directory = context
        .artifact_directory
        .join("discovery")
        .join(variant.as_str());
    let mut entrypoint = vec![
        "--discover".to_string(),
        "--project".to_string(),
        "docs".to_string(),
    ];
    entrypoint.extend(files.to_vec());
    entrypoint.extend(vitest_args.to_vec());
    let args = docker_args(context, variant, &output_directory, &entrypoint)?;
    let (status, _) = execute(&package_root(), "docker", &args, false)?;
    if status != 0 {
        return Err(format!("{} eval discovery failed.", variant.as_str()));
    }
    Ok(output_directory.join("discovered-tests.json"))
}

/// Upstream `taskDirectoryName`: sha256 of the task identity JSON.
pub fn task_directory_name(task: &EvalTask) -> String {
    let identity = to_json_string_with_js_numbers(&serde_json::json!([
        task.eval_set,
        task.case_id,
        task.variant.as_str(),
        task.model,
        task.run_number
    ]))
    .expect("array serializes");
    hex_digest(&identity, None)
}

/// Upstream `runTask`: returns the report path when the container produced
/// one.
pub fn run_task(context: &DockerContext, task: &EvalTask) -> Result<Option<PathBuf>, String> {
    let output_directory = context
        .artifact_directory
        .join("tasks")
        .join(task_directory_name(task));
    let test_name = format!("{} {}", task.eval_set, task.case_id);
    let exact_name = format!("^{}$", regex_escape(&test_name));
    let entrypoint = vec![
        "--project".to_string(),
        "docs".to_string(),
        task.file.clone(),
        "--testNamePattern".to_string(),
        exact_name,
    ];
    let args = docker_args(context, task.variant, &output_directory, &entrypoint)?;
    let _ = execute(&package_root(), "docker", &args, false)?;
    let report_path = output_directory.join("vitest.json");
    if report_path.exists() {
        Ok(Some(report_path))
    } else {
        Ok(None)
    }
}

/// Upstream regex-escape character class (`[.*+?^${}()|[\]\\]`).
fn regex_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        if ".*+?^${}()|[]\\".contains(character) {
            out.push('\\');
        }
        out.push(character);
    }
    out
}

#[cfg(test)]
#[path = "docker_tests.rs"]
mod tests;
