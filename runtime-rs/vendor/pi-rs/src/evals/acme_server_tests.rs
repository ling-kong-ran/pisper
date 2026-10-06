//! Ports of `pi/packages/evals/test/acme-server.test.ts` (both fixtures, all
//! five scenarios each) over the std-TCP server, plus a byte-level oracle
//! comparison against the node server (`tests/fixtures/evals_m6/oracle/acme.json`).

use super::{
    AcmeServer, ServerMode, OPENAI_MODEL_ID, OPENAI_PROBE_PROMPT, OPENAI_PROBE_RESPONSE,
    STREAM_API_DOCUMENTATION, STREAM_MODEL_ID, STREAM_PROBE_PROMPT, STREAM_PROBE_RESPONSE,
};
use std::io::{Read, Write};
use std::sync::Arc;

struct StartedServer {
    server: Arc<AcmeServer>,
}

impl StartedServer {
    fn new(mode: ServerMode) -> Self {
        Self {
            server: AcmeServer::start(mode).expect("bind loopback"),
        }
    }
}

impl Drop for StartedServer {
    fn drop(&mut self) {
        self.server.stop();
    }
}

fn body(model: &str, prompt: &str, stream: bool) -> String {
    format!(
        "{{\"model\":{model},\"messages\":[{{\"role\":\"user\",\"content\":{prompt}}}],\"stream\":{stream}}}",
        model = serde_json::to_string(model).unwrap(),
        prompt = serde_json::to_string(prompt).unwrap(),
        stream = stream,
    )
}

struct HttpResponse {
    status: u16,
    content_type: String,
    body: String,
}

fn post(url: &str, headers: &[(&str, &str)], payload: &str) -> HttpResponse {
    let authority = url
        .trim_start_matches("http://")
        .split('/')
        .next()
        .expect("authority")
        .to_string();
    let mut stream = std::net::TcpStream::connect(&authority).expect("connect");
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .expect("timeout");
    let after_authority = url
        .find("://")
        .map(|index| &url[index + 3..])
        .unwrap_or(url);
    let path = after_authority
        .find('/')
        .map(|index| after_authority[index..].to_string())
        .unwrap_or_else(|| "/".to_string());
    let mut request = format!(
        "POST {path} HTTP/1.1\r\nhost: {authority}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n",
        payload.len()
    );
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");
    request.push_str(payload);
    stream.write_all(request.as_bytes()).expect("write request");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).expect("read response");
    let text = String::from_utf8_lossy(&raw).to_string();
    let (head, body) = text.split_once("\r\n\r\n").expect("response head/body");
    let status: u16 = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .expect("status line");
    let content_type = head
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("content-type"))
                .map(|(_, value)| value.trim().to_string())
        })
        .unwrap_or_default();
    HttpResponse {
        status,
        content_type,
        body: strip_chunked(body),
    }
}

/// Node responds with chunked framing on the oracle; this port writes
/// content-length. Decode chunked framing when present so bodies compare.
fn strip_chunked(body: &str) -> String {
    if body.contains("\r\n")
        && body
            .chars()
            .next()
            .map(|c| c.is_ascii_hexdigit())
            .unwrap_or(false)
    {
        let mut out = String::new();
        let mut rest = body;
        while let Some(line_end) = rest.find("\r\n") {
            let size_text = &rest[..line_end];
            let Ok(size) = usize::from_str_radix(size_text.trim(), 16) else {
                return body.to_string();
            };
            if size == 0 {
                break;
            }
            let start = line_end + 2;
            out.push_str(&rest[start..start + size]);
            rest = &rest[(start + size + 2).min(rest.len())..];
        }
        return out;
    }
    body.to_string()
}

fn streamed_text_openai(raw: &str) -> String {
    raw.split('\n')
        .filter(|line| line.starts_with("data: ") && *line != "data: [DONE]")
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(&line["data: ".len()..]).ok())
        .filter_map(|chunk| {
            chunk
                .get("choices")
                .and_then(|choices| choices.get(0))
                .and_then(|choice| choice.get("delta"))
                .and_then(|delta| delta.get("content"))
                .and_then(|content| content.as_str())
                .map(str::to_string)
        })
        .collect()
}

fn streamed_text_stream(raw: &str) -> String {
    raw.split('\n')
        .filter(|line| !line.is_empty())
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|event| event.get("type").and_then(|value| value.as_str()) == Some("text_delta"))
        .filter_map(|event| {
            event
                .get("text")
                .and_then(|text| text.as_str())
                .map(str::to_string)
        })
        .collect()
}

struct Fixture {
    openai: bool,
    url: fn(&AcmeServer) -> String,
    headers: Vec<(&'static str, &'static str)>,
    unauthorized_headers: Vec<(&'static str, &'static str)>,
    model: &'static str,
    prompt: &'static str,
    response: &'static str,
    content_type: &'static str,
    unauthorized: &'static str,
    invalid: &'static str,
}

fn fixtures() -> Vec<Fixture> {
    vec![
        Fixture {
            openai: true,
            url: |server| format!("{}/chat/completions", server.base_url()),
            headers: vec![("authorization", "Bearer resolved-acme-key")],
            unauthorized_headers: vec![("authorization", "Bearer wrong-key")],
            model: OPENAI_MODEL_ID,
            prompt: OPENAI_PROBE_PROMPT,
            response: OPENAI_PROBE_RESPONSE,
            content_type: "text/event-stream",
            unauthorized: "Invalid Acme credential",
            invalid: "Invalid OpenAI-compatible request",
        },
        Fixture {
            openai: false,
            url: |server| format!("{}/generate", server.origin()),
            headers: vec![("x-acme-key", "resolved-stream-key")],
            unauthorized_headers: vec![("x-acme-key", "wrong-key")],
            model: STREAM_MODEL_ID,
            prompt: STREAM_PROBE_PROMPT,
            response: STREAM_PROBE_RESPONSE,
            content_type: "application/x-ndjson",
            unauthorized: "Invalid Acme Stream credential",
            invalid: "Invalid Acme Stream request",
        },
    ]
}

fn fixture_scenarios(fixture: &Fixture, server: &Arc<AcmeServer>) {
    let url = (fixture.url)(server);

    // accepts the probe request and records it
    server.reset();
    let response = post(
        &url,
        &fixture.headers,
        &body(fixture.model, fixture.prompt, true),
    );
    assert_eq!(response.status, 200, "{}", response.body);
    assert!(
        response.content_type.contains(fixture.content_type),
        "{}",
        response.content_type
    );
    let text = if fixture.openai {
        streamed_text_openai(&response.body)
    } else {
        streamed_text_stream(&response.body)
    };
    assert_eq!(text, fixture.response);
    assert!(server.valid_request_received());

    // rejects a bad credential with 401 and does not record a probe
    server.reset();
    let response = post(
        &url,
        &fixture.unauthorized_headers,
        &body(fixture.model, fixture.prompt, true),
    );
    assert_eq!(response.status, 401);
    assert_eq!(response.body, fixture.unauthorized);
    assert!(!server.valid_request_received());

    // rejects a malformed request with 422 and does not record a probe
    let response = post(
        &url,
        &fixture.headers,
        &body(fixture.model, fixture.prompt, false),
    );
    assert_eq!(response.status, 422);
    assert_eq!(response.body, fixture.invalid);
    assert!(!server.valid_request_received());

    // does not treat a non-probe success as the probe
    server.reset();
    let response = post(&url, &fixture.headers, &body(fixture.model, "hello", true));
    assert_eq!(response.status, 200);
    assert!(!server.valid_request_received());

    // clears the probe flag on reset
    let response = post(
        &url,
        &fixture.headers,
        &body(fixture.model, fixture.prompt, true),
    );
    assert_eq!(response.status, 200);
    assert!(server.valid_request_received());
    server.reset();
    assert!(!server.valid_request_received());
}

#[test]
fn openai_fixture_behaviors() {
    let started = StartedServer::new(ServerMode::OpenAi);
    fixture_scenarios(&fixtures().remove(0), &started.server);
}

#[test]
fn stream_fixture_behaviors() {
    let started = StartedServer::new(ServerMode::Stream);
    fixture_scenarios(&fixtures().remove(1), &started.server);
}

fn oracle() -> serde_json::Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/evals_m6/oracle/acme.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).expect("acme oracle")).expect("JSON")
}

#[test]
fn oracle_wire_bodies_match_the_node_server() {
    let oracle = oracle();
    let server = StartedServer::new(ServerMode::OpenAi);
    server.server.reset();
    let response = post(
        &format!("{}/chat/completions", server.server.base_url()),
        &[("authorization", "Bearer resolved-acme-key")],
        &body(OPENAI_MODEL_ID, OPENAI_PROBE_PROMPT, true),
    );
    let expected = &oracle["openai"]["probe"];
    assert_eq!(response.status, expected["status"].as_u64().unwrap() as u16);
    assert_eq!(
        response.content_type,
        expected["contentType"].as_str().unwrap()
    );
    assert_eq!(response.body, expected["body"].as_str().unwrap());
    let unauthorized = post(
        &format!("{}/chat/completions", server.server.base_url()),
        &[("authorization", "Bearer wrong-key")],
        &body(OPENAI_MODEL_ID, OPENAI_PROBE_PROMPT, true),
    );
    assert_eq!(
        unauthorized.body,
        oracle["openai"]["unauthorized"]["body"].as_str().unwrap()
    );

    let stream_server = StartedServer::new(ServerMode::Stream);
    let response = post(
        &format!("{}/generate", stream_server.server.origin()),
        &[("x-acme-key", "resolved-stream-key")],
        &body(STREAM_MODEL_ID, STREAM_PROBE_PROMPT, true),
    );
    let expected = &oracle["stream"]["probe"];
    assert_eq!(response.status, expected["status"].as_u64().unwrap() as u16);
    assert_eq!(
        response.content_type,
        expected["contentType"].as_str().unwrap()
    );
    assert_eq!(response.body, expected["body"].as_str().unwrap());
}

#[test]
fn stream_api_documentation_is_the_upstream_spec_text() {
    // Byte-identical to `JSON.stringify(spec, null, 2)` + "\n".
    assert!(STREAM_API_DOCUMENTATION.starts_with("{\n  \"name\": \"Acme Streaming API\",\n"));
    assert!(STREAM_API_DOCUMENTATION.ends_with("  }\n}\n"));
    assert!(STREAM_API_DOCUMENTATION.contains("\"model\": \"acme-stream-chat\""));
}
