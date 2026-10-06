//! Transport oracle tests (stdio, streamable HTTP, SSE parser) replaying
//! `tests/fixtures/mcp_oracle/mcp_oracle.json`; see
//! [`crate::mcp::mcp_oracle_tests`] for the provenance and comparison
//! conventions.
//!
//! The stdio scenarios spawn the SAME fixture scripts the capture used (the
//! capture's staged `stdio-fixture.mjs`, plain `.mjs` — any Node runs it),
//! so the recorded stdin bytes are the byte oracle. Node is resolved from
//! `MCP_ORACLE_NODE`, then `PATH`, then the capture machine's Anaconda
//! install.

use std::sync::Arc;
use std::sync::Mutex;

use futures::future::BoxFuture;
use serde_json::{json, Value};

use crate::mcp::auth_provider::AuthProvider;
use crate::mcp::auth_provider::{
    ByteStream, FetchRequest, FetchResponse, McpFetch, UnauthorizedContext,
};
use crate::mcp::client::{ClientState, McpClient, McpClientOptions, McpRequestOptions};
use crate::mcp::mcp_oracle_tests::{assert_canonical, oracle, stringify};
use crate::mcp::protocol::jsonrpc::McpClientError;
use crate::mcp::transports::{
    consume_sse_stream, ConsumeSseOptions, McpTransport, SseEvent, StdioTransportOptions,
    StreamableHttpTransport, StreamableHttpTransportOptions,
};

// ---------------------------------------------------------------------------
// Recorders
// ---------------------------------------------------------------------------

/// One recorded request (the capture's `{url, method, headers, body}`).
pub(crate) fn record(
    url: &url::Url,
    method: &str,
    headers: Vec<(String, String)>,
    body: Option<String>,
    lowercase: bool,
) -> Value {
    let mut record = serde_json::Map::new();
    record.insert("url".into(), json!(url.to_string()));
    record.insert("method".into(), json!(method));
    let mut header_map = serde_json::Map::new();
    for (name, value) in headers {
        let name = if lowercase {
            name.to_ascii_lowercase()
        } else {
            name
        };
        header_map.insert(name, json!(value));
    }
    record.insert("headers".into(), Value::Object(header_map));
    if let Some(body) = body {
        record.insert("body".into(), json!(body));
    }
    Value::Object(record)
}

/// An answer from the recorder handler (the capture's
/// `{status, headers, body|chunks}`).
#[derive(Clone, Default)]
pub(crate) struct Answer {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub chunks: Vec<Vec<u8>>,
    /// The capture's GET fixture hands `fetch` a stream that never delivers a
    /// parseable event; the observable behavior is a pending stream.
    pub never: bool,
}

pub(crate) fn answer(status: u16, headers: &[(&str, &str)], body: &str) -> Answer {
    Answer {
        status,
        headers: headers
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect(),
        chunks: vec![body.as_bytes().to_vec()],
        never: false,
    }
}

pub(crate) fn answer_json(status: u16, headers: &[(&str, &str)], body: Value) -> Answer {
    answer(status, headers, &stringify(&body))
}

pub(crate) fn sse_answer(status: u16, headers: &[(&str, &str)], chunks: Vec<String>) -> Answer {
    Answer {
        status,
        headers: headers
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect(),
        chunks: chunks.into_iter().map(String::into_bytes).collect(),
        never: false,
    }
}

pub(crate) type RecorderHandler =
    Arc<dyn Fn(&Value, usize) -> Result<Answer, String> + Send + Sync>;

/// The capture's `recorderFetch`: records requests and answers through the
/// handler. `lowercase` emulates the capture's Headers-object path (the
/// streamable HTTP transport hands `fetch` a `Headers` instance, whose
/// iteration lowercases names; OAuth passes plain objects, preserving case).
pub(crate) fn recorder_fetch(
    handler: RecorderHandler,
    lowercase: bool,
) -> (McpFetch, Arc<Mutex<Vec<Value>>>) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let request_log = Arc::clone(&requests);
    let fetch: McpFetch = Arc::new(move |request: FetchRequest| {
        let log = Arc::clone(&request_log);
        let handler = Arc::clone(&handler);
        let lowercase = lowercase;
        Box::pin(async move {
            let record = record(
                &request.url,
                &request.method,
                request.headers.clone(),
                request.body.clone(),
                lowercase,
            );
            let index = {
                let mut guard = log.lock().expect("request log");
                guard.push(record.clone());
                guard.len() - 1
            };
            let answer = match handler(&record, index) {
                Ok(answer) => answer,
                Err(error) => {
                    return Err(crate::mcp::auth_provider::FetchError::network(error));
                }
            };
            let body: ByteStream = if answer.never {
                Box::pin(futures::stream::pending())
            } else {
                Box::pin(futures::stream::iter(
                    answer.chunks.into_iter().map(Ok::<_, std::io::Error>),
                ))
            };
            Ok(FetchResponse {
                status: answer.status,
                headers: answer.headers,
                body,
            })
        })
    });
    (fetch, requests)
}

fn http_transport(
    url: &str,
    fetch: McpFetch,
    open_get_stream: bool,
) -> Arc<StreamableHttpTransport> {
    let mut options = StreamableHttpTransportOptions::new(url::Url::parse(url).unwrap());
    options.fetch = Some(fetch);
    options.open_get_stream = Some(open_get_stream);
    Arc::new(StreamableHttpTransport::new(options))
}

fn plain_client() -> McpClient {
    McpClient::new(McpClientOptions::new("t", "1"))
}

// ---------------------------------------------------------------------------
// 8. transports/streamable-http.ts
// ---------------------------------------------------------------------------

/// The shared fixture initialize result/response.
fn http_fixture() -> (Value, Value) {
    let server_info = json!({ "name": "http-fixture", "version": "2.0" });
    let initialize_result = json!({
        "protocolVersion": "2025-06-18",
        "capabilities": { "tools": {} },
        "serverInfo": server_info,
    });
    let initialize_response = json!({ "jsonrpc": "2.0", "id": 1, "result": initialize_result });
    (initialize_result, initialize_response)
}

// ---------------------------------------------------------------------------
// 7. transports/stdio.ts over a real spawned fixture
// ---------------------------------------------------------------------------

/// Resolves a Node executable for the fixture scripts: `MCP_ORACLE_NODE`,
/// then `node` on PATH, then the capture machine's Anaconda install.
fn node_path() -> String {
    static NODE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    NODE.get_or_init(|| {
        let mut candidates = Vec::new();
        if let Ok(from_env) = std::env::var("MCP_ORACLE_NODE") {
            candidates.push(from_env);
        }
        candidates.push("node".to_string());
        candidates.push("C:/Users/13063/anaconda3/node.exe".to_string());
        for candidate in candidates {
            if let Ok(output) = std::process::Command::new(&candidate)
                .arg("--version")
                .output()
            {
                if output.status.success() {
                    return candidate;
                }
            }
        }
        panic!(
            "no Node found for the stdio fixture (set MCP_ORACLE_NODE); \
             the capture scripts are plain .mjs and run on any modern Node"
        );
    })
    .clone()
}

/// The capture's staged `stdio-fixture.mjs` (the cross-spawn import
/// substitution applied; the redundant `raw.push` line removed, exactly like
/// the capture's `.replace`).
const STDIO_FIXTURE: &str = r#"import { createInterface } from "node:readline";
import * as fs from "node:fs";

const [reportPath, mode] = process.argv.slice(2);
const raw = [];
process.stdin.on("data", (chunk) => raw.push(...chunk));
let plan = mode === "chaos" ? JSON.parse(fs.readFileSync(process.argv[4], "utf8")) : null;

const write = (message) => process.stdout.write(JSON.stringify(message) + "\n");

if (mode === "stderr") {
  console.error("stdio fixture ready");
  process.stderr.write("a".repeat(300));
  process.stdin.once("end", () => {
    fs.writeFileSync(reportPath, JSON.stringify({ stderrDone: true }));
    process.exit(0);
  });
} else if (mode === "chaos") {
  (async () => {
    for (const step of plan) {
      if (step === "partial") { process.stdout.write('{"jsonrpc":"2.0","id":9,"res'); }
      else { process.stdout.write(step); await new Promise((r) => setTimeout(r, 15)); }
    }
    process.exit(0);
  })();
} else {
  console.error("stdio fixture ready");
  const lines = createInterface({ input: process.stdin, crlfDelay: Infinity });
  for await (const line of lines) {
    let message;
    try { message = JSON.parse(line); } catch { continue; }
    if (!("id" in message)) continue;
    if (message.method === "initialize") {
      write({ jsonrpc: "2.0", id: message.id, result: { protocolVersion: "2025-06-18", capabilities: { tools: {} }, serverInfo: { name: "stdio-fixture", version: "1.0.0" } } });
    } else if (message.method === "tools/list") {
      write({ jsonrpc: "2.0", id: message.id, result: { tools: [{ name: "echo", inputSchema: { type: "object" } }] } });
    } else if (message.method === "tools/call") {
      write({ jsonrpc: "2.0", id: message.id, result: { content: [{ type: "text", text: String(message.params.arguments.text) }] } });
    } else if (message.method === "ping") {
      write({ jsonrpc: "2.0", id: message.id, result: {} });
    } else {
      write({ jsonrpc: "2.0", id: message.id, error: { code: -32601, message: "not found" } });
    }
  }
  fs.writeFileSync(reportPath, JSON.stringify({ receivedText: Buffer.from(raw).toString("utf8"), receivedHex: Buffer.from(raw).toString("hex") }));
}
"#;

const OVERSIZE_FIXTURE: &str = r#"import * as fs from "node:fs";
const [reportPath] = process.argv.slice(2);
const write = (text) => process.stdout.write(text);
write('{"pad":"' + "x".repeat(120) + '","jsonrpc":"2.0","id":1,"result":{}}' + "\n");
write("y".repeat(150));
await new Promise((r) => setTimeout(r, 40));
write('{"jsonrpc":"2.0","method":"after/reset"}' + "\n");
await new Promise((r) => setTimeout(r, 40));
fs.writeFileSync(reportPath, JSON.stringify({ done: true }));
process.exit(0);
"#;

const STDERR_FIXTURE: &str = r#"process.stderr.write("short|");
process.stderr.write("L".repeat(200) + "|");
process.stderr.write("tail");
process.stdin.once("end", () => process.exit(0));
"#;

fn write_fixture(dir: &std::path::Path, name: &str, contents: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, contents).expect("fixture written");
    path
}

/// Polls `predicate` for up to `seconds` (real time; the stdio fixtures are
/// subprocess-driven).
async fn eventually(predicate: impl Fn() -> bool, seconds: u64) -> bool {
    for _ in 0..seconds * 20 {
        if predicate() {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    predicate()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stdio_serve_matches_the_capture() {
    let staging = tempfile::tempdir().expect("tempdir");
    let fixture = write_fixture(staging.path(), "stdio-fixture.mjs", STDIO_FIXTURE);
    let report_path = staging.path().join("stdio-report.json");
    let stderr_chunks = Arc::new(Mutex::new(Vec::new()));
    let chunk_log = Arc::clone(&stderr_chunks);
    let mut options = StdioTransportOptions::new(node_path());
    options.args = vec![
        fixture.to_string_lossy().into_owned(),
        report_path.to_string_lossy().into_owned(),
        "serve".to_string(),
    ];
    options.on_stderr = Some(Arc::new(move |chunk: &str| {
        chunk_log
            .lock()
            .expect("stderr chunks")
            .push(chunk.to_string());
    }));
    let transport = Arc::new(crate::mcp::transports::StdioTransport::new(options));
    let client = McpClient::new(McpClientOptions::new("stdio-test", "1.0.0"));
    client
        .connect(Arc::clone(&transport) as Arc<dyn McpTransport>)
        .await
        .expect("connect");
    let tools = client
        .list_tools(McpRequestOptions::default())
        .await
        .expect("tools");
    let called = client
        .call_tool(
            "echo",
            Some(serde_json::from_value(json!({ "text": "hello" })).unwrap()),
            McpRequestOptions::default(),
        )
        .await
        .expect("call");
    client
        .ping(McpRequestOptions::default())
        .await
        .expect("ping");
    let expected = &oracle()["stdio_serve"];
    assert!(transport.pid().is_some(), "pid is a number");
    assert_canonical("tools", json!(tools), &expected["tools"]);
    assert_canonical("called", json!(called), &expected["called"]);
    assert!(
        eventually(
            || stderr_chunks
                .lock()
                .expect("chunks")
                .concat()
                .contains("stdio fixture ready"),
            5,
        )
        .await,
        "stderr callback saw the ready banner"
    );
    client.close().await.expect("close");
    assert!(
        eventually(|| report_path.exists(), 5).await,
        "fixture wrote its report"
    );
    let report: Value =
        serde_json::from_str(&std::fs::read_to_string(&report_path).expect("report")).unwrap();
    // THE byte oracle: exactly what the client wrote to the server's stdin.
    assert_eq!(
        report["receivedHex"], expected["clientToServerHex"],
        "raw stdin bytes"
    );
    assert_eq!(
        report["receivedText"], expected["clientToServerText"],
        "raw stdin text"
    );
    assert!(
        transport.stderr().contains("stdio fixture ready"),
        "stderr buffer saw the ready banner"
    );
    assert_eq!(client.connection_state(), ClientState::Closed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stdio_chaos_classifies_like_the_capture() {
    let staging = tempfile::tempdir().expect("tempdir");
    let fixture = write_fixture(staging.path(), "stdio-fixture.mjs", STDIO_FIXTURE);
    let init = stringify(&json!({
        "jsonrpc": "2.0", "id": 1,
        "result": {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "serverInfo": { "name": "c", "version": "1" },
        },
    }));
    let plan = json!([
        &init[0..20],
        &init[20..60],
        format!("{}\r\n", &init[60..]),
        "\n\n",
        format!(
            "{}\n",
            stringify(&json!({ "jsonrpc": "2.0", "method": "custom/n" }))
        ),
        "{not json}\n",
        format!("{}\n", stringify(&json!({ "hello": "world" }))),
        format!("{}\n", stringify(&json!({ "jsonrpc": "2.0", "method": 5 }))),
        format!(
            "{}\n",
            stringify(&json!({ "jsonrpc": "2.0", "id": 777, "result": {} }))
        ),
        "   \n",
        "partial",
    ]);
    let plan_path = write_fixture(staging.path(), "chaos-plan.json", &stringify(&plan));
    let report_path = staging.path().join("chaos-report.json");
    let mut options = StdioTransportOptions::new(node_path());
    options.args = vec![
        fixture.to_string_lossy().into_owned(),
        report_path.to_string_lossy().into_owned(),
        "chaos".to_string(),
        plan_path.to_string_lossy().into_owned(),
    ];
    let transport = Arc::new(crate::mcp::transports::StdioTransport::new(options));
    let client = McpClient::new(McpClientOptions::new("c", "1"));
    let errors = Arc::new(Mutex::new(Vec::new()));
    let notifications = Arc::new(Mutex::new(Vec::new()));
    let error_log = Arc::clone(&errors);
    client.on_error(Arc::new(move |error: &McpClientError| {
        error_log.lock().expect("errors").push(error.clone());
    }));
    let notification_log = Arc::clone(&notifications);
    client.on_notification(
        "custom/n",
        Arc::new(move |params: &Value| {
            notification_log
                .lock()
                .expect("notifications")
                .push(params.clone());
        }),
    );
    client
        .connect(Arc::clone(&transport) as Arc<dyn McpTransport>)
        .await
        .expect("connect");
    assert!(
        eventually(|| errors.lock().expect("errors").len() >= 5, 10).await,
        "all chaos lines produced errors: {:?}",
        errors.lock().expect("errors")
    );
    client.close().await.expect("close");
    let classified = errors.lock().expect("errors").clone();
    // Error classification vs the capture's six errors. The capture's first
    // entry is a V8 artifact (its listener calls json(undefined) on the
    // undefined notification params); the port delivers {} instead (JSON has
    // no undefined), leaving five: one JSON parse failure (V8 SyntaxError
    // texts are Node-specific; serde_json messages differ — disclosed), two
    // invalid JSON-RPC messages, the unknown response id, and the trailing
    // incomplete message.
    assert_eq!(classified.len(), 5, "classified: {classified:?}");
    assert!(
        matches!(classified[0], McpClientError::Other(_)),
        "parse failure: {:?}",
        classified[0]
    );
    for index in [1, 2] {
        let McpClientError::Mcp(mcp_error) = &classified[index] else {
            panic!(
                "error {index} should be invalid JSON-RPC: {:?}",
                classified[index]
            );
        };
        assert_eq!(mcp_error.message, "Invalid JSON-RPC message");
    }
    assert_eq!(
        classified[3].to_string(),
        "Received response for unknown MCP request 777"
    );
    assert_eq!(
        classified[4].to_string(),
        "MCP stdio server closed with an incomplete JSON-RPC message"
    );
    // The notification was delivered. Upstream hands listeners `undefined`
    // params (the capture's `json(undefined)` then throws into its own error
    // listener and pushes nothing); the port delivers `{}` (JSON has no
    // undefined), so the capture's first V8 SyntaxError and empty
    // notifications become one clean delivery here — disclosed.
    assert_eq!(
        notifications.lock().expect("notifications").clone(),
        vec![json!({})]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stdio_oversize_matches_the_capture() {
    let staging = tempfile::tempdir().expect("tempdir");
    let fixture = write_fixture(staging.path(), "oversize-fixture.mjs", OVERSIZE_FIXTURE);
    let report_path = staging.path().join("oversize-report.json");
    let mut options = StdioTransportOptions::new(node_path());
    options.args = vec![
        fixture.to_string_lossy().into_owned(),
        report_path.to_string_lossy().into_owned(),
    ];
    options.max_message_bytes = Some(100);
    let transport = Arc::new(crate::mcp::transports::StdioTransport::new(options));
    let errors = Arc::new(Mutex::new(Vec::new()));
    let messages = Arc::new(Mutex::new(Vec::new()));
    let error_log = Arc::clone(&errors);
    let message_log = Arc::clone(&messages);
    transport.on_error(Arc::new(move |error: &McpClientError| {
        error_log.lock().expect("errors").push(error.to_string());
    }));
    transport.on_message(Arc::new(move |message: &_| {
        message_log
            .lock()
            .expect("messages")
            .push(message.to_value());
    }));
    transport.start().await.expect("start");
    assert!(
        eventually(
            || {
                errors.lock().expect("errors").len() >= 2
                    && !messages.lock().expect("messages").is_empty()
            },
            10,
        )
        .await,
        "oversize errors and the healthy line arrived"
    );
    let _ = transport.close().await;
    let expected = &oracle()["stdio_oversize"];
    assert_canonical(
        "oversize errors",
        json!(errors.lock().expect("errors").clone()),
        &expected["errors"],
    );
    assert_canonical(
        "oversize messages",
        json!(messages.lock().expect("messages").clone()),
        &expected["messages"],
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stdio_stderr_cap_matches_the_capture() {
    let staging = tempfile::tempdir().expect("tempdir");
    let fixture = write_fixture(staging.path(), "stderr-fixture.mjs", STDERR_FIXTURE);
    let chunks = Arc::new(Mutex::new(Vec::new()));
    let chunk_log = Arc::clone(&chunks);
    let mut options = StdioTransportOptions::new(node_path());
    options.args = vec![fixture.to_string_lossy().into_owned()];
    options.max_stderr_bytes = Some(100);
    options.on_stderr = Some(Arc::new(move |chunk: &str| {
        chunk_log.lock().expect("chunks").push(chunk.to_string());
    }));
    let transport = Arc::new(crate::mcp::transports::StdioTransport::new(options));
    transport.start().await.expect("start");
    assert!(
        eventually(
            || chunks.lock().expect("chunks").concat().ends_with("tail"),
            10
        )
        .await,
        "all stderr chunks arrived"
    );
    let _ = transport.close().await;
    let expected = &oracle()["stdio_stderr_cap"];
    // Chunk boundaries are pipe-buffer artifacts (Node data events vs tokio
    // reads); the pinned contract is the concatenation and the capped buffer.
    assert_eq!(
        chunks.lock().expect("chunks").concat(),
        expected["chunks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().to_string())
            .collect::<Vec<_>>()
            .concat(),
    );
    assert_eq!(
        transport.stderr(),
        expected["stderrBuffer"].as_str().unwrap()
    );
    assert_eq!(
        transport.stderr().chars().count(),
        expected["stderrBufferLength"].as_u64().unwrap() as usize
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stdio_spawn_failure_matches_the_capture() {
    let options = StdioTransportOptions::new("definitely-not-a-real-command-xyz");
    let transport = crate::mcp::transports::StdioTransport::new(options);
    let error = transport.start().await.expect_err("spawn fails");
    // Upstream surfaces the spawn error from `start()` (ENOENT); Rust reports
    // the OS program-not-found error with the same shape ("The system cannot
    // find the file specified" on Windows, "No such file or directory" on
    // unix) — match case-insensitively.
    let message = error.to_string();
    let lowered = message.to_ascii_lowercase();
    assert!(
        lowered.contains("not found")
            || lowered.contains("no such file")
            || lowered.contains("cannot find"),
        "spawn failure should mention the missing program: {message}"
    );
    // The capture's `startedFlag: true`: the started flag is set before the
    // spawn attempt, so a retry reports "already started".
    let second = transport.start().await.expect_err("second start");
    assert!(second.to_string().contains("already started"), "{second}");
}

// ---------------------------------------------------------------------------
// 8. transports/streamable-http.ts
// ---------------------------------------------------------------------------

/// The capture's `httpHandler`: initialize / tools/list / tools/call / 202 /
/// 405 / DELETE.
fn http_handler() -> RecorderHandler {
    let (_result, initialize_response) = http_fixture();
    Arc::new(move |record, _index| {
        let initialize_response = initialize_response.clone();
        Ok(match record["method"].as_str().unwrap_or("") {
            "POST" => {
                let message: Value = serde_json::from_str(record["body"].as_str().unwrap_or(""))
                    .unwrap_or(json!({}));
                match message["method"].as_str().unwrap_or("") {
                    "initialize" => answer_json(
                        200,
                        &[
                            ("content-type", "application/json"),
                            ("mcp-session-id", "session-1"),
                        ],
                        initialize_response,
                    ),
                    "tools/list" => answer_json(
                        200,
                        &[("content-type", "application/json")],
                        json!({ "jsonrpc": "2.0", "id": message["id"], "result": { "tools": [{ "name": "echo", "inputSchema": { "type": "object" } }] } }),
                    ),
                    "tools/call" => answer_json(
                        200,
                        &[("content-type", "application/json")],
                        json!({ "jsonrpc": "2.0", "id": message["id"], "result": { "content": [{ "type": "text", "text": "hello" }] } }),
                    ),
                    _ => answer(202, &[], ""),
                }
            }
            "GET" => answer(405, &[], ""),
            "DELETE" => answer(200, &[], ""),
            _ => answer(404, &[], ""),
        })
    })
}

#[tokio::test(start_paused = true)]
async fn http_json_roundtrip_matches_the_capture() {
    let (fetch, requests) = recorder_fetch(http_handler(), true);
    let transport = http_transport("http://mcp.test/mcp", fetch, false);
    let client = McpClient::new(McpClientOptions::new("http-test", "1.0.0"));
    let result = client
        .connect(Arc::clone(&transport) as Arc<dyn McpTransport>)
        .await
        .expect("connect");
    let tools = client
        .list_tools(McpRequestOptions::default())
        .await
        .expect("tools");
    let called = client
        .call_tool(
            "echo",
            Some(serde_json::from_value(json!({ "text": "hello" })).unwrap()),
            McpRequestOptions::default(),
        )
        .await
        .expect("call");
    client
        .notify(
            "status/update",
            Some(serde_json::from_value(json!({ "s": 1 })).unwrap()),
        )
        .await
        .expect("notify");
    let session_id = transport.session_id();
    client.close().await.expect("close");
    let expected = &oracle()["http_json_roundtrip"];
    assert_canonical(
        "connect result",
        serde_json::to_value(&result).unwrap(),
        &expected["connectResult"],
    );
    assert_canonical("tools", json!(tools), &expected["tools"]);
    assert_canonical("called", json!(called), &expected["called"]);
    assert_eq!(
        session_id.as_deref(),
        Some(expected["sessionId"].as_str().unwrap())
    );
    assert_canonical(
        "requests",
        json!(requests.lock().expect("log").clone()),
        &expected["requests"],
    );
}

#[tokio::test(start_paused = true)]
async fn http_sse_response_matches_the_capture() {
    let (_result, initialize_response) = http_fixture();
    let init_chunks = vec![format!("event: message\ndata: {initialize_response}\n\n")];
    let handler: RecorderHandler = Arc::new(move |record, _index| {
        Ok(if record["method"] == "POST" {
            let message: Value =
                serde_json::from_str(record["body"].as_str().unwrap_or("")).unwrap_or(json!({}));
            match message["method"].as_str().unwrap_or("") {
                "initialize" => sse_answer(
                    200,
                    &[
                        ("content-type", "text/event-stream"),
                        ("mcp-session-id", "sse-session"),
                    ],
                    init_chunks.clone(),
                ),
                "tools/call" => {
                    let answer = json!({
                        "jsonrpc": "2.0", "id": message["id"],
                        "result": { "content": [{ "type": "text", "text": "sse" }] },
                    });
                    let text = stringify(&answer);
                    sse_answer(
                        200,
                        &[("content-type", "text/event-stream")],
                        vec![
                            "data: not json\n\n".to_string(),
                            format!("data: {}", &text[..30]),
                            format!("{}\n\n", &text[30..]),
                        ],
                    )
                }
                _ => answer(202, &[], ""),
            }
        } else {
            answer(405, &[], "")
        })
    });
    let (fetch, requests) = recorder_fetch(handler, true);
    let transport = http_transport("http://mcp.test/sse", fetch, false);
    let client = McpClient::new(McpClientOptions::new("http-test", "1.0.0"));
    let errors = Arc::new(Mutex::new(Vec::new()));
    let error_log = Arc::clone(&errors);
    client.on_error(Arc::new(move |error: &McpClientError| {
        error_log.lock().expect("errors").push(error.to_string());
    }));
    client
        .connect(Arc::clone(&transport) as Arc<dyn McpTransport>)
        .await
        .expect("connect");
    let called = client
        .call_tool("echo", None, McpRequestOptions::default())
        .await
        .expect("call over SSE");
    let expected = &oracle()["http_sse_response"];
    assert_canonical("called", json!(called), &expected["called"]);
    client.close().await.expect("close");
    // One error for the invalid SSE data event (V8's JSON parse message is
    // Node-specific; serde_json's text differs — disclosed).
    assert_eq!(errors.lock().expect("errors").len(), 1);
    let expected_requests: Vec<Value> = expected["requests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|request| {
            let mut subset = serde_json::Map::new();
            subset.insert("method".into(), request["method"].clone());
            if let Some(body) = request.get("body") {
                subset.insert("body".into(), body.clone());
            }
            Value::Object(subset)
        })
        .collect();
    let actual_requests: Vec<Value> = requests
        .lock()
        .expect("log")
        .clone()
        .iter()
        .map(|request| {
            let mut subset = serde_json::Map::new();
            subset.insert("method".into(), request["method"].clone());
            if let Some(body) = request.get("body") {
                subset.insert("body".into(), body.clone());
            }
            Value::Object(subset)
        })
        .collect();
    assert_canonical(
        "requests",
        json!(actual_requests),
        &Value::Array(expected_requests),
    );
}

#[tokio::test(start_paused = true)]
async fn http_status_classification_matches_the_capture() {
    let expected = &oracle()["http_status_classification"];
    let (_result, initialize_response) = http_fixture();

    // 401 with challenge.
    {
        let (fetch, _requests) = recorder_fetch(
            Arc::new(|_record, _index| {
                Ok(answer(
                    401,
                    &[(
                        "www-authenticate",
                        "Bearer resource_metadata=\"https://example.com/meta\"",
                    )],
                    "login required",
                ))
            }),
            true,
        );
        let transport = http_transport("http://mcp.test/a", fetch, false);
        let client = plain_client();
        let error = client.connect(transport).await.expect_err("401 connect");
        let McpClientError::AuthRequired(auth_required) = error else {
            panic!("expected McpAuthRequiredError");
        };
        assert_eq!(
            auth_required.body,
            expected["authRequired"]["body"].as_str().unwrap()
        );
        assert_eq!(
            auth_required.www_authenticate,
            Some(
                expected["authRequired"]["wwwAuthenticate"]
                    .as_str()
                    .unwrap()
                    .to_string()
            )
        );
        assert_eq!(
            McpClientError::AuthRequired(auth_required).to_string(),
            expected["authRequired"]["message"].as_str().unwrap()
        );
    }
    // 404 without a session -> plain HTTP error.
    {
        let (fetch, _requests) = recorder_fetch(
            Arc::new(|_record, _index| Ok(answer(404, &[], "gone"))),
            true,
        );
        let transport = http_transport("http://mcp.test/b", fetch, false);
        let client = plain_client();
        let error = client.connect(transport).await.expect_err("404 connect");
        let McpClientError::Http(http_error) = error else {
            panic!("expected McpHttpError");
        };
        assert_eq!(
            http_error.status,
            expected["notFound"]["status"].as_u64().unwrap() as u16
        );
        assert_eq!(
            http_error.message,
            expected["notFound"]["message"].as_str().unwrap()
        );
    }
    // With a session -> session expired.
    {
        let handler: RecorderHandler = Arc::new(move |record, _index| {
            Ok(if record["method"] != "POST" {
                answer(405, &[], "")
            } else {
                let message: Value = serde_json::from_str(record["body"].as_str().unwrap_or(""))
                    .unwrap_or(json!({}));
                match message["method"].as_str().unwrap_or("") {
                    "initialize" => answer_json(
                        200,
                        &[
                            ("content-type", "application/json"),
                            ("mcp-session-id", "s"),
                        ],
                        initialize_response.clone(),
                    ),
                    "ping" => answer(404, &[], "gone"),
                    _ => answer(202, &[], ""),
                }
            })
        });
        let (fetch, _requests) = recorder_fetch(handler, true);
        let transport = http_transport("http://mcp.test/c", fetch, false);
        let client = plain_client();
        client.connect(transport).await.expect("connect");
        let error = client
            .ping(McpRequestOptions::default())
            .await
            .expect_err("session expired");
        let McpClientError::SessionExpired(expired) = error else {
            panic!("expected McpSessionExpiredError");
        };
        assert_eq!(expired.body, "gone");
        assert_eq!(
            McpClientError::SessionExpired(expired).to_string(),
            expected["sessionExpired"]["message"].as_str().unwrap()
        );
    }
    // 500 with a very long body: describeHttpFailure truncation.
    {
        let long = "z".repeat(900);
        let (fetch, _requests) = recorder_fetch(
            Arc::new(move |_record, _index| Ok(answer(500, &[], &long))),
            true,
        );
        let transport = http_transport("http://mcp.test/d", fetch, false);
        let client = plain_client();
        let error = client.connect(transport).await.expect_err("500 connect");
        let McpClientError::Http(http_error) = error else {
            panic!("expected McpHttpError");
        };
        assert_eq!(
            http_error.status,
            expected["serverError"]["status"].as_u64().unwrap() as u16
        );
        assert_eq!(
            http_error.message,
            expected["serverError"]["message"].as_str().unwrap()
        );
        assert_eq!(
            http_error.message.chars().count(),
            expected["serverError"]["messageLength"].as_u64().unwrap() as usize
        );
        assert_eq!(
            http_error.body.chars().count(),
            expected["serverError"]["bodyLength"].as_u64().unwrap() as usize
        );
    }
    // 202 for a request (no response body).
    {
        let first = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let seen_first = Arc::clone(&first);
        let (fetch, _requests) = recorder_fetch(
            Arc::new(move |_record, _index| {
                if seen_first.swap(false, std::sync::atomic::Ordering::SeqCst) {
                    let (_r, initialize_response) = http_fixture();
                    return Ok(answer_json(
                        200,
                        &[("content-type", "application/json")],
                        initialize_response,
                    ));
                }
                Ok(answer(202, &[], ""))
            }),
            true,
        );
        let transport = http_transport("http://mcp.test/e", fetch, false);
        let client = plain_client();
        client.connect(transport).await.expect("connect");
        let error = client
            .ping(McpRequestOptions::default())
            .await
            .expect_err("202 for a request");
        let McpClientError::Http(http_error) = error else {
            panic!("expected McpHttpError");
        };
        assert_eq!(
            http_error.status,
            expected["acceptedWithoutResponse"]["status"]
                .as_u64()
                .unwrap() as u16
        );
        assert_eq!(
            http_error.message,
            expected["acceptedWithoutResponse"]["message"]
                .as_str()
                .unwrap()
        );
    }
    // Unsupported content type.
    {
        let first = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let seen_first = Arc::clone(&first);
        let (fetch, _requests) = recorder_fetch(
            Arc::new(move |_record, _index| {
                if seen_first.swap(false, std::sync::atomic::Ordering::SeqCst) {
                    let (_r, initialize_response) = http_fixture();
                    return Ok(answer_json(
                        200,
                        &[("content-type", "application/json")],
                        initialize_response,
                    ));
                }
                Ok(answer(200, &[("content-type", "text/plain")], "hi"))
            }),
            true,
        );
        let transport = http_transport("http://mcp.test/f", fetch, false);
        let client = plain_client();
        client.connect(transport).await.expect("connect");
        let error = client
            .ping(McpRequestOptions::default())
            .await
            .expect_err("unsupported type");
        let McpClientError::Http(http_error) = error else {
            panic!("expected McpHttpError");
        };
        assert_eq!(
            http_error.status,
            expected["unsupportedType"]["status"].as_u64().unwrap() as u16
        );
        assert_eq!(
            http_error.message,
            expected["unsupportedType"]["message"].as_str().unwrap()
        );
    }
    // JSON array response body.
    {
        let first = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let seen_first = Arc::clone(&first);
        let (fetch, _requests) = recorder_fetch(
            Arc::new(move |record, _index| {
                if record["method"] != "POST" {
                    return Ok(answer(405, &[], ""));
                }
                if seen_first.swap(false, std::sync::atomic::Ordering::SeqCst) {
                    let (_r, initialize_response) = http_fixture();
                    return Ok(answer_json(
                        200,
                        &[("content-type", "application/json")],
                        initialize_response,
                    ));
                }
                Ok(answer_json(
                    200,
                    &[("content-type", "application/json")],
                    json!([
                        { "jsonrpc": "2.0", "method": "n1" },
                        { "jsonrpc": "2.0", "method": "n2" },
                    ]),
                ))
            }),
            true,
        );
        let transport = http_transport("http://mcp.test/g", fetch, false);
        let client = plain_client();
        let notifications = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&notifications);
        client.on_notification(
            "n1",
            Arc::new(move |_params: &Value| {
                log.lock().expect("n1").push("n1".to_string());
            }),
        );
        let log = Arc::clone(&notifications);
        client.on_notification(
            "n2",
            Arc::new(move |_params: &Value| {
                log.lock().expect("n2").push("n2".to_string());
            }),
        );
        client.connect(transport).await.expect("connect");
        let _ = client.ping(McpRequestOptions::default()).await;
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        client.close().await.expect("close");
        assert_canonical(
            "array body notifications",
            json!(notifications.lock().expect("log").clone()),
            &expected["arrayBodyNotifications"],
        );
    }
}

#[tokio::test(start_paused = true)]
async fn http_stream_failures_match_the_capture() {
    let expected = &oracle()["http_stream_failures"];
    let (_result, initialize_response) = http_fixture();

    // SSE response stream with invalid JSON, no event ids -> fails this
    // request only, with the synthetic internalError response.
    {
        let init_json = initialize_response.clone();
        let handler: RecorderHandler = Arc::new(move |record, _index| {
            Ok(if record["method"] == "POST" {
                let message: Value = serde_json::from_str(record["body"].as_str().unwrap_or(""))
                    .unwrap_or(json!({}));
                if message["method"] == "initialize" {
                    answer_json(
                        200,
                        &[("content-type", "application/json")],
                        init_json.clone(),
                    )
                } else {
                    sse_answer(
                        200,
                        &[("content-type", "text/event-stream")],
                        vec![
                            "data: not json\n\n".to_string(),
                            "data: also not\n\n".to_string(),
                        ],
                    )
                }
            } else {
                answer(405, &[], "")
            })
        });
        let (fetch, _requests) = recorder_fetch(handler, true);
        let transport = http_transport("http://mcp.test/h", fetch, false);
        let client = plain_client();
        let errors = Arc::new(Mutex::new(Vec::new()));
        let error_log = Arc::clone(&errors);
        client.on_error(Arc::new(move |error: &McpClientError| {
            error_log.lock().expect("errors").push(error.to_string());
        }));
        client.connect(transport).await.expect("connect");
        let error = client
            .ping(McpRequestOptions::default())
            .await
            .expect_err("bad sse");
        let McpClientError::Mcp(mcp_error) = error else {
            panic!("expected McpError for the synthetic stream failure");
        };
        assert_eq!(mcp_error.code, expected["badSse"]["code"].as_i64().unwrap());
        assert_eq!(
            mcp_error.message,
            expected["badSse"]["message"].as_str().unwrap()
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        // Two transport-level parse errors (V8 texts; serde_json differs).
        assert_eq!(errors.lock().expect("errors").len(), 2);
        client.close().await.expect("close");
    }
    // Stream ends without any events -> "stream ended without a response".
    {
        let init_json = initialize_response.clone();
        let handler: RecorderHandler = Arc::new(move |record, _index| {
            Ok(if record["method"] == "POST" {
                let message: Value = serde_json::from_str(record["body"].as_str().unwrap_or(""))
                    .unwrap_or(json!({}));
                if message["method"] == "initialize" {
                    answer_json(
                        200,
                        &[("content-type", "application/json")],
                        init_json.clone(),
                    )
                } else {
                    sse_answer(
                        200,
                        &[("content-type", "text/event-stream")],
                        vec![": keepalive\n\n".to_string()],
                    )
                }
            } else {
                answer(405, &[], "")
            })
        });
        let (fetch, _requests) = recorder_fetch(handler, true);
        let transport = http_transport("http://mcp.test/i", fetch, false);
        let client = plain_client();
        client.connect(transport).await.expect("connect");
        let error = client
            .ping(McpRequestOptions::default())
            .await
            .expect_err("empty stream");
        let McpClientError::Mcp(mcp_error) = error else {
            panic!("expected McpError for the empty stream");
        };
        assert_eq!(
            mcp_error.code,
            expected["emptyStream"]["code"].as_i64().unwrap()
        );
        assert_eq!(
            mcp_error.message,
            expected["emptyStream"]["message"].as_str().unwrap()
        );
        client.close().await.expect("close");
    }
    // Resumption: priming id + retry, stream breaks, GET resume with
    // Last-Event-ID answers the request.
    {
        let init_json = initialize_response.clone();
        let resume_answer = json!({
            "jsonrpc": "2.0", "id": 2,
            "result": { "content": [{ "type": "text", "text": "resumed" }] },
        });
        let resume_text = stringify(&resume_answer);
        let handler: RecorderHandler = Arc::new(move |record, _index| {
            Ok(if record["method"] == "POST" {
                let message: Value = serde_json::from_str(record["body"].as_str().unwrap_or(""))
                    .unwrap_or(json!({}));
                if message["method"] == "initialize" {
                    answer_json(
                        200,
                        &[("content-type", "application/json")],
                        init_json.clone(),
                    )
                } else {
                    sse_answer(
                        200,
                        &[("content-type", "text/event-stream")],
                        vec!["id: 1\nretry: 5\ndata:\n\n".to_string()],
                    )
                }
            } else if record["method"] == "GET" {
                sse_answer(
                    200,
                    &[("content-type", "text/event-stream")],
                    vec![format!("id: 2\ndata: {resume_text}\n\n")],
                )
            } else {
                answer(405, &[], "")
            })
        });
        let (fetch, requests) = recorder_fetch(handler, true);
        let mut options =
            StreamableHttpTransportOptions::new(url::Url::parse("http://mcp.test/j").unwrap());
        options.fetch = Some(fetch);
        options.open_get_stream = Some(false);
        options.reconnect = Some(crate::mcp::transports::StreamableHttpReconnectOptions {
            initial_delay_ms: Some(1),
            max_delay_ms: Some(4),
            max_retries: Some(2),
        });
        let transport = Arc::new(StreamableHttpTransport::new(options));
        let client = plain_client();
        client
            .connect(Arc::clone(&transport) as Arc<dyn McpTransport>)
            .await
            .expect("connect");
        let called = client
            .call_tool("echo", None, McpRequestOptions::default())
            .await
            .expect("resumed call");
        assert_canonical("resumed", json!(called), &expected["resume"]["called"]);
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        client.close().await.expect("close");
        let expected_headers = expected["resumeRequestLog"].as_array().unwrap();
        let get_requests: Vec<Value> = requests
            .lock()
            .expect("log")
            .clone()
            .iter()
            .filter(|request| request["method"] == "GET")
            .cloned()
            .collect();
        assert_eq!(get_requests.len(), expected_headers.len());
        for (actual, expected_request) in get_requests.iter().zip(expected_headers) {
            for key in ["accept", "last-event-id", "mcp-protocol-version"] {
                assert_eq!(
                    actual["headers"][key], expected_request[key],
                    "resume header {key}"
                );
            }
        }
    }
}

#[tokio::test(start_paused = true)]
async fn http_get_stream_matches_the_capture() {
    let expected = &oracle()["http_get_stream"];
    let (_result, initialize_response) = http_fixture();
    let init_json = initialize_response.clone();

    // GET stream opened after notifications/initialized; the capture's GET
    // fixture hands fetch a stream that never yields a parseable event, so
    // `pushes` stays empty (reproduced with a pending stream).
    {
        let handler: RecorderHandler = Arc::new(move |record, _index| {
            Ok(if record["method"] == "POST" {
                let message: Value = serde_json::from_str(record["body"].as_str().unwrap_or(""))
                    .unwrap_or(json!({}));
                if message["method"] == "initialize" {
                    answer_json(
                        200,
                        &[
                            ("content-type", "application/json"),
                            ("mcp-session-id", "gs"),
                        ],
                        init_json.clone(),
                    )
                } else {
                    answer(202, &[], "")
                }
            } else if record["method"] == "GET" {
                Answer {
                    status: 200,
                    headers: vec![("content-type".to_string(), "text/event-stream".to_string())],
                    chunks: Vec::new(),
                    never: true,
                }
            } else {
                answer(200, &[], "")
            })
        });
        let (fetch, requests) = recorder_fetch(handler, true);
        let transport = http_transport("http://mcp.test/k", fetch, true);
        let client = plain_client();
        let pushes = Arc::new(Mutex::new(Vec::new()));
        let push_log = Arc::clone(&pushes);
        client.on_notification(
            "server/push",
            Arc::new(move |params: &Value| {
                push_log.lock().expect("pushes").push(params.clone());
            }),
        );
        client
            .connect(Arc::clone(&transport) as Arc<dyn McpTransport>)
            .await
            .expect("connect");
        client
            .notify("status/update", Some(serde_json::Map::new()))
            .await
            .expect("notify");
        tokio::time::sleep(std::time::Duration::from_millis(60)).await;
        assert!(pushes.lock().expect("pushes").is_empty());
        let get_headers: Vec<Value> = requests
            .lock()
            .expect("log")
            .clone()
            .iter()
            .filter(|request| request["method"] == "GET")
            .map(|request| request["headers"].clone())
            .collect();
        assert_canonical(
            "GET session headers",
            json!(get_headers),
            &expected["getSessionHeaders"],
        );
        client.close().await.expect("close");
        let delete_requests: Vec<Value> = requests
            .lock()
            .expect("log")
            .clone()
            .iter()
            .filter(|request| request["method"] == "DELETE")
            .map(|request| json!({ "headers": request["headers"] }))
            .collect();
        assert_canonical(
            "DELETE requests",
            json!(delete_requests),
            &expected["deleteRequests"],
        );
    }
    // 405 GET stream: silent, transport still works.
    {
        let init_json = initialize_response.clone();
        let handler: RecorderHandler = Arc::new(move |record, _index| {
            Ok(if record["method"] == "POST" {
                let message: Value = serde_json::from_str(record["body"].as_str().unwrap_or(""))
                    .unwrap_or(json!({}));
                if message["method"] == "initialize" {
                    answer_json(
                        200,
                        &[("content-type", "application/json")],
                        init_json.clone(),
                    )
                } else {
                    answer(202, &[], "")
                }
            } else {
                answer(405, &[], "")
            })
        });
        let (fetch, requests) = recorder_fetch(handler, true);
        let transport = http_transport("http://mcp.test/l", fetch, true);
        let client = plain_client();
        let errors = Arc::new(Mutex::new(Vec::new()));
        let error_log = Arc::clone(&errors);
        client.on_error(Arc::new(move |error: &McpClientError| {
            error_log.lock().expect("errors").push(error.to_string());
        }));
        client
            .connect(Arc::clone(&transport) as Arc<dyn McpTransport>)
            .await
            .expect("connect");
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        client.close().await.expect("close");
        assert!(errors.lock().expect("errors").is_empty());
        let get_count = requests
            .lock()
            .expect("log")
            .iter()
            .filter(|request| request["method"] == "GET")
            .count();
        assert_eq!(
            get_count,
            expected["getAttempts"].as_u64().unwrap() as usize
        );
    }
}

/// The `http_auth_provider` scenario: a 401 handed to the provider once, the
/// request retried with the fresh token.
#[tokio::test(start_paused = true)]
async fn http_auth_provider_matches_the_capture() {
    let expected = &oracle()["http_auth_provider"];
    let (_result, initialize_response) = http_fixture();
    let handler: RecorderHandler = Arc::new(move |record, _index| {
        Ok(if record["method"] != "POST" {
            answer(405, &[], "")
        } else if record["headers"]["authorization"] == "Bearer fresh-token" {
            let message: Value =
                serde_json::from_str(record["body"].as_str().unwrap_or("")).unwrap_or(json!({}));
            if message["method"] == "initialize" {
                answer_json(
                    200,
                    &[("content-type", "application/json")],
                    initialize_response.clone(),
                )
            } else if message.get("id").is_some() {
                answer_json(
                    200,
                    &[("content-type", "application/json")],
                    json!({ "jsonrpc": "2.0", "id": message["id"], "result": {} }),
                )
            } else {
                answer(202, &[], "")
            }
        } else {
            answer(401, &[("www-authenticate", "Bearer")], "auth required")
        })
    });
    let (fetch, requests) = recorder_fetch(handler, true);
    let state = Arc::new(Mutex::new(AuthProviderState {
        token: "stale-token".to_string(),
        ..AuthProviderState::default()
    }));
    let provider: Arc<dyn AuthProvider> = Arc::new(TestAuthProvider {
        state: Arc::clone(&state),
    });
    let mut options =
        StreamableHttpTransportOptions::new(url::Url::parse("http://mcp.test/m").unwrap());
    options.fetch = Some(fetch);
    options.open_get_stream = Some(false);
    options.auth_provider = Some(provider);
    let transport = Arc::new(StreamableHttpTransport::new(options));
    let client = plain_client();
    client
        .connect(Arc::clone(&transport) as Arc<dyn McpTransport>)
        .await
        .expect("connect");
    client
        .ping(McpRequestOptions::default())
        .await
        .expect("ping");
    {
        let state = state.lock().expect("auth state");
        let (calls, first_context) = (state.calls, state.first_context.clone());
        assert_eq!(
            calls,
            expected["unauthorizedCalls"].as_u64().unwrap() as u32
        );
        let (server_url, token) = first_context.expect("first context");
        assert_eq!(
            server_url,
            expected["firstContext"]["serverUrl"].as_str().unwrap()
        );
        assert_eq!(token, expected["firstContext"]["token"].as_str().unwrap());
    }
    let auth_headers: Vec<String> = requests
        .lock()
        .expect("log")
        .iter()
        .map(|request| {
            request["headers"]["authorization"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    let expected_headers: Vec<String> = expected["requestAuthHeaders"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap().to_string())
        .collect();
    assert_eq!(auth_headers, expected_headers);
    client.close().await.expect("close");
}

/// The capture's inline auth provider double state: the current token, the
/// unauthorized-call count, and the first rejected-request context.
#[derive(Default)]
struct AuthProviderState {
    token: String,
    calls: u32,
    first_context: Option<(String, String)>,
}

/// The capture's inline auth provider double.
struct TestAuthProvider {
    state: Arc<Mutex<AuthProviderState>>,
}

impl AuthProvider for TestAuthProvider {
    fn token(&self) -> BoxFuture<'_, Option<String>> {
        Box::pin(async move {
            let state = self.state.lock().expect("auth state");
            Some(state.token.clone())
        })
    }

    fn handles_unauthorized(&self) -> bool {
        true
    }

    fn on_unauthorized<'a>(
        &'a self,
        context: UnauthorizedContext,
    ) -> BoxFuture<'a, Result<(), McpClientError>> {
        Box::pin(async move {
            {
                let mut state = self.state.lock().expect("auth state");
                state.calls += 1;
                state.first_context = Some((
                    context.server_url.to_string(),
                    context.token.clone().unwrap_or_default(),
                ));
                state.token = "fresh-token".to_string();
            }
            let _ = context.response;
            Ok(())
        })
    }
}

// ---------------------------------------------------------------------------
// SSE parser (consumeSseStream), inputs from the capture source
// ---------------------------------------------------------------------------

fn byte_stream(chunks: Vec<&str>) -> ByteStream {
    let chunks: Vec<Vec<u8>> = chunks
        .into_iter()
        .map(|chunk| chunk.as_bytes().to_vec())
        .collect();
    Box::pin(futures::stream::iter(
        chunks.into_iter().map(Ok::<_, std::io::Error>),
    ))
}

#[tokio::test]
async fn sse_parser_matches_the_capture() {
    let cases = &oracle()["sse_parser"]["cases"];
    let inputs: &[(&str, Vec<String>, Option<usize>)] = &[
        ("basic", vec!["data: hello\n\n".to_string()], None),
        (
            "crlf",
            vec!["data: a\r\n\r\ndata: b\r\nevent: tick\r\n\r\n".to_string()],
            None,
        ),
        (
            "multiline_data",
            vec!["data: one\ndata: two\ndata: three\n\n".to_string()],
            None,
        ),
        (
            "comment_and_bom",
            vec!["\u{FEFF}: comment\ndata: after bom\n\n".to_string()],
            None,
        ),
        (
            "space_stripping",
            vec!["data:    padded   \n\n".to_string()],
            None,
        ),
        (
            "no_colon_line",
            vec!["data\n\ndata: x\n\n".to_string()],
            None,
        ),
        (
            "id_and_retry",
            vec![
                "id: 42\nretry: 2500\ndata: x\n\n".to_string(),
                "id: bad\0id\ndata: y\n\n".to_string(),
                "retry: 1.5\ndata: z\n\n".to_string(),
                "retry: -3\ndata: w\n\n".to_string(),
            ],
            None,
        ),
        (
            "id_without_data",
            vec!["id: prime\n\n".to_string(), "data: real\n\n".to_string()],
            None,
        ),
        (
            "event_type_filtered_later",
            vec!["event: custom\ndata: {}\n\n".to_string()],
            None,
        ),
        (
            "fragmented",
            vec![
                "dat".to_string(),
                "a: fr".to_string(),
                "agmented\n".to_string(),
                "\n".to_string(),
            ],
            None,
        ),
        ("no_trailing_newline", vec!["data: tail".to_string()], None),
        (
            "crlf_split_across_chunks",
            vec![
                "data: x\r".to_string(),
                "\n\r".to_string(),
                "\n".to_string(),
            ],
            None,
        ),
        (
            "max_event_bytes",
            vec![format!("data: {}\n\n", "x".repeat(30))],
            Some(10),
        ),
        ("max_buffer_bytes", vec!["z".repeat(30)], Some(10)),
        (
            "many_short_data_lines",
            std::iter::repeat_n("data: ab\n".to_string(), 12)
                .chain(std::iter::once("\n".to_string()))
                .collect(),
            Some(20),
        ),
    ];
    for (name, chunks, max_event_bytes) in inputs {
        let case = cases
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["name"] == *name)
            .unwrap_or_else(|| panic!("case {name} missing from the fixture"));
        let events = Arc::new(Mutex::new(Vec::new()));
        let ids = Arc::new(Mutex::new(Vec::new()));
        let retries = Arc::new(Mutex::new(Vec::new()));
        let event_log = Arc::clone(&events);
        let id_log = Arc::clone(&ids);
        let retry_log = Arc::clone(&retries);
        let result = consume_sse_stream(
            byte_stream(chunks.iter().map(String::as_str).collect()),
            ConsumeSseOptions {
                max_event_bytes: *max_event_bytes,
                on_event: Box::new(move |event: SseEvent| {
                    event_log
                        .lock()
                        .expect("events")
                        .push(serde_json::to_value(&event).unwrap());
                }),
                on_id: Some(Box::new(move |id: &str| {
                    id_log.lock().expect("ids").push(id.to_string());
                })),
                on_retry: Some(Box::new(move |delay_ms: u64| {
                    retry_log.lock().expect("retries").push(delay_ms);
                })),
            },
        )
        .await;
        assert_canonical(
            &format!("{name} events"),
            json!(events.lock().expect("events").clone()),
            &case["events"],
        );
        assert_canonical(
            &format!("{name} ids"),
            json!(ids.lock().expect("ids").clone()),
            &case["ids"],
        );
        assert_canonical(
            &format!("{name} retries"),
            json!(retries.lock().expect("retries").clone()),
            &case["retries"],
        );
        match (result, case.get("error")) {
            (Ok(()), None) => {}
            (Ok(()), Some(expected_error)) => {
                panic!(
                    "{name} should have failed with {}",
                    expected_error.as_str().unwrap()
                );
            }
            (Err(error), Some(expected_error)) => {
                assert_eq!(
                    error.to_string(),
                    expected_error.as_str().unwrap(),
                    "{name} error"
                );
            }
            (Err(error), None) => panic!("{name} failed unexpectedly: {error}"),
        }
    }
}
