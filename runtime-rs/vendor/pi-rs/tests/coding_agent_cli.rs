//! Native process tests: real CLI + services + session runtime + HTTP provider.
//! No parent credentials are inherited; all writable paths and model endpoints
//! are fixture-owned. Offline disables catalog traffic, not localhost inference.
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    process::{Output, Stdio},
    time::Duration,
};
use tokio::{io::AsyncWriteExt, process::Command};
use wiremock::{
    matchers::{method, path},
    Mock, MockServer, ResponseTemplate,
};

struct Cli {
    root: tempfile::TempDir,
    cwd: PathBuf,
    agent: PathBuf,
    sessions: PathBuf,
}
impl Cli {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path().join("project");
        let agent = root.path().join("agent");
        let sessions = root.path().join("sessions");
        for dir in [&cwd, &agent, &sessions] {
            std::fs::create_dir_all(dir).unwrap();
        }
        Self {
            root,
            cwd,
            agent,
            sessions,
        }
    }
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_pi-rust"));
        command.env_clear();
        // Windows loader/process necessities only; never forward provider auth,
        // proxy settings, user config, npm configuration or shell startup flags.
        for key in ["SystemRoot", "WINDIR", "COMSPEC", "PATH", "PATHEXT"] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        for key in [
            "HOME",
            "USERPROFILE",
            "APPDATA",
            "LOCALAPPDATA",
            "XDG_CONFIG_HOME",
            "XDG_CACHE_HOME",
            "TEMP",
            "TMP",
            "TMPDIR",
        ] {
            command.env(key, self.root.path());
        }
        command
            .env("PI_CODING_AGENT_DIR", &self.agent)
            .env("PI_CODING_AGENT_SESSION_DIR", &self.sessions)
            .env("PI_OFFLINE", "1")
            .env("PI_SKIP_VERSION_CHECK", "1")
            .env("NO_PROXY", "127.0.0.1,localhost")
            .env("NO_COLOR", "1")
            .env("TERM", "dumb")
            .current_dir(&self.cwd)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        command
    }
    async fn run(&self, args: &[&str], input: &[u8]) -> Output {
        let mut child = self.command(args).spawn().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        if !input.is_empty() {
            stdin.write_all(input).await.unwrap();
        }
        drop(stdin);
        tokio::time::timeout(Duration::from_secs(50), child.wait_with_output())
            .await
            .expect("native CLI timed out")
            .unwrap()
    }
    fn models(&self, endpoint: &str) {
        std::fs::write(
            self.agent.join("models.json"),
            json!({"providers": {"local-cli": {
                "baseUrl": format!("{endpoint}/v1"), "api": "openai-completions",
                "apiKey": "offline-test-only",
                "models": [{"id":"tiny","name":"Tiny","reasoning":false,
                    "input":["text"],"contextWindow":20000,"maxTokens":1000,
                    "cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0}}]
            }}})
            .to_string(),
        )
        .unwrap();
    }
}
fn stdout(output: &Output) -> &str {
    std::str::from_utf8(&output.stdout).unwrap()
}
fn stderr(output: &Output) -> &str {
    std::str::from_utf8(&output.stderr).unwrap()
}
fn success(output: &Output) {
    assert!(
        output.status.success(),
        "status {:?}\nstdout: {}\nstderr: {}",
        output.status,
        stdout(output),
        stderr(output)
    );
    assert_eq!(stderr(output), "");
}
fn rows(output: &Output) -> Vec<Value> {
    stdout(output)
        .lines()
        .map(|line| serde_json::from_str(line).expect("stdout must contain only JSONL"))
        .collect()
}
fn text_sse(text: &str) -> ResponseTemplate {
    sse(&[
        json!({"id":"chatcmpl-local", "choices":[{"index":0,"delta":{"role":"assistant","content":text},"finish_reason":null}]}),
        json!({"id":"chatcmpl-local", "choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":4,"completion_tokens":2,"total_tokens":6}}),
    ])
}
fn sse(chunks: &[Value]) -> ResponseTemplate {
    let mut body = chunks
        .iter()
        .map(|chunk| format!("data: {chunk}\n\n"))
        .collect::<String>();
    body.push_str("data: [DONE]\n\n");
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(body)
}

#[tokio::test]
async fn native_version_help_and_flag_errors_exit_without_stdin_eof() {
    let cli = Cli::new();
    let expected = [
        (vec!["--version"], 0, "0.85.1\n"),
        (vec!["--mode", "rpc", "@input.txt"], 1, ""),
        (vec!["--session-id", "../bad"], 1, ""),
        (vec!["auth", "check", "--nonesuch"], 1, ""),
    ];
    for (args, code, text) in expected {
        let mut child = cli.command(&args).spawn().unwrap();
        let held_stdin = child.stdin.take().unwrap();
        let output = tokio::time::timeout(Duration::from_secs(12), child.wait_with_output())
            .await
            .expect("preflight must not read stdin")
            .unwrap();
        drop(held_stdin);
        assert_eq!(
            output.status.code(),
            Some(code),
            "{:?}: {}",
            args,
            stderr(&output)
        );
        assert_eq!(stdout(&output), text);
        if code != 0 {
            assert!(!stderr(&output).is_empty());
        }
    }
    let output = cli.run(&["--help"], b"").await;
    success(&output);
    assert!(stdout(&output).contains("--mode"));
    assert!(stdout(&output).contains("--session"));
    assert_eq!(std::fs::read_dir(&cli.sessions).unwrap().count(), 0);
}

#[tokio::test]
async fn native_auth_readiness_is_credential_opt_in_and_readonly_failure_preserves_bytes() {
    let cli = Cli::new();
    let auth = cli.agent.join("auth.json");
    std::fs::write(
        &auth,
        r#"{"anthropic":{"type":"api_key","key":"offline-test-only"}}"#,
    )
    .unwrap();
    let args = [
        "auth",
        "check",
        "--provider",
        "anthropic",
        "--no-refresh",
        "--json",
    ];
    let output = cli.run(&args, b"").await;
    success(&output);
    assert_eq!(
        stdout(&output),
        "{\"status\":\"ready\",\"provider\":\"anthropic\",\"authType\":\"api_key\"}\n"
    );
    assert!(!stdout(&output).contains("offline-test-only"));
    let mut credentials = args.to_vec();
    credentials.push("--credentials");
    let output = cli.run(&credentials, b"").await;
    success(&output);
    assert_eq!(rows(&output)[0]["credentials"], "offline-test-only");
    let corrupt = b"{ bad-json\r\n";
    std::fs::write(&auth, corrupt).unwrap();
    let output = cli.run(&args, b"").await;
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(stderr(&output), "");
    assert_eq!(
        stdout(&output),
        "{\"status\":\"invalid\",\"provider\":\"anthropic\",\"reason\":\"invalid_state\"}\n"
    );
    assert_eq!(std::fs::read(auth).unwrap(), corrupt);
}

#[tokio::test]
async fn native_print_uses_local_http_and_persists_combined_input_in_real_session() {
    let cli = Cli::new();
    let server = MockServer::start().await;
    cli.models(&server.uri());
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(text_sse("native answer"))
        .expect(1)
        .mount(&server)
        .await;
    std::fs::write(cli.cwd.join("input.txt"), "file payload").unwrap();
    let output = cli
        .run(
            &[
                "--print",
                "--model",
                "local-cli/tiny",
                "--no-tools",
                "--name",
                " CLI session ",
                "@input.txt",
                "explain",
            ],
            b"  piped payload \n",
        )
        .await;
    success(&output);
    assert_eq!(stdout(&output), "native answer\n");
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].headers.get("authorization").unwrap(),
        "Bearer offline-test-only"
    );
    let body: Value = requests[0].body_json().unwrap();
    assert_eq!(body["model"], "tiny");
    assert_eq!(body["stream"], true);
    let wire = body.to_string();
    assert!(
        wire.contains("piped payload") && wire.contains("file payload") && wire.contains("explain")
    );
    let sessions = pi_rust::coding_agent::session_manager::SessionManager::list(
        cli.cwd.to_str().unwrap(),
        Some(cli.sessions.to_str().unwrap()),
        None,
    );
    assert_eq!(sessions.len(), 1);
    let wire = std::fs::read_to_string(&sessions[0].path).unwrap();
    assert!(wire.contains("\"name\":\"CLI session\""));
    assert!(wire.contains("native answer") && wire.contains("piped payload"));
}

#[tokio::test]
async fn native_json_is_protocol_only_and_no_session_does_not_persist() {
    let cli = Cli::new();
    let server = MockServer::start().await;
    cli.models(&server.uri());
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(text_sse("native json answer"))
        .expect(1)
        .mount(&server)
        .await;
    let output = cli
        .run(
            &[
                "--mode",
                "json",
                "--model",
                "local-cli/tiny",
                "--no-tools",
                "--no-session",
                "hi",
            ],
            b"",
        )
        .await;
    success(&output);
    let rows = rows(&output);
    assert!(rows
        .iter()
        .any(|row| row["type"] == "message_end" && row["message"]["role"] == "assistant"));
    assert!(stdout(&output).contains("native json answer"));
    assert_eq!(std::fs::read_dir(&cli.sessions).unwrap().count(), 0);
}

#[tokio::test]
async fn native_rpc_reads_commands_without_making_model_requests() {
    let cli = Cli::new();
    let server = MockServer::start().await;
    cli.models(&server.uri());
    let output = cli
        .run(
            &[
                "--mode",
                "rpc",
                "--model",
                "local-cli/tiny",
                "--no-tools",
                "--no-session",
            ],
            b"{\"id\":\"state\",\"type\":\"get_state\"}\n",
        )
        .await;
    success(&output);
    let rows = rows(&output);
    let state = rows.iter().find(|row| row["id"] == "state").unwrap();
    assert_eq!(state["type"], "response");
    assert_eq!(state["success"], true);
    assert_eq!(state["data"]["model"]["provider"], "local-cli");
    assert_eq!(state["data"]["model"]["id"], "tiny");
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn native_tool_loop_reads_project_file_then_resumes_provider() {
    let cli = Cli::new();
    let server = MockServer::start().await;
    cli.models(&server.uri());
    std::fs::write(cli.cwd.join("tool.txt"), "tool-local-content").unwrap();
    Mock::given(method("POST")).and(path("/v1/chat/completions"))
        .respond_with(|request: &wiremock::Request| {
            let body: Value = request.body_json().unwrap();
            let done = body["messages"].as_array().unwrap().iter().any(|message| message["role"] == "tool");
            if done { text_sse("read completed") } else {
                sse(&[
                    json!({"id":"chatcmpl-tool","choices":[{"index":0,"delta":{"role":"assistant","tool_calls":[{"index":0,"id":"call_read","type":"function","function":{"name":"read","arguments":"{\"path\":\"tool.txt\"}"}}]},"finish_reason":null}]}),
                    json!({"id":"chatcmpl-tool","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}),
                ])
            }
        }).expect(2).mount(&server).await;
    let output = cli
        .run(
            &[
                "--print",
                "--model",
                "local-cli/tiny",
                "--tools",
                "read",
                "--no-session",
                "read tool.txt",
            ],
            b"",
        )
        .await;
    success(&output);
    assert_eq!(stdout(&output), "read completed\n");
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    let body: Value = requests[1].body_json().unwrap();
    let result = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["role"] == "tool")
        .unwrap();
    assert_eq!(result["tool_call_id"], "call_read");
    assert!(result["content"].to_string().contains("tool-local-content"));
}

#[tokio::test]
async fn native_tool_error_still_resumes_provider_and_reaches_agent_end() {
    let cli = Cli::new();
    let server = MockServer::start().await;
    cli.models(&server.uri());
    Mock::given(method("POST")).and(path("/v1/chat/completions"))
        .respond_with(|request: &wiremock::Request| {
            let body: Value = request.body_json().unwrap();
            let done = body["messages"].as_array().unwrap().iter().any(|message| message["role"] == "tool");
            if done { text_sse("read completed") } else {
                sse(&[
                    json!({"id":"chatcmpl-tool","choices":[{"index":0,"delta":{"role":"assistant","tool_calls":[{"index":0,"id":"call_read","type":"function","function":{"name":"read","arguments":"{\"path\":\"tool.txt\"}"}}]},"finish_reason":null}]}),
                    json!({"id":"chatcmpl-tool","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}),
                ])
            }
        }).expect(2).mount(&server).await;
    let output = cli
        .run(
            &[
                "--mode",
                "json",
                "--model",
                "local-cli/tiny",
                "--tools",
                "read",
                "--no-session",
                "read tool.txt",
            ],
            b"",
        )
        .await;
    success(&output);
    assert!(rows(&output).iter().any(|row| row["type"] == "agent_end"));
    assert!(stdout(&output).contains("read completed"));
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    let body: Value = requests[1].body_json().unwrap();
    let result = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["role"] == "tool")
        .unwrap();
    assert_eq!(result["tool_call_id"], "call_read");
    assert!(result["content"].to_string().contains("ENOENT"));
}

#[tokio::test]
async fn native_startup_migrates_credentials_and_files_without_protocol_contamination() {
    for mode in ["json", "rpc"] {
        let cli = Cli::new();
        let server = MockServer::start().await;
        cli.models(&server.uri());
        let models_file = cli.agent.join("models.json");
        let mut models: Value =
            serde_json::from_slice(&std::fs::read(&models_file).unwrap()).unwrap();
        models["providers"]["local-cli"]
            .as_object_mut()
            .unwrap()
            .shift_remove("apiKey");
        std::fs::write(models_file, models.to_string()).unwrap();
        std::fs::write(
            cli.agent.join("settings.json"),
            r#"{"apiKeys":{"local-cli":"legacy-fixture"},"theme":"dark"}"#,
        )
        .unwrap();
        std::fs::write(
            cli.agent.join("keybindings.json"),
            r#"{"interrupt":"ctrl+c"}"#,
        )
        .unwrap();
        std::fs::create_dir(cli.agent.join("hooks")).unwrap();
        for base in [&cli.agent, &cli.cwd.join(".pi")] {
            std::fs::create_dir_all(base.join("commands")).unwrap();
            std::fs::write(base.join("commands/example.md"), "legacy prompt").unwrap();
        }
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(text_sse("migration reached provider"))
            .expect(if mode == "json" { 1 } else { 0 })
            .mount(&server)
            .await;
        let mut args = vec![
            "--mode",
            mode,
            "--model",
            "local-cli/tiny",
            "--no-tools",
            "--no-session",
        ];
        if mode == "json" {
            args.push("hi");
        }
        let input = if mode == "rpc" {
            b"{\"id\":\"state\",\"type\":\"get_state\"}\n".as_slice()
        } else {
            b"".as_slice()
        };
        let output = cli.run(&args, input).await;
        assert!(
            output.status.success(),
            "{}\n{}",
            stdout(&output),
            stderr(&output)
        );
        let rows = rows(&output);
        assert!(!rows.is_empty());
        assert_eq!(
            stderr(&output),
            "Migrated Global commands/ → prompts/\nMigrated Project commands/ → prompts/\n"
        );
        assert!(!stdout(&output).contains("legacy-fixture"));
        for base in [&cli.agent, &cli.cwd.join(".pi")] {
            assert!(!base.join("commands").exists());
            assert_eq!(
                std::fs::read_to_string(base.join("prompts/example.md")).unwrap(),
                "legacy prompt"
            );
        }
        let auth: Value =
            serde_json::from_slice(&std::fs::read(cli.agent.join("auth.json")).unwrap()).unwrap();
        assert_eq!(
            auth,
            json!({"local-cli":{"type":"api_key","key":"legacy-fixture"}})
        );
        let settings: Value =
            serde_json::from_slice(&std::fs::read(cli.agent.join("settings.json")).unwrap())
                .unwrap();
        assert_eq!(settings, json!({"theme":"dark"}));
        let bindings: Value =
            serde_json::from_slice(&std::fs::read(cli.agent.join("keybindings.json")).unwrap())
                .unwrap();
        assert_eq!(bindings, json!({"app.interrupt":"ctrl+c"}));
        let requests = server.received_requests().await.unwrap();
        if mode == "json" {
            assert!(rows.iter().any(|row| row["type"] == "agent_end"));
            assert_eq!(
                requests[0].headers.get("authorization").unwrap(),
                "Bearer legacy-fixture"
            );
        } else {
            assert!(requests.is_empty());
            assert!(rows
                .iter()
                .any(|row| row["id"] == "state" && row["success"] == true));
        }
    }
}

#[tokio::test]
async fn native_version_and_preflight_errors_leave_legacy_files_untouched() {
    let cli = Cli::new();
    let oauth = r#"{"legacy":{"access":"offline-only"}}"#;
    std::fs::write(cli.agent.join("oauth.json"), oauth).unwrap();
    std::fs::create_dir(cli.agent.join("commands")).unwrap();
    std::fs::write(cli.agent.join("commands/keep.md"), "keep").unwrap();
    for args in [
        vec!["--version"],
        vec!["--mode", "rpc", "@file.txt"],
        vec!["--session-id", "../bad"],
    ] {
        let output = cli.run(&args, b"").await;
        assert_eq!(output.status.success(), args == ["--version"]);
        assert!(!cli.agent.join("auth.json").exists());
        assert!(!cli.agent.join("oauth.json.migrated").exists());
        assert_eq!(
            std::fs::read_to_string(cli.agent.join("oauth.json")).unwrap(),
            oauth
        );
        assert!(cli.agent.join("commands/keep.md").exists());
    }
}

#[tokio::test]
async fn native_shell_tools_execute_with_current_session_settings_and_resume_provider() {
    let names = if cfg!(windows) {
        vec!["bash", "powershell"]
    } else {
        vec!["bash"]
    };
    for name in names {
        let cli = Cli::new();
        let server = MockServer::start().await;
        cli.models(&server.uri());
        let command = if name == "bash" {
            let shell = pi_rust::coding_agent::utils::shell_config::get_shell_config(None)
                .await
                .unwrap();
            std::fs::write(cli.agent.join("settings.json"),json!({"shellPath":shell.shell,"shellCommandPrefix":"export PI_RUST_PREFIX=from-prefix"}).to_string()).unwrap();
            "printf '%s|%s|%s|%s|%s|%s\\n' \"$PI_RUST_PREFIX\" \"$PI_PROVIDER\" \"$PI_MODEL\" \"$PI_REASONING_LEVEL\" \"$PI_SESSION_ID\" \"${PI_SESSION_FILE-unset}\"; printf 'SHELL_STDERR\\n' >&2; printf 'native-shell' > shell-file.txt"
        } else {
            // PowerShell must not inherit bash-only shellPath or commandPrefix.
            std::fs::write(
                cli.agent.join("settings.json"),
                json!({"shellPath":"/absent-bash","shellCommandPrefix":"not valid shell code"})
                    .to_string(),
            )
            .unwrap();
            "[Console]::WriteLine('POWER|'+$env:PI_PROVIDER+'|'+$env:PI_MODEL+'|'+$env:PI_REASONING_LEVEL+'|'+$env:PI_SESSION_ID); [Console]::Error.WriteLine('SHELL_STDERR'); Set-Content -LiteralPath 'shell-file.txt' -Value 'native-shell' -NoNewline"
        };
        let args = json!({"command":command,"timeout":15}).to_string();
        let tool = name.to_owned();
        Mock::given(method("POST")).and(path("/v1/chat/completions")).respond_with(move |request:&wiremock::Request|{
            let body:Value=request.body_json().unwrap();
            if body["messages"].as_array().unwrap().iter().any(|m|m["role"]=="tool"){text_sse("shell completed")}
            else{sse(&[
                json!({"id":"shell-call","choices":[{"index":0,"delta":{"role":"assistant","tool_calls":[{"index":0,"id":"call_shell","type":"function","function":{"name":tool,"arguments":args}}]},"finish_reason":null}]}),
                json!({"id":"shell-call","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}),
            ])}
        }).expect(2).mount(&server).await;
        let output = cli
            .run(
                &[
                    "--mode",
                    "json",
                    "--model",
                    "local-cli/tiny",
                    "--tools",
                    name,
                    "--no-session",
                    "run a local shell",
                ],
                b"",
            )
            .await;
        success(&output);
        assert!(rows(&output).iter().any(|row| row["type"] == "agent_end"));
        assert_eq!(
            std::fs::read_to_string(cli.cwd.join("shell-file.txt")).unwrap(),
            "native-shell"
        );
        let requests = server.received_requests().await.unwrap();
        let first: Value = requests[0].body_json().unwrap();
        let tools = first["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["function"]["name"], name);
        assert_eq!(
            tools[0]["function"]["parameters"]["properties"]["command"]["type"],
            "string"
        );
        let body: Value = requests[1].body_json().unwrap();
        let result = body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["role"] == "tool")
            .unwrap();
        let text = result["content"].to_string();
        let prefix = if name == "bash" {
            "from-prefix"
        } else {
            "POWER"
        };
        assert!(
            text.contains(&format!("{prefix}|local-cli|tiny|off|")),
            "{text}"
        );
        assert!(text.contains("SHELL_STDERR"), "{text}");
        assert!(!text.contains("Shell environment requires"));
        assert_eq!(std::fs::read_dir(&cli.sessions).unwrap().count(), 0);
    }
}
#[tokio::test]
async fn native_shell_nonzero_exit_is_tool_error_not_cli_or_provider_failure() {
    let cli = Cli::new();
    let server = MockServer::start().await;
    cli.models(&server.uri());
    let shell = pi_rust::coding_agent::utils::shell_config::get_shell_config(None)
        .await
        .unwrap();
    std::fs::write(
        cli.agent.join("settings.json"),
        json!({"shellPath":shell.shell}).to_string(),
    )
    .unwrap();
    Mock::given(method("POST")).and(path("/v1/chat/completions")).respond_with(|request:&wiremock::Request|{
        let body:Value=request.body_json().unwrap();
        if body["messages"].as_array().unwrap().iter().any(|m|m["role"]=="tool"){text_sse("failure handled")}
        else{sse(&[
            json!({"id":"shell-failure","choices":[{"index":0,"delta":{"role":"assistant","tool_calls":[{"index":0,"id":"call_shell","type":"function","function":{"name":"bash","arguments":"{\"command\":\"printf failure-output; exit 7\"}"}}]},"finish_reason":null}]}),
            json!({"id":"shell-failure","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}),
        ])}
    }).expect(2).mount(&server).await;
    let output = cli
        .run(
            &[
                "--mode",
                "json",
                "--model",
                "local-cli/tiny",
                "--tools",
                "bash",
                "--no-session",
                "run a failing command",
            ],
            b"",
        )
        .await;
    success(&output);
    assert!(rows(&output).iter().any(|row| row["type"] == "agent_end"));
    assert!(stdout(&output).contains("failure handled"));
    let requests = server.received_requests().await.unwrap();
    let body: Value = requests[1].body_json().unwrap();
    let result = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "tool")
        .unwrap();
    let text = result["content"].to_string();
    assert!(
        text.contains("failure-output") && text.contains("Command exited with code 7"),
        "{text}"
    );
}

async fn read_rpc_until(
    lines: &mut tokio::io::Lines<tokio::io::BufReader<tokio::process::ChildStdout>>,
    seen: &mut Vec<Value>,
    matches: impl Fn(&Value) -> bool,
) -> Value {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let line = lines
                .next_line()
                .await
                .unwrap()
                .expect("RPC stdout closed early");
            let value: Value = serde_json::from_str(&line).expect("only JSONL on RPC stdout");
            seen.push(value.clone());
            if matches(&value) {
                return value;
            }
        }
    })
    .await
    .expect("RPC response timed out")
}
async fn send_rpc(stdin: &mut tokio::process::ChildStdin, value: Value) {
    stdin
        .write_all(format!("{value}\n").as_bytes())
        .await
        .unwrap();
    stdin.flush().await.unwrap();
}
#[tokio::test]
async fn native_rpc_shell_streams_sanitizes_records_and_cancels_without_provider_calls() {
    use tokio::io::AsyncBufReadExt;
    let cli = Cli::new();
    let server = MockServer::start().await;
    cli.models(&server.uri());
    let shell = pi_rust::coding_agent::utils::shell_config::get_shell_config(None)
        .await
        .unwrap();
    std::fs::write(
        cli.agent.join("settings.json"),
        json!({"shellPath":shell.shell,"shellCommandPrefix":"export PI_RUST_PREFIX=rpc-prefix"})
            .to_string(),
    )
    .unwrap();
    let mut child = cli
        .command(&[
            "--mode",
            "rpc",
            "--model",
            "local-cli/tiny",
            "--no-tools",
            "--no-session",
        ])
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut lines = tokio::io::BufReader::new(child.stdout.take().unwrap()).lines();
    let mut seen = Vec::new();
    let command = "printf '\\357\\273\\277\\033[31m%s\\033[0m\\r\\n' \"$PI_RUST_PREFIX\"; printf 'err\\r\\n' >&2; printf rpc-shell > rpc-file.txt; exit 7";
    send_rpc(
        &mut stdin,
        json!({"id":"shell-success","type":"bash","command":command,"excludeFromContext":true}),
    )
    .await;
    let result = read_rpc_until(&mut lines, &mut seen, |v| {
        v["type"] == "response" && v["id"] == "shell-success"
    })
    .await;
    assert_eq!(result["success"], true, "{result}");
    assert_eq!(result["data"]["exitCode"], 7);
    assert_eq!(result["data"]["cancelled"], false);
    let text = result["data"]["output"].as_str().unwrap();
    assert!(
        text.contains("rpc-prefix\n") && text.contains("err\n"),
        "{text:?}"
    );
    assert!(!text.contains('\r') && !text.contains('\u{1b}') && !text.contains('\u{feff}'));
    let deltas = seen
        .iter()
        .filter(|v| v["type"] == "bash_execution_update" && v["id"] == "shell-success")
        .map(|v| v["delta"].as_str().unwrap())
        .collect::<String>();
    assert_eq!(deltas, text);
    assert_eq!(
        std::fs::read_to_string(cli.cwd.join("rpc-file.txt")).unwrap(),
        "rpc-shell"
    );

    send_rpc(&mut stdin, json!({"id":"shell-cancel","type":"bash","command":"printf CANCEL_READY; sleep 8; printf NEVER; printf leaked > cancelled-marker.txt"})).await;
    read_rpc_until(&mut lines, &mut seen, |v| {
        v["type"] == "bash_execution_update"
            && v["id"] == "shell-cancel"
            && v["delta"]
                .as_str()
                .is_some_and(|s| s.contains("CANCEL_READY"))
    })
    .await;
    send_rpc(&mut stdin, json!({"id":"cancel","type":"abort_bash"})).await;
    let result = read_rpc_until(&mut lines, &mut seen, |v| {
        v["type"] == "response" && v["id"] == "shell-cancel"
    })
    .await;
    assert_eq!(result["success"], true, "{result}");
    assert_eq!(result["data"]["cancelled"], true);
    assert!(result["data"].get("exitCode").is_none());
    assert_eq!(result["data"]["output"], "CANCEL_READY");
    send_rpc(&mut stdin, json!({"id":"messages","type":"get_messages"})).await;
    let messages = read_rpc_until(&mut lines, &mut seen, |v| {
        v["type"] == "response" && v["id"] == "messages"
    })
    .await;
    let entries = messages["data"]["messages"].as_array().unwrap();
    assert_eq!(entries.len(), 2, "{messages}");
    assert_eq!(entries[0]["role"], "bashExecution");
    assert_eq!(entries[0]["command"], command);
    assert_eq!(entries[0]["excludeFromContext"], true);
    assert_eq!(entries[1]["cancelled"], true);
    assert!(seen
        .iter()
        .any(|v| v["id"] == "cancel" && v["success"] == true));
    drop(stdin);
    let output = tokio::time::timeout(Duration::from_secs(15), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    success(&output);
    assert!(!cli.cwd.join("cancelled-marker.txt").exists());
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn native_search_tools_use_local_rg_and_report_offline_fd_without_polluting_protocol() {
    // the grep leg copies the host's ripgrep binary into the agent bin dir;
    // shared runners ship no ripgrep, so the probe degrades to the find leg
    // there instead of failing the host-environment assertion
    #[cfg(windows)]
    let filename = "rg.exe";
    #[cfg(not(windows))]
    let filename = "rg";
    let ripgrep_on_host = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join(filename))
        .any(|path| path.is_file());
    for kind in ["grep", "find"] {
        if kind == "grep" && !ripgrep_on_host {
            continue;
        }
        let cli = Cli::new();
        let server = MockServer::start().await;
        cli.models(&server.uri());
        std::fs::write(
            cli.cwd.join("matches.txt"),
            "before\nneedle present\nafter\n",
        )
        .unwrap();
        std::fs::write(cli.cwd.join(".gitignore"), "ignored.txt\n").unwrap();
        std::fs::write(cli.cwd.join("ignored.txt"), "needle ignored\n").unwrap();
        std::fs::create_dir(cli.cwd.join(".git")).unwrap();
        if kind == "grep" {
            let path = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
                .map(|dir| dir.join(filename))
                .find(|path| path.is_file())
                .expect("ripgrep presence was checked before the loop");
            let bin = cli.agent.join("bin");
            std::fs::create_dir_all(&bin).unwrap();
            std::fs::copy(path, bin.join(filename)).unwrap();
        }
        let tool = kind.to_string();
        let arguments = if kind == "grep" {
            json!({"pattern":"needle","context":1})
        } else {
            json!({"pattern":"*.txt"})
        }
        .to_string();
        Mock::given(method("POST")).and(path("/v1/chat/completions")).respond_with(move |request:&wiremock::Request| {
            let body:Value=request.body_json().unwrap();
            if body["messages"].as_array().unwrap().iter().any(|m|m["role"]=="tool"){text_sse("search completed")}
            else{sse(&[
                json!({"id":"search-call","choices":[{"index":0,"delta":{"role":"assistant","tool_calls":[{"index":0,"id":"call_search","type":"function","function":{"name":tool,"arguments":arguments}}]},"finish_reason":null}]}),
                json!({"id":"search-call","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}),
            ])}
        }).expect(2).mount(&server).await;
        let mut command = cli.command(&[
            "--mode",
            "json",
            "--model",
            "local-cli/tiny",
            "--tools",
            kind,
            "--no-session",
            "search local files",
        ]);
        // Only the managed rg binary can be used; fd must fail closed offline.
        command.env("PATH", cli.root.path().join("no-system-tools"));
        let mut child = command.spawn().unwrap();
        drop(child.stdin.take());
        let output = tokio::time::timeout(Duration::from_secs(50), child.wait_with_output())
            .await
            .unwrap()
            .unwrap();
        success(&output);
        assert!(rows(&output).iter().any(|row| row["type"] == "agent_end"));
        let requests = server.received_requests().await.unwrap();
        let body: Value = requests[0].body_json().unwrap();
        assert_eq!(
            body["tools"][0]["function"]["parameters"]["properties"]["pattern"]["type"],
            "string"
        );
        let body: Value = requests[1].body_json().unwrap();
        let result = body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["role"] == "tool")
            .unwrap();
        let text = result["content"].to_string();
        if kind == "grep" {
            assert!(
                text.contains("matches.txt-1- before")
                    && text.contains("matches.txt:2: needle present")
                    && text.contains("matches.txt-3- after"),
                "{text}"
            );
            assert!(!text.contains("needle ignored"), "{text}");
        } else {
            assert!(
                text.contains("fd is not available and could not be downloaded"),
                "{text}"
            );
        }
        server.verify().await;
    }
}

#[tokio::test]
async fn native_ls_lists_dotfiles_directories_and_continues_provider_without_external_tools() {
    let cli = Cli::new();
    let server = MockServer::start().await;
    cli.models(&server.uri());
    for name in ["z.txt", "a.txt", ".hidden"] {
        std::fs::write(cli.cwd.join(name), "local").unwrap();
    }
    std::fs::create_dir(cli.cwd.join("dir")).unwrap();
    Mock::given(method("POST")).and(path("/v1/chat/completions")).respond_with(|request: &wiremock::Request| {
        let body: Value = request.body_json().unwrap();
        if body["messages"].as_array().unwrap().iter().any(|m| m["role"] == "tool") { text_sse("listing completed") }
        else { sse(&[
            json!({"id":"ls-call","choices":[{"index":0,"delta":{"role":"assistant","tool_calls":[{"index":0,"id":"call_ls","type":"function","function":{"name":"ls","arguments":"{}"}}]},"finish_reason":null}]}),
            json!({"id":"ls-call","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}),
        ]) }
    }).expect(2).mount(&server).await;
    let mut command = cli.command(&[
        "--mode",
        "json",
        "--model",
        "local-cli/tiny",
        "--tools",
        "ls",
        "--no-session",
        "list files",
    ]);
    command.env("PATH", cli.root.path().join("no-external-tools"));
    let mut child = command.spawn().unwrap();
    drop(child.stdin.take());
    let output = tokio::time::timeout(Duration::from_secs(50), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    success(&output);
    assert!(rows(&output).iter().any(|row| row["type"] == "agent_end"));
    let requests = server.received_requests().await.unwrap();
    let body: Value = requests[0].body_json().unwrap();
    assert_eq!(
        body["tools"][0]["function"]["parameters"]["properties"]["limit"]["type"],
        "number"
    );
    let body: Value = requests[1].body_json().unwrap();
    let result = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "tool")
        .unwrap();
    assert_eq!(result["tool_call_id"], "call_ls");
    assert!(
        result["content"]
            .to_string()
            .contains(r".hidden\na.txt\ndir/\nz.txt"),
        "{result}"
    );
    server.verify().await;
}
