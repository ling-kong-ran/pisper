//! Tests for the ported experimental CLI (`cli/experimental/*`) — oracle
//! byte-comparisons for the parse battery plus the upstream
//! `experimental-cli-command.test.ts` / `experimental-cli-resolution.test.ts`
//! execute scenarios.

use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{json, Value};

use crate::coding_agent::cli::experimental::cli::{execute, parse};
use crate::coding_agent::cli::experimental::command::{BuiltInvocation, CommandActionContext};
use crate::coding_agent::cli::experimental::command_options::{parse_transport_address, AuthInput};
use crate::coding_agent::cli::experimental::commands::client::ClientCommand;
use crate::coding_agent::cli::experimental::commands::server::ServerCommand;

const ORACLE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/cli_oracle/oracle.json"
));

fn oracle() -> &'static Value {
    use std::sync::OnceLock;
    static ORACLE_VALUE: OnceLock<Value> = OnceLock::new();
    ORACLE_VALUE.get_or_init(|| serde_json::from_str(ORACLE).expect("oracle json"))
}

fn argv(key: &str) -> Vec<String> {
    serde_json::from_str(key).expect("JSON argv")
}

fn auth_to_json(auth: &AuthInput) -> Value {
    match auth {
        AuthInput::Token { token } => json!({ "type": "token", "token": token }),
        AuthInput::File { path } => json!({ "type": "file", "path": path }),
    }
}

fn invocation_to_json(invocation: &BuiltInvocation) -> Value {
    let mut out = match invocation {
        BuiltInvocation::Server(ServerCommand {
            command,
            auth,
            provider,
            model,
            plugin_packages,
            server_id,
            session_dir,
        }) => {
            let mut out = json!({ "command": command });
            if let Some(auth) = auth {
                out["auth"] = auth_to_json(auth);
            }
            if let Some(provider) = provider {
                out["provider"] = json!(provider);
            }
            if let Some(model) = model {
                out["model"] = json!(model);
            }
            if let Some(plugin_packages) = plugin_packages {
                out["pluginPackages"] = json!(plugin_packages);
            }
            if let Some(server_id) = server_id {
                out["serverId"] = json!(server_id);
            }
            if let Some(session_dir) = session_dir {
                out["sessionDir"] = json!(session_dir);
            }
            out
        }
        BuiltInvocation::Client(ClientCommand {
            command,
            auth,
            connect,
            session_id,
            r#continue,
            resume,
            provider,
            model,
            plugin_packages,
            prompt,
        }) => {
            let mut out = json!({ "command": command });
            if let Some(auth) = auth {
                out["auth"] = auth_to_json(auth);
            }
            if let Some(connect) = connect {
                out["connect"] = match connect {
                    crate::coding_agent::cli::experimental::command_options::TransportAddress::Unix { path } => {
                        json!({ "transport": "unix", "path": path })
                    }
                    crate::coding_agent::cli::experimental::command_options::TransportAddress::Radius { server_id } => {
                        json!({ "transport": "radius", "serverId": server_id })
                    }
                };
            }
            if let Some(session_id) = session_id {
                out["sessionId"] = json!(session_id);
            }
            if *r#continue {
                out["continue"] = json!(true);
            }
            if *resume {
                out["resume"] = json!(true);
            }
            if let Some(provider) = provider {
                out["provider"] = json!(provider);
            }
            if let Some(model) = model {
                out["model"] = json!(model);
            }
            if let Some(plugin_packages) = plugin_packages {
                out["pluginPackages"] = json!(plugin_packages);
            }
            if let Some(prompt) = prompt {
                out["prompt"] = json!(prompt);
            }
            out
        }
        BuiltInvocation::Group(name) => json!({ "command": name }),
    };
    // Key order is irrelevant for comparison; sort object keys via round-trip.
    normalize(&mut out);
    out
}

fn normalize(value: &mut Value) {
    match value {
        Value::Object(map) => {
            let keys: Vec<String> = map.keys().cloned().collect();
            for key in keys {
                if let Some(mut entry) = map.shift_remove(&key) {
                    normalize(&mut entry);
                    map.insert(key, entry);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(normalize),
        _ => {}
    }
}

#[test]
fn experimental_parse_matches_the_node_oracle_byte_for_byte() {
    let cases = oracle()["experimental"]
        .as_object()
        .expect("experimental object");
    for (key, expected) in cases {
        let actual = match parse(&argv(key)) {
            Ok(invocation) => json!({ "ok": true, "command": invocation_to_json(&invocation) }),
            Err(errors) => json!({ "ok": false, "errors": errors }),
        };
        assert_eq!(actual, *expected, "experimental parse mismatch for {key}");
    }
}

#[test]
fn transport_address_grammar_matches_upstream() {
    assert_eq!(
        parse_transport_address("unix:///tmp/pi.sock"),
        Ok(
            crate::coding_agent::cli::experimental::command_options::TransportAddress::Unix {
                path: "/tmp/pi.sock".to_string()
            }
        )
    );
    assert!(parse_transport_address("unix:///tmp/pi.sock%zz").is_err());
    assert!(parse_transport_address("unix:///tmp/%C3%A9").is_ok());
}

#[derive(Default)]
struct RecordingContext {
    server_calls: AtomicUsize,
    client_calls: AtomicUsize,
    last_server: std::sync::Mutex<Option<ServerCommand>>,
}

impl CommandActionContext for RecordingContext {
    fn run_server(&self, command: &ServerCommand) {
        self.server_calls.fetch_add(1, Ordering::SeqCst);
        *self.last_server.lock().unwrap() = Some(command.clone());
    }
    fn run_client(&self, _command: &ClientCommand) {
        self.client_calls.fetch_add(1, Ordering::SeqCst);
    }
}

/// Upstream experimental-cli-command.test.ts: "passes server options to the
/// command action".
#[test]
fn execute_passes_server_options_to_the_action() {
    let context = RecordingContext::default();
    let result = execute(
        &[
            "server",
            "--server-id",
            "00000000-0000-4000-8000-000000000001",
            "--session-dir",
            "./sessions",
            "--provider",
            "anthropic",
            "--model",
            "claude-sonnet-4-5",
        ]
        .iter()
        .map(|part| part.to_string())
        .collect::<Vec<_>>(),
        &context,
    )
    .unwrap();

    let expected = ServerCommand {
        command: "server".to_string(),
        server_id: Some("00000000-0000-4000-8000-000000000001".to_string()),
        session_dir: Some("./sessions".to_string()),
        provider: Some("anthropic".to_string()),
        model: Some("claude-sonnet-4-5".to_string()),
        ..ServerCommand::default()
    };
    assert_eq!(result, BuiltInvocation::Server(expected.clone()));
    assert_eq!(context.server_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        context.last_server.lock().unwrap().as_ref(),
        Some(&expected)
    );
}

/// Upstream experimental-cli-command.test.ts: "executes the parsed
/// server/client command".
#[test]
fn execute_dispatches_to_the_selected_command() {
    for (name, server_expected, client_expected) in [("server", 1usize, 0usize), ("client", 0, 1)] {
        let context = RecordingContext::default();
        let result = execute(&[name.to_string()], &context).unwrap();
        assert!(matches!(
            result,
            BuiltInvocation::Server(_) | BuiltInvocation::Client(_)
        ));
        assert_eq!(context.server_calls.load(Ordering::SeqCst), server_expected);
        assert_eq!(context.client_calls.load(Ordering::SeqCst), client_expected);
    }
}

/// Upstream experimental-cli-resolution.test.ts: "requires an experimental
/// subcommand".
#[test]
fn requires_an_experimental_subcommand() {
    assert_eq!(
        parse(&[]),
        Err(vec![
            "Expected experimental command: server or client".to_string()
        ])
    );
}

/// Upstream experimental-cli-entry.test.ts (the node spawn run): with
/// experiments enabled the development entry parses `--version` as an
/// unsupported option — the server command surfaces the parse failure
/// ("Invalid --server-id" for `--server-id invalid`) exactly like the child
/// process stderr.
#[test]
fn server_id_validation_fails_before_unsupported_options() {
    let result = parse(&argv(
        "[\"server\",\"--server-id\",\"invalid\",\"--version\"]",
    ));
    let Err(errors) = result else {
        panic!("expected parse failure");
    };
    assert!(errors
        .iter()
        .any(|error| error.contains("Invalid --server-id")));
}

/// `ParsedCommandInput` remaining-args passthrough (the `--` grammar).
#[test]
fn remaining_args_capture_the_double_dash_grammar() {
    use crate::coding_agent::cli::experimental::command::{string_option, Command};
    let command = Command::new("probe")
        .option(string_option("--session-id", false))
        .build(|_input| Err(Vec::new()));
    let parsed = command.parse_options_public(&[
        "--session-id".to_string(),
        "demo".to_string(),
        "--".to_string(),
        "-x".to_string(),
        "prompt".to_string(),
    ]);
    assert_eq!(
        parsed.remaining_args,
        vec!["--".to_string(), "-x".to_string(), "prompt".to_string()]
    );
}
