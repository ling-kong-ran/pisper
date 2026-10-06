//! Actual CLI parser -> filesystem session -> trust -> services -> SDK -> mode
//! -> native faux provider. Only terminal/model IO is replaced.
use super::*;
use crate::{
    ai::{
        auth::credential_store::InMemoryCredentialStore,
        models::{
            faux_assistant_message, faux_provider, FauxMessageOptions, FauxProviderHandle,
            FauxProviderOptions,
        },
    },
    coding_agent::{
        core::{
            model_runtime::{CreateModelRuntimeOptions, ModelRuntime},
            models_store::InMemoryCodingAgentModelsStore,
            output_guard::{OutputError, OutputStream, WriteCallback},
        },
        extensions::types::{sync_handler, FlagType},
        modes::print_mode::{PrintSignal, SignalHandler, SignalRegistration},
        session_manager::SessionManager,
    },
};
use serde_json::Value;
use std::{
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering},
    task::{Context, Poll},
    time::Duration,
};
use tokio::io::ReadBuf;
#[derive(Default)]
struct MemoryOutput(Mutex<String>, AtomicUsize);
impl MemoryOutput {
    fn text(&self) -> String {
        self.0.lock().unwrap().clone()
    }
}
impl OutputStream for MemoryOutput {
    fn write(
        &self,
        chunk: OutputChunk,
        _: Option<String>,
        callback: Option<WriteCallback>,
    ) -> std::result::Result<bool, Arc<OutputError>> {
        let OutputChunk::Text(text) = chunk else {
            panic!("text expected")
        };
        if text.is_empty() {
            self.1.fetch_add(1, Ordering::SeqCst);
        }
        self.0.lock().unwrap().push_str(&text);
        if let Some(callback) = callback {
            callback(Ok(()));
        }
        Ok(true)
    }
}
#[derive(Default)]
struct TestProcess {
    errors: Mutex<Vec<String>>,
    exits: Mutex<Vec<i32>>,
}
impl PrintProcess for TestProcess {
    fn signals(&self) -> Vec<PrintSignal> {
        vec![]
    }
    fn on_signal(&self, _: PrintSignal, _: SignalHandler) -> Result<SignalRegistration> {
        unreachable!()
    }
    fn error(&self, message: &str) {
        self.errors.lock().unwrap().push(message.into());
    }
    fn kill_tracked_children(&self) {}
    fn exit(&self, code: i32) {
        self.exits.lock().unwrap().push(code);
    }
}
struct NoRead;
impl AsyncRead for NoRead {
    fn poll_read(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        _: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        panic!("metadata/early error must not read stdin")
    }
}
struct Fixture {
    _dir: tempfile::TempDir,
    cwd: String,
    agent: String,
    sessions: String,
    output: OutputGuard,
    stdout: Arc<MemoryOutput>,
    redirected: Arc<MemoryOutput>,
    process: Arc<TestProcess>,
    creates: Arc<AtomicUsize>,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().join("project");
        let agent = dir.path().join("agent");
        let sessions = dir.path().join("sessions");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&agent).unwrap();
        std::fs::create_dir_all(&sessions).unwrap();
        let stdout = Arc::new(MemoryOutput::default());
        let redirected = Arc::new(MemoryOutput::default());
        let process = Arc::new(TestProcess::default());
        let exit = process.clone();
        let output = OutputGuard::new(
            stdout.clone(),
            redirected.clone(),
            Arc::new(move |code| exit.exit(code)),
        );
        Self {
            _dir: dir,
            cwd: cwd.to_string_lossy().into(),
            agent: agent.to_string_lossy().into(),
            sessions: sessions.to_string_lossy().into(),
            stdout,
            redirected,
            process,
            output,
            creates: Arc::new(AtomicUsize::new(0)),
        }
    }
    fn options(&self, faux: Option<&FauxProviderHandle>) -> CliMainOptions {
        let mut options = CliMainOptions::new(&self.cwd, &self.agent);
        options.offline = true;
        options.env_session_dir = Some(self.sessions.clone());
        let creates = self.creates.clone();
        options.model_runtime_factory = Some(Arc::new(move |_, _, signal| {
            let creates = creates.clone();
            Box::pin(async move {
                creates.fetch_add(1, Ordering::SeqCst);
                ModelRuntime::create(CreateModelRuntimeOptions {
                    credentials: Some(Arc::new(InMemoryCredentialStore::default())),
                    models_path: Some(None),
                    models_store: Some(Arc::new(InMemoryCodingAgentModelsStore::default())),
                    allow_model_network: false,
                    refresh_on_create: Some(false),
                    signal: Some(signal),
                    ..Default::default()
                })
                .await
                .map_err(anyhow::Error::msg)
            })
        }));
        if let Some(faux) = faux {
            let provider = faux.provider.clone();
            options
                .extension_factories
                .push(InlineExtension::Factory(Arc::new(move |api| {
                    api.register_native_provider(&provider)
                })));
        }
        options
    }
    async fn run<I: AsyncRead + Unpin>(
        &self,
        args: &[&str],
        options: CliMainOptions,
        input: I,
    ) -> Result<i32> {
        let ui = Arc::new(HeadlessSessionUi {
            output: self.output.clone(),
            process: self.process.clone(),
        });
        tokio::time::timeout(
            Duration::from_secs(35),
            run_cli_with_io(
                &args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                options,
                input,
                self.output.clone(),
                self.process.clone(),
                ui,
            ),
        )
        .await
        .expect("CLI must finish")
    }
    fn errors(&self) -> String {
        self.process.errors.lock().unwrap().join("\n")
    }
}
fn faux() -> FauxProviderHandle {
    faux_provider(FauxProviderOptions {
        provider: Some("cli-entry-test".into()),
        ..Default::default()
    })
}
fn spec(faux: &FauxProviderHandle) -> String {
    let model = faux.get_model(None).unwrap();
    format!("{}/{}", model.provider, model.id)
}

#[tokio::test]
async fn early_errors_and_version_do_not_create_runtime_or_consume_stdin() {
    let oracle: Value = serde_json::from_str(include_str!("entry_oracle.json")).unwrap();
    for case in oracle["main"].as_array().unwrap().iter().filter(|case| {
        [
            "version",
            "rpc-file",
            "fork-conflict",
            "invalid-session-id",
            "empty-name",
        ]
        .contains(&case["id"].as_str().unwrap())
    }) {
        let args = case["argv"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect::<Vec<_>>();
        let f = Fixture::new();
        assert_eq!(
            f.run(&args, f.options(None), NoRead).await.unwrap(),
            case["exitCode"].as_i64().unwrap() as i32,
            "{}",
            case["id"]
        );
        assert_eq!(f.creates.load(Ordering::SeqCst), 0, "{}", case["id"]);
        assert_eq!(
            f.stdout.text(),
            case["stdout"].as_str().unwrap(),
            "{}",
            case["id"]
        );
        let stderr = if f.errors().is_empty() {
            String::new()
        } else {
            f.errors() + "\n"
        };
        assert_eq!(stderr, case["stderr"].as_str().unwrap(), "{}", case["id"]);
        assert!(!f.output.is_stdout_taken_over());
    }
}
#[tokio::test]
async fn unsupported_commands_are_never_dispatched_as_prompts() {
    for args in [
        vec!["install", "some-package"],
        vec!["config"],
        vec!["--export", "a.jsonl"],
    ] {
        let f = Fixture::new();
        assert_eq!(f.run(&args, f.options(None), NoRead).await.unwrap(), 1);
        assert!(f.errors().contains("not yet available"));
        assert_eq!(f.creates.load(Ordering::SeqCst), 0);
    }
    let f = Fixture::new();
    let mut options = f.options(None);
    options.stdin_is_tty = true;
    options.stdout_is_tty = true;
    assert_eq!(f.run(&[], options, NoRead).await.unwrap(), 1);
    assert!(f.errors().contains("Interactive mode"));
}
#[tokio::test]
async fn help_uses_real_extension_flags_and_metadata_ignores_runtime_errors() {
    for forced_print in [false, true] {
        let f = Fixture::new();
        let mut options = f.options(None);
        options
            .extension_factories
            .push(InlineExtension::Factory(Arc::new(|api| {
                api.register_flag(
                    "fixture-label",
                    Some("Native test flag".into()),
                    FlagType::String,
                    None,
                )
            })));
        let args = if forced_print {
            vec!["--print", "--help", "--unknown"]
        } else {
            vec!["--help", "--unknown"]
        };
        assert_eq!(f.run(&args, options, NoRead).await.unwrap(), 0);
        assert_eq!(f.creates.load(Ordering::SeqCst), 1);
        let text = if forced_print {
            assert!(f.stdout.text().is_empty());
            f.redirected.text()
        } else {
            f.stdout.text()
        };
        assert!(text.contains("--fixture-label"));
        assert!(!f.errors().contains("Unknown option"));
        assert!(!f.output.is_stdout_taken_over());
        assert!(SessionManager::list(&f.cwd, Some(&f.sessions), None).is_empty());
    }
}
#[tokio::test]
async fn list_models_uses_actual_runtime_and_does_not_poll_stdin() {
    let f = Fixture::new();
    let model = faux();
    assert_eq!(
        f.run(
            &["--list-models", "cli-entry-test"],
            f.options(Some(&model)),
            NoRead
        )
        .await
        .unwrap(),
        0
    );
    assert!(f.stdout.text().contains("cli-entry-test"));
    assert_eq!(model.state().lock().unwrap().call_count, 0);
    assert_eq!(f.stdout.1.load(Ordering::SeqCst), 0);
    assert!(f.errors().is_empty(), "{}", f.errors());
}
#[tokio::test]
async fn print_combines_file_stdin_and_two_messages_and_persists_named_session() {
    let f = Fixture::new();
    let model = faux();
    model.set_responses(vec![
        faux_assistant_message("first answer", FauxMessageOptions::default()).into(),
        faux_assistant_message("second answer", FauxMessageOptions::default()).into(),
    ]);
    std::fs::write(std::path::Path::new(&f.cwd).join("input.txt"), "file body").unwrap();
    let options = f.options(Some(&model));
    let result = f
        .run(
            &[
                "--print",
                "--no-tools",
                "--model",
                &spec(&model),
                "--name",
                " named ",
                "@input.txt",
                "first",
                "second",
            ],
            options,
            &b" piped input \n"[..],
        )
        .await
        .unwrap();
    assert_eq!(result, 0);
    // Upstream text mode runs all prompts, but prints only the last response.
    assert_eq!(f.stdout.text(), "second answer\n");
    assert_eq!(f.stdout.1.load(Ordering::SeqCst), 1);
    assert_eq!(model.state().lock().unwrap().call_count, 2);
    assert!(!f.output.is_stdout_taken_over());
    let sessions = SessionManager::list(&f.cwd, Some(&f.sessions), None);
    assert_eq!(sessions.len(), 1);
    let wire = std::fs::read_to_string(&sessions[0].path).unwrap();
    assert!(wire.contains("piped input"));
    assert!(wire.contains("file body"));
    assert!(wire.contains("first answer"));
    assert!(wire.contains("second answer"));
    assert!(wire.contains("\"name\":\"named\""));
}
#[tokio::test]
async fn json_cli_keeps_stdout_as_jsonl_and_shutdown_fires_once() {
    let f = Fixture::new();
    let model = faux();
    model.set_responses(vec![faux_assistant_message(
        "json answer",
        FauxMessageOptions::default(),
    )
    .into()]);
    let shutdown = Arc::new(AtomicUsize::new(0));
    let mut options = f.options(Some(&model));
    let counter = shutdown.clone();
    options
        .extension_factories
        .push(InlineExtension::Factory(Arc::new(move |api| {
            let counter = counter.clone();
            api.on(
                "session_shutdown",
                sync_handler(move |_, _| {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Ok(None)
                }),
            )?;
            Ok(())
        })));
    assert_eq!(
        f.run(
            &[
                "--mode",
                "json",
                "--no-session",
                "--no-tools",
                "--model",
                &spec(&model),
                "hi"
            ],
            options,
            &b""[..]
        )
        .await
        .unwrap(),
        0
    );
    let rows = f
        .stdout
        .text()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert!(rows.iter().any(|row| row["type"] == "message_end"));
    assert!(f.stdout.text().contains("json answer"));
    assert_eq!(shutdown.load(Ordering::SeqCst), 1);
    assert_eq!(f.stdout.1.load(Ordering::SeqCst), 1);
    assert!(f.errors().is_empty(), "{}", f.errors());
}
#[tokio::test]
async fn rpc_cli_preserves_stdin_for_protocol_and_gets_real_state() {
    let f = Fixture::new();
    let model = faux();
    let options = f.options(Some(&model));
    assert_eq!(
        f.run(
            &[
                "--mode",
                "rpc",
                "--no-session",
                "--no-tools",
                "--model",
                &spec(&model)
            ],
            options,
            &b"{\"id\":\"state\",\"type\":\"get_state\"}\n"[..]
        )
        .await
        .unwrap(),
        0
    );
    let rows = f
        .stdout
        .text()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    let state = rows.iter().find(|row| row["id"] == "state").unwrap();
    assert_eq!(state["success"], true);
    assert_eq!(state["data"]["model"]["provider"], "cli-entry-test");
    assert_eq!(model.state().lock().unwrap().call_count, 0);
    assert_eq!(f.stdout.1.load(Ordering::SeqCst), 1);
    assert!(f.errors().is_empty(), "{}", f.errors());
    assert!(!f.output.is_stdout_taken_over());
}
#[tokio::test]
async fn runtime_errors_prevent_generation_and_protocol_contamination() {
    let f = Fixture::new();
    let model = faux();
    assert_eq!(
        f.run(
            &[
                "--mode",
                "json",
                "--no-session",
                "--no-tools",
                "--model",
                &spec(&model),
                "--unregistered",
                "hello"
            ],
            f.options(Some(&model)),
            &b""[..]
        )
        .await
        .unwrap(),
        1
    );
    assert!(
        f.errors().contains("Unknown option: --unregistered"),
        "{}",
        f.errors()
    );
    assert!(f.stdout.text().is_empty());
    assert_eq!(model.state().lock().unwrap().call_count, 0);
    assert!(!f.output.is_stdout_taken_over());
}
#[tokio::test]
async fn benchmark_rejects_noninteractive_after_model_validation() {
    let f = Fixture::new();
    let model = faux();
    let mut options = f.options(Some(&model));
    options.startup_benchmark = true;
    assert_eq!(
        f.run(
            &[
                "--print",
                "--no-session",
                "--no-tools",
                "--model",
                &spec(&model),
                "hi"
            ],
            options,
            &b""[..]
        )
        .await
        .unwrap(),
        1
    );
    assert_eq!(
        f.errors(),
        "Error: PI_STARTUP_BENCHMARK only supports interactive mode"
    );
    assert_eq!(model.state().lock().unwrap().call_count, 0);
}
#[tokio::test]
async fn early_error_preserves_takeover_owned_by_embedding_host() {
    let f = Fixture::new();
    f.output.take_over_stdout();
    assert_eq!(
        f.run(&["--mode", "rpc", "@file"], f.options(None), NoRead)
            .await
            .unwrap(),
        1
    );
    assert!(f.output.is_stdout_taken_over());
    f.output.restore_stdout();
}
#[test]
fn terminal_capability_conversion_keeps_absent_and_explicit_false_distinct() {
    assert_eq!(
        terminal_capabilities(TerminalCapabilityOverrides::default()),
        CapabilityOverrides::default()
    );
    assert_eq!(
        terminal_capabilities(TerminalCapabilityOverrides {
            images: Some(TerminalImagesOverride::Cleared),
            true_color: Some(false),
            hyperlinks: Some(false)
        }),
        CapabilityOverrides {
            images: Some(None),
            true_color: Some(false),
            hyperlinks: Some(false)
        }
    );
    for (kind, name) in [
        (TerminalImagesOverride::Kitty, "kitty"),
        (TerminalImagesOverride::Iterm2, "iterm2"),
    ] {
        assert_eq!(
            terminal_capabilities(TerminalCapabilityOverrides {
                images: Some(kind),
                ..Default::default()
            })
            .images,
            Some(Some(name))
        );
    }
}
