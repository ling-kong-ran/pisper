//! Port of `pi/packages/evals/evals/acme-server.ts` — the loopback fixture
//! provider servers ("acme" OpenAI-compatible SSE and "acme-stream" NDJSON)
//! used by the provider documentation evals. The port binds a std
//! `TcpListener` and speaks HTTP/1.1 directly, byte-for-byte the same wire
//! bodies as the node server.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub const OPENAI_PROVIDER_ID: &str = "acme";
pub const OPENAI_MODEL_ID: &str = "acme-chat";
pub const OPENAI_PROBE_PROMPT: &str = "Reply with ACME_OK.";
pub const OPENAI_PROBE_RESPONSE: &str = "ACME_OK";
pub const STREAM_PROVIDER_ID: &str = "acme-stream";
pub const STREAM_MODEL_ID: &str = "acme-stream-chat";
pub const STREAM_PROBE_PROMPT: &str = "Reply with ACME_STREAM_OK.";
pub const STREAM_PROBE_RESPONSE: &str = "ACME_STREAM_OK";

/// Upstream `STREAM_API_DOCUMENTATION` (`JSON.stringify(spec, null, 2)`).
pub const STREAM_API_DOCUMENTATION: &str = concat!(
    "{\n",
    "  \"name\": \"Acme Streaming API\",\n",
    "  \"request\": {\n",
    "    \"method\": \"POST\",\n",
    "    \"path\": \"/generate\",\n",
    "    \"headers\": { \"content-type\": \"application/json\", \"x-acme-key\": \"resolved credential\" },\n",
    "    \"body\": { \"model\": \"acme-stream-chat\", \"messages\": [{ \"role\": \"user\", \"content\": \"Hello\" }], \"stream\": true },\n",
    "  },\n",
    "  \"response\": {\n",
    "    \"contentType\": \"application/x-ndjson\",\n",
    "    \"events\": [\n",
    "      { \"type\": \"text_delta\", \"text\": \"Hello\" },\n",
    "      { \"type\": \"usage\", \"input_tokens\": 3, \"output_tokens\": 2 },\n",
    "      { \"type\": \"done\", \"reason\": \"stop\" }\n",
    "    ]\n",
    "  }\n",
    "}\n"
);

/// Upstream `ServerMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerMode {
    OpenAi,
    Stream,
}

/// Upstream `AcmeServer`.
pub struct AcmeServer {
    mode: ServerMode,
    shutdown: Arc<AtomicBool>,
    valid: Arc<AtomicBool>,
    handle: Mutex<Option<std::thread::JoinHandle<()>>>,
    origin: Mutex<String>,
}

fn reject(stream: &mut TcpStream, status: u16, message: &str) {
    let body = message.to_string();
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len(),
        reason = match status {
            400 => "Bad Request",
            401 => "Unauthorized",
            404 => "Not Found",
            405 => "Method Not Allowed",
            415 => "Unsupported Media Type",
            _ => "Unprocessable Entity",
        }
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

/// One parsed HTTP/1.1 request: method, path, lower-cased header pairs, body.
type RawHttpRequest = (String, String, Vec<(String, String)>, String);

/// Reads one HTTP/1.1 request (headers + content-length body) from the
/// stream.
fn read_request(stream: &mut TcpStream) -> Option<RawHttpRequest> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line).ok()?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();
    let mut headers = Vec::new();
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).ok()?;
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim().to_string();
            if name == "content-length" {
                content_length = value.parse().unwrap_or(0);
            }
            headers.push((name, value));
        }
    }
    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body).ok()?;
    Some((
        method,
        path,
        headers,
        String::from_utf8_lossy(&body).to_string(),
    ))
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

fn is_record(value: &serde_json::Value) -> bool {
    value.is_object()
}

fn handle_connection(mode: ServerMode, valid: &AtomicBool, mut stream: TcpStream) {
    let Some((method, path, headers, body_text)) = read_request(&mut stream) else {
        return;
    };
    let expected_path = match mode {
        ServerMode::OpenAi => "/v1/chat/completions",
        ServerMode::Stream => "/generate",
    };
    if path != expected_path {
        return reject(&mut stream, 404, "Unknown endpoint");
    }
    if method != "POST" {
        return reject(&mut stream, 405, "Expected POST");
    }
    if !header(&headers, "content-type")
        .unwrap_or_default()
        .starts_with("application/json")
    {
        return reject(&mut stream, 415, "Expected application/json");
    }
    let payload: serde_json::Value = match serde_json::from_str(&body_text) {
        Ok(payload) => payload,
        Err(_) => return reject(&mut stream, 400, "Invalid JSON"),
    };
    if !is_record(&payload) {
        return reject(&mut stream, 422, "Expected a JSON object");
    }
    let messages = payload
        .get("messages")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();
    let prompt = messages
        .iter()
        .find(|message| {
            is_record(message) && message.get("role").and_then(|role| role.as_str()) == Some("user")
        })
        .and_then(|message| message.get("content"))
        .and_then(|content| content.as_str());

    match mode {
        ServerMode::Stream => {
            if header(&headers, "x-acme-key") != Some("resolved-stream-key") {
                return reject(&mut stream, 401, "Invalid Acme Stream credential");
            }
            if payload.get("model").and_then(|value| value.as_str()) != Some(STREAM_MODEL_ID)
                || prompt.is_none()
                || payload.get("stream").and_then(|value| value.as_bool()) != Some(true)
            {
                return reject(&mut stream, 422, "Invalid Acme Stream request");
            }
            valid.store(prompt == Some(STREAM_PROBE_PROMPT), Ordering::SeqCst);
            let mut body = String::new();
            body.push_str(&format!(
                "{{\"type\":\"text_delta\",\"text\":{}}}\n",
                serde_json::to_string("ACME_").expect("string serializes")
            ));
            body.push_str(&format!(
                "{{\"type\":\"text_delta\",\"text\":{}}}\n",
                serde_json::to_string("STREAM_OK").expect("string serializes")
            ));
            body.push_str("{\"type\":\"usage\",\"input_tokens\":4,\"output_tokens\":3}\n");
            body.push_str("{\"type\":\"done\",\"reason\":\"stop\"}\n");
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/x-ndjson\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
        ServerMode::OpenAi => {
            if header(&headers, "authorization") != Some("Bearer resolved-acme-key") {
                return reject(&mut stream, 401, "Invalid Acme credential");
            }
            if payload.get("model").and_then(|value| value.as_str()) != Some(OPENAI_MODEL_ID)
                || prompt.is_none()
                || payload.get("stream").and_then(|value| value.as_bool()) != Some(true)
            {
                return reject(&mut stream, 422, "Invalid OpenAI-compatible request");
            }
            valid.store(prompt == Some(OPENAI_PROBE_PROMPT), Ordering::SeqCst);
            // Build the two SSE frames explicitly to preserve upstream
            // `JSON.stringify` key order exactly.
            let first = format!(
                "data: {{\"id\":\"chatcmpl-acme\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"{OPENAI_MODEL_ID}\",\"choices\":[{{\"index\":0,\"delta\":{{\"role\":\"assistant\",\"content\":{}}},\"finish_reason\":null}}]}}\n\n",
                serde_json::to_string(OPENAI_PROBE_RESPONSE).expect("string serializes")
            );
            let second = "data: {\"id\":\"chatcmpl-acme\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"acme-chat\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2}}\n\n";
            let mut body = first;
            body.push_str(second);
            body.push_str("data: [DONE]\n\n");
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncache-control: no-cache\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    }
}

impl AcmeServer {
    /// Upstream `start`: bind a loopback port and serve on a background
    /// thread.
    pub fn start(mode: ServerMode) -> std::io::Result<Arc<AcmeServer>> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        let server = Arc::new(AcmeServer {
            mode,
            shutdown: Arc::new(AtomicBool::new(false)),
            valid: Arc::new(AtomicBool::new(false)),
            handle: Mutex::new(None),
            origin: Mutex::new(format!("http://127.0.0.1:{port}")),
        });
        let worker_server = Arc::clone(&server);
        let handle = std::thread::spawn(move || {
            listener
                .set_nonblocking(true)
                .expect("nonblocking listener");
            while !worker_server.shutdown.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let valid = Arc::clone(&worker_server.valid);
                        let mode = worker_server.mode;
                        std::thread::spawn(move || {
                            let _ = stream.set_nonblocking(false);
                            handle_connection(mode, &valid, stream);
                        });
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });
        *server.handle.lock().expect("handle lock") = Some(handle);
        Ok(server)
    }

    /// Upstream `stop`.
    pub fn stop(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.lock().expect("handle lock").take() {
            let _ = handle.join();
        }
    }

    /// Upstream `reset`.
    pub fn reset(&self) {
        self.valid.store(false, Ordering::SeqCst);
    }

    /// Upstream `origin`.
    pub fn origin(&self) -> String {
        self.origin.lock().expect("origin lock").clone()
    }

    /// Upstream `baseUrl`.
    pub fn base_url(&self) -> String {
        format!("{}/v1", self.origin())
    }

    /// Upstream `validRequestReceived`.
    pub fn valid_request_received(&self) -> bool {
        self.valid.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
#[path = "acme_server_tests.rs"]
mod tests;
