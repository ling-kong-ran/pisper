//! Shared plumbing for the M2b API ports: the uniform [`ApiImpl`] stream
//! contract (upstream `ProviderStreams`, `packages/ai/src/types.ts:272-285`,
//! with the deferred-response members as additive capabilities:
//! `stream_deferred` and the `cancel_deferred`/`supports_deferred_cancel`
//! pair) and the shared HTTP client factory used by every provider request.

pub mod anthropic;
pub mod azure_openai_responses;
pub mod bedrock;
pub mod cloudflare_workers_ai_system_one;
pub mod google_generative_ai;
pub mod google_shared;
pub mod google_vertex;
pub mod llama_cpp_classify;
pub mod mistral;
pub mod openai_codex_responses;
pub mod openai_completions;
pub mod openai_responses;
pub mod openai_responses_shared;
pub mod pi_messages;
pub mod system_one_shared;
pub mod typesafe_system_one;

#[cfg(test)]
mod ai_delta_oracle_tests;

use crate::ai::transcript::TranscriptContext;
use crate::ai::types::events::AssistantMessageEvent;
use crate::ai::types::options::{SimpleStreamOptions, StreamOptions};
use crate::ai::types::Model;
use crate::ai::ProviderConfig;
use std::sync::OnceLock;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Upstream `createAbortError()` (provider-retry.ts:81-85): the abort thrown
/// when the request signal fires during request setup or a retry backoff
/// sleep (`name: "AbortError"`, message `"Request aborted"`). Request-setup
/// aborts surface this message through the API catch blocks.
pub const REQUEST_ABORTED: &str = "Request aborted";

/// Upstream's per-API mid-stream abort (`new Error("Request was aborted")`,
/// thrown by the SSE readers and the post-loop `signal?.aborted` checks in
/// every HTTP API): a cancellation after `Start` settles the message with
/// this errorMessage.
pub const REQUEST_WAS_ABORTED: &str = "Request was aborted";

/// The effective per-request signal: a set token is cloned; `None` (upstream
/// `options.signal === undefined`) becomes a fresh token that never cancels,
/// so use sites need no `Option` branching.
pub(crate) fn request_signal(signal: &Option<CancellationToken>) -> CancellationToken {
    signal.clone().unwrap_or_default()
}

/// Upstream `ProviderStreams` (types.ts:272-285): the two entry points every
/// API implementation module exports upstream (`stream`, `streamSimple`),
/// plus the deferred-response capabilities as additive trait methods. Both
/// stream entries replay the normalized [`TranscriptContext`] against one
/// endpoint/model pair and flow events out of the returned channel following
/// the upstream `AssistantMessageEvent` protocol (`start` first,
/// `done`/`error` last).
pub trait ApiImpl: Send + Sync {
    /// Whether this adapter implements the request lifecycle callbacks.
    /// Legacy adapters remain usable without callbacks; Models refuses a
    /// callback-bearing request rather than silently dropping a harness hook.
    fn supports_request_callbacks(&self) -> bool {
        false
    }

    /// Upstream optional `provider.fetchDeferred` (`models.ts:726-732`): a
    /// provider capability, not a requirement. The trait default reproduces
    /// the upstream absence path — a setup-error stream carrying
    /// `ModelsError("provider", "Provider X does not support deferred
    /// responses")` — so capability upgrades stay additive.
    fn stream_deferred(
        &self,
        _cfg: &ProviderConfig,
        model: &Model,
        _handle: &crate::ai::types::options::DeferredHandle,
        _options: &StreamOptions,
    ) -> mpsc::Receiver<AssistantMessageEvent> {
        let (tx, rx) = mpsc::channel(4);
        let model = model.clone();
        tokio::spawn(async move {
            let _ = tx
                .send(AssistantMessageEvent::Error {
                    reason: crate::ai::types::events::ErrorReason::Error,
                    error: crate::ai::models::setup_error_message(
                        &model,
                        format!(
                            "Provider {} does not support deferred responses",
                            model.provider
                        ),
                    ),
                })
                .await;
        });
        rx
    }

    /// Upstream `provider.cancelDeferred !== undefined` (models.ts:749): the
    /// capability probe [`Models::cancel_deferred`](crate::ai::models::Models::cancel_deferred)
    /// checks before auth resolution, exactly like the upstream capability
    /// check runs before `applyAuth`. The trait default reproduces the
    /// upstream absence (`false`); capability upgrades stay additive.
    fn supports_deferred_cancel(&self) -> bool {
        false
    }

    /// Upstream optional `provider.cancelDeferred` (models.ts:155, attached
    /// conditionally at models.ts:870-881): best-effort remote cancellation
    /// of one deferred response. `Models::cancel_deferred` gates on
    /// [`ApiImpl::supports_deferred_cancel`], so the default here is only
    /// reachable through direct callers; it carries the same upstream
    /// `ModelsError("provider", "Provider X does not support deferred
    /// responses")` message as the routing-side rejection. The error channel
    /// is the port's plain-message form (the `route_stream` precedent);
    /// upstream throws, and every production call site swallows.
    fn cancel_deferred(
        &self,
        _cfg: &ProviderConfig,
        model: &Model,
        _handle: &crate::ai::types::options::DeferredHandle,
        _options: &StreamOptions,
    ) -> futures::future::BoxFuture<'static, Result<(), String>> {
        let message = format!(
            "Provider {} does not support deferred responses",
            model.provider
        );
        Box::pin(async move { Err(message) })
    }
    fn stream(
        &self,
        cfg: &ProviderConfig,
        model: &Model,
        ctx: &TranscriptContext,
        options: &StreamOptions,
    ) -> mpsc::Receiver<AssistantMessageEvent>;

    fn stream_simple(
        &self,
        cfg: &ProviderConfig,
        model: &Model,
        ctx: &TranscriptContext,
        options: &SimpleStreamOptions,
    ) -> mpsc::Receiver<AssistantMessageEvent>;
}

/// Shared HTTP client for every provider request, cached behind a
/// `OnceLock` (reqwest::Client is an internally reference-counted handle,
/// so cloning the returned value is cheap).
///
/// Divergence from upstream, per the M2b ruling: the upstream SDKs default
/// to a 10-minute TOTAL request timeout, which is stream-hostile — a long
/// generation would be cut off mid-stream. This client applies a 60-second
/// CONNECT timeout only and no total timeout, mirroring the M2a deferral
/// closed here.
///
/// Idle keep-alive pooling is disabled (`pool_max_idle_per_host(0)`): pi
/// issues one streaming POST per turn, so reuse buys nothing, and pooled
/// sockets to short-lived endpoints (tests, gateways) outlive their peers —
/// a recycled port then resets the stale connection and surfaces as a
/// spurious transport error.
pub fn http_client() -> reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(60))
                .pool_max_idle_per_host(0)
                .user_agent(pi_user_agent())
                .build()
                .expect("shared reqwest client must build")
        })
        .clone()
}

/// Upstream `getPiUserAgent` (`packages/ai/src/utils/pi-user-agent.ts`):
/// `pi ({platform} {release}; {arch})` with Node `os` module spellings.
pub fn pi_user_agent() -> String {
    pi_user_agent_from(os_platform(), &os_release(), os_arch())
}

/// Pure formatter behind [`pi_user_agent`]; the byte-exact format is pinned
/// by tests.
fn pi_user_agent_from(platform: &str, release: &str, arch: &str) -> String {
    format!("pi ({platform} {release}; {arch})")
}

/// Node `os.platform()` names (`win32`/`darwin`/`linux`/...) from the Rust
/// build-target constant.
fn os_platform() -> &'static str {
    match std::env::consts::OS {
        "windows" => "win32",
        "macos" => "darwin",
        other => other,
    }
}

/// Node `os.arch()` names (`x64`/`arm64`/...) from the Rust build-target
/// constant.
fn os_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    }
}

/// Node `os.release()`: the OS version string. Windows reports
/// `major.minor.build` via `RtlGetVersion` (the API node uses), macOS the
/// Darwin kernel version via `sysctl`, Linux the kernel release from procfs;
/// anything else degrades to `"unknown"`.
#[cfg(windows)]
fn os_release() -> String {
    #[repr(C)]
    #[allow(non_snake_case)]
    struct OsVersionInfoW {
        dwOSVersionInfoSize: u32,
        dwMajorVersion: u32,
        dwMinorVersion: u32,
        dwBuildNumber: u32,
        dwPlatformId: u32,
        szCSDVersion: [u16; 128],
    }
    #[link(name = "ntdll")]
    #[allow(non_snake_case)]
    extern "system" {
        fn RtlGetVersion(lpVersionInformation: *mut OsVersionInfoW) -> i32;
    }
    let mut info = OsVersionInfoW {
        dwOSVersionInfoSize: std::mem::size_of::<OsVersionInfoW>() as u32,
        dwMajorVersion: 0,
        dwMinorVersion: 0,
        dwBuildNumber: 0,
        dwPlatformId: 0,
        szCSDVersion: [0; 128],
    };
    // SAFETY: `info` is a fully initialized OSVERSIONINFOW whose size is
    // declared in `dwOSVersionInfoSize`; RtlGetVersion only writes to it.
    if unsafe { RtlGetVersion(&mut info) } == 0 {
        format!(
            "{}.{}.{}",
            info.dwMajorVersion, info.dwMinorVersion, info.dwBuildNumber
        )
    } else {
        "unknown".to_string()
    }
}

#[cfg(target_os = "macos")]
fn os_release() -> String {
    const KERN_OSRELEASE: &[u8] = b"kern.osrelease\0";
    // sysctlbyname lives in libSystem, which std already links on macOS.
    extern "C" {
        fn sysctlbyname(
            name: *const u8,
            oldp: *mut std::ffi::c_void,
            oldlenp: *mut usize,
            newp: *mut std::ffi::c_void,
            newlen: usize,
        ) -> i32;
    }
    let mut len = 0usize;
    // SAFETY: name is NUL-terminated; the size query accepts null buffers.
    let status = unsafe {
        sysctlbyname(
            KERN_OSRELEASE.as_ptr(),
            std::ptr::null_mut(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if status != 0 || len == 0 {
        return "unknown".to_string();
    }
    let mut buf = vec![0u8; len];
    // SAFETY: buf has `len` writable bytes and oldlenp points at that length.
    let status = unsafe {
        sysctlbyname(
            KERN_OSRELEASE.as_ptr(),
            buf.as_mut_ptr() as *mut std::ffi::c_void,
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if status != 0 {
        return "unknown".to_string();
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(len);
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

#[cfg(target_os = "linux")]
fn os_release() -> String {
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|release| release.trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn os_release() -> String {
    "unknown".to_string()
}

#[cfg(test)]
pub(crate) mod abort_test_support {
    //! Shared test server for the abort-surface plumbing tests: a raw TCP
    //! listener that answers one request with SSE response headers and then
    //! holds the socket open without body bytes, so a cancellation during the
    //! body wait can only exit through the per-API abort path (no event ever
    //! arrives and the stream never ends on its own).

    /// Spawns the server and resolves with its base URL.
    pub(crate) async fn stalled_sse_server() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let mut buf = vec![0u8; 8192];
            let _ = socket.read(&mut buf).await;
            let _ = socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n",
                )
                .await;
            // Hold the socket open (no body bytes) until the test runtime
            // drops the task.
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
            }
        });
        format!("http://{addr}")
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    //! Shared env-mutating test support: process env is process-global, so
    //! one lock serializes every env-mutating provider test and the saved
    //! values restore on drop (the oracle suites' `stubEnv`/`afterEach`).
    //!
    //! One shared `TestEnv` replaces the per-module copies (bedrock's AWS
    //! surfaces, google_vertex's GCP surfaces); the single lock only orders
    //! them — the var sets are disjoint, so outcomes are unchanged.

    use std::sync::{Mutex, MutexGuard};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    pub(crate) struct TestEnv {
        _lock: MutexGuard<'static, ()>,
        saved: Vec<(&'static str, Option<String>)>,
    }

    impl TestEnv {
        /// Sets `settings`, removes `cleared`, restoring everything on drop.
        pub(crate) fn apply(settings: &[(&'static str, &str)], cleared: &[&'static str]) -> Self {
            let lock = ENV_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let mut saved = Vec::new();
            for (name, value) in settings {
                saved.push((*name, std::env::var(name).ok()));
                std::env::set_var(name, value);
            }
            for name in cleared {
                saved.push((*name, std::env::var(name).ok()));
                std::env::remove_var(name);
            }
            TestEnv { _lock: lock, saved }
        }
    }

    impl Drop for TestEnv {
        fn drop(&mut self) {
            for (name, value) in &self.saved {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{http_client, os_arch, os_platform, pi_user_agent, pi_user_agent_from, ApiImpl};
    use crate::ai::transcript::TranscriptContext;
    use crate::ai::types::events::AssistantMessageEvent;
    use crate::ai::types::options::{SimpleStreamOptions, StreamOptions};
    use crate::ai::types::{Model, ModelInput};
    use crate::ai::ProviderConfig;
    use tokio::sync::mpsc;

    fn model() -> Model {
        Model {
            r#type: None,
            prompt_cache: None,
            input_limits: None,
            id: "test-model".to_string(),
            name: "Test Model".to_string(),
            api: "openai-completions".to_string(),
            provider: "openai".to_string(),
            base_url: "https://api.example.com".to_string(),
            reasoning: false,
            thinking_level_map: None,
            input: vec![ModelInput::Text],
            cost: crate::ai::types::ModelCost::default(),
            context_window: 100000,
            max_tokens: 4096,
            sampling_params: None,
            sampling_params_by_thinking_level: None,
            headers: None,
            compat: None,
        }
    }

    fn cfg() -> ProviderConfig {
        ProviderConfig {
            base_url: "https://api.example.com".to_string(),
            api_key: "k".to_string(),
            max_tokens: 4096,
        }
    }

    /// The trait must be usable as `dyn ApiImpl` with exactly the brief's
    /// signatures (`cfg, model, ctx, options`); this pins both the method
    /// shapes and object safety for the provider registries built on it.
    #[test]
    fn api_impl_is_object_safe_with_upstream_signature() {
        struct Stub;

        impl ApiImpl for Stub {
            fn stream(
                &self,
                _cfg: &ProviderConfig,
                _model: &Model,
                _ctx: &TranscriptContext,
                _options: &StreamOptions,
            ) -> mpsc::Receiver<AssistantMessageEvent> {
                let (tx, rx) = mpsc::channel(1);
                drop(tx);
                rx
            }

            fn stream_simple(
                &self,
                _cfg: &ProviderConfig,
                _model: &Model,
                _ctx: &TranscriptContext,
                _options: &SimpleStreamOptions,
            ) -> mpsc::Receiver<AssistantMessageEvent> {
                let (tx, rx) = mpsc::channel(1);
                drop(tx);
                rx
            }
        }

        let stub: Box<dyn ApiImpl> = Box::new(Stub);
        let ctx = TranscriptContext::default();
        let options = StreamOptions::default();
        let mut rx = stub.stream(&cfg(), &model(), &ctx, &options);
        assert!(rx.try_recv().is_err(), "closed channel yields no events");

        let options = SimpleStreamOptions::default();
        let mut rx = stub.stream_simple(&cfg(), &model(), &ctx, &options);
        assert!(rx.try_recv().is_err(), "closed channel yields no events");
    }

    /// Upstream `getPiUserAgent` (`packages/ai/src/utils/pi-user-agent.ts`)
    /// emits `pi ({platform} {release}; {arch})` with Node `os` names; pin
    /// the format byte-for-byte.
    #[test]
    fn pi_user_agent_matches_upstream_format() {
        assert_eq!(
            pi_user_agent_from("win32", "10.0.26200", "x64"),
            "pi (win32 10.0.26200; x64)"
        );
        assert_eq!(
            pi_user_agent_from("darwin", "24.3.0", "arm64"),
            "pi (darwin 24.3.0; arm64)"
        );
        assert_eq!(
            pi_user_agent_from("linux", "6.8.0-45-generic", "x64"),
            "pi (linux 6.8.0-45-generic; x64)"
        );
    }

    /// At runtime the UA must carry the real platform/arch names (Node
    /// spellings) and a non-empty OS release on the three mainstream hosts.
    #[test]
    fn pi_user_agent_reflects_runtime_os() {
        let ua = pi_user_agent();
        let inner = ua
            .strip_prefix("pi (")
            .unwrap_or_else(|| panic!("UA must open with `pi (`: {ua}"))
            .strip_suffix(')')
            .unwrap_or_else(|| panic!("UA must close with `)`: {ua}"));
        let (platform_release, arch) = inner
            .split_once("; ")
            .unwrap_or_else(|| panic!("UA must contain `; `: {ua}"));
        let (platform, release) = platform_release
            .split_once(' ')
            .unwrap_or_else(|| panic!("UA must contain platform and release: {ua}"));

        assert_eq!(platform, os_platform());
        assert_eq!(arch, os_arch());
        if cfg!(any(windows, target_os = "macos", target_os = "linux")) {
            assert!(!release.is_empty() && release != "unknown", "UA: {ua}");
        }
        if cfg!(windows) {
            let parts: Vec<&str> = release.split('.').collect();
            assert_eq!(parts.len(), 3, "Windows release is major.minor.build: {ua}");
            assert!(
                parts
                    .iter()
                    .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())),
                "UA: {ua}"
            );
        }
    }

    /// The shared client factory must build (and keep building on repeated
    /// calls — it caches one client behind a `OnceLock`).
    #[test]
    fn http_client_builds_repeatedly() {
        let first = http_client();
        let second = http_client();
        // Both must be usable; reqwest::Client has no identity comparison,
        // so exercise them and pin that neither construction path panics.
        let _ = first.get("https://example.invalid").build().unwrap();
        let _ = second.get("https://example.invalid").build().unwrap();
    }

    /// The shared client sends the upstream pi User-Agent header.
    #[tokio::test]
    async fn http_client_sends_pi_user_agent() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let client = http_client();
        let status = client.get(server.uri()).send().await.unwrap().status();
        assert_eq!(status, 200);

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        let ua = requests[0]
            .headers
            .get("user-agent")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(ua, pi_user_agent());
    }
}
