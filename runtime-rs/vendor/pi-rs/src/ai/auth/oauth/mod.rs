//! OAuth login flows ported from upstream `packages/ai/src/auth/oauth/`:
//! PKCE utilities (`pkce`), the local redirect landing page (`oauth_page`),
//! the generic device-code poll engine (`device_code`), the shared loopback
//! redirect handler (`callback_server`, upstream `callback-server.ts`), the
//! Anthropic (Claude Pro/Max) flow (`anthropic`, exposing
//! [`AnthropicOAuth`]), the OpenAI Codex (ChatGPT Plus/Pro) flow
//! (`openai_codex`, exposing [`OpenAICodexOAuth`]), the Sign in with ChatGPT
//! flow (`openai_chatgpt`, exposing [`OpenAIChatGptOAuth`]), the Meta Muse
//! flow (`meta`, exposing [`MetaOAuth`]), the GitHub Copilot flow
//! (`github_copilot`, exposing [`GitHubCopilotOAuth`]), the OpenRouter flow
//! (`openrouter`, exposing [`OpenRouterOAuth`]), the xAI flow (`xai`,
//! exposing [`XaiOAuth`]), the Kimi Code flow (`kimi_coding`, exposing
//! [`KimiCodingOAuth`]) and the Radius gateway flow (`radius`, exposing
//! [`RadiusOAuth`]); `load` is the flow-loader registry (upstream `load.ts`).
//!
//! Flows are interactive through [`crate::ai::auth::types::AuthInteraction`]
//! only: the browser gets the authorize URL via the `auth_url` event, the
//! user's paste arrives through the `manual_code` prompt, and progress is
//! reported as events — flows never touch stdio or a browser directly
//! (M2d controller ruling).
//!
//! The pieces below are shared infrastructure used by more than one flow.
//! Upstream hoisted the loopback callback server into `callback-server.ts`
//! with the auth delta (previously duplicated per flow file); the port also
//! shares `parse_authorization_input` (byte-identical in the upstream
//! anthropic/openai-codex copies) here once.

pub mod anthropic;
pub mod callback_server;
pub mod device_code;
pub mod github_copilot;
pub mod kimi_coding;
pub mod load;
pub mod meta;
pub mod oauth_page;

#[cfg(test)]
mod oauth_oracle_tests;
pub mod openai_chatgpt;
pub mod openai_codex;
pub mod openrouter;
pub mod pkce;
pub mod radius;
pub mod xai;

pub use anthropic::AnthropicOAuth;
pub use github_copilot::GitHubCopilotOAuth;
pub use kimi_coding::KimiCodingOAuth;
pub use meta::MetaOAuth;
pub use openai_chatgpt::OpenAIChatGptOAuth;
pub use openai_codex::OpenAICodexOAuth;
pub use openrouter::OpenRouterOAuth;
pub use radius::{create_radius_oauth, RadiusOAuth, RadiusOAuthOptions};
pub use xai::XaiOAuth;

/// Deterministic entropy injection for the oracle tests: the capture stubs
/// `crypto.getRandomValues`/`randomBytes` with a fixed byte stream, and the
/// injected draws here replay the same bytes in the same order. Only live
/// under `cfg(test)`; every consumer falls back to OS entropy when the queue
/// is empty.
#[cfg(test)]
pub(crate) mod test_entropy {
    use std::sync::Mutex;

    fn queue() -> &'static Mutex<Vec<Vec<u8>>> {
        static QUEUE: std::sync::OnceLock<Mutex<Vec<Vec<u8>>>> = std::sync::OnceLock::new();
        QUEUE.get_or_init(|| Mutex::new(Vec::new()))
    }

    /// Queue one deterministic draw (FIFO, consumed by the next 32-byte
    /// request).
    pub(crate) fn push(bytes: Vec<u8>) {
        queue().lock().unwrap().push(bytes);
    }

    /// Take the next draw, truncated/zero-padded to `len` bytes. `None`
    /// when the queue is empty (the caller falls back to OS entropy).
    pub(crate) fn take(len: usize) -> Option<Vec<u8>> {
        let mut guard = queue().lock().unwrap();
        if guard.is_empty() {
            return None;
        }
        let mut bytes = guard.remove(0);
        bytes.resize(len, 0);
        Some(bytes)
    }

    /// Drop any queued draws (oracle tests run serially and clear between
    /// scenarios).
    #[allow(dead_code)]
    pub(crate) fn clear() {
        queue().lock().unwrap().clear();
    }
}

use std::sync::Arc;

use tokio::io::AsyncReadExt;
#[cfg(test)]
use tokio::io::AsyncWriteExt;
use tokio::sync::watch;

/// Response body content type of the OAuth landing pages (upstream
/// `res.setHeader("Content-Type", "text/html; charset=utf-8")`).
pub(crate) const HTML_CONTENT_TYPE: &str = "text/html; charset=utf-8";

/// Upstream `parseAuthorizationInput` (anthropic.ts:52-80, openai-codex.ts:
/// 73-101; byte-identical): a pasted redirect URL, a `code#state` pair, a
/// bare query string, or a bare code.
pub(crate) fn parse_authorization_input(input: &str) -> ParsedAuthorizationInput {
    let value = input.trim();
    if value.is_empty() {
        return ParsedAuthorizationInput {
            code: None,
            state: None,
        };
    }

    // WHATWG `new URL(value)` — absolute URLs only; anything else falls
    // through like the upstream catch. A parsed URL returns immediately, even
    // when it carries no code/state at all.
    if let Ok(url) = url::Url::parse(value) {
        let pairs: Vec<(String, String)> = url
            .query_pairs()
            .map(|(name, value)| (name.into_owned(), value.into_owned()))
            .collect();
        return ParsedAuthorizationInput {
            code: first_pair(&pairs, "code"),
            state: first_pair(&pairs, "state"),
        };
    }

    if value.contains('#') {
        // JS `value.split("#", 2)`: at most two elements, the rest dropped.
        let mut parts = value.split('#');
        let code = parts.next().map(str::to_string);
        let state = parts.next().map(str::to_string);
        return ParsedAuthorizationInput { code, state };
    }

    if value.contains("code=") {
        // `new URLSearchParams` strips a single leading `?`, so a pasted
        // `?code=…&state=…` (browser URL bar) parses like upstream. Only this
        // branch: the callback routers split the target off the request line
        // first and never see a leading `?` on their query.
        let query = value.strip_prefix('?').unwrap_or(value);
        let pairs = parse_urlencoded_pairs(query);
        return ParsedAuthorizationInput {
            code: first_pair(&pairs, "code"),
            state: first_pair(&pairs, "state"),
        };
    }

    ParsedAuthorizationInput {
        code: Some(value.to_string()),
        state: None,
    }
}

pub(crate) struct ParsedAuthorizationInput {
    pub code: Option<String>,
    pub state: Option<String>,
}

/// First `URLSearchParams.get` match for a name.
pub(crate) fn first_pair(pairs: &[(String, String)], name: &str) -> Option<String> {
    pairs
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.clone())
}

/// `application/x-www-form-urlencoded` pair parsing (`new URLSearchParams`):
/// split on `&`, name/value split at the first `=`, `+` reads as space and
/// `%XX` sequences decode.
pub(crate) fn parse_urlencoded_pairs(input: &str) -> Vec<(String, String)> {
    input
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((name, value)) => (decode_urlencoded(name), decode_urlencoded(value)),
            None => (decode_urlencoded(pair), String::new()),
        })
        .collect()
}

fn decode_urlencoded(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let high = (bytes[index + 1] as char).to_digit(16);
                let low = (bytes[index + 2] as char).to_digit(16);
                match (high, low) {
                    (Some(high), Some(low)) => {
                        decoded.push((high * 16 + low) as u8);
                        index += 3;
                    }
                    _ => {
                        decoded.push(b'%');
                        index += 1;
                    }
                }
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// The raw request target (request line's second token), like
/// `new URL(req.url, "http://localhost")` input. `None` = malformed line.
pub(crate) fn request_target(request_line: &str) -> Option<&str> {
    request_line.split_whitespace().nth(1)
}

/// Reads the request head and returns the request line. `None` when the peer
/// never completes a head (drop the connection, like an abandoned browser
/// request; the cap and timeout keep stuck sockets from leaking).
pub(crate) async fn read_request_head(stream: &mut tokio::net::TcpStream) -> Option<String> {
    let mut buffer = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        let read =
            tokio::time::timeout(std::time::Duration::from_secs(30), stream.read(&mut chunk))
                .await
                .ok()?
                .ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if buffer.len() > 16 * 1024 {
            return None;
        }
        if let Some(head_end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            return Some(String::from_utf8_lossy(&buffer[..head_end]).into_owned());
        }
    }
}

/// Writes one HTTP/1.1 response and closes the connection (the callback
/// servers never keep-alive). The auth-delta refactor moved the anthropic/
/// openai-codex/openrouter/radius flows onto the shared callback server's
/// own writer (which adds `Cache-Control: no-store`); only test doubles
/// still use this minimal form.
#[cfg(test)]
pub(crate) async fn write_response(
    stream: &mut tokio::net::TcpStream,
    status: u16,
    reason: &str,
    content_type: &str,
    body: &str,
) {
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.write_all(body.as_bytes()).await;
    let _ = stream.flush().await;
    let _ = stream.shutdown().await;
}

/// The upstream callback servers' once-only settle flag (anthropic.ts:103-111,
/// openai-codex.ts:326-333) as a first-settle-wins slot: a delivery wins over
/// a cancel and neither can be overwritten. Three states, because a cancel is
/// observable: unset, then either [`Settle::Cancelled`] or
/// [`Settle::Delivered`].
#[derive(Clone)]
pub(crate) struct Waiter<T> {
    settled: Arc<watch::Sender<Option<Settle<T>>>>,
}

pub(crate) enum Settle<T> {
    Cancelled,
    Delivered(T),
}

impl<T: Clone + Send> Waiter<T> {
    fn new() -> Self {
        let (settled, _) = watch::channel(None);
        Waiter {
            settled: Arc::new(settled),
        }
    }

    fn settle(&self, value: Option<T>) {
        let value = value.map(Settle::Delivered).unwrap_or(Settle::Cancelled);
        self.settled.send_if_modified(|slot| {
            if slot.is_none() {
                *slot = Some(value);
                true
            } else {
                false
            }
        });
    }

    /// Resolves with the first settle: `None` once cancelled, the value once
    /// delivered, and immediately when the settle already happened.
    async fn wait(&self) -> Option<T> {
        let mut receiver = self.settled.subscribe();
        loop {
            // The borrow is confined to the block so no watch ref is alive
            // across the await (they are not Send).
            let settled = match &*receiver.borrow_and_update() {
                Some(Settle::Delivered(value)) => Some(Some(value.clone())),
                Some(Settle::Cancelled) => Some(None),
                None => None,
            };
            if let Some(delivered) = settled {
                return delivered;
            }
            if receiver.changed().await.is_err() {
                return None;
            }
        }
    }
}

/// `crypto.randomUUID()`: a random RFC 4122 version-4 UUID from 16 `rand`
/// bytes (version and variant bits set by hand). Upstream duplicates this
/// per flow (`radius.ts`, `openrouter.ts`); the port hoists it here once.
pub(crate) fn uuid_v4() -> String {
    let mut bytes = [0u8; 16];
    #[cfg(test)]
    if let Some(injected) = test_entropy::take(16) {
        bytes.copy_from_slice(&injected);
    } else {
        rand::fill(&mut bytes);
    }
    #[cfg(not(test))]
    rand::fill(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- parse_authorization_input (upstream lines 52-80 / 73-101) ----

    #[test]
    fn parse_authorization_input_follows_the_upstream_branches() {
        let parsed = |input: &str| {
            let parsed = parse_authorization_input(input);
            (parsed.code, parsed.state)
        };

        // Absolute redirect URL: query params win, early return.
        assert_eq!(
            parsed("http://localhost:53692/callback?code=abc&state=xyz"),
            (Some("abc".to_string()), Some("xyz".to_string()))
        );
        // Query values percent-decode (and `+` reads as space).
        assert_eq!(
            parsed("https://claude.ai/test?code=a%20b&state=c+d"),
            (Some("a b".to_string()), Some("c d".to_string()))
        );
        // Fragments do not leak into the query.
        assert_eq!(
            parsed("http://localhost:53692/callback?code=abc#frag"),
            (Some("abc".to_string()), None)
        );
        // A parseable URL without code/state returns empty immediately.
        assert_eq!(parsed("http://localhost:53692/callback"), (None, None));
        // `localhost:53692` parses as scheme + path, like WHATWG `new URL`.
        assert_eq!(parsed("localhost:53692"), (None, None));

        // `code#state` pairs.
        assert_eq!(
            parsed("the-code#the-state"),
            (Some("the-code".to_string()), Some("the-state".to_string()))
        );
        // JS `split("#", 2)` drops everything after the second element.
        assert_eq!(
            parsed("a#b#c"),
            (Some("a".to_string()), Some("b".to_string()))
        );
        assert_eq!(
            parsed("code#"),
            (Some("code".to_string()), Some(String::new()))
        );

        // Bare query strings.
        assert_eq!(
            parsed("code=a&state=b"),
            (Some("a".to_string()), Some("b".to_string()))
        );
        // A URL-bar paste keeps its leading `?`: `new URLSearchParams` strips
        // exactly one, so the input still parses (post-review fix).
        assert_eq!(
            parsed("?code=x&state=y"),
            (Some("x".to_string()), Some("y".to_string()))
        );
        // Only one `?` is stripped: the remainder names a `?code` pair, so
        // `code` is absent (same as upstream).
        assert_eq!(parsed("??code=x"), (None, None));
        // First occurrence wins (`URLSearchParams.get`).
        assert_eq!(parsed("code=a&code=b"), (Some("a".to_string()), None));
        // A name merely containing "code=" matches nothing.
        assert_eq!(parsed("xcode=y"), (None, None));

        // Bare codes, trimmed input, empty input.
        assert_eq!(parsed("the-code"), (Some("the-code".to_string()), None));
        assert_eq!(parsed("  the-code  "), (Some("the-code".to_string()), None));
        assert_eq!(parsed(""), (None, None));
        assert_eq!(parsed("   "), (None, None));
    }

    #[test]
    fn uuid_v4_matches_the_rfc_4122_shape() {
        let uuid = uuid_v4();
        assert_eq!(uuid.len(), 36);
        let parts: Vec<&str> = uuid.split('-').collect();
        assert_eq!(
            parts.iter().map(|part| part.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12]
        );
        assert!(uuid
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-'));
        // Version 4 nibble and RFC 4122 variant bits.
        assert!(uuid.as_bytes()[14] == b'4');
        assert!(matches!(uuid.as_bytes()[19], b'8' | b'9' | b'a' | b'b'));
        assert_ne!(uuid, uuid_v4());
    }

    #[tokio::test]
    async fn waiter_settles_once_and_a_code_wins_over_a_cancel() {
        let waiter: Waiter<String> = Waiter::new();
        waiter.settle(None);
        assert_eq!(waiter.wait().await, None);
        // A late delivery cannot overwrite the cancel (upstream `settled`).
        waiter.settle(Some("late".to_string()));
        assert_eq!(waiter.wait().await, None);

        // A delivery wins over a later cancel.
        let waiter: Waiter<String> = Waiter::new();
        waiter.settle(Some("code".to_string()));
        waiter.settle(None);
        assert_eq!(waiter.wait().await, Some("code".to_string()));
    }
}
