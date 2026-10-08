//! Pisper regressions exercising native extension dispatch and MCP JSON-RPC.
//! No networks, models, or user configuration are accessed. The Windows
//! resource regression starts only its own isolated stdio fixture process.

use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use pi_rust::ai::types::ordered_map::OrderedMap;
use pi_rust::coding_agent::core::event_bus::EventBusController;
use pi_rust::coding_agent::core::mcp_servers::validate_mcp_server_config;
use pi_rust::coding_agent::extensions::loader::{
    load_extension_from_factory, ExtensionApi, ExtensionFactory, ExtensionRuntime,
};
use pi_rust::coding_agent::extensions::mcp::oauth::McpOAuthCredentialStore;
use pi_rust::coding_agent::extensions::mcp::runtime::{
    CreatedTransport, McpServerConnection, McpServerConnectionOptions, ServerState,
};
use pi_rust::coding_agent::extensions::mcp::{
    create_mcp_extension, LoadedMcpConfig, McpConfigScope, McpExtensionOptions, McpServerEntry,
};
use pi_rust::coding_agent::extensions::runner::ExtensionRunner;
use pi_rust::coding_agent::extensions::types::{
    NoopProviderRegistry, ToolDefinition, ToolExposure,
};
use pi_rust::mcp::protocol::jsonrpc::{JsonRpcErrorObject, JsonRpcMessage, JsonRpcResponse};
use pi_rust::mcp::protocol::types::LATEST_PROTOCOL_VERSION;
use pi_rust::mcp::transports::{create_in_memory_transport_pair, McpTransport, StdioTransport};
use serde_json::{json, Value};

// A std::Mutex self-lock blocks its executor thread before timeout futures can
// run. Use a detached OS thread and an independent deadline so the original
// regression fails promptly instead of hanging the test executable forever.
fn bounded_native<F, Fut>(work: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = ()> + 'static,
{
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(work());
        send.send(()).unwrap();
    });
    receive
        .recv_timeout(Duration::from_secs(5))
        .expect("native extension did not finish within its independent deadline");
}

fn make_runner(factory: ExtensionFactory, cwd: &str) -> ExtensionRunner {
    let runtime = ExtensionRuntime::new();
    let events = EventBusController::new();
    let extension = load_extension_from_factory(
        factory,
        cwd,
        events.bus().clone(),
        &runtime,
        Some("<pisper-mcp-regression>"),
    )
    .unwrap();
    ExtensionRunner::new(
        vec![extension],
        runtime,
        cwd,
        Arc::new(()),
        Arc::new(NoopProviderRegistry),
    )
}

fn isolated_options(directory: &std::path::Path) -> McpExtensionOptions {
    McpExtensionOptions {
        load_config: Some(Arc::new(|_| LoadedMcpConfig::default())),
        credentials: Some(Arc::new(McpOAuthCredentialStore::with_paths(
            directory
                .join("mcp-auth.json")
                .to_string_lossy()
                .into_owned(),
            None,
        ))),
        log_path: Some(directory.join("mcp.log").to_string_lossy().into_owned()),
        startup_wait_ms: Some(1_000),
        ..Default::default()
    }
}

fn resource_connection(
    directory: &std::path::Path,
    transport: Arc<dyn McpTransport>,
    stdio: Option<Arc<StdioTransport>>,
) -> Arc<McpServerConnection> {
    let raw = OrderedMap::from_pairs([
        ("command".into(), json!("isolated-fixture")),
        ("exposure".into(), json!("direct")),
        ("timeout".into(), json!(60)),
    ]);
    McpServerConnection::new(McpServerConnectionOptions {
        entry: McpServerEntry {
            name: "resource".into(),
            config: validate_mcp_server_config("resource", &raw).unwrap(),
            source: directory.join("mcp.json").to_string_lossy().into_owned(),
            scope: Some(McpConfigScope::Global),
            override_: None,
        },
        cwd: directory.to_string_lossy().into_owned(),
        create_transport: Arc::new(move |_, _, _| {
            Ok(CreatedTransport {
                transport: transport.clone(),
                stdio: stdio.clone(),
            })
        }),
        credentials: Arc::new(McpOAuthCredentialStore::with_paths(
            directory
                .join("mcp-auth.json")
                .to_string_lossy()
                .into_owned(),
            None,
        )),
        provider_token: None,
        on_tools: Arc::new(|_| {}),
        on_change: None,
        log: None,
    })
}

#[test]
fn cancelled_initialize_is_closed_and_cached_opening_does_not_retain_connection() {
    bounded_native(|| async {
        let directory = tempfile::tempdir().unwrap();
        let (client, server) = create_in_memory_transport_pair();
        let initialize_seen = Arc::new(tokio::sync::Notify::new());
        let seen = initialize_seen.clone();
        let messages = server.on_message(Arc::new(move |message| {
            if matches!(message, JsonRpcMessage::Request { method, .. } if method == "initialize") {
                seen.notify_one(); // Deliberately never answer initialize.
            }
        }));
        let closed = Arc::new(AtomicUsize::new(0));
        let observed = closed.clone();
        let _close = server.on_close(Arc::new(move || {
            observed.fetch_add(1, Ordering::SeqCst);
        }));
        server.start().await.unwrap();
        let connection = resource_connection(directory.path(), client, None);
        let weak = Arc::downgrade(&connection);
        let pending_connection = connection.clone();
        let pending = tokio::spawn(async move { pending_connection.get_client().await });
        tokio::time::timeout(Duration::from_secs(1), initialize_seen.notified())
            .await
            .unwrap();
        // Simulate cancellation of the API's outer timeout while the cached
        // Shared future is still waiting for its handshake response.
        pending.abort();
        let _ = pending.await;
        tokio::time::timeout(Duration::from_secs(1), connection.close())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(closed.load(Ordering::SeqCst), 1);
        assert_eq!(connection.state(), ServerState::Closed);
        assert!(connection.get_client().await.is_err());
        drop(connection);
        assert!(
            weak.upgrade().is_none(),
            "cached opening must not retain the connection"
        );
        messages.unsubscribe();
    });
}

#[test]
fn failed_initial_tools_list_closes_the_real_mcp_transport() {
    bounded_native(|| async {
        let directory = tempfile::tempdir().unwrap();
        let (client, server) = create_in_memory_transport_pair();
        let replies = server.clone();
        let messages = server.on_message(Arc::new(move |message| {
            let JsonRpcMessage::Request { id, method, .. } = message else { return; };
            let response = match method.as_str() {
                "initialize" => JsonRpcResponse::Success {
                    id: id.clone(),
                    result: json!({"protocolVersion":LATEST_PROTOCOL_VERSION,"capabilities":{"tools":{}},
                        "serverInfo":{"name":"failure-fixture","version":"1"}}),
                },
                "tools/list" => JsonRpcResponse::Error {
                    id: id.clone(),
                    error: JsonRpcErrorObject::new(-32603, "forced initial tools/list failure"),
                },
                other => panic!("unexpected request: {other}"),
            };
            let replies = replies.clone();
            tokio::spawn(async move {
                replies.send(&JsonRpcMessage::Response(response)).await.unwrap();
            });
        }));
        let closed = Arc::new(AtomicUsize::new(0));
        let observed = closed.clone();
        let _close = server.on_close(Arc::new(move || {
            observed.fetch_add(1, Ordering::SeqCst);
        }));
        server.start().await.unwrap();
        let connection = resource_connection(directory.path(), client, None);
        let error = connection
            .get_client()
            .await
            .err()
            .expect("tools/list must fail");
        assert!(error.contains("forced initial tools/list failure"));
        assert_eq!(
            closed.load(Ordering::SeqCst),
            1,
            "setup failure must close the transport"
        );
        assert_eq!(connection.state(), ServerState::Failed);
        assert!(connection.tools().is_empty());
        connection.close().await.unwrap();
        assert_eq!(connection.state(), ServerState::Closed);
        assert_eq!(closed.load(Ordering::SeqCst), 1);
        messages.unsubscribe();
    });
}

#[cfg(windows)]
#[test]
fn cancelled_initialize_closes_and_reaps_the_actual_stdio_fixture() {
    use pi_rust::mcp::transports::StdioTransportOptions;
    bounded_native(|| async {
        let directory = tempfile::tempdir().unwrap();
        let initialize_seen = Arc::new(tokio::sync::Notify::new());
        let observed = initialize_seen.clone();
        let mut options = StdioTransportOptions::new("powershell.exe").args([
            "-NoLogo", "-NoProfile", "-NonInteractive", "-Command",
            "while ($null -ne ($fixtureLine = [Console]::In.ReadLine())) { [Console]::Error.WriteLine('received initialize'); [Console]::Error.Flush() }",
        ]);
        options.cwd = Some(directory.path().to_string_lossy().into_owned());
        options.close_timeout_ms = Some(500);
        options.on_stderr = Some(Arc::new(move |text| {
            if text.contains("received initialize") {
                observed.notify_one();
            }
        }));
        let stdio = Arc::new(StdioTransport::new(options));
        let closed = Arc::new(AtomicUsize::new(0));
        let observed = closed.clone();
        let _close = stdio.on_close(Arc::new(move || {
            observed.fetch_add(1, Ordering::SeqCst);
        }));
        let connection = resource_connection(directory.path(), stdio.clone(), Some(stdio.clone()));
        let pending_connection = connection.clone();
        let pending = tokio::spawn(async move { pending_connection.get_client().await });
        tokio::time::timeout(Duration::from_secs(3), initialize_seen.notified())
            .await
            .unwrap();
        assert!(
            stdio.pid().is_some(),
            "the fixture must actually be spawned"
        );
        pending.abort();
        let _ = pending.await;
        tokio::time::timeout(Duration::from_secs(1), connection.close())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(connection.state(), ServerState::Closed);
        // Stdio close waits for its child-exit watch before emitting close;
        // this checks actual process teardown, not merely a Connection flag.
        assert_eq!(closed.load(Ordering::SeqCst), 1);
    });
}

#[test]
fn native_session_start_without_servers_returns_and_shuts_down() {
    bounded_native(|| async {
        let directory = tempfile::tempdir().unwrap();
        let cwd = directory.path().to_string_lossy();
        let runner = make_runner(
            create_mcp_extension(isolated_options(directory.path())),
            &cwd,
        );
        runner.emit(&mut json!({"type":"session_start"})).await;
        runner
            .emit(&mut json!({"type":"before_agent_start", "systemPromptOptions":{"sections":{}}}))
            .await;
        assert!(runner.get_all_registered_tools().is_empty());
        runner.emit(&mut json!({"type":"session_shutdown"})).await;
    });
}

#[test]
fn post_factory_tool_registration_is_visible_and_replaces_without_duplicates() {
    let retained_api = Arc::new(Mutex::new(None::<ExtensionApi>));
    let retained = retained_api.clone();
    let runner = make_runner(
        Arc::new(move |api| {
            api.register_tool(ToolDefinition::new(
                "static",
                "static",
                "initial",
                json!({}),
            ))?;
            *retained.lock().unwrap() = Some(api.clone());
            Ok(())
        }),
        ".",
    );
    assert_eq!(runner.get_all_registered_tools().len(), 1);
    assert_eq!(
        runner.get_tool_definition("static").unwrap().description,
        "initial"
    );
    let api = retained_api.lock().unwrap().clone().unwrap();
    let mut replacement = ToolDefinition::new("static", "static", "replacement", json!({}));
    replacement.exposure = ToolExposure::Hidden;
    api.register_tool(replacement).unwrap();
    api.register_tool(ToolDefinition::new("late", "late", "dynamic", json!({})))
        .unwrap();
    let tools = runner.get_all_registered_tools();
    assert_eq!(tools.len(), 2);
    assert_eq!(tools[0].definition.name, "static");
    assert_eq!(tools[0].definition.description, "replacement");
    assert_eq!(tools[0].definition.exposure, ToolExposure::Hidden);
    assert_eq!(tools[1].definition.name, "late");
    assert_eq!(
        runner.get_tool_definition("late").unwrap().description,
        "dynamic"
    );
    assert_eq!(
        runner.get_tool_definition("static").unwrap().exposure,
        ToolExposure::Hidden
    );
}

#[test]
fn ordinary_native_startup_discovers_and_calls_real_mock_mcp_tools() {
    bounded_native(|| async {
        let directory = tempfile::tempdir().unwrap();
        let cwd = directory.path().to_string_lossy();
        let (client_transport, mock_server) = create_in_memory_transport_pair();
        let requests = Arc::new(Mutex::new(Vec::<(String, Value)>::new()));
        let observed = requests.clone();
        let replies = mock_server.clone();
        let _messages = mock_server.on_message(Arc::new(move |message| {
            let JsonRpcMessage::Request { id, method, params } = message else {
                return;
            };
            let params = params.clone().unwrap_or(Value::Null);
            observed.lock().unwrap().push((method.clone(), params.clone()));
            let result = match method.as_str() {
                "initialize" => json!({
                    "protocolVersion": LATEST_PROTOCOL_VERSION,
                    "capabilities": {"tools":{}},
                    "serverInfo": {"name":"pisper-isolated-mock", "version":"1"}
                }),
                "tools/list" => json!({"tools":[{
                    "name":"echo", "description":"Echo input through JSON-RPC",
                    "inputSchema":{"type":"object", "properties":{"text":{"type":"string"}}, "required":["text"]}
                }]}),
                "tools/call" => json!({"content":[{
                    "type":"text", "text":format!("echo:{}", params["arguments"]["text"].as_str().unwrap())
                }]}),
                other => panic!("unexpected mock MCP request: {other}"),
            };
            let response = JsonRpcMessage::Response(JsonRpcResponse::Success {
                id: id.clone(),
                result,
            });
            let replies = replies.clone();
            tokio::spawn(async move { replies.send(&response).await.unwrap(); });
        }));
        let closed = Arc::new(AtomicUsize::new(0));
        let closed_callback = closed.clone();
        let _close = mock_server.on_close(Arc::new(move || {
            closed_callback.fetch_add(1, Ordering::SeqCst);
        }));
        mock_server.start().await.unwrap();

        let raw = OrderedMap::from_pairs([
            ("command".into(), json!("unused-mock-command")),
            ("exposure".into(), json!("direct")),
        ]);
        let entry = McpServerEntry {
            name: "regression".into(),
            config: validate_mcp_server_config("regression", &raw).unwrap(),
            source: directory
                .path()
                .join("mcp.json")
                .to_string_lossy()
                .into_owned(),
            scope: Some(McpConfigScope::Global),
            override_: None,
        };
        let mut options = isolated_options(directory.path());
        options.load_config = Some(Arc::new(move |_| LoadedMcpConfig {
            servers: vec![entry.clone()],
            ..Default::default()
        }));
        options.create_transport = Some(Arc::new(move |_, _, _| {
            Ok(CreatedTransport {
                transport: client_transport.clone(),
                stdio: None,
            })
        }));
        let runner = make_runner(create_mcp_extension(options), &cwd);
        let errors = Arc::new(Mutex::new(Vec::new()));
        let error_sink = errors.clone();
        let _error_listener = runner.on_error(Arc::new(move |error| {
            error_sink.lock().unwrap().push(error.error.clone());
        }));

        runner.emit(&mut json!({"type":"session_start"})).await;
        // The ordinary first prompt must wait for direct MCP declarations;
        // no /mcp command is invoked to secretly drive the startup future.
        runner
            .emit(&mut json!({"type":"before_agent_start", "systemPromptOptions":{"sections":{}}}))
            .await;
        let definition = runner
            .get_tool_definition("mcp__regression__echo")
            .expect("direct MCP tool must be visible before the first prompt");
        assert_eq!(definition.exposure, ToolExposure::Direct);
        assert_eq!(
            runner
                .get_all_registered_tools()
                .iter()
                .filter(|tool| tool.definition.name == definition.name)
                .count(),
            1
        );
        for text in ["first", "second"] {
            let result = definition.execute_async.as_ref().unwrap()(
                format!("call-{text}"),
                json!({"text":text}),
                None,
                None,
                runner.create_context(),
            )
            .await
            .unwrap();
            assert_eq!(result["content"][0]["text"], format!("echo:{text}"));
        }
        let calls = requests.lock().unwrap().clone();
        assert_eq!(
            calls
                .iter()
                .filter(|(method, _)| method == "initialize")
                .count(),
            1
        );
        let tool_calls: Vec<_> = calls
            .iter()
            .filter(|(method, _)| method == "tools/call")
            .collect();
        assert_eq!(tool_calls.len(), 2);
        assert_eq!(tool_calls[0].1["name"], "echo");
        assert_eq!(tool_calls[1].1["arguments"]["text"], "second");
        assert!(errors.lock().unwrap().is_empty());
        runner.emit(&mut json!({"type":"session_shutdown"})).await;
        assert_eq!(closed.load(Ordering::SeqCst), 1);
        drop(runner);

        // Reload creates a fresh extension/runtime: removed servers must not
        // retain declarations from the preceding native MCP registration.
        let reloaded = make_runner(
            create_mcp_extension(isolated_options(directory.path())),
            &cwd,
        );
        reloaded.emit(&mut json!({"type":"session_start"})).await;
        assert!(reloaded
            .get_tool_definition("mcp__regression__echo")
            .is_none());
        assert!(reloaded.get_all_registered_tools().is_empty());
        reloaded.emit(&mut json!({"type":"session_shutdown"})).await;
    });
}

#[test]
fn shutdown_before_background_poll_prevents_stale_connection_creation() {
    bounded_native(|| async {
        let directory = tempfile::tempdir().unwrap();
        let cwd = directory.path().to_string_lossy();
        let started = Arc::new(AtomicUsize::new(0));
        let observed = started.clone();
        let entry = McpServerEntry {
            name: "stale".into(),
            config: validate_mcp_server_config(
                "stale",
                &OrderedMap::from_pairs([
                    ("command".into(), json!("unused-mock-command")),
                    ("exposure".into(), json!("direct")),
                ]),
            )
            .unwrap(),
            source: directory
                .path()
                .join("mcp.json")
                .to_string_lossy()
                .into_owned(),
            scope: Some(McpConfigScope::Global),
            override_: None,
        };
        let mut options = isolated_options(directory.path());
        options.load_config = Some(Arc::new(move |_| LoadedMcpConfig {
            servers: vec![entry.clone()],
            ..Default::default()
        }));
        options.create_transport = Some(Arc::new(move |_, _, _| {
            observed.fetch_add(1, Ordering::SeqCst);
            Err("stale transport should never be created".into())
        }));
        let runner = make_runner(create_mcp_extension(options), &cwd);
        runner.emit(&mut json!({"type":"session_start"})).await;
        runner.emit(&mut json!({"type":"session_shutdown"})).await;
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        assert_eq!(started.load(Ordering::SeqCst), 0);
        assert!(runner.get_all_registered_tools().is_empty());
    });
}
