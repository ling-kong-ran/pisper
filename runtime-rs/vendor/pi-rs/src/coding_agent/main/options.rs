//! Observable main.ts startup decisions, independent of process exits.
use crate::coding_agent::{
    agent_session::ScopedModel as SessionScopedModel,
    cli::{
        args::{Args, Mode},
        project_trust::AppMode,
    },
    core::{
        agent_session_services::{AgentSessionRuntimeDiagnostic, DiagnosticType},
        model_resolver::{
            models_are_equal, resolve_cli_model, ModelRuntimeReads, ResolveCliModelOptions,
            ScopedModel,
        },
        sdk::{CreateAgentSessionOptions, NoTools},
        settings_manager::SettingsManager,
    },
    session_manager::assert_valid_session_id,
    utils::paths::{is_local_path, resolve_path},
};

pub fn is_truthy_env_flag(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        value == "1" || value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("yes")
    })
}

pub fn resolve_app_mode(parsed: &Args, stdin_is_tty: bool, stdout_is_tty: bool) -> AppMode {
    match parsed.mode {
        Some(Mode::Rpc) => AppMode::Rpc,
        Some(Mode::Json) => AppMode::Json,
        _ if parsed.print == Some(true) || !stdin_is_tty || !stdout_is_tty => AppMode::Print,
        _ => AppMode::Interactive,
    }
}

pub fn to_print_output_mode(mode: AppMode) -> Mode {
    if mode == AppMode::Json {
        Mode::Json
    } else {
        Mode::Text
    }
}

pub fn is_plain_runtime_metadata_command(parsed: &Args) -> bool {
    parsed.print != Some(true)
        && parsed.mode.is_none()
        && (parsed.help == Some(true) || parsed.list_models.is_some())
}

pub fn validate_fork_flags(parsed: &Args) -> Result<(), String> {
    if parsed.fork.as_ref().is_none_or(|s| s.is_empty()) {
        return Ok(());
    }
    let mut flags = Vec::new();
    if parsed.session.as_ref().is_some_and(|s| !s.is_empty()) {
        flags.push("--session");
    }
    if parsed.r#continue == Some(true) {
        flags.push("--continue");
    }
    if parsed.resume == Some(true) {
        flags.push("--resume");
    }
    if parsed.no_session == Some(true) {
        flags.push("--no-session");
    }
    if flags.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Error: --fork cannot be combined with {}",
            flags.join(", ")
        ))
    }
}

pub fn validate_session_id_flags(parsed: &Args) -> Result<(), String> {
    let Some(id) = &parsed.session_id else {
        return Ok(());
    };
    let mut flags = Vec::new();
    if parsed.session.as_ref().is_some_and(|s| !s.is_empty()) {
        flags.push("--session");
    }
    if parsed.r#continue == Some(true) {
        flags.push("--continue");
    }
    if parsed.resume == Some(true) {
        flags.push("--resume");
    }
    if !flags.is_empty() {
        return Err(format!(
            "Error: --session-id cannot be combined with {}",
            flags.join(", ")
        ));
    }
    assert_valid_session_id(id).map_err(|e| format!("Error: {e}"))
}

pub fn resolve_cli_paths(
    cwd: &str,
    paths: Option<&[String]>,
) -> Result<Option<Vec<String>>, String> {
    paths
        .map(|paths| {
            paths
                .iter()
                .map(|path| {
                    if is_local_path(path) {
                        resolve_path(path, cwd).map_err(|e| e.to_string())
                    } else {
                        Ok(path.clone())
                    }
                })
                .collect()
        })
        .transpose()
}

pub struct BuiltSessionOptions {
    pub options: CreateAgentSessionOptions,
    pub cli_thinking_from_model: bool,
    pub diagnostics: Vec<AgentSessionRuntimeDiagnostic>,
}

pub fn build_session_options(
    parsed: &Args,
    scoped_models: &[ScopedModel],
    has_existing_session: bool,
    model_runtime: &dyn ModelRuntimeReads,
    settings_manager: &SettingsManager,
) -> BuiltSessionOptions {
    let mut options = CreateAgentSessionOptions::default();
    let mut diagnostics = Vec::new();
    let mut cli_thinking_from_model = false;
    // Upstream v1.0.0: `--provider` alone is an error; it only narrows `--model` search.
    if parsed.provider.as_deref().is_some_and(|p| !p.is_empty())
        && parsed.model.as_deref().is_none_or(|m| m.is_empty())
    {
        diagnostics.push(AgentSessionRuntimeDiagnostic {
            kind: DiagnosticType::Error,
            message: format!(
                "--provider requires --model (for example: --provider {} --model <pattern>)",
                parsed.provider.as_deref().unwrap_or_default()
            ),
        });
    }
    if parsed.model.as_ref().is_some_and(|s| !s.is_empty()) {
        let resolved = resolve_cli_model(ResolveCliModelOptions {
            cli_provider: parsed.provider.as_deref(),
            cli_model: parsed.model.as_deref(),
            cli_thinking: parsed.thinking,
            model_runtime,
        });
        if let Some(message) = resolved.warning {
            diagnostics.push(AgentSessionRuntimeDiagnostic {
                kind: DiagnosticType::Warning,
                message,
            });
        }
        if let Some(message) = resolved.error {
            diagnostics.push(AgentSessionRuntimeDiagnostic {
                kind: DiagnosticType::Error,
                message,
            });
        }
        if let Some(model) = resolved.model {
            options.model = Some(model);
            if parsed.thinking.is_none() && resolved.thinking_level.is_some() {
                options.thinking_level = resolved.thinking_level;
                cli_thinking_from_model = true;
            }
        }
    }
    if options.model.is_none() && !scoped_models.is_empty() && !has_existing_session {
        let saved = settings_manager
            .get_default_provider()
            .filter(|s| !s.is_empty())
            .zip(
                settings_manager
                    .get_default_model()
                    .filter(|s| !s.is_empty()),
            )
            .and_then(|(provider, id)| model_runtime.get_model(&provider, &id));
        let selected = saved
            .as_ref()
            .and_then(|saved| {
                scoped_models
                    .iter()
                    .find(|sm| models_are_equal(&sm.model, saved))
            })
            .unwrap_or(&scoped_models[0]);
        options.model = Some(selected.model.clone());
        if parsed.thinking.is_none() {
            options.thinking_level = selected.thinking_level;
        }
    }
    if parsed.thinking.is_some() {
        options.thinking_level = parsed.thinking;
    }
    options.scoped_models = scoped_models
        .iter()
        .map(|sm| SessionScopedModel {
            model: sm.model.clone(),
            thinking_level: sm.thinking_level,
        })
        .collect();
    options.no_tools = if parsed.no_tools == Some(true) {
        Some(NoTools::All)
    } else if parsed.no_builtin_tools == Some(true) {
        Some(NoTools::Builtin)
    } else {
        None
    };
    options.tools = parsed.tools.clone();
    options.exclude_tools = parsed.exclude_tools.clone();
    BuiltSessionOptions {
        options,
        cli_thinking_from_model,
        diagnostics,
    }
}
