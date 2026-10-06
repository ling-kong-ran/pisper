//! Byte-oracle tests for the OAuth/auth-delta slice: every expectation here
//! was captured by EXECUTING the actual upstream TypeScript (2bbfcca43) under
//! Node `--experimental-strip-types` — see
//! `tests/fixtures/ai_oauth_oracle/capture_oauth_oracle.mjs` (the generator,
//! kept with the fixture) and `manifest.json` (the SHA-256 provenance of
//! every captured source, plus the deterministic RNG/substitution
//! declaration).
//!
//! Determinism contract shared with the capture:
//! - `Date.now` is stubbed to 1758240000000; the Rust side cannot freeze the
//!   clock, so a now-derived `expires` is compared by DELTA against the
//!   fixture (`fixture.expires - FIXED_NOW == actual.expires - now_ms()`).
//! - The RNG stream is draw k = bytes (k*32 + i) & 0xFF, consumed in order
//!   (PKCE verifier, then state, then nonce per login); the Rust side
//!   replays it through [`test_entropy`].
//! - `crypto.randomUUID()` is the fixed UUID
//!   `11223344-5566-4888-99aa-bbccddeeff00`, which survives the port's
//!   version/variant bit overwrite (raw bytes pushed through the same
//!   queue).
//! - `globalThis.fetch` was a recorder; the Rust flows point at wiremock
//!   instead, so request URLs compare by PATH and the request bodies
//!   byte-for-byte (JSON bodies canonically, per the ai-delta precedent for
//!   serde map ordering).
//! - Loopback page bytes compare in full (status, content type, cache
//!   control, exact HTML body); the raw socket framing (Node's `Date`
//!   header and chunked encoding) has no Rust counterpart and is out of
//!   scope by construction.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::BoxFuture;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::ai::auth::oauth::test_entropy;
use crate::ai::auth::types::{
    AuthError, AuthEvent, AuthInteraction, AuthPrompt, AuthPromptKind, OAuthAuth, OAuthCredential,
    ProviderAuthInteraction,
};

/// The capture's frozen clock (manifest.json `fixedNow`).
const FIXED_NOW: i64 = 1_758_240_000_000;

/// The capture's fixed UUID (manifest.json `fixedUuid`): the raw bytes whose
/// version/variant bits produce exactly this string through the port's
/// `uuid_v4` overwrite.
const FIXED_UUID_BYTES: [u8; 16] = [
    0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x88, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x00,
];

/// Draw k of the documented RNG stream.
fn draw(k: usize) -> Vec<u8> {
    (0..32).map(|i| ((k * 32 + i) & 0xFF) as u8).collect()
}

/// Seeds the entropy queue: the PKCE verifier (draw 0) plus any further
/// draws a flow consumes.
fn seed_entropy(extra_draws: &[&[u8]]) {
    test_entropy::clear();
    test_entropy::push(draw(0));
    for extra in extra_draws {
        test_entropy::push(extra.to_vec());
    }
}

/// The first 16 bytes of a draw (upstream `randomBytes(16)`).
fn draw_prefix(k: usize, len: usize) -> Vec<u8> {
    draw(k).into_iter().take(len).collect()
}

fn oracle(name: &str) -> Value {
    serde_json::from_str(match name {
        "callback_server" => {
            include_str!("../../../../tests/fixtures/ai_oauth_oracle/callback_server_oracle.json")
        }
        "openai_chatgpt" => {
            include_str!("../../../../tests/fixtures/ai_oauth_oracle/openai_chatgpt_oracle.json")
        }
        "meta" => include_str!("../../../../tests/fixtures/ai_oauth_oracle/meta_oracle.json"),
        "anthropic" => {
            include_str!("../../../../tests/fixtures/ai_oauth_oracle/anthropic_oracle.json")
        }
        "openai_codex" => {
            include_str!("../../../../tests/fixtures/ai_oauth_oracle/openai_codex_oracle.json")
        }
        "openrouter" => {
            include_str!("../../../../tests/fixtures/ai_oauth_oracle/openrouter_oracle.json")
        }
        "radius" => include_str!("../../../../tests/fixtures/ai_oauth_oracle/radius_oracle.json"),
        other => panic!("unknown oracle fixture {other}"),
    })
    .expect("oracle fixture must parse")
}

/// The full loopback response (status + pinned headers + exact body).
async fn http_get(port: u16, target: &str) -> (u16, String, Option<String>, String) {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    stream
        .write_all(
            format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut response))
        .await
        .unwrap()
        .unwrap();
    let raw = String::from_utf8_lossy(&response).into_owned();
    let (head, body) = raw.split_once("\r\n\r\n").expect("response head");
    let status: u16 = head
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .expect("status line");
    let header = |name: &str| {
        head.to_ascii_lowercase()
            .split("\r\n")
            .find_map(|line| line.strip_prefix(&format!("{name}:")))
            .map(str::trim)
            .map(str::to_string)
    };
    (
        status,
        header("content-type").expect("content-type"),
        header("cache-control"),
        body.to_string(),
    )
}

/// Checks one captured page (`{status, contentType, cacheControl, body}`)
/// against a live loopback GET.
async fn expect_page(port: u16, target: &str, expected: &Value, label: &str) {
    let (status, content_type, cache_control, body) = http_get(port, target).await;
    assert_eq!(
        status,
        expected["status"].as_u64().unwrap() as u16,
        "{label} status"
    );
    assert_eq!(
        content_type.to_ascii_lowercase(),
        expected["contentType"]
            .as_str()
            .unwrap()
            .to_ascii_lowercase(),
        "{label} content type"
    );
    if let Some(expected_cache) = expected["cacheControl"].as_str() {
        assert_eq!(
            cache_control.as_deref().map(str::to_ascii_lowercase),
            Some(expected_cache.to_ascii_lowercase()),
            "{label} cache control"
        );
    }
    assert_eq!(body, expected["body"].as_str().unwrap(), "{label} body");
}

/// The request-URL path of a captured request record (the Rust flows run
/// against injected wiremock URLs; the path pins the endpoint).
fn url_path(url: &str) -> String {
    Url::parse(url)
        .map(|url| url.path().to_string())
        .unwrap_or_else(|_| url.to_string())
}

/// Compares a recorded request against the fixture record: same path, same
/// method, same header values (names lowercased), and a byte-exact body.
fn expect_request(actual: &wiremock::Request, expected: &Value, label: &str) {
    assert_eq!(
        actual.url.path(),
        url_path(expected["url"].as_str().unwrap()),
        "{label} url"
    );
    assert_eq!(
        actual.method.as_str().to_ascii_lowercase(),
        expected["method"].as_str().unwrap().to_ascii_lowercase(),
        "{label} method"
    );
    // The `host` header carries the per-run loopback port (the capture's
    // recorder and the port's wiremock bind differently); every other
    // pinned header is compared by value.
    let expected_headers: Vec<(String, String)> = expected["headers"]
        .as_object()
        .unwrap()
        .iter()
        // Transport-managed headers (the capture's Node fetch emits
        // host/connection/accept-language/sec-fetch-mode/user-agent/
        // accept-encoding/content-length; the port's reqwest sets its own
        // equivalents) are skipped — the flows' own headers compare by value.
        .filter(|(name, _)| {
            let name = name.to_ascii_lowercase();
            !matches!(
                name.as_str(),
                "host"
                    | "connection"
                    | "accept-language"
                    | "sec-fetch-mode"
                    | "user-agent"
                    | "accept-encoding"
                    | "content-length"
            )
        })
        .map(|(name, value)| {
            (
                name.to_ascii_lowercase(),
                value.as_str().unwrap().to_string(),
            )
        })
        .collect();
    for (name, value) in expected_headers {
        let actual_value = actual
            .headers
            .get(&name)
            .map(|value| value.to_str().unwrap().to_string());
        assert_eq!(actual_value, Some(value), "{label} header {name}");
    }
    let expected_body = expected["body"].as_str().unwrap_or("");
    assert_eq!(
        String::from_utf8_lossy(&actual.body),
        expected_body,
        "{label} body"
    );
}

type Respond =
    Box<dyn Fn(AuthPrompt) -> BoxFuture<'static, Result<String, AuthError>> + Send + Sync>;

/// Minimal interaction: records events and prompts, answers prompts through
/// the injected responder, and mirrors `auth_url` events into a shared slot
/// the driver tasks read while login is in flight.
struct OracleInteraction {
    auth_url: Arc<Mutex<Option<String>>>,
    events: Mutex<Vec<AuthEvent>>,
    prompts: Mutex<Vec<AuthPrompt>>,
    respond: Respond,
}

impl AuthInteraction for OracleInteraction {
    fn signal(&self) -> Option<CancellationToken> {
        None
    }

    fn prompt(&self, prompt: AuthPrompt) -> BoxFuture<'_, Result<String, AuthError>> {
        self.prompts.lock().unwrap().push(prompt.clone());
        (self.respond)(prompt)
    }

    fn notify(&self, event: AuthEvent) {
        if let AuthEvent::AuthUrl { url, .. } = &event {
            *self.auth_url.lock().unwrap() = Some(url.clone());
        }
        self.events.lock().unwrap().push(event);
    }
}

fn oracle_interaction(respond: Respond) -> (Arc<OracleInteraction>, ProviderAuthInteraction) {
    // The v1.0.0 Anthropic login-method select is answered with the browser
    // flow at the fake level (recorded), so per-scenario responders only see
    // the later prompts. Gated on the copy_code option, unique to the
    // Anthropic select: the Codex login has its own select the scenario
    // responders answer themselves.
    let respond: Respond = Box::new(move |prompt| {
        if let crate::ai::auth::types::AuthPromptKind::Select { options, .. } = &prompt.kind {
            let is_anthropic = options.iter().any(|option| option.id == "copy_code");
            if let Some(browser) = options
                .iter()
                .find(|option| option.id == "browser")
                .filter(|_| is_anthropic)
            {
                let id = browser.id.clone();
                return Box::pin(async move { Ok(id) })
                    as BoxFuture<'static, Result<String, AuthError>>;
            }
        }
        respond(prompt)
    });
    let fake = Arc::new(OracleInteraction {
        auth_url: Arc::new(Mutex::new(None)),
        events: Mutex::new(Vec::new()),
        prompts: Mutex::new(Vec::new()),
        respond,
    });
    let interaction = ProviderAuthInteraction::new(
        Arc::clone(&fake) as Arc<dyn AuthInteraction>,
        CancellationToken::new(),
    );
    (fake, interaction)
}

fn hanging_respond() -> Respond {
    Box::new(move |prompt| {
        // The v1.0.0 login-method select is answered with the browser flow;
        // later prompts hang like a real pending UI.
        if let crate::ai::auth::types::AuthPromptKind::Select { options, .. } = prompt.kind {
            if let Some(browser) = options.iter().find(|option| option.id == "browser") {
                let id = browser.id.clone();
                return Box::pin(async move { Ok(id) })
                    as BoxFuture<'static, Result<String, AuthError>>;
            }
        }
        Box::pin(async move {
            prompt.signal.unwrap_or_default().cancelled().await;
            Err(AuthError::Cancelled)
        })
    })
}

/// The pasted-query suffix builder shared by the paste responders.
type SuffixFn = Arc<dyn Fn(&str, &str) -> String + Send + Sync>;

/// A responder that pastes the final redirect URL built from the emitted
/// authorize URL, with the pasted query suffix. The v1.0.0 login-method
/// select is answered with the browser flow; later prompts paste.
fn pasting_respond(auth_url_slot: Arc<Mutex<Option<String>>>, suffix: SuffixFn) -> Respond {
    Box::new(move |prompt| {
        let slot = Arc::clone(&auth_url_slot);
        let suffix = Arc::clone(&suffix);
        Box::pin(async move {
            if let crate::ai::auth::types::AuthPromptKind::Select { options, .. } = prompt.kind {
                let browser = options
                    .iter()
                    .find(|option| option.id == "browser")
                    .expect("browser login option");
                return Ok(browser.id.clone());
            }
            let auth_url = loop {
                if let Some(auth_url) = slot.lock().unwrap().clone() {
                    break auth_url;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            };
            let url = Url::parse(&auth_url).unwrap();
            let pair = |name: &str| {
                url.query_pairs()
                    .find(|(key, _)| key == name)
                    .map(|(_, value)| value.into_owned())
                    .unwrap_or_else(|| panic!("missing auth URL parameter {name}"))
            };
            Ok(suffix(&pair("redirect_uri"), &pair("state")))
        })
    })
}

async fn wait_for_auth_url(slot: Arc<Mutex<Option<String>>>) -> String {
    loop {
        if let Some(auth_url) = slot.lock().unwrap().clone() {
            return auth_url;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

fn auth_url_param(auth_url: &str, name: &str) -> String {
    Url::parse(auth_url)
        .unwrap()
        .query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
        .unwrap_or_else(|| panic!("missing auth URL parameter {name}"))
}

fn free_port() -> u16 {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Compares a login error against the captured upstream message: the bare
/// upstream message is the fixture string, and the port's
/// [`AuthError::Operation`] carries it (Display adds the port-level
/// "auth operation failed: " prefix, which the fixtures do not include).
/// The captured "Login cancelled" surfaces as [`AuthError::Cancelled`].
/// The upstream message an [`AuthError`] carries: [`AuthError::Operation`]'s
/// own message (Display adds the port-level prefix the fixtures do not
/// include) and the literal "Login cancelled" for cancellation.
fn upstream_message(error: &AuthError) -> String {
    match error {
        AuthError::Operation(message) => message.clone(),
        AuthError::Cancelled => "Login cancelled".to_string(),
        other => other.to_string(),
    }
}

fn assert_upstream_error(error: &AuthError, expected: &str) {
    if expected == "Login cancelled" {
        assert_eq!(error, &AuthError::Cancelled, "expected: {expected}");
    } else {
        assert_eq!(
            error,
            &AuthError::Operation(expected.to_string()),
            "expected: {expected}"
        );
    }
}

/// Compares a now-derived `expires` against the capture by delta (the
/// capture froze `Date.now`; the port cannot freeze the clock).
fn assert_expires_delta(actual: i64, fixture_expires: i64, label: &str) {
    let expected_delta = fixture_expires - FIXED_NOW;
    let actual_delta = actual - crate::ai::now_ms();
    assert!(
        (actual_delta - expected_delta).abs() <= 2000,
        "{label}: expires delta {actual_delta} != {expected_delta}"
    );
}

// ---------------------------------------------------------------------------
// 1. Shared callback server (callback-server.ts)
// ---------------------------------------------------------------------------
mod callback_server {
    use super::*;

    /// Oracle: "ignores stray requests and resolves with the completed code"
    /// (route order, page bytes, wait result).
    #[tokio::test]
    async fn ignores_stray_requests_and_resolves_with_the_completed_code() {
        let fixture = &oracle("callback_server")["strayRequests"];
        let server = crate::ai::auth::oauth::callback_server::start_oauth_callback_server(
            crate::ai::auth::oauth::callback_server::CallbackServerOptions {
                provider_name: "Example".to_string(),
                host: "127.0.0.1".to_string(),
                port: 0,
                path: "/callback".to_string(),
                redirect_host: None,
                state: Some("expected-state".to_string()),
                complete: Arc::new(|code| Box::pin(async move { Ok(format!("completed:{code}")) })),
                signal: CancellationToken::new(),
                timeout_ms: None,
            },
        )
        .await
        .unwrap();
        // The masked redirect URI matches upstream's shape.
        let masked = regex_mask_port(server.redirect_uri());
        assert_eq!(masked, fixture["redirectUri"].as_str().unwrap());

        let port = Url::parse(server.redirect_uri()).unwrap().port().unwrap();
        expect_page(port, "/other", &fixture["wrongPath"], "wrong path").await;
        expect_page(
            port,
            "/callback?code=c&state=other",
            &fixture["wrongState"],
            "wrong state",
        )
        .await;
        // POST → 404 with the same page.
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        stream
            .write_all(
                b"POST /callback?code=c&state=expected-state HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
        let mut response = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut response))
            .await
            .unwrap()
            .unwrap();
        let raw = String::from_utf8_lossy(&response).into_owned();
        assert!(raw.starts_with("HTTP/1.1 404 Not Found\r\n"), "{raw}");
        assert_eq!(
            raw.contains("Callback route not found."),
            fixture["wrongMethod"]["body"]
                .as_str()
                .unwrap()
                .contains("Callback route not found.")
        );
        expect_page(
            port,
            "/callback?state=expected-state",
            &fixture["missingCode"],
            "missing code",
        )
        .await;
        expect_page(
            port,
            "/callback?code=the-code&state=expected-state",
            &fixture["success"],
            "success",
        )
        .await;
        let settled = server.wait().await.unwrap();
        assert_eq!(settled, Some(fixture["wait"].as_str().unwrap().to_string()));
        server.close().await;
    }

    fn regex_mask_port(uri: &str) -> String {
        // Replaces `host:PORT` with `host:<port>` for ephemeral ports.
        let bytes = uri.as_bytes();
        let mut out = String::new();
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] == b':'
                && uri[index..].len() > 1
                && uri[index + 1..].starts_with(|c: char| c.is_ascii_digit())
            {
                let start = index + 1;
                let mut end = start;
                while end < uri.len() && uri.as_bytes()[end].is_ascii_digit() {
                    end += 1;
                }
                let rest = &uri[end..];
                if rest.starts_with('/') || rest.is_empty() {
                    out.push_str(":<port>");
                    index = end;
                    continue;
                }
            }
            out.push(uri[index..].chars().next().unwrap());
            index += uri[index..].chars().next().unwrap().len_utf8();
        }
        out
    }

    /// Oracle: "uses the redirect host and skips the state check when none is
    /// expected".
    #[tokio::test]
    async fn uses_the_redirect_host_and_skips_the_state_check_when_none_is_expected() {
        let fixture = &oracle("callback_server")["redirectHostNoState"];
        let server = crate::ai::auth::oauth::callback_server::start_oauth_callback_server(
            crate::ai::auth::oauth::callback_server::CallbackServerOptions {
                provider_name: "Example".to_string(),
                host: "127.0.0.1".to_string(),
                port: 0,
                path: "/callback".to_string(),
                redirect_host: Some("localhost".to_string()),
                state: None,
                complete: Arc::new(|code| Box::pin(async move { Ok(format!("completed:{code}")) })),
                signal: CancellationToken::new(),
                timeout_ms: None,
            },
        )
        .await
        .unwrap();
        assert!(server.redirect_uri().starts_with("http://localhost:"));
        let port = Url::parse(server.redirect_uri()).unwrap().port().unwrap();
        expect_page(
            port,
            "/callback?code=no-state",
            &fixture["success"],
            "success",
        )
        .await;
        let settled = server.wait().await.unwrap();
        assert_eq!(settled, Some(fixture["wait"].as_str().unwrap().to_string()));
        server.close().await;
    }

    /// Oracle: "shows completion failures on the page and rejects the wait".
    #[tokio::test]
    async fn shows_completion_failures_on_the_page_and_rejects_the_wait() {
        let fixture = &oracle("callback_server")["completionFailure"];
        let server = crate::ai::auth::oauth::callback_server::start_oauth_callback_server(
            crate::ai::auth::oauth::callback_server::CallbackServerOptions {
                provider_name: "Example".to_string(),
                host: "127.0.0.1".to_string(),
                port: 0,
                path: "/callback".to_string(),
                redirect_host: None,
                state: Some("expected-state".to_string()),
                complete: Arc::new(|_| {
                    Box::pin(async move {
                        Err::<String, AuthError>(AuthError::Operation(
                            "token exchange failed".to_string(),
                        ))
                    })
                }),
                signal: CancellationToken::new(),
                timeout_ms: None,
            },
        )
        .await
        .unwrap();
        let port = Url::parse(server.redirect_uri()).unwrap().port().unwrap();
        expect_page(
            port,
            "/callback?code=c&state=expected-state",
            &fixture["failure"],
            "failure",
        )
        .await;
        let error = server.wait().await.unwrap_err();
        assert_upstream_error(&error, fixture["waitError"].as_str().unwrap());
        server.close().await;
    }

    /// Oracle: "rejects the wait when the provider redirects with an error".
    #[tokio::test]
    async fn rejects_the_wait_when_the_provider_redirects_with_an_error() {
        let fixture = &oracle("callback_server")["providerError"];
        let server = start_example_server().await;
        let port = Url::parse(server.redirect_uri()).unwrap().port().unwrap();
        expect_page(
            port,
            "/callback?error=access_denied&error_description=User%20denied%20access&state=expected-state",
            &fixture["failure"],
            "failure",
        )
        .await;
        let error = server.wait().await.unwrap_err();
        assert_upstream_error(&error, fixture["waitError"].as_str().unwrap());
        server.close().await;
    }

    async fn start_example_server(
    ) -> crate::ai::auth::oauth::callback_server::OAuthCallbackServer<String> {
        crate::ai::auth::oauth::callback_server::start_oauth_callback_server(
            crate::ai::auth::oauth::callback_server::CallbackServerOptions {
                provider_name: "Example".to_string(),
                host: "127.0.0.1".to_string(),
                port: 0,
                path: "/callback".to_string(),
                redirect_host: None,
                state: Some("expected-state".to_string()),
                complete: Arc::new(|code| Box::pin(async move { Ok(code) })),
                signal: CancellationToken::new(),
                timeout_ms: None,
            },
        )
        .await
        .unwrap()
    }

    /// Oracle: "completes only the first callback" — 409 for the second, the
    /// first keeps completing across a cancel.
    #[tokio::test]
    async fn completes_only_the_first_callback() {
        let fixture = &oracle("callback_server")["firstCallbackWins"];
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let release_rx = Arc::new(tokio::sync::Mutex::new(Some(release_rx)));
        let server = crate::ai::auth::oauth::callback_server::start_oauth_callback_server(
            crate::ai::auth::oauth::callback_server::CallbackServerOptions {
                provider_name: "Example".to_string(),
                host: "127.0.0.1".to_string(),
                port: 0,
                path: "/callback".to_string(),
                redirect_host: None,
                state: Some("expected-state".to_string()),
                complete: Arc::new(move |code| {
                    let release_rx = Arc::clone(&release_rx);
                    Box::pin(async move {
                        let mut guard = release_rx.lock().await;
                        let release_rx = guard.take().expect("complete runs once");
                        drop(guard);
                        let _ = release_rx.await;
                        let _ = code;
                        Ok("done".to_string())
                    })
                }),
                signal: CancellationToken::new(),
                timeout_ms: None,
            },
        )
        .await
        .unwrap();
        let port = Url::parse(server.redirect_uri()).unwrap().port().unwrap();

        let first = tokio::spawn(async move {
            let (status, _, _, body) =
                http_get(port, "/callback?code=c&state=expected-state").await;
            (status, body)
        });
        tokio::time::sleep(Duration::from_millis(150)).await;
        let (second_status, _, _, second_body) =
            http_get(port, "/callback?code=c&state=expected-state").await;
        assert_eq!(
            second_status,
            fixture["second"]["status"].as_u64().unwrap() as u16
        );
        assert_eq!(second_body, fixture["second"]["body"].as_str().unwrap());

        server.cancel();
        let _ = release_tx.send(());
        let (first_status, first_body) = first.await.unwrap();
        assert_eq!(
            first_status,
            fixture["first"]["status"].as_u64().unwrap() as u16
        );
        assert_eq!(first_body, fixture["first"]["body"].as_str().unwrap());
        let settled = server.wait().await.unwrap();
        assert_eq!(settled, Some(fixture["wait"].as_str().unwrap().to_string()));
        server.close().await;
    }

    /// Oracle: "resolves with undefined after cancel" — and a late browser
    /// request gets the 409 page.
    #[tokio::test]
    async fn resolves_with_none_after_cancel() {
        let fixture = &oracle("callback_server")["cancelResolvesUndefined"];
        let server = start_example_server().await;
        server.cancel();
        let settled = server.wait().await.unwrap();
        assert!(settled.is_none());
        let port = Url::parse(server.redirect_uri()).unwrap().port().unwrap();
        expect_page(
            port,
            "/callback?code=c&state=expected-state",
            &fixture["late"],
            "late",
        )
        .await;
        server.close().await;
    }

    /// Oracle: "rejects the wait on abort and on timeout" + the entry abort.
    #[tokio::test]
    async fn rejects_the_wait_on_abort_and_on_timeout() {
        let fixture = &oracle("callback_server")["abortAndTimeout"];
        let signal = CancellationToken::new();
        let aborted = crate::ai::auth::oauth::callback_server::start_oauth_callback_server(
            crate::ai::auth::oauth::callback_server::CallbackServerOptions {
                provider_name: "Example".to_string(),
                host: "127.0.0.1".to_string(),
                port: 0,
                path: "/callback".to_string(),
                redirect_host: None,
                state: Some("expected-state".to_string()),
                complete: Arc::new(|code| Box::pin(async move { Ok(code) })),
                signal: signal.clone(),
                timeout_ms: None,
            },
        )
        .await
        .unwrap();
        signal.cancel();
        let error = aborted.wait().await.unwrap_err();
        assert_upstream_error(&error, fixture["abortError"].as_str().unwrap());
        aborted.close().await;

        let timed_out = crate::ai::auth::oauth::callback_server::start_oauth_callback_server(
            crate::ai::auth::oauth::callback_server::CallbackServerOptions {
                provider_name: "Example".to_string(),
                host: "127.0.0.1".to_string(),
                port: 0,
                path: "/callback".to_string(),
                redirect_host: None,
                state: Some("expected-state".to_string()),
                complete: Arc::new(|code| Box::pin(async move { Ok(code) })),
                signal: CancellationToken::new(),
                timeout_ms: Some(10),
            },
        )
        .await
        .unwrap();
        let error = timed_out.wait().await.unwrap_err();
        assert_upstream_error(&error, fixture["timeoutError"].as_str().unwrap());
        timed_out.close().await;

        let already = CancellationToken::new();
        already.cancel();
        let error = crate::ai::auth::oauth::callback_server::start_oauth_callback_server::<String>(
            crate::ai::auth::oauth::callback_server::CallbackServerOptions {
                provider_name: "Example".to_string(),
                host: "127.0.0.1".to_string(),
                port: 0,
                path: "/callback".to_string(),
                redirect_host: None,
                state: Some("expected-state".to_string()),
                complete: Arc::new(|code| Box::pin(async move { Ok(code) })),
                signal: already,
                timeout_ms: None,
            },
        )
        .await
        .unwrap_err();
        assert_upstream_error(&error, fixture["entryError"].as_str().unwrap());
    }

    /// Oracle waitForCallbackOrManualInput: "returns the browser callback and
    /// aborts the manual prompt".
    #[tokio::test]
    async fn returns_the_browser_callback_and_aborts_the_manual_prompt() {
        let fixture = &oracle("callback_server")["manualPromptAbortedByCallback"];
        use crate::ai::auth::oauth::callback_server as cs;
        let server: cs::OAuthCallbackServer<String> =
            cs::start_oauth_callback_server(cs::CallbackServerOptions {
                provider_name: "Example".to_string(),
                host: "127.0.0.1".to_string(),
                port: 0,
                path: "/callback".to_string(),
                redirect_host: None,
                state: None,
                complete: Arc::new(|code| Box::pin(async move { Ok(code) })),
                signal: CancellationToken::new(),
                timeout_ms: None,
            })
            .await
            .unwrap();
        let auth_url_slot = Arc::new(Mutex::new(None));
        let manual_signal: Arc<Mutex<Option<CancellationToken>>> = Arc::new(Mutex::new(None));
        let signal_slot = Arc::clone(&manual_signal);
        let fake = Arc::new(OracleInteraction {
            auth_url: auth_url_slot,
            events: Mutex::new(Vec::new()),
            prompts: Mutex::new(Vec::new()),
            respond: Box::new(move |prompt| {
                let signal_slot = Arc::clone(&signal_slot);
                Box::pin(async move {
                    *signal_slot.lock().unwrap() = prompt.signal.clone();
                    prompt.signal.unwrap_or_default().cancelled().await;
                    Err(AuthError::Cancelled)
                })
            }),
        });
        let interaction = ProviderAuthInteraction::new(
            Arc::clone(&fake) as Arc<dyn AuthInteraction>,
            CancellationToken::new(),
        );
        let redirect = server.redirect_uri().to_string();
        let port = Url::parse(&redirect).unwrap().port().unwrap();
        let handle = server.clone();
        let result =
            cs::wait_for_callback_or_manual_input(&interaction, Some(&handle), "paste", &redirect);
        tokio::pin!(result);
        let driver = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let _ = http_get(port, "/callback?code=from-browser").await;
        });
        let outcome = tokio::time::timeout(Duration::from_secs(5), result)
            .await
            .unwrap()
            .unwrap();
        driver.await.unwrap();
        // Upstream resolves `{ type: "callback", value }` and aborts the prompt.
        let value = match &outcome {
            crate::ai::auth::oauth::callback_server::CallbackOrManual::Callback(value) => {
                value.clone()
            }
            other => panic!("expected the callback path, got {other:?}"),
        };
        assert_eq!(value, fixture["result"]["value"].as_str().unwrap());
        assert!(manual_signal
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .is_cancelled());
        server.close().await;
    }

    /// Oracle: "returns pasted input and stops waiting for the browser".
    #[tokio::test]
    async fn returns_pasted_input_and_stops_waiting_for_the_browser() {
        let fixture = &oracle("callback_server")["manualWins"];
        use crate::ai::auth::oauth::callback_server as cs;
        let server: cs::OAuthCallbackServer<String> =
            cs::start_oauth_callback_server(cs::CallbackServerOptions {
                provider_name: "Example".to_string(),
                host: "127.0.0.1".to_string(),
                port: 0,
                path: "/callback".to_string(),
                redirect_host: None,
                state: None,
                complete: Arc::new(|code| Box::pin(async move { Ok(code) })),
                signal: CancellationToken::new(),
                timeout_ms: None,
            })
            .await
            .unwrap();
        let redirect = server.redirect_uri().to_string();
        let (_fake, interaction) = oracle_interaction(Box::new(|_prompt| {
            Box::pin(async move { Ok("pasted".to_string()) })
        }));
        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            cs::wait_for_callback_or_manual_input(&interaction, Some(&server), "paste", &redirect),
        )
        .await
        .unwrap()
        .unwrap();
        match outcome {
            crate::ai::auth::oauth::callback_server::CallbackOrManual::Manual(input) => {
                assert_eq!(input, fixture["result"]["input"].as_str().unwrap());
            }
            other => panic!("expected the manual path, got {other:?}"),
        }
        server.close().await;
    }

    /// Oracle: "uses only the manual prompt without a callback server".
    #[tokio::test]
    async fn uses_only_the_manual_prompt_without_a_callback_server() {
        let fixture = &oracle("callback_server")["manualOnlyWithoutServer"];
        use crate::ai::auth::oauth::callback_server as cs;
        let (fake, interaction) = oracle_interaction(Box::new(|_prompt| {
            Box::pin(async move { Ok("pasted".to_string()) })
        }));
        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            cs::wait_for_callback_or_manual_input::<String>(
                &interaction,
                None,
                "paste",
                "http://localhost/callback",
            ),
        )
        .await
        .unwrap()
        .unwrap();
        match outcome {
            crate::ai::auth::oauth::callback_server::CallbackOrManual::Manual(input) => {
                assert_eq!(input, fixture["result"]["input"].as_str().unwrap());
            }
            other => panic!("expected the manual path, got {other:?}"),
        }
        let _ = fake;
    }

    /// Oracle: "propagates manual prompt failures".
    #[tokio::test]
    async fn propagates_manual_prompt_failures() {
        let fixture = &oracle("callback_server")["promptFailurePropagates"];
        use crate::ai::auth::oauth::callback_server as cs;
        let server: cs::OAuthCallbackServer<String> =
            cs::start_oauth_callback_server(cs::CallbackServerOptions {
                provider_name: "Example".to_string(),
                host: "127.0.0.1".to_string(),
                port: 0,
                path: "/callback".to_string(),
                redirect_host: None,
                state: None,
                complete: Arc::new(|code| Box::pin(async move { Ok(code) })),
                signal: CancellationToken::new(),
                timeout_ms: None,
            })
            .await
            .unwrap();
        let redirect = server.redirect_uri().to_string();
        let (_fake, interaction) = oracle_interaction(Box::new(|_prompt| {
            Box::pin(async move { Err(AuthError::Operation("prompt cancelled".to_string())) })
        }));
        let error = tokio::time::timeout(
            Duration::from_secs(5),
            cs::wait_for_callback_or_manual_input(&interaction, Some(&server), "paste", &redirect),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert_upstream_error(&error, fixture["failure"].as_str().unwrap());
        server.close().await;
    }

    // The capture's "manual error wins over a delivered callback" scenario
    // is a genuine microtask race in both implementations (the prompt
    // rejection and the settled wait resolve in either order); it is not
    // pinned as a byte-oracle. The deterministic ordering cases above cover
    // the shared decision table.
}

// ---------------------------------------------------------------------------
// 2. openai-chatgpt (NEW flow)
// ---------------------------------------------------------------------------
mod openai_chatgpt {
    use super::*;

    const DEVICE_ID: &str = "e61bbe28-07ef-466d-8e5d-a344f94ab305";

    fn flow_with(
        token_server: &MockServer,
    ) -> crate::ai::auth::oauth::openai_chatgpt::OpenAIChatGptOAuth {
        flow_with_callback_port(token_server, 1455)
    }

    /// The pinned redirect URI always says `127.0.0.1:1455` (the capture's
    /// bytes live in the flow's constant redirect URI); only the test-only
    /// listener moves, so the browser-driving tests do not fight a foreign
    /// process holding 1455.
    fn flow_with_callback_port(
        token_server: &MockServer,
        callback_port: u16,
    ) -> crate::ai::auth::oauth::openai_chatgpt::OpenAIChatGptOAuth {
        crate::ai::auth::oauth::openai_chatgpt::OpenAIChatGptOAuth::with_endpoints(
            format!("{}/api/accounts/oauth/token", token_server.uri()),
            "127.0.0.1".to_string(),
            callback_port,
        )
    }

    async fn mount_token(server: &MockServer, body: String, status: u16) {
        Mock::given(method("POST"))
            .and(path("/api/accounts/oauth/token"))
            .respond_with(ResponseTemplate::new(status).set_body_raw(body, "application/json"))
            .mount(server)
            .await;
    }

    fn token_body() -> String {
        serde_json::json!({
            "access_token": "access-token",
            "refresh_token": "refresh-token",
            "expires_in": 3600,
            "id_token": "id-token",
            "scope": "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct",
        })
        .to_string()
    }

    /// Interaction with the device-id LoginOptions (upstream passes
    /// `{ getDeviceId }` as the login options).
    fn oracle_interaction_with_device_id(
        respond: Respond,
    ) -> (Arc<OracleInteraction>, ProviderAuthInteraction) {
        let (fake, interaction) = oracle_interaction(respond);
        let interaction =
            interaction.with_login_options(Some(Arc::new(crate::ai::auth::types::LoginOptions {
                get_device_id: Some(Box::new(|| DEVICE_ID.to_string())),
            })));
        (fake, interaction)
    }

    /// The authorize URL is fully deterministic under the injected entropy:
    /// byte-equality with the capture.
    #[tokio::test]
    async fn authorize_url_and_callback_login_match_the_capture_byte_for_byte() {
        let fixture = &oracle("openai_chatgpt")["callbackLogin"];
        // Draws: PKCE verifier (0), state (1), nonce (2).
        test_entropy::clear();
        test_entropy::push(draw(0));
        test_entropy::push(draw(1));
        test_entropy::push(draw(2));

        let server = MockServer::start().await;
        mount_token(&server, token_body(), 200).await;
        let callback_port = free_port();
        let oauth = flow_with_callback_port(&server, callback_port);
        let (fake, interaction) = oracle_interaction_with_device_id(hanging_respond());

        let login = tokio::spawn(async move { oauth.login(interaction).await });
        let auth_url = wait_for_auth_url(Arc::clone(&fake.auth_url)).await;
        // The emitted authorize URL is byte-identical (the capture stores the
        // parsed parameter map; rebuild and compare every value in order).
        let expected_url =
            crate::ai::auth::oauth::openai_chatgpt::authorize_url_for_test(&fixture["authorize"]);
        assert_eq!(auth_url, expected_url);

        // The browser redirect carries the captured state.
        let state = auth_url_param(&auth_url, "state");
        let callback_target =
            format!("/auth/callback?code=authorization-code&state={state}&client_id=oaiapp_issued");
        expect_page(
            callback_port,
            &callback_target,
            &fixture["callbackResponse"],
            "callback",
        )
        .await;

        let credential = tokio::time::timeout(Duration::from_secs(10), login)
            .await
            .unwrap()
            .unwrap()
            .unwrap();

        // The exchange request: byte-exact body (the verifier is the fixture's).
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        expect_request(&requests[0], &fixture["tokenRequest"], "token request");

        // The stored credential (expires by delta against the frozen clock).
        assert_eq!(
            credential.access,
            fixture["credential"]["access"].as_str().unwrap()
        );
        assert_eq!(
            credential.refresh,
            fixture["credential"]["refresh"].as_str().unwrap()
        );
        assert_expires_delta(
            credential.expires,
            fixture["credential"]["expires"].as_i64().unwrap(),
            "credential expires",
        );
        assert_eq!(
            credential.extra.get("clientId").and_then(Value::as_str),
            fixture["credential"]["clientId"].as_str()
        );
        let scopes: Vec<Value> = fixture["credential"]["scopes"].as_array().unwrap().clone();
        assert_eq!(
            credential
                .extra
                .get("scopes")
                .and_then(Value::as_array)
                .unwrap(),
            &scopes
        );
        assert!(fake.prompts.lock().unwrap()[0]
            .signal
            .as_ref()
            .unwrap()
            .is_cancelled());
    }

    /// Oracle: the registration callback without the issued client id pages
    /// the message, exchanges nothing, and keeps waiting.
    #[tokio::test]
    async fn registration_without_client_id_pages_and_keeps_waiting() {
        let fixture = &oracle("openai_chatgpt")["registrationWithoutClientId"];
        seed_entropy(&[&draw(1), &draw(2)]);
        let server = MockServer::start().await;
        mount_token(&server, token_body(), 200).await;
        let callback_port = free_port();
        let oauth = flow_with_callback_port(&server, callback_port);
        let (fake, interaction) = oracle_interaction_with_device_id(hanging_respond());
        let signal = interaction.signal.clone();

        let login = tokio::spawn(async move { oauth.login(interaction).await });
        let auth_url = wait_for_auth_url(Arc::clone(&fake.auth_url)).await;
        let state = auth_url_param(&auth_url, "state");
        let (status, _, _, body) = http_get(
            callback_port,
            &format!("/auth/callback?code=authorization-code&state={state}"),
        )
        .await;
        assert_eq!(
            status,
            fixture["callbackResponse"]["status"].as_u64().unwrap() as u16
        );
        let expected_message = fixture_page_message(&fixture["callbackResponse"]["body"]);
        assert!(body.contains(expected_message), "{body}");
        // Still waiting: cancel via the interaction signal (the capture's
        // "Login cancelled").
        signal.cancel();
        let error = tokio::time::timeout(Duration::from_secs(5), login)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_upstream_error(&error, fixture["loginError"].as_str().unwrap());
        assert_eq!(fixture["tokenRequestCount"].as_u64().unwrap(), 0);
        assert_eq!(server.received_requests().await.unwrap().len(), 0);
    }

    /// Oracle: the manual paste completes the login (prompt placeholder,
    /// exchange progress, exchange body, credential).
    #[tokio::test]
    async fn manual_paste_login_matches_the_capture() {
        let fixture = &oracle("openai_chatgpt")["manualPasteLogin"];
        seed_entropy(&[&draw(1), &draw(2)]);
        let server = MockServer::start().await;
        mount_token(&server, token_body(), 200).await;
        let oauth = flow_with(&server);
        let auth_url_slot = Arc::new(Mutex::new(None));
        let fake = Arc::new(OracleInteraction {
            auth_url: Arc::clone(&auth_url_slot),
            events: Mutex::new(Vec::new()),
            prompts: Mutex::new(Vec::new()),
            respond: pasting_respond(
                auth_url_slot,
                Arc::new(|redirect, state| {
                    format!(
                        "{redirect}?code=authorization-code&state={state}&client_id=oaiapp_issued"
                    )
                }),
            ),
        });
        let base_interaction = ProviderAuthInteraction::new(
            Arc::clone(&fake) as Arc<dyn AuthInteraction>,
            CancellationToken::new(),
        );
        let interaction = base_interaction.with_login_options(Some(Arc::new(
            crate::ai::auth::types::LoginOptions {
                get_device_id: Some(Box::new(|| DEVICE_ID.to_string())),
            },
        )));

        let credential = tokio::time::timeout(Duration::from_secs(5), oauth.login(interaction))
            .await
            .unwrap()
            .unwrap();

        let (message, placeholder) = {
            let prompts = fake.prompts.lock().unwrap();
            // The method select precedes the manual prompt (v1.0.0).
            let AuthPromptKind::ManualCode {
                message,
                placeholder,
            } = &prompts[0].kind
            else {
                panic!("expected a manual_code prompt");
            };
            (message.clone(), placeholder.clone())
        };
        assert_eq!(message, fixture["prompt"]["message"].as_str().unwrap());
        assert_eq!(
            placeholder.as_deref(),
            fixture["prompt"]["placeholder"].as_str()
        );
        let events = fake.events.lock().unwrap().clone();
        assert!(events
            .iter()
            .any(|event| matches!(event, AuthEvent::Progress { message }
                if message == fixture["events"][0]["message"].as_str().unwrap())));
        let requests = server.received_requests().await.unwrap();
        expect_request(&requests[0], &fixture["tokenRequest"], "token request");
        assert_eq!(
            credential.access,
            fixture["credential"]["access"].as_str().unwrap()
        );
        assert_expires_delta(
            credential.expires,
            fixture["credential"]["expires"].as_i64().unwrap(),
            "credential expires",
        );
        assert_eq!(
            credential.extra.get("clientId").and_then(Value::as_str),
            fixture["credential"]["clientId"].as_str()
        );
    }

    /// Oracle: device-ID validation happens before any authorization work.
    #[tokio::test]
    async fn device_id_required_before_any_authorization() {
        let fixture = &oracle("openai_chatgpt")["deviceIdRequired"];
        let server = MockServer::start().await;
        mount_token(&server, token_body(), 200).await;
        let oauth = flow_with(&server);
        let (fake, interaction) = oracle_interaction(hanging_respond());

        let error = oauth.login(interaction.clone()).await.unwrap_err();
        assert_upstream_error(&error, fixture["error1"].as_str().unwrap());
        let interaction_without = ProviderAuthInteraction::new(
            Arc::clone(&fake) as Arc<dyn AuthInteraction>,
            CancellationToken::new(),
        );
        let error = oauth
            .login(interaction_without.with_login_options(Some(Arc::new(
                crate::ai::auth::types::LoginOptions {
                    get_device_id: Some(Box::new(|| "not-a-uuid".to_string())),
                },
            ))))
            .await
            .unwrap_err();
        assert_upstream_error(&error, fixture["error2"].as_str().unwrap());
        assert!(!fixture["authorizeEmitted"].as_bool().unwrap());
        assert!(fake.auth_url.lock().unwrap().is_none());
        assert!(fake.prompts.lock().unwrap().is_empty());
        assert_eq!(server.received_requests().await.unwrap().len(), 0);
    }

    /// Oracle: manual-paste validation messages (state-mismatch included).
    #[tokio::test]
    async fn manual_validation_messages_match_the_capture() {
        let fixture = &oracle("openai_chatgpt")["manualValidation"];
        let server = MockServer::start().await;
        mount_token(&server, token_body(), 200).await;
        for (name, expected) in fixture.as_object().unwrap() {
            seed_entropy(&[&draw(1), &draw(2)]);
            let pasted = match name.as_str() {
                "not_a_url" => "garbage".to_string(),
                "wrong_origin" => "http://127.0.0.1:9999/auth/callback?code=c&state=s".to_string(),
                "wrong_path" => "http://127.0.0.1:1455/other?code=c&state=s".to_string(),
                "provider_error" => {
                    "http://127.0.0.1:1455/auth/callback?error=access_denied&error_description=nope"
                        .to_string()
                }
                // The capture pasted a fixed state that mismatches the flow's
                // drawn state, so upstream surfaced the mismatch first.
                "missing_client_id" => {
                    "http://127.0.0.1:1455/auth/callback?code=c&state=s".to_string()
                }
                other => panic!("unexpected validation case {other}"),
            };
            let oauth = flow_with(&server);
            let (_fake, interaction) = oracle_interaction_with_device_id(Box::new({
                let pasted = pasted.clone();
                move |_prompt| {
                    let pasted = pasted.clone();
                    Box::pin(async move { Ok(pasted) })
                }
            }));
            let error = tokio::time::timeout(Duration::from_secs(5), oauth.login(interaction))
                .await
                .unwrap()
                .unwrap_err();
            assert_upstream_error(&error, expected.as_str().unwrap());
        }
    }

    /// Oracle: refresh re-uses the issued client id and stores replacement
    /// scopes; the response validators carry the upstream messages.
    #[tokio::test]
    async fn refresh_matches_the_capture() {
        let fixture = &oracle("openai_chatgpt")["refresh"];
        let _server = MockServer::start().await;

        mount_token(&_server, token_body(), 200).await;
        // Drop the refresh token: invalid refresh_token.
        let mut body: Value = serde_json::from_str(&token_body()).unwrap();
        body.as_object_mut().unwrap().remove("refresh_token");
        let server2 = MockServer::start().await;
        mount_token(&server2, body.to_string(), 200).await;
        let oauth_missing =
            crate::ai::auth::oauth::openai_chatgpt::OpenAIChatGptOAuth::with_endpoints(
                format!("{}/api/accounts/oauth/token", server2.uri()),
                "127.0.0.1".to_string(),
                free_port(),
            );
        let error = oauth_missing
            .refresh(connected_credential(), &Default::default())
            .await
            .unwrap_err();
        assert_upstream_error(&error, fixture["missingRefresh"]["error"].as_str().unwrap());

        // Narrow scope: the direct-token scope is required.
        let narrow = serde_json::json!({
            "access_token": "access-token", "refresh_token": "refresh-token",
            "expires_in": 3600, "id_token": "id-token",
            "scope": "openid profile email offline_access resource.invoke",
        });
        let server_narrow = MockServer::start().await;
        mount_token(&server_narrow, narrow.to_string(), 200).await;
        let oauth_narrow =
            crate::ai::auth::oauth::openai_chatgpt::OpenAIChatGptOAuth::with_endpoints(
                format!("{}/api/accounts/oauth/token", server_narrow.uri()),
                "127.0.0.1".to_string(),
                free_port(),
            );
        let error = oauth_narrow
            .refresh(connected_credential(), &Default::default())
            .await
            .unwrap_err();
        assert_upstream_error(&error, fixture["narrowScope"]["error"].as_str().unwrap());

        // The happy refresh.
        let mut response: Value = serde_json::from_str(&token_body()).unwrap();
        let object = response.as_object_mut().unwrap();
        object.insert(
            "access_token".to_string(),
            Value::String("new-access".to_string()),
        );
        object.insert(
            "refresh_token".to_string(),
            Value::String("new-refresh".to_string()),
        );
        let server3 = MockServer::start().await;
        mount_token(&server3, response.to_string(), 200).await;
        let oauth3 = crate::ai::auth::oauth::openai_chatgpt::OpenAIChatGptOAuth::with_endpoints(
            format!("{}/api/accounts/oauth/token", server3.uri()),
            "127.0.0.1".to_string(),
            free_port(),
        );
        let credential = oauth3
            .refresh(connected_credential(), &Default::default())
            .await
            .unwrap();
        let expected = &fixture["refreshed"]["tokenRequest"];
        let requests = server3.received_requests().await.unwrap();
        expect_request(&requests[0], expected, "refresh request");
        assert_eq!(
            credential.access,
            fixture["refreshed"]["credential"]["access"]
                .as_str()
                .unwrap()
        );
        assert_eq!(
            credential.refresh,
            fixture["refreshed"]["credential"]["refresh"]
                .as_str()
                .unwrap()
        );
        assert_expires_delta(
            credential.expires,
            fixture["refreshed"]["credential"]["expires"]
                .as_i64()
                .unwrap(),
            "refreshed credential expires",
        );
        assert_eq!(
            credential.extra.get("clientId").and_then(Value::as_str),
            fixture["refreshed"]["credential"]["clientId"].as_str()
        );
        let _ = _server;
    }

    fn connected_credential() -> OAuthCredential {
        OAuthCredential {
            refresh: "old-refresh".to_string(),
            access: "old-access".to_string(),
            expires: 0,
            extra: std::collections::BTreeMap::from([
                (
                    "clientId".to_string(),
                    Value::String("oaiapp_existing".to_string()),
                ),
                (
                    "scopes".to_string(),
                    Value::Array(
                        "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct"
                            .split(' ')
                            .map(|scope| Value::String(scope.to_string()))
                            .collect(),
                    ),
                ),
            ]),
        }
    }

    /// Oracle: the token endpoint's non-ok shape.
    #[tokio::test]
    async fn token_http_error_matches_the_capture() {
        let fixture = &oracle("openai_chatgpt")["tokenHttpError"];
        let server = MockServer::start().await;
        mount_token(&server, "denied".to_string(), 400).await;
        let oauth = flow_with(&server);
        let error = oauth
            .refresh(connected_credential(), &Default::default())
            .await
            .unwrap_err();
        assert_upstream_error(&error, fixture["error"].as_str().unwrap());
    }

    /// Extracts the `<p>…</p>` message from a captured page body.
    fn fixture_page_message(body: &Value) -> &str {
        let body = body.as_str().unwrap();
        let start = body.find("<p>").unwrap() + 3;
        let end = body[start..].find("</p>").unwrap() + start;
        &body[start..end]
    }
}

// ---------------------------------------------------------------------------
// 3. meta (NEW flow)
// ---------------------------------------------------------------------------
mod meta {
    use super::*;

    fn flow_with(server: &MockServer) -> crate::ai::auth::oauth::meta::MetaOAuth {
        let base = Url::parse(&server.uri()).unwrap();
        crate::ai::auth::oauth::meta::MetaOAuth::with_endpoints(
            format!("{base}oidc/device/authorization/"),
            format!("{base}oidc/device/token/"),
            format!("{base}muse-code/key"),
        )
        .with_interval_scale(0.05)
    }

    async fn mount(server: &MockServer, route: &str, responder: ResponseTemplate) {
        Mock::given(method("POST"))
            .and(path(route))
            .respond_with(responder)
            .mount(server)
            .await;
    }

    fn body_json(body: &str) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_string(body)
    }

    fn meta_interaction() -> (Arc<OracleInteraction>, ProviderAuthInteraction) {
        oracle_interaction(Box::new(|_prompt| {
            Box::pin(async {
                Err(AuthError::Operation(
                    "Meta login should not prompt".to_string(),
                ))
            })
        }))
    }

    /// Oracle: "logs in with the device flow and mints a Model API key" —
    /// request records (bodies byte-exact), the device-code event, the
    /// pending poll, and the minted credential.
    #[tokio::test]
    async fn login_matches_the_capture() {
        let fixture = &oracle("meta")["login"];
        let server = MockServer::start().await;
        mount(
            &server,
            "/oidc/device/authorization/",
            body_json(
                r#"{"device_code":"device-code-123","user_code":"ABCD-1234","verification_uri":"https://auth.meta.com/oauth/device/","verification_uri_complete":"https://auth.meta.com/oauth/device/?code=ABCD-1234","interval":5,"expires_in":600}"#,
            ),
        )
        .await;
        let poll_state = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let poll_state_mount = Arc::clone(&poll_state);
        Mock::given(method("POST"))
            .and(path("/oidc/device/token/"))
            .respond_with(move |_request: &wiremock::Request| {
                let count = poll_state_mount.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if count == 0 {
                    ResponseTemplate::new(400)
                        .set_body_string(r#"{"error":"authorization_pending"}"#)
                } else {
                    ResponseTemplate::new(200).set_body_string(
                        r#"{"access_token":"identity-token","token_type":"Bearer"}"#,
                    )
                }
            })
            .mount(&server)
            .await;
        mount(
            &server,
            "/muse-code/key",
            body_json(r#"{"api_key":"LLM|minted-key"}"#),
        )
        .await;
        let oauth = flow_with(&server);
        let (fake, interaction) = meta_interaction();

        let credential = tokio::time::timeout(Duration::from_secs(10), oauth.login(interaction))
            .await
            .unwrap()
            .unwrap();

        // The request records: paths, methods, header values, byte-exact
        // form bodies.
        let requests = server.received_requests().await.unwrap();
        let expected_requests = fixture["requests"].as_array().unwrap();
        assert_eq!(requests.len(), expected_requests.len());
        for (actual, expected) in requests.iter().zip(expected_requests) {
            expect_request(actual, expected, "meta request");
        }
        // Two device-token polls (pending, then complete).
        assert_eq!(
            poll_state.load(std::sync::atomic::Ordering::SeqCst),
            fixture["pollCount"].as_u64().unwrap() as usize
        );

        // The events: the device-code offer, then the mint progress.
        let events = fake.events.lock().unwrap().clone();
        let expected_events = fixture["events"].as_array().unwrap();
        assert_eq!(events.len(), expected_events.len());
        for (actual, expected) in events.iter().zip(expected_events) {
            match (actual, expected) {
                (
                    AuthEvent::DeviceCode {
                        user_code,
                        verification_uri,
                        interval_seconds,
                        expires_in_seconds,
                    },
                    expected,
                ) if expected["type"] == "device_code" => {
                    assert_eq!(user_code, expected["userCode"].as_str().unwrap());
                    assert_eq!(
                        verification_uri,
                        expected["verificationUri"].as_str().unwrap()
                    );
                    assert_eq!(
                        interval_seconds.map(|value| value.to_string()),
                        expected["intervalSeconds"]
                            .as_u64()
                            .map(|value| value.to_string())
                    );
                    assert_eq!(
                        expires_in_seconds.map(|value| value.to_string()),
                        expected["expiresInSeconds"]
                            .as_u64()
                            .map(|value| value.to_string())
                    );
                }
                (AuthEvent::Progress { message }, expected) if expected["type"] == "progress" => {
                    assert_eq!(message, expected["message"].as_str().unwrap());
                }
                _ => panic!("event mismatch: {actual:?} vs {expected}"),
            }
        }

        // The credential (expires by delta: now + 24h).
        assert_eq!(
            credential.refresh,
            fixture["credential"]["refresh"].as_str().unwrap()
        );
        assert_eq!(
            credential.access,
            fixture["credential"]["access"].as_str().unwrap()
        );
        assert_expires_delta(
            credential.expires,
            fixture["credential"]["expires"].as_i64().unwrap(),
            "credential expires",
        );
    }

    /// Oracle: "re-mints the API key from the stored identity token on
    /// refresh" and the mint error shapes.
    #[tokio::test]
    async fn refresh_and_mint_errors_match_the_capture() {
        let fixture = &oracle("meta");

        // Happy refresh.
        let server = MockServer::start().await;
        mount(
            &server,
            "/muse-code/key",
            body_json(r#"{"api_key":"LLM|fresh-key"}"#),
        )
        .await;
        let oauth = flow_with(&server);
        let credential = oauth
            .refresh(
                OAuthCredential {
                    refresh: "identity-token".to_string(),
                    access: "LLM|old-key".to_string(),
                    expires: 1,
                    extra: Default::default(),
                },
                &Default::default(),
            )
            .await
            .unwrap();
        let expected = &fixture["refresh"]["requests"][0];
        let requests = server.received_requests().await.unwrap();
        expect_request(&requests[0], expected, "refresh request");
        assert_eq!(
            credential.refresh,
            fixture["refresh"]["credential"]["refresh"]
                .as_str()
                .unwrap()
        );
        assert_eq!(
            credential.access,
            fixture["refresh"]["credential"]["access"].as_str().unwrap()
        );
        assert_expires_delta(
            credential.expires,
            fixture["refresh"]["credential"]["expires"]
                .as_i64()
                .unwrap(),
            "refresh credential expires",
        );

        // The mint error shapes (session expiry with detail precedence, the
        // generic failure, and the 503 with an empty object body).
        for (name, status, body, route) in [
            (
                "sessionExpired",
                401,
                r#"{"error":"invalid_token","error_description":"expired"}"#,
                "/muse-code/key",
            ),
            (
                "forbiddenDetail",
                403,
                r#"{"detail":"no seats"}"#,
                "/muse-code/key",
            ),
            (
                "genericFailure",
                500,
                r#"{"message":"mint offline"}"#,
                "/muse-code/key",
            ),
            ("invalidDeviceAuth", 503, "{}", "/muse-code/key"),
        ] {
            let server = MockServer::start().await;
            mount(
                &server,
                route,
                ResponseTemplate::new(status).set_body_string(body),
            )
            .await;
            let oauth = flow_with(&server);
            let error = oauth
                .refresh(
                    OAuthCredential {
                        refresh: "identity-token".to_string(),
                        access: String::new(),
                        expires: 1,
                        extra: Default::default(),
                    },
                    &Default::default(),
                )
                .await
                .unwrap_err();
            assert_upstream_error(&error, fixture["mintErrors"][name].as_str().unwrap());
            let _ = name;
        }

        // The device-authorization failure shape.
        let server = MockServer::start().await;
        mount(
            &server,
            "/oidc/device/authorization/",
            ResponseTemplate::new(429)
                .set_body_string(r#"{"error":"slow_down","error_description":"easy"}"#),
        )
        .await;
        let oauth = flow_with(&server);
        let (_fake, interaction) = meta_interaction();
        let error = oauth.login(interaction).await.unwrap_err();
        assert_upstream_error(
            &error,
            fixture["mintErrors"]["deviceAuthFailed"].as_str().unwrap(),
        );
    }

    /// Oracle: no key issued — the setup URL surfaces; the trusted-URL
    /// rejection; the poll terminal errors; toAuth.
    #[tokio::test]
    async fn error_branches_match_the_capture() {
        let fixture = &oracle("meta");

        // No key issued (action_url).
        let server = MockServer::start().await;
        mount(
            &server,
            "/muse-code/key",
            body_json(r#"{"require_payment":true,"action_url":"https://dev.meta.ai/billing"}"#),
        )
        .await;
        let oauth = flow_with(&server);
        let error = oauth
            .refresh(
                OAuthCredential {
                    refresh: "identity-token".to_string(),
                    access: String::new(),
                    expires: 1,
                    extra: Default::default(),
                },
                &Default::default(),
            )
            .await
            .unwrap_err();
        assert_upstream_error(&error, fixture["noKeySetupUrl"]["error"].as_str().unwrap());

        // Untrusted verification URI (javascript:).
        let server = MockServer::start().await;
        mount(
            &server,
            "/oidc/device/authorization/",
            body_json(
                r#"{"device_code":"d","user_code":"U","verification_uri":"javascript:alert(1)","interval":5,"expires_in":600}"#,
            ),
        )
        .await;
        let oauth = flow_with(&server);
        let (_fake, interaction) = meta_interaction();
        let error = oauth.login(interaction).await.unwrap_err();
        assert_upstream_error(
            &error,
            fixture["untrustedVerificationUri"]["error"]
                .as_str()
                .unwrap(),
        );

        // Poll terminal errors.
        for (name, body) in [
            ("accessDenied", r#"{"error":"access_denied"}"#),
            ("expired", r#"{"error":"expired_token"}"#),
            (
                "unknownError",
                r#"{"error":"weird_thing","message":"boom"}"#,
            ),
        ] {
            let server = MockServer::start().await;
            mount(
                &server,
                "/oidc/device/authorization/",
                body_json(
                    r#"{"device_code":"d","user_code":"U","verification_uri":"https://meta.example/device","interval":5,"expires_in":600}"#,
                ),
            )
            .await;
            mount(
                &server,
                "/oidc/device/token/",
                ResponseTemplate::new(400).set_body_string(body),
            )
            .await;
            let oauth = flow_with(&server);
            let (_fake, interaction) = meta_interaction();
            let error = oauth.login(interaction).await.unwrap_err();
            assert_upstream_error(&error, fixture["pollErrors"][name].as_str().unwrap());
            let _ = name;
        }

        // toAuth derives the request api key.
        let oauth = crate::ai::auth::oauth::meta::MetaOAuth::new();
        let auth = oauth
            .to_auth(OAuthCredential {
                refresh: "identity-token".to_string(),
                access: fixture["toAuth"]["apiKey"].as_str().unwrap().to_string(),
                expires: 1,
                extra: Default::default(),
            })
            .await
            .unwrap();
        assert_eq!(
            auth.api_key.as_deref(),
            Some(fixture["toAuth"]["apiKey"].as_str().unwrap())
        );
    }
}

// ---------------------------------------------------------------------------
// 4. anthropic (refactored flow)
// ---------------------------------------------------------------------------
mod anthropic {
    use super::*;

    /// The authorize URL is byte-identical under the injected entropy (the
    /// state is the PKCE verifier, draw 0).
    #[tokio::test]
    async fn callback_login_matches_the_capture() {
        let fixture = &oracle("anthropic")["callbackLogin"];
        // The captured state is the PKCE verifier (draw 0).
        seed_entropy(&[]);
        let server = MockServer::start().await;
        mount_token(
            &server,
            200,
            r#"{"access_token":"access-token","refresh_token":"refresh-token","expires_in":3600}"#,
        )
        .await;
        let port = 53692;
        let oauth = crate::ai::auth::oauth::anthropic::AnthropicOAuth::with_endpoints(
            format!("{}/v1/oauth/token", server.uri()),
            "127.0.0.1".to_string(),
            port,
        );
        let (fake, interaction) = oracle_interaction(hanging_respond());

        let login = tokio::spawn(async move { oauth.login(interaction).await });
        let auth_url = wait_for_auth_url(Arc::clone(&fake.auth_url)).await;
        assert_eq!(auth_url, fixture["authorizeUrl"].as_str().unwrap());

        let state = auth_url_param(&auth_url, "state");
        expect_page(
            port,
            &format!("/callback?code=cb-code&state={state}"),
            &fixture["success"],
            "success",
        )
        .await;

        let credential = tokio::time::timeout(Duration::from_secs(10), login)
            .await
            .unwrap()
            .unwrap()
            .unwrap();

        // The JSON body compares canonically (serde map ordering divergence).
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].url.path(),
            url_path(fixture["tokenRequest"]["url"].as_str().unwrap())
        );
        let actual: Value = serde_json::from_slice(&requests[0].body).unwrap();
        let expected: Value =
            serde_json::from_str(fixture["tokenRequest"]["body"].as_str().unwrap()).unwrap();
        assert_eq!(actual, expected, "token request body");
        assert_eq!(
            credential.access,
            fixture["credential"]["access"].as_str().unwrap()
        );
        assert_eq!(
            credential.refresh,
            fixture["credential"]["refresh"].as_str().unwrap()
        );
        assert_expires_delta(
            credential.expires,
            fixture["credential"]["expires"].as_i64().unwrap(),
            "credential expires",
        );
        // The progress event.
        let events = fake.events.lock().unwrap().clone();
        assert!(events
            .iter()
            .any(|event| matches!(event, AuthEvent::Progress { message }
                if message == fixture["progressEvents"][0]["message"].as_str().unwrap())));
    }

    async fn mount_token(server: &MockServer, status: u16, body: &str) {
        Mock::given(method("POST"))
            .and(path("/v1/oauth/token"))
            .respond_with(
                ResponseTemplate::new(status).set_body_raw(body.to_string(), "application/json"),
            )
            .mount(server)
            .await;
    }

    /// Oracle: the provider-error redirect fails the login with the shared
    /// message (the refactor's new behavior).
    #[tokio::test]
    async fn provider_error_matches_the_capture() {
        let fixture = &oracle("anthropic")["providerError"];
        let server = MockServer::start().await;
        mount_token(&server, 200, "{}").await;
        let oauth = crate::ai::auth::oauth::anthropic::AnthropicOAuth::with_endpoints(
            format!("{}/v1/oauth/token", server.uri()),
            "127.0.0.1".to_string(),
            53692,
        );
        let (fake, interaction) = oracle_interaction(hanging_respond());
        let login = tokio::spawn(async move { oauth.login(interaction).await });
        let auth_url = wait_for_auth_url(Arc::clone(&fake.auth_url)).await;
        let state = auth_url_param(&auth_url, "state");
        expect_page(
            53692,
            &format!(
                "/callback?error=access_denied&error_description=User%20said%20no&state={state}"
            ),
            &fixture["failure"],
            "failure",
        )
        .await;
        let error = tokio::time::timeout(Duration::from_secs(10), login)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_upstream_error(&error, fixture["loginError"].as_str().unwrap());
    }

    /// Oracle: the manual paste with a mismatched state, and the prompt shape.
    #[tokio::test]
    async fn manual_state_mismatch_and_prompt_match_the_capture() {
        let fixture = &oracle("anthropic")["manualStateMismatch"];
        let server = MockServer::start().await;
        mount_token(&server, 200, "{}").await;
        let port = free_port();
        let oauth = crate::ai::auth::oauth::anthropic::AnthropicOAuth::with_endpoints(
            format!("{}/v1/oauth/token", server.uri()),
            "127.0.0.1".to_string(),
            port,
        );
        let (fake, interaction) = oracle_interaction(Box::new(move |_prompt| {
            let pasted = format!("http://localhost:{port}/callback?code=x&state=wrong-state");
            Box::pin(async move { Ok(pasted) })
        }));
        let error = tokio::time::timeout(Duration::from_secs(5), oauth.login(interaction))
            .await
            .unwrap()
            .unwrap_err();
        assert_upstream_error(&error, fixture["loginError"].as_str().unwrap());
        let (message, placeholder) = {
            let prompts = fake.prompts.lock().unwrap();
            // The method select precedes the manual prompt (v1.0.0).
            let AuthPromptKind::ManualCode {
                message,
                placeholder,
            } = &prompts[1].kind
            else {
                panic!("expected a manual_code prompt");
            };
            (message.clone(), placeholder.clone())
        };
        assert_eq!(message, fixture["prompt"]["message"].as_str().unwrap());
        // The placeholder carries the run's port (the capture pins 53692).
        assert_eq!(
            placeholder.as_deref(),
            Some(format!("http://localhost:{port}/callback").as_str())
        );
    }

    /// Oracle: a taken callback port degrades to manual-paste-only login.
    #[tokio::test]
    async fn bind_failure_degrades_to_manual_paste() {
        let fixture = &oracle("anthropic")["bindFailureManualPaste"];
        // The pasted state is the flow's verifier (draw 0 in the capture).
        seed_entropy(&[]);
        let server = MockServer::start().await;
        mount_token(
            &server,
            200,
            r#"{"access_token":"access-token","refresh_token":"refresh-token","expires_in":3600}"#,
        )
        .await;
        let blocker = std::net::TcpListener::bind(("127.0.0.1", 53692)).unwrap();
        let oauth = crate::ai::auth::oauth::anthropic::AnthropicOAuth::with_endpoints(
            format!("{}/v1/oauth/token", server.uri()),
            "127.0.0.1".to_string(),
            53692,
        );
        let auth_url_slot = Arc::new(Mutex::new(None));
        let paste_slot = Arc::clone(&auth_url_slot);
        let fake = Arc::new(OracleInteraction {
            auth_url: auth_url_slot,
            events: Mutex::new(Vec::new()),
            prompts: Mutex::new(Vec::new()),
            respond: pasting_respond(
                paste_slot,
                Arc::new(|redirect, state| format!("{redirect}?code=manual-code&state={state}")),
            ),
        });
        let interaction = ProviderAuthInteraction::new(
            Arc::clone(&fake) as Arc<dyn AuthInteraction>,
            CancellationToken::new(),
        );
        let credential = tokio::time::timeout(Duration::from_secs(5), oauth.login(interaction))
            .await
            .unwrap()
            .unwrap();
        drop(blocker);

        // The exchange (JSON body, canonical compare).
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].url.path(),
            url_path(fixture["tokenRequest"]["url"].as_str().unwrap())
        );
        let actual: Value = serde_json::from_slice(&requests[0].body).unwrap();
        let expected: Value =
            serde_json::from_str(fixture["tokenRequest"]["body"].as_str().unwrap()).unwrap();
        assert_eq!(actual, expected, "token request body");
        assert_eq!(
            credential.access,
            fixture["credential"]["access"].as_str().unwrap()
        );
        assert_eq!(
            credential.refresh,
            fixture["credential"]["refresh"].as_str().unwrap()
        );
        let _ = fake;
    }

    /// Oracle: the exchange/refresh HTTP error message shapes. The
    /// `formatErrorDetails` tail (Node's `stack=…`) is platform-specific and
    /// asserted as a prefix.
    #[tokio::test]
    async fn exchange_and_refresh_http_errors_match_the_capture_prefixes() {
        let server = MockServer::start().await;
        mount_token(&server, 400, "denied").await;
        let port = free_port();
        let token_url = format!("{}/v1/oauth/token", server.uri());
        let oauth = crate::ai::auth::oauth::anthropic::AnthropicOAuth::with_endpoints(
            token_url.clone(),
            "127.0.0.1".to_string(),
            port,
        );
        let (_fake, interaction) = oracle_interaction(Box::new(|_prompt| {
            Box::pin(async move { Ok("the-code".to_string()) })
        }));
        let error = oauth.login(interaction).await.unwrap_err();
        // The message shape with the run's own URLs (the capture pins the
        // same shape over the production constants; the platform-specific
        // transport detail follows after "details=").
        let redirect = format!("http://localhost:{port}/callback");
        let expected_prefix = format!(
            "Token exchange request failed. url={token_url}; redirect_uri={redirect}; \
             response_type=authorization_code; details="
        );
        assert!(
            upstream_message(&error).starts_with(&expected_prefix),
            "{}",
            error
        );

        // Refresh: compare up to the last `body=` segment (the Node-only
        // `; stack=…` tail follows).
        let server = MockServer::start().await;
        mount_token(&server, 400, "nope").await;
        let oauth = crate::ai::auth::oauth::anthropic::AnthropicOAuth::with_endpoints(
            format!("{}/v1/oauth/token", server.uri()),
            "127.0.0.1".to_string(),
            free_port(),
        );
        let token_url = format!("{}/v1/oauth/token", server.uri());
        let error = oauth
            .refresh(
                OAuthCredential {
                    refresh: "r".to_string(),
                    access: "a".to_string(),
                    expires: 0,
                    extra: Default::default(),
                },
                &Default::default(),
            )
            .await
            .unwrap_err();
        // The message shape over the run's URL (the Node-only `; stack=` tail
        // of the capture is platform-specific and asserted as a prefix).
        let expected = format!(
            "Anthropic token refresh request failed. url={token_url}; details=Error: HTTP \
             request failed. status=400; url={token_url}; body=nope"
        );
        assert!(
            upstream_message(&error).starts_with(&expected),
            "actual: {error}"
        );
    }
}

// ---------------------------------------------------------------------------
// 5. openai-codex (refactored flow)
// ---------------------------------------------------------------------------
mod openai_codex {
    use super::*;

    /// The padded standard-base64 JWT used by the capture (Buffer.toString("base64")).
    fn b64(data: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in data.chunks(3) {
            let b0 = chunk[0] as u32;
            let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
            let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
            let group = (b0 << 16) | (b1 << 8) | b2;
            out.push(ALPHABET[(group >> 18) as usize & 63] as char);
            out.push(ALPHABET[(group >> 12) as usize & 63] as char);
            out.push(if chunk.len() > 1 {
                ALPHABET[(group >> 6) as usize & 63] as char
            } else {
                '='
            });
            out.push(if chunk.len() > 2 {
                ALPHABET[group as usize & 63] as char
            } else {
                '='
            });
        }
        out
    }

    fn access_token(account_id: &str) -> String {
        format!(
            "{}.{}.signature",
            b64(br#"{"alg":"none"}"#),
            b64(format!(
                r#"{{"https://api.openai.com/auth":{{"chatgpt_account_id":"{account_id}"}}}}"#
            )
            .as_bytes())
        )
    }

    async fn mount_token(server: &MockServer, body: String) {
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
            .mount(server)
            .await;
    }

    fn select_browser_then(respond: Respond) -> Respond {
        // The first prompt is the login-method select; the second is the
        // manual paste (the passed responder answers it).
        Box::new(move |prompt| {
            if let AuthPromptKind::Select { .. } = prompt.kind {
                return Box::pin(async move { Ok("browser".to_string()) });
            }
            respond(prompt)
        })
    }

    /// Oracle: the browser login through the shared callback server — the
    /// authorize URL is byte-identical (state from the entropy stream), the
    /// success page matches, and the exchange body is byte-exact.
    #[tokio::test]
    async fn browser_login_matches_the_capture() {
        let fixture = &oracle("openai_codex")["browserLogin"];
        // Draws: PKCE verifier (0), state (1 — 16 bytes for createState).
        seed_entropy(&[&draw_prefix(1, 16)]);
        let server = MockServer::start().await;
        mount_token(
            &server,
            serde_json::json!({
                "access_token": access_token("cb-account"),
                "refresh_token": "cb-refresh",
                "expires_in": 3600,
            })
            .to_string(),
        )
        .await;
        // The pinned redirect URI keeps `localhost:1455` (the capture's
        // bytes); only the test-only listener moves off the well-known port
        // so a foreign holder of 1455 cannot starve the oracle.
        let callback_port = free_port();
        let oauth = crate::ai::auth::oauth::openai_codex::OpenAICodexOAuth::with_endpoints(
            format!("{}/oauth/token", server.uri()),
            format!("{}/api/accounts/deviceauth/usercode", server.uri()),
            format!("{}/api/accounts/deviceauth/token", server.uri()),
            "127.0.0.1".to_string(),
            callback_port,
        );
        let (fake, interaction) = oracle_interaction(select_browser_then(hanging_respond()));

        let login = tokio::spawn(async move { oauth.login(interaction).await });
        let auth_url = wait_for_auth_url(Arc::clone(&fake.auth_url)).await;
        assert_eq!(auth_url, fixture["authorizeUrl"].as_str().unwrap());
        assert_eq!(
            fixture["instructions"].as_str().unwrap(),
            "A browser window should open. Complete login to finish."
        );

        let state = auth_url_param(&auth_url, "state");
        expect_page(
            callback_port,
            &format!("/auth/callback?code=cb-code&state={state}"),
            &fixture["success"],
            "success",
        )
        .await;

        let credential = tokio::time::timeout(Duration::from_secs(10), login)
            .await
            .unwrap()
            .unwrap()
            .unwrap();

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        expect_request(&requests[0], &fixture["tokenRequest"], "token request");
        assert_eq!(
            credential.access,
            fixture["credential"]["access"].as_str().unwrap()
        );
        assert_eq!(
            credential.refresh,
            fixture["credential"]["refresh"].as_str().unwrap()
        );
        assert_expires_delta(
            credential.expires,
            fixture["credential"]["expires"].as_i64().unwrap(),
            "credential expires",
        );
        assert_eq!(
            credential.extra.get("accountId").and_then(Value::as_str),
            fixture["credential"]["accountId"].as_str()
        );
    }

    /// Oracle: the shared-server routes on the codex flow — a missing-code or
    /// state-mismatch callback keeps waiting; a provider error fails the
    /// login with the shared message.
    #[tokio::test]
    async fn routes_match_the_capture() {
        let fixture = &oracle("openai_codex")["routes"];
        seed_entropy(&[&draw_prefix(1, 16)]);
        let server = MockServer::start().await;
        mount_token(
            &server,
            serde_json::json!({
                "access_token": access_token("a"),
                "refresh_token": "r",
                "expires_in": 3600,
            })
            .to_string(),
        )
        .await;
        // The pinned redirect URI keeps `localhost:1455` (the capture's
        // bytes); only the test-only listener moves off the well-known port
        // so a foreign holder of 1455 cannot starve the oracle.
        let callback_port = free_port();
        let oauth = crate::ai::auth::oauth::openai_codex::OpenAICodexOAuth::with_endpoints(
            format!("{}/oauth/token", server.uri()),
            format!("{}/api/accounts/deviceauth/usercode", server.uri()),
            format!("{}/api/accounts/deviceauth/token", server.uri()),
            "127.0.0.1".to_string(),
            callback_port,
        );
        let (fake, interaction) = oracle_interaction(select_browser_then(hanging_respond()));

        let login = tokio::spawn(async move { oauth.login(interaction).await });
        let auth_url = wait_for_auth_url(Arc::clone(&fake.auth_url)).await;
        let state = auth_url_param(&auth_url, "state");
        expect_page(
            callback_port,
            "/auth/callback?code=cb&state=wrong",
            &fixture["mismatch"],
            "mismatch",
        )
        .await;
        expect_page(
            callback_port,
            &format!("/auth/callback?state={state}"),
            &fixture["missingCode"],
            "missing code",
        )
        .await;
        expect_page(
            callback_port,
            &format!("/auth/callback?error=access_denied&error_description=nope&state={state}"),
            &fixture["failure"],
            "provider failure",
        )
        .await;
        let error = tokio::time::timeout(Duration::from_secs(10), login)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_upstream_error(&error, fixture["loginError"].as_str().unwrap());
    }

    /// Oracle: a taken callback port degrades to manual-paste-only login.
    #[tokio::test]
    async fn bind_failure_degrades_to_manual_paste() {
        let fixture = &oracle("openai_codex")["bindFailureManualPaste"];
        // The exchange body carries the PKCE verifier (draw 0 in the capture).
        seed_entropy(&[]);
        let server = MockServer::start().await;
        mount_token(
            &server,
            serde_json::json!({
                "access_token": access_token("manual-account"),
                "refresh_token": "manual-refresh",
                "expires_in": 3600,
            })
            .to_string(),
        )
        .await;
        // The blocker takes a fresh ephemeral port (the capture's pinned
        // `localhost:1455` lives in the flow's constant redirect URI, not the
        // test-only listener), and the flow's bind of the same port then
        // fails with `AddrInUse`.
        let blocker = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let callback_port = blocker.local_addr().unwrap().port();
        let oauth = crate::ai::auth::oauth::openai_codex::OpenAICodexOAuth::with_endpoints(
            format!("{}/oauth/token", server.uri()),
            format!("{}/api/accounts/deviceauth/usercode", server.uri()),
            format!("{}/api/accounts/deviceauth/token", server.uri()),
            "127.0.0.1".to_string(),
            callback_port,
        );
        let auth_url_slot = Arc::new(Mutex::new(None));
        let paste_slot = Arc::clone(&auth_url_slot);
        let fake = Arc::new(OracleInteraction {
            auth_url: auth_url_slot,
            events: Mutex::new(Vec::new()),
            prompts: Mutex::new(Vec::new()),
            respond: select_browser_then(pasting_respond(
                paste_slot,
                Arc::new(|redirect, state| format!("{redirect}?code=manual-code&state={state}")),
            )),
        });
        let interaction = ProviderAuthInteraction::new(
            Arc::clone(&fake) as Arc<dyn AuthInteraction>,
            CancellationToken::new(),
        );
        let credential = tokio::time::timeout(Duration::from_secs(5), oauth.login(interaction))
            .await
            .unwrap()
            .unwrap();
        drop(blocker);
        let requests = server.received_requests().await.unwrap();
        expect_request(&requests[0], &fixture["tokenRequest"], "token request");
        assert_eq!(
            credential.access,
            fixture["credential"]["access"].as_str().unwrap()
        );
        assert_eq!(
            credential.refresh,
            fixture["credential"]["refresh"].as_str().unwrap()
        );
        assert_eq!(
            credential.extra.get("accountId").and_then(Value::as_str),
            fixture["credential"]["accountId"].as_str()
        );
    }

    /// Oracle: the manual paste with a wrong state fails with "State
    /// mismatch".
    #[tokio::test]
    async fn manual_state_mismatch_matches_the_capture() {
        let fixture = &oracle("openai_codex")["manualStateMismatch"];
        let server = MockServer::start().await;
        mount_token(
            &server,
            serde_json::json!({
                "access_token": access_token("a"), "refresh_token": "r", "expires_in": 3600,
            })
            .to_string(),
        )
        .await;
        let oauth = crate::ai::auth::oauth::openai_codex::OpenAICodexOAuth::with_endpoints(
            format!("{}/oauth/token", server.uri()),
            format!("{}/api/accounts/deviceauth/usercode", server.uri()),
            format!("{}/api/accounts/deviceauth/token", server.uri()),
            "127.0.0.1".to_string(),
            free_port(),
        );
        let (_fake, interaction) = oracle_interaction(select_browser_then(Box::new(|_prompt| {
            Box::pin(async move {
                Ok("http://localhost:1455/auth/callback?code=x&state=wrong-state".to_string())
            })
        })));
        let error = tokio::time::timeout(Duration::from_secs(5), oauth.login(interaction))
            .await
            .unwrap()
            .unwrap_err();
        assert_upstream_error(&error, fixture["loginError"].as_str().unwrap());
    }

    /// Oracle: the device-code login (request bodies, device-code event,
    /// credential).
    #[tokio::test]
    async fn device_code_login_matches_the_capture() {
        let fixture = &oracle("openai_codex")["deviceCodeLogin"];
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/accounts/deviceauth/usercode"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"device_auth_id":"device-auth-id","user_code":"ABCD-1234","interval":"1"}"#,
            ))
            .mount(&server)
            .await;
        let poll_state = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let poll_state_mount = Arc::clone(&poll_state);
        Mock::given(method("POST"))
            .and(path("/api/accounts/deviceauth/token"))
            .respond_with(move |_request: &wiremock::Request| {
                let count = poll_state_mount.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if count == 0 {
                    ResponseTemplate::new(403).set_body_string(
                        r#"{"error":{"code":"deviceauth_authorization_pending"}}"#,
                    )
                } else {
                    ResponseTemplate::new(200).set_body_string(
                        r#"{"authorization_code":"oauth-code","code_verifier":"device-code-verifier"}"#,
                    )
                }
            })
            .mount(&server)
            .await;
        mount_token(
            &server,
            serde_json::json!({
                "access_token": access_token("device-account"),
                "refresh_token": "device-refresh",
                "expires_in": 3600,
            })
            .to_string(),
        )
        .await;
        let oauth = crate::ai::auth::oauth::openai_codex::OpenAICodexOAuth::with_endpoints(
            format!("{}/oauth/token", server.uri()),
            format!("{}/api/accounts/deviceauth/usercode", server.uri()),
            format!("{}/api/accounts/deviceauth/token", server.uri()),
            "127.0.0.1".to_string(),
            free_port(),
        );
        let (fake, interaction) = oracle_interaction(Box::new(|prompt| {
            if let AuthPromptKind::Select { .. } = prompt.kind {
                return Box::pin(async move { Ok("device_code".to_string()) });
            }
            panic!("unexpected prompt");
        }));

        let credential = tokio::time::timeout(Duration::from_secs(15), oauth.login(interaction))
            .await
            .unwrap()
            .unwrap();

        // The device-code event.
        let events = fake.events.lock().unwrap().clone();
        let expected_event = &fixture["deviceCodeEvent"];
        assert!(events.iter().any(|event| match event {
            AuthEvent::DeviceCode {
                user_code,
                verification_uri,
                interval_seconds,
                expires_in_seconds,
            } => {
                user_code == expected_event["userCode"].as_str().unwrap()
                    && verification_uri == expected_event["verificationUri"].as_str().unwrap()
                    && *interval_seconds
                        == Some(expected_event["intervalSeconds"].as_u64().unwrap())
                    && *expires_in_seconds
                        == Some(expected_event["expiresInSeconds"].as_u64().unwrap())
            }
            _ => false,
        }));

        // The request bodies (byte-exact, in order).
        let requests = server.received_requests().await.unwrap();
        let expected_requests = fixture["requests"].as_array().unwrap();
        assert_eq!(requests.len(), expected_requests.len());
        for (actual, expected) in requests.iter().zip(expected_requests) {
            expect_request(actual, expected, "codex device request");
        }

        assert_eq!(
            credential.access,
            fixture["credential"]["access"].as_str().unwrap()
        );
        assert_eq!(
            credential.refresh,
            fixture["credential"]["refresh"].as_str().unwrap()
        );
        assert_eq!(
            credential.extra.get("accountId").and_then(Value::as_str),
            fixture["credential"]["accountId"].as_str()
        );
    }
}

// ---------------------------------------------------------------------------
// 6. openrouter (refactored flow)
// ---------------------------------------------------------------------------
mod openrouter {
    use super::*;

    fn flow_with(token_server: &MockServer) -> crate::ai::auth::oauth::openrouter::OpenRouterOAuth {
        crate::ai::auth::oauth::openrouter::OpenRouterOAuth::with_endpoints(
            format!("{}/api/v1/auth/keys", token_server.uri()),
            "127.0.0.1".to_string(),
        )
    }

    async fn mount_exchange(server: &MockServer, body: &str, status: u16) {
        Mock::given(method("POST"))
            .and(path("/api/v1/auth/keys"))
            .respond_with(
                ResponseTemplate::new(status).set_body_raw(body.to_string(), "application/json"),
            )
            .mount(server)
            .await;
    }

    /// Oracle: the login through the shared callback server (fixed UUID
    /// callback path, authorize URL, exchange request, credential, success
    /// page, prompt shape).
    #[tokio::test]
    async fn login_matches_the_capture() {
        let fixture = &oracle("openrouter")["login"];
        seed_entropy(&[FIXED_UUID_BYTES.as_slice()]);
        let server = MockServer::start().await;
        mount_exchange(&server, r#"{"key":"sk-or-test"}"#, 200).await;
        let oauth = flow_with(&server);
        let (fake, interaction) = oracle_interaction(hanging_respond());

        let login = tokio::spawn(async move { oauth.login(interaction).await });
        let auth_url = wait_for_auth_url(Arc::clone(&fake.auth_url)).await;
        // The authorize URL matches with the run's ephemeral callback URL
        // substituted for the capture's (both are
        // `http://127.0.0.1:<port>/oauth/callback/<fixed-uuid>`; the path
        // pins the fixed UUID).
        let expected = fixture["authorizeUrl"].as_str().unwrap();
        let actual_callback = auth_url_param(&auth_url, "callback_url");
        let expected_callback = auth_url_param(expected, "callback_url");
        let encoded = |callback: &str| {
            url::form_urlencoded::Serializer::new(String::new())
                .append_pair("callback_url", callback)
                .finish()
        };
        let expected_url =
            expected.replace(&encoded(&expected_callback), &encoded(&actual_callback));
        assert_eq!(auth_url, expected_url);

        // The callback path carries the fixed UUID.
        let callback_url = auth_url_param(&auth_url, "callback_url");
        let parsed = Url::parse(&callback_url).unwrap();
        assert_eq!(parsed.path(), fixture["callbackPath"].as_str().unwrap());
        // The progress message is identical modulo the per-run port: compare
        // against the capture's own callback URL.
        let fixture_callback = auth_url_param(expected, "callback_url");
        assert_eq!(
            fixture["progressEvents"][0]["message"].as_str().unwrap(),
            format!("Listening for OpenRouter OAuth callback on {fixture_callback}")
        );

        expect_page(
            parsed.port().unwrap(),
            &format!("{}?code=authorization-code", parsed.path()),
            &fixture["callbackSuccess"],
            "callback success",
        )
        .await;

        let credential = tokio::time::timeout(Duration::from_secs(10), login)
            .await
            .unwrap()
            .unwrap()
            .unwrap();

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        expect_request(
            &requests[0],
            &fixture["exchangeRequest"],
            "exchange request",
        );
        assert_eq!(
            credential.access,
            fixture["credential"]["access"].as_str().unwrap()
        );
        assert_eq!(
            credential.refresh,
            fixture["credential"]["refresh"].as_str().unwrap()
        );
        assert_eq!(
            credential.expires.to_string(),
            fixture["credential"]["expires"].to_string()
        );

        // The manual prompt shape.
        let (message, placeholder) = {
            let prompts = fake.prompts.lock().unwrap();
            // The method select precedes the manual prompt (v1.0.0).
            let AuthPromptKind::ManualCode {
                message,
                placeholder,
            } = &prompts[0].kind
            else {
                panic!("expected a manual_code prompt");
            };
            (message.clone(), placeholder.clone())
        };
        assert_eq!(message, fixture["prompt"]["message"].as_str().unwrap());
        assert_eq!(placeholder.as_deref(), Some(callback_url.as_str()));
    }

    /// Oracle: the no-key exchange failure renders the 502 page through the
    /// callback and fails the login; the HTTP-error message matches.
    #[tokio::test]
    async fn exchange_errors_match_the_capture() {
        let fixture = &oracle("openrouter")["exchangeErrors"];
        seed_entropy(&[FIXED_UUID_BYTES.as_slice()]);
        let server = MockServer::start().await;
        mount_exchange(&server, r#"{"user_id":"user-1"}"#, 200).await;
        let oauth = flow_with(&server);
        let (fake, interaction) = oracle_interaction(hanging_respond());
        let login = tokio::spawn(async move { oauth.login(interaction).await });
        let auth_url = wait_for_auth_url(Arc::clone(&fake.auth_url)).await;
        let callback_url = auth_url_param(&auth_url, "callback_url");
        let parsed = Url::parse(&callback_url).unwrap();
        expect_page(
            parsed.port().unwrap(),
            &format!("{}?code=code-without-key", parsed.path()),
            &fixture["noKey"],
            "no key page",
        )
        .await;
        let error = tokio::time::timeout(Duration::from_secs(10), login)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_upstream_error(&error, fixture["noKeyError"].as_str().unwrap());

        // The HTTP-error message via the manual paste.
        let server = MockServer::start().await;
        mount_exchange(&server, r#"{"error":"invalid code"}"#, 403).await;
        let oauth = flow_with(&server);
        let (_fake, interaction) = oracle_interaction(Box::new(|_prompt| {
            Box::pin(async move {
                Ok("http://localhost:9000/oauth/callback/deadbeef?code=the-code".to_string())
            })
        }));
        let error = tokio::time::timeout(Duration::from_secs(5), oauth.login(interaction))
            .await
            .unwrap()
            .unwrap_err();
        assert_upstream_error(&error, fixture["httpError"].as_str().unwrap());
    }

    /// Oracle: the provider redirect error fails the login through the
    /// shared server.
    #[tokio::test]
    async fn provider_error_matches_the_capture() {
        let fixture = &oracle("openrouter")["providerError"];
        seed_entropy(&[FIXED_UUID_BYTES.as_slice()]);
        let server = MockServer::start().await;
        mount_exchange(&server, r#"{"key":"unused"}"#, 200).await;
        let oauth = flow_with(&server);
        let (fake, interaction) = oracle_interaction(hanging_respond());
        let login = tokio::spawn(async move { oauth.login(interaction).await });
        let auth_url = wait_for_auth_url(Arc::clone(&fake.auth_url)).await;
        let callback_url = auth_url_param(&auth_url, "callback_url");
        let parsed = Url::parse(&callback_url).unwrap();
        expect_page(
            parsed.port().unwrap(),
            &format!(
                "{}?error=access_denied&error_description=User%20denied",
                parsed.path()
            ),
            &fixture["failure"],
            "provider failure",
        )
        .await;
        let error = tokio::time::timeout(Duration::from_secs(10), login)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_upstream_error(&error, fixture["loginError"].as_str().unwrap());
    }
}

// ---------------------------------------------------------------------------
// 7. radius (refactored flow)
// ---------------------------------------------------------------------------
mod radius {
    use super::*;

    const TOKEN_SUCCESS: &str = r#"{"access_token":"access-token","refresh_token":"refresh-token","expires_in":3600,"scope":"gateway offline_access"}"#;

    fn oauth_with_port(gateway: &str, port: u16) -> crate::ai::auth::oauth::radius::RadiusOAuth {
        crate::ai::auth::oauth::radius::RadiusOAuth::with_callback_port(
            "Radius".to_string(),
            gateway.to_string(),
            port,
        )
    }

    fn select(option_id: &'static str) -> Respond {
        Box::new(move |prompt| {
            if let AuthPromptKind::Select { .. } = prompt.kind {
                let option_id = option_id;
                return Box::pin(async move { Ok(option_id.to_string()) });
            }
            panic!("unexpected prompt");
        })
    }

    /// Oracle: the browser login with the exchange INSIDE the callback
    /// handler — authorize URL (fixed UUID state), the 200 success page, the
    /// token request body, the progress events, and the select prompt.
    #[tokio::test]
    async fn browser_login_matches_the_capture() {
        let fixture = &oracle("radius")["browserLogin"];
        seed_entropy(&[FIXED_UUID_BYTES.as_slice()]);
        let gateway = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/oauth"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"authorizationEndpoint":"https://radius-ui.example/authorize"}"#,
            ))
            .mount(&gateway)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/oauth/token"))
            .respond_with(ResponseTemplate::new(200).set_body_string(TOKEN_SUCCESS))
            .mount(&gateway)
            .await;
        let port = 1456;
        let oauth = oauth_with_port(&gateway.uri(), port);
        let (fake, interaction) = oracle_interaction(select("browser"));

        let login = tokio::spawn(async move { oauth.login(interaction).await });
        let auth_url = wait_for_auth_url(Arc::clone(&fake.auth_url)).await;
        assert_eq!(auth_url, fixture["authorizeUrl"].as_str().unwrap());

        let state = auth_url_param(&auth_url, "state");
        expect_page(
            port,
            &format!("/oauth/callback?code=the-code&state={state}"),
            &fixture["success"],
            "success",
        )
        .await;

        let credential = tokio::time::timeout(Duration::from_secs(10), login)
            .await
            .unwrap()
            .unwrap()
            .unwrap();

        // The token request: path, method, headers, byte-exact body.
        let requests = gateway.received_requests().await.unwrap();
        let token_request = requests
            .iter()
            .find(|request| request.url.path() == "/v1/oauth/token")
            .unwrap();
        expect_request(
            token_request,
            &fixture["tokenRequest"],
            "radius token request",
        );

        // The progress events announce the listener and the URL.
        let events = fake.events.lock().unwrap().clone();
        assert!(events
            .iter()
            .any(|event| matches!(event, AuthEvent::Progress { message }
                if *message == fixture["progressEvents"][0]["message"].as_str().unwrap())));

        // The select prompt shape.
        let prompts = fake.prompts.lock().unwrap();
        let AuthPromptKind::Select { message, options } = &prompts[0].kind else {
            panic!("expected a select prompt");
        };
        assert_eq!(
            message,
            fixture["selectPrompt"]["message"].as_str().unwrap()
        );
        let expected_options = fixture["selectPrompt"]["options"].as_array().unwrap();
        assert_eq!(options.len(), expected_options.len());
        for (actual, expected) in options.iter().zip(expected_options) {
            assert_eq!(actual.id, expected["id"].as_str().unwrap());
            assert_eq!(actual.label, expected["label"].as_str().unwrap());
        }

        assert_eq!(
            credential.access,
            fixture["credential"]["access"].as_str().unwrap()
        );
        assert_eq!(
            credential.refresh,
            fixture["credential"]["refresh"].as_str().unwrap()
        );
        assert_expires_delta(
            credential.expires,
            fixture["credential"]["expires"].as_i64().unwrap(),
            "credential expires",
        );
    }

    async fn mount_discovery(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/v1/oauth"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"authorizationEndpoint":"https://radius-ui.example/authorize"}"#,
            ))
            .mount(server)
            .await;
    }

    /// Oracle: the exchange failing inside the handler renders the 502 page
    /// and fails the login with the OAuth error detail.
    #[tokio::test]
    async fn exchange_failure_matches_the_capture() {
        let fixture = &oracle("radius")["exchangeFailure"];
        seed_entropy(&[FIXED_UUID_BYTES.as_slice()]);
        let gateway = MockServer::start().await;
        mount_discovery(&gateway).await;
        Mock::given(method("POST"))
            .and(path("/v1/oauth/token"))
            .respond_with(
                ResponseTemplate::new(400)
                    .set_body_string(r#"{"error":"invalid_grant","error_description":"nope"}"#),
            )
            .mount(&gateway)
            .await;
        let oauth = oauth_with_port(&gateway.uri(), 1456);
        let (fake, interaction) = oracle_interaction(select("browser"));

        let login = tokio::spawn(async move { oauth.login(interaction).await });
        let auth_url = wait_for_auth_url(Arc::clone(&fake.auth_url)).await;
        let state = auth_url_param(&auth_url, "state");
        expect_page(
            1456,
            &format!("/oauth/callback?code=the-code&state={state}"),
            &fixture["failure"],
            "failure page",
        )
        .await;
        let error = tokio::time::timeout(Duration::from_secs(10), login)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_upstream_error(&error, fixture["loginError"].as_str().unwrap());
        let _ = fake;
    }

    /// Oracle: the state mismatch keeps waiting; the provider error fails
    /// with the shared message.
    #[tokio::test]
    async fn routes_match_the_capture() {
        let fixture = &oracle("radius")["routes"];
        seed_entropy(&[FIXED_UUID_BYTES.as_slice()]);
        let gateway = MockServer::start().await;
        mount_discovery(&gateway).await;
        Mock::given(method("POST"))
            .and(path("/v1/oauth/token"))
            .respond_with(ResponseTemplate::new(500).set_body_string("{}"))
            .mount(&gateway)
            .await;
        let oauth = oauth_with_port(&gateway.uri(), 1456);
        let (fake, interaction) = oracle_interaction(select("browser"));

        let login = tokio::spawn(async move { oauth.login(interaction).await });
        let auth_url = wait_for_auth_url(Arc::clone(&fake.auth_url)).await;
        let state = auth_url_param(&auth_url, "state");
        let (mismatch_status, _, _, mismatch_body) =
            http_get(1456, "/oauth/callback?code=the-code&state=wrong").await;
        assert_eq!(
            mismatch_status,
            fixture["mismatch"]["status"].as_u64().unwrap() as u16
        );
        assert_eq!(mismatch_body, fixture["mismatch"]["body"].as_str().unwrap());
        let (provider_status, _, _, provider_body) = http_get(
            1456,
            &format!("/oauth/callback?error=access_denied&error_description=nope&state={state}"),
        )
        .await;
        assert_eq!(
            provider_status,
            fixture["providerFailure"]["status"].as_u64().unwrap() as u16
        );
        assert_eq!(
            provider_body,
            fixture["providerFailure"]["body"].as_str().unwrap()
        );
        let error = tokio::time::timeout(Duration::from_secs(10), login)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_upstream_error(&error, fixture["loginError"].as_str().unwrap());
    }

    /// Oracle: the device-code login (request bodies, device-code event,
    /// credential).
    #[tokio::test]
    async fn device_code_login_matches_the_capture() {
        let fixture = &oracle("radius")["deviceCodeLogin"];
        let gateway = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/oauth/device"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"device_code":"device-code","user_code":"ABCD-1234","verification_uri":"https://radius-ui.example/pair","expires_in":600,"interval":1}"#,
            ))
            .mount(&gateway)
            .await;
        let poll_state = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let poll_state_mount = Arc::clone(&poll_state);
        // The capture's token queue: pending, then the minted device token.
        Mock::given(method("POST"))
            .and(path("/v1/oauth/token"))
            .respond_with(move |_request: &wiremock::Request| {
                let count = poll_state_mount.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if count == 0 {
                    ResponseTemplate::new(400)
                        .set_body_string(r#"{"error":"authorization_pending"}"#)
                } else {
                    ResponseTemplate::new(200).set_body_string(
                        r#"{"access_token":"device-access","refresh_token":"device-refresh","expires_in":3600}"#,
                    )
                }
            })
            .mount(&gateway)
            .await;
        let oauth = crate::ai::auth::oauth::radius::create_radius_oauth(
            crate::ai::auth::oauth::radius::RadiusOAuthOptions {
                name: "Radius".to_string(),
                gateway: gateway.uri(),
            },
        );
        let (fake, interaction) = oracle_interaction(select("device-code"));

        let credential = tokio::time::timeout(Duration::from_secs(15), oauth.login(interaction))
            .await
            .unwrap()
            .unwrap();

        // The device-authorization and token request bodies.
        let requests = gateway.received_requests().await.unwrap();
        let device_requests: Vec<_> = requests
            .iter()
            .filter(|request| request.url.path() == "/v1/oauth/device")
            .collect();
        assert_eq!(device_requests.len(), 1);
        expect_request(
            device_requests[0],
            &fixture["deviceRequest"],
            "radius device request",
        );
        let token_requests: Vec<_> = requests
            .iter()
            .filter(|request| request.url.path() == "/v1/oauth/token")
            .collect();
        let expected_tokens = fixture["tokenRequests"].as_array().unwrap();
        assert_eq!(token_requests.len(), expected_tokens.len());
        for (actual, expected) in token_requests.iter().zip(expected_tokens) {
            // The captured record carries only method/url/body.
            assert_eq!(
                String::from_utf8_lossy(&actual.body),
                expected["body"].as_str().unwrap()
            );
        }

        // The device-code event.
        let events = fake.events.lock().unwrap().clone();
        let expected_event = &fixture["deviceCodeEvents"][0];
        assert!(events.iter().any(|event| match event {
            AuthEvent::DeviceCode {
                user_code,
                verification_uri,
                interval_seconds,
                expires_in_seconds,
            } => {
                user_code == expected_event["userCode"].as_str().unwrap()
                    && verification_uri == expected_event["verificationUri"].as_str().unwrap()
                    && *interval_seconds
                        == Some(expected_event["intervalSeconds"].as_u64().unwrap())
                    && *expires_in_seconds
                        == Some(expected_event["expiresInSeconds"].as_u64().unwrap())
            }
            _ => false,
        }));

        assert_eq!(
            credential.access,
            fixture["credential"]["access"].as_str().unwrap()
        );
        assert_eq!(
            credential.refresh,
            fixture["credential"]["refresh"].as_str().unwrap()
        );
        assert_expires_delta(
            credential.expires,
            fixture["credential"]["expires"].as_i64().unwrap(),
            "device credential expires",
        );
    }
}
