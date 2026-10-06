//! Native noninteractive process orchestration from upstream `main.ts`.
//!
//! Parsing, session selection, trust, services/SDK, metadata and print/RPC
//! dispatch and legacy startup migrations are real. Interactive presentation,
//! HTML export, llama.cpp's built-in extension and the package-command native
//! host are not yet implemented here. Unsupported commands fail explicitly;
//! they are never sent to a model as an accidental prompt. TS extension files
//! use the resource loader's explicit host boundary and fail if unbound.
use super::{
    auth::run_auth_command,
    input::prepare_initial_message,
    options::{
        is_plain_runtime_metadata_command, resolve_app_mode, validate_fork_flags,
        validate_session_id_flags,
    },
    runtime::{
        create_cli_runtime_factory, CliModelRuntimeFactory, CliRuntimeFactoryOptions,
        OperationDeadline,
    },
    sessions::{create_session_manager, SessionSelection, SessionStartupUi, StartupOutput},
};
use crate::{
    ai::{auth::types::AuthOperationOptions, models::ModelsRefreshOptions},
    coding_agent::{
        cli::{
            args::{
                normalize_session_name, parse_args, print_help, DiagnosticType as ArgDiagnosticType,
            },
            file_processor::{native_process_image, ProcessImageFn},
            list_models::render_models_table,
            project_trust::AppMode,
        },
        core::{
            agent_session_runtime::{
                create_agent_session_runtime, AgentSessionRuntime, CreateAgentSessionRuntimeOptions,
            },
            agent_session_services::{AgentSessionRuntimeDiagnostic, DiagnosticType},
            auth_guidance::format_no_models_available_message,
            http_dispatcher::{
                apply_http_proxy_settings, configure_http_dispatcher, DEFAULT_HTTP_IDLE_TIMEOUT_MS,
            },
            output_guard::{OutputChunk, OutputGuard},
            resource_loader::InlineExtension,
            session_cwd::{get_missing_session_cwd_issue, MissingSessionCwdError},
            settings_diagnostics::{collect_settings_diagnostics, deduplicate_diagnostics},
            settings_manager::{
                SettingsManager, SettingsManagerCreateOptions, TerminalCapabilityOverrides,
                TerminalImagesOverride,
            },
        },
        extensions::loader::ExtensionModuleLoader,
        migrations::run_migrations,
        modes::{
            print_mode::{run_print_mode_with_io, PrintMode, PrintModeOptions, PrintProcess},
            rpc::mode::run_rpc_mode_with_io,
        },
        package_manager::cli::{
            parse_package_command, render_config_command_help, render_package_command_help,
        },
        utils::paths::normalize_path,
    },
    tui::terminal_image::{set_capability_overrides, CapabilityOverrides},
};
use anyhow::Result;
use futures::future::BoxFuture;
use std::sync::{Arc, Mutex};
use tokio::io::AsyncRead;

/// The checked-in upstream source version, not this Rust crate's development version.
pub const UPSTREAM_VERSION: &str = "0.85.1";

/// Process facts are captured before startup. Tests inject only these facts,
/// native extension/provider bindings and IO, never a fake AgentSession.
pub struct CliMainOptions {
    pub cwd: String,
    pub agent_dir: String,
    pub stdin_is_tty: bool,
    pub stdout_is_tty: bool,
    pub env_session_dir: Option<String>,
    pub offline: bool,
    pub startup_benchmark: bool,
    pub extension_factories: Vec<InlineExtension>,
    pub extension_module_loader: Option<Arc<dyn ExtensionModuleLoader>>,
    pub model_runtime_factory: Option<CliModelRuntimeFactory>,
    /// Process-global proxy/terminal state: enabled by the native binary only.
    pub configure_process: bool,
    pub process_image: ProcessImageFn,
}
impl CliMainOptions {
    pub fn new(cwd: impl Into<String>, agent_dir: impl Into<String>) -> Self {
        Self {
            cwd: cwd.into(),
            agent_dir: agent_dir.into(),
            stdin_is_tty: false,
            stdout_is_tty: false,
            env_session_dir: None,
            offline: false,
            startup_benchmark: false,
            extension_factories: vec![],
            extension_module_loader: None,
            model_runtime_factory: None,
            configure_process: false,
            process_image: native_process_image,
        }
    }
}

pub(super) async fn write_console(output: &OutputGuard, text: String) -> Result<()> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    output.write_stdout(
        OutputChunk::Text(text),
        None,
        Some(Box::new(move |result| {
            let _ = tx.send(result);
        })),
    )?;
    rx.await??;
    Ok(())
}
fn diagnostics(process: &dyn PrintProcess, items: &[AgentSessionRuntimeDiagnostic]) {
    for item in items {
        let prefix = match item.kind {
            DiagnosticType::Error => "Error: ",
            DiagnosticType::Warning => "Warning: ",
            DiagnosticType::Info => "",
        };
        process.error(&format!("{prefix}{}", item.message));
    }
}
fn configure_http(settings: &SettingsManager, startup: bool) -> Result<()> {
    let global = settings.get_global_settings();
    apply_http_proxy_settings(global.get("httpProxy").and_then(|value| value.as_str()));
    let timeout = if startup {
        DEFAULT_HTTP_IDLE_TIMEOUT_MS
    } else {
        settings
            .get_http_idle_timeout_ms()
            .map_err(anyhow::Error::msg)?
    };
    configure_http_dispatcher(timeout as f64).map_err(anyhow::Error::msg)?;
    Ok(())
}
pub fn terminal_capabilities(settings: TerminalCapabilityOverrides) -> CapabilityOverrides {
    CapabilityOverrides {
        images: settings.images.map(|kind| match kind {
            TerminalImagesOverride::Kitty => Some("kitty"),
            TerminalImagesOverride::Iterm2 => Some("iterm2"),
            TerminalImagesOverride::Cleared => None,
        }),
        true_color: settings.true_color,
        hyperlinks: settings.hyperlinks,
    }
}

/// Restores only the takeover owned by this invocation, including early errors
/// and dropped futures. An embedding host's pre-existing takeover is preserved.
struct StdoutLease {
    output: OutputGuard,
    owned: bool,
}
impl Drop for StdoutLease {
    fn drop(&mut self) {
        if self.owned {
            self.output.restore_stdout();
        }
    }
}
struct RefreshTask {
    signal: tokio_util::sync::CancellationToken,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for RefreshTask {
    fn drop(&mut self) {
        self.signal.cancel();
        self.task.abort();
    }
}

/// Headless default never consumes RPC or prompt bytes to answer a terminal
/// picker. A terminal host must explicitly supply its selector/confirmation UI.
pub struct HeadlessSessionUi {
    pub output: OutputGuard,
    pub process: Arc<dyn PrintProcess>,
}
impl SessionStartupUi for HeadlessSessionUi {
    fn report(&self, kind: StartupOutput, message: &str) {
        match kind {
            StartupOutput::Error | StartupOutput::Warning => self.process.error(message),
            StartupOutput::Notice | StartupOutput::Dim => {
                if let Err(error) =
                    self.output
                        .write_stdout(OutputChunk::Text(format!("{message}\n")), None, None)
                {
                    self.process.error(&error.to_string());
                }
            }
        }
    }
    fn confirm<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<bool>> {
        Box::pin(async {
            anyhow::bail!("Session confirmation requires a terminal UI; pass the explicit session file path instead")
        })
    }
    fn select_session<'a>(
        &'a self,
        _: &'a str,
        _: Option<&'a str>,
        _: &'a SettingsManager,
    ) -> BoxFuture<'a, Result<Option<String>>> {
        Box::pin(async {
            anyhow::bail!("The interactive --resume picker is not yet available in pi-rust; use --session <path>")
        })
    }
    fn stop_theme_watcher(&self) {}
}

/// Real print/JSON/RPC startup. The native boundary translates errors into
/// `Error: ...` on stderr and an exit status, never stdout protocol frames.
pub async fn run_cli_with_io<I: AsyncRead + Unpin>(
    args: &[String],
    options: CliMainOptions,
    mut input: I,
    output: OutputGuard,
    process: Arc<dyn PrintProcess>,
    startup_ui: Arc<dyn SessionStartupUi>,
) -> Result<i32> {
    if let Some(auth) = run_auth_command(args, &options.agent_dir).await {
        for message in auth.stderr {
            process.error(&message);
        }
        if !auth.stdout.is_empty() {
            write_console(&output, auth.stdout).await?;
        }
        return Ok(auth.exit_code);
    }
    // Do not silently reinterpret package/config commands as model prompts.
    if let Some(command) = parse_package_command(args) {
        if command.help {
            write_console(
                &output,
                format!("{}\n", render_package_command_help(command.command)),
            )
            .await?;
            return Ok(0);
        }
        process.error("Error: Native package-command execution is not yet available in pi-rust");
        return Ok(1);
    }
    if args.first().is_some_and(|arg| arg == "config") {
        if args
            .iter()
            .skip(1)
            .any(|arg| matches!(arg.as_str(), "--help" | "-h"))
        {
            write_console(&output, format!("{}\n", render_config_command_help())).await?;
            return Ok(0);
        }
        process.error("Error: The interactive config selector is not yet available in pi-rust");
        return Ok(1);
    }
    let bootstrap = SettingsManager::create_with(
        &options.cwd,
        &options.agent_dir,
        SettingsManagerCreateOptions {
            project_trusted: false,
        },
    )?;
    if options.configure_process {
        configure_http(&bootstrap, true)?;
    }
    let mut parsed = parse_args(args);
    for diagnostic in &parsed.diagnostics {
        process.error(&format!(
            "{}: {}",
            if diagnostic.kind == ArgDiagnosticType::Error {
                "Error"
            } else {
                "Warning"
            },
            diagnostic.message
        ));
    }
    if parsed
        .diagnostics
        .iter()
        .any(|d| d.kind == ArgDiagnosticType::Error)
    {
        return Ok(1);
    }
    if parsed.version == Some(true) {
        write_console(&output, format!("{UPSTREAM_VERSION}\n")).await?;
        return Ok(0);
    }
    if parsed.export.is_some() {
        process.error("Error: HTML export is not yet available in pi-rust");
        return Ok(1);
    }
    let mut app_mode = resolve_app_mode(&parsed, options.stdin_is_tty, options.stdout_is_tty);
    let takeover = app_mode != AppMode::Interactive && !is_plain_runtime_metadata_command(&parsed);
    let stdout_lease = StdoutLease {
        owned: takeover && !output.is_stdout_taken_over(),
        output: output.clone(),
    };
    if takeover {
        output.take_over_stdout();
    }
    if app_mode == AppMode::Rpc && !parsed.file_args.is_empty() {
        process.error("Error: @file arguments are not supported in RPC mode");
        return Ok(1);
    }
    if let Err(error) =
        validate_fork_flags(&parsed).and_then(|()| validate_session_id_flags(&parsed))
    {
        process.error(&error);
        return Ok(1);
    }
    // Upstream runs migrations after argument preflight and before any settings,
    // credentials or session resources are loaded. Console output must pass
    // through the stdout lease: JSON/RPC stdout remains protocol-only.
    let migration_messages = std::cell::RefCell::new(Vec::new());
    let migration_result = run_migrations(
        std::path::Path::new(&options.cwd),
        std::path::Path::new(&options.agent_dir),
        &|message| migration_messages.borrow_mut().push(message.to_owned()),
    );
    for message in migration_messages.into_inner() {
        write_console(&output, format!("{message}\n")).await?;
    }
    // Interactive presentation will consume this report when that host lands;
    // noninteractive startup must not print warnings or await a keypress.
    let _migration_result = migration_result?;
    if app_mode == AppMode::Interactive && parsed.help != Some(true) && parsed.list_models.is_none()
    {
        process.error(
            "Error: Interactive mode is not yet available in pi-rust; use --print or --mode rpc",
        );
        return Ok(1);
    }
    let startup_settings = SettingsManager::create_with(
        &options.cwd,
        &options.agent_dir,
        SettingsManagerCreateOptions::default(),
    )?;
    let startup_diagnostics = collect_settings_diagnostics(&startup_settings);
    let session_dir = parsed
        .session_dir
        .as_deref()
        .filter(|s| !s.is_empty())
        .or(options.env_session_dir.as_deref().filter(|s| !s.is_empty()))
        .map(normalize_path)
        .transpose()?
        .or_else(|| startup_settings.get_session_dir());
    let mut manager = match create_session_manager(
        &parsed,
        &options.cwd,
        session_dir.as_deref(),
        &startup_settings,
        startup_ui,
    )
    .await?
    {
        SessionSelection::Open(manager) => *manager,
        SessionSelection::Exit(code) => return Ok(code),
    };
    if let Some(issue) = get_missing_session_cwd_issue(&manager, &options.cwd) {
        process.error(&MissingSessionCwdError { issue }.to_string());
        return Ok(1);
    }
    if let Some(name) = parsed.name.as_deref() {
        let Some(name) = normalize_session_name(name) else {
            process.error("Error: --name requires a non-empty value");
            return Ok(1);
        };
        manager.append_session_info(&name)?;
    }
    let warning_process = process.clone();
    let factory = create_cli_runtime_factory(CliRuntimeFactoryOptions {
        parsed: parsed.clone(),
        startup_cwd: options.cwd.clone(),
        initial_session_cwd: manager.get_cwd().into(),
        agent_dir: options.agent_dir.clone(),
        startup_settings_manager: startup_settings,
        app_mode,
        extension_factories: options.extension_factories,
        extension_module_loader: options.extension_module_loader,
        model_runtime_factory: options.model_runtime_factory,
        custom_tool_factory: None,
        model_scope_warning: Some(Arc::new(move |message| warning_process.error(message))),
    })?;
    let runtime = Arc::new(
        create_agent_session_runtime(
            factory.create_runtime,
            CreateAgentSessionRuntimeOptions {
                cwd: manager.get_cwd().into(),
                agent_dir: options.agent_dir,
                session_manager: Arc::new(Mutex::new(manager)),
                session_start_event: None,
                project_trust_context: None,
            },
        )
        .await?,
    );
    let services = runtime.services();
    // No session_shutdown on metadata/preflight failures: upstream exits before
    // binding mode extensions. Dropping this guard only disposes native handles.
    struct PreflightGuard(Option<Arc<AgentSessionRuntime>>);
    impl Drop for PreflightGuard {
        fn drop(&mut self) {
            if let Some(runtime) = self.0.take() {
                runtime.session().dispose();
            }
        }
    }
    let mut preflight = PreflightGuard(Some(runtime.clone()));
    if options.configure_process {
        set_capability_overrides(terminal_capabilities(
            services
                .settings_manager
                .get_terminal_capability_overrides(),
        ));
        configure_http(&services.settings_manager, false)?;
    }
    if parsed.help == Some(true) {
        diagnostics(process.as_ref(), &startup_diagnostics);
        let flags = services
            .resource_loader
            .lock()
            .expect("resources")
            .get_extensions()
            .extensions
            .iter()
            .flat_map(|extension| extension.flags.iter().map(|(_, flag)| flag.clone()))
            .collect::<Vec<_>>();
        write_console(&output, format!("{}\n", print_help(&flags))).await?;
        return Ok(0);
    }
    if let Some(pattern) = &parsed.list_models {
        diagnostics(process.as_ref(), &startup_diagnostics);
        if let Some(error) = services.model_runtime.get_error() {
            process.error(&format!("Warning: errors loading models.json:\n{error}"));
        }
        let mut deadline = OperationDeadline::new();
        let models = services
            .model_runtime
            .get_available(
                None,
                Some(&AuthOperationOptions {
                    signal: Some(deadline.token.clone()),
                }),
            )
            .await?;
        deadline.finish();
        let mut text = vec![];
        render_models_table(&mut text, &models, pattern.as_deref())?;
        write_console(&output, String::from_utf8(text)?).await?;
        return Ok(0);
    }
    let stdin_content = if app_mode == AppMode::Rpc {
        None
    } else {
        super::input::read_piped_stdin(&mut input, options.stdin_is_tty).await?
    };
    if stdin_content.is_some() && app_mode == AppMode::Interactive {
        app_mode = AppMode::Print;
    }
    let initial = prepare_initial_message(
        &mut parsed,
        services.settings_manager.get_image_auto_resize(),
        stdin_content.as_deref(),
        &options.cwd,
        options.process_image,
    )?;
    let runtime_diagnostics = runtime.diagnostics();
    let startup_diagnostics =
        deduplicate_diagnostics(&[startup_diagnostics, runtime_diagnostics.clone()].concat());
    let has_runtime_errors = runtime_diagnostics
        .iter()
        .any(|diagnostic| diagnostic.kind == DiagnosticType::Error);
    diagnostics(process.as_ref(), &startup_diagnostics);
    if has_runtime_errors {
        if runtime_diagnostics
            .iter()
            .any(|d| d.message.contains("Failed to load extension"))
        {
            process.error("Hint: Start without extensions using \"pi -ne\".");
        }
        return Ok(1);
    }
    if runtime.session().model().is_none() {
        process.error(&format_no_models_available_message());
        return Ok(1);
    }
    if options.startup_benchmark {
        process.error("Error: PI_STARTUP_BENCHMARK only supports interactive mode");
        return Ok(1);
    }
    let offline = options.offline || parsed.offline == Some(true);
    let _refresh = if !offline && app_mode == AppMode::Rpc {
        let deadline = OperationDeadline::new();
        let signal = deadline.token.clone();
        let models = services.model_runtime.clone();
        let task = tokio::spawn(async move {
            let mut deadline = deadline;
            let _ = models
                .refresh(ModelsRefreshOptions {
                    signal: Some(deadline.token.clone()),
                    ..Default::default()
                })
                .await;
            deadline.finish();
        });
        Some(RefreshTask { signal, task })
    } else {
        None
    };
    // The modes now own disposal; do not emit a second shutdown from this layer.
    preflight.0.take();
    let code = if app_mode == AppMode::Rpc {
        run_rpc_mode_with_io(runtime, input, output.clone(), process).await?
    } else {
        run_print_mode_with_io(
            runtime,
            PrintModeOptions {
                mode: if app_mode == AppMode::Json {
                    PrintMode::Json
                } else {
                    PrintMode::Text
                },
                messages: parsed.messages,
                initial_message: initial.initial_message,
                initial_images: initial.initial_images,
            },
            output.clone(),
            process,
        )
        .await?
    };
    // Each mode owns its final flush (RPC deliberately skips it on SIGTERM).
    // A second empty write would be observable and could fail after success.
    drop(stdout_lease);
    Ok(code)
}

/// Native process adapter. Call after `cli::setup::setup_cli` and the offline
/// environment flags have been set, before creating any runtime services.
pub async fn run_cli(args: &[String]) -> Result<i32> {
    use std::io::IsTerminal;
    let mut options = CliMainOptions::new(
        std::env::current_dir()?.to_string_lossy(),
        crate::coding_agent::core::get_agent_dir(),
    );
    options.stdin_is_tty = std::io::stdin().is_terminal();
    options.stdout_is_tty = std::io::stdout().is_terminal();
    options.env_session_dir = std::env::var(crate::coding_agent::cli::ENV_SESSION_DIR).ok();
    options.offline = args.iter().any(|arg| arg == "--offline")
        || super::options::is_truthy_env_flag(std::env::var("PI_OFFLINE").ok().as_deref());
    options.startup_benchmark =
        super::options::is_truthy_env_flag(std::env::var("PI_STARTUP_BENCHMARK").ok().as_deref());
    options.configure_process = true;
    let output = crate::coding_agent::core::output_guard::process_guard().clone();
    let process: Arc<dyn PrintProcess> =
        Arc::new(crate::coding_agent::modes::print_mode::NativePrintProcess);
    let ui = Arc::new(HeadlessSessionUi {
        output: output.clone(),
        process: process.clone(),
    });
    run_cli_with_io(args, options, tokio::io::stdin(), output, process, ui).await
}

#[cfg(test)]
#[path = "entry_tests.rs"]
mod tests;
