//! Tests for [`crate::coding_agent::utils::clipboard`] — the port of upstream
//! `clipboard.ts` / `clipboard-command.ts` (subprocess route).
//!
//! Three layers, mirroring the upstream suites:
//!
//! 1. Scripted-runner flow tests for `readClipboardText`/`copyToClipboard`
//!    (the upstream suite mocks `runClipboardCommand` exactly the same way)
//!    with argv sequences, options, error texts and OSC 52 bytes pinned to the
//!    oracle captured from the vendored byte-identical upstream sources under
//!    node (`tests/fixtures/clipboard_oracle/clipboard.oracle.json`, capture script
//!    `capture_clipboard_oracle.mjs`). Oracle scenario ids are cited in
//!    comments.
//! 2. Real-subprocess integration tests of [`RealClipboardCommandRunner`]
//!    against `node -e` probes (the upstream clipboard-command.test.ts
//!    fixtures), `cmd /c echo`, and a real Windows `clip` write (sanctioned by
//!    the M4 slice; verified through `Get-Clipboard`).
//! 3. Constant and helper pins.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;

use super::{
    base64_encode, copy_to_clipboard_with, emit_osc52_with, env_flag, is_remote_session,
    read_clipboard_text_with, ClipboardCommandOptions, ClipboardCommandRunner, ClipboardEnv,
    ClipboardPlatform, DEFAULT_CLIPBOARD_MAX_BUFFER_BYTES, DEFAULT_CLIPBOARD_TIMEOUT_MS,
    MAX_OSC52_ENCODED_LENGTH, READ_CLIPBOARD_TIMEOUT_MS, WRITE_CLIPBOARD_TIMEOUT_MS,
};

// ---------------------------------------------------------------------------
// Oracle-pinned values (tests/fixtures/clipboard_oracle/clipboard.oracle.json).
// ---------------------------------------------------------------------------

/// Oracle `copy:linux-fail-termux-error` / `copy:linux-fail-wayland-error` /
/// `copy:linux-fail-x11-error` / `copy:linux-fail-no-display-error` /
/// `copy:darwin-native-fail-pbcopy-fail` (and `copy:other-platform-generic-error`).
const ERR_TERMUX: &str =
    "Clipboard unavailable: install the Termux:API app and `termux-api` package";
const ERR_WAYLAND: &str =
    "Clipboard unavailable: install `wl-clipboard` (`wl-copy`) or check Wayland access";
const ERR_X11: &str = "Clipboard unavailable: install `xclip` or `xsel`, or check X11 access";
const ERR_NO_DISPLAY: &str = "Clipboard unavailable: no Wayland or X11 display detected";
const ERR_GENERIC: &str = "Clipboard unavailable";

/// Oracle `copy:*` osc52 captures for "hello": `ESC]52;c;aGVsbG8=BEL` (16
/// bytes, base64 `G101MjtjO2FHVnNiRzg9Bw==` in the JSON).
const OSC52_HELLO: &str = "\x1b]52;c;aGVsbG8=\x07";

// ---------------------------------------------------------------------------
// Test doubles.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
struct Call {
    command: String,
    args: Vec<String>,
    input: Option<String>,
    timeout_ms: Option<u64>,
    max_buffer_bytes: Option<usize>,
}

/// The upstream suite's `mocks.command`: records (command, args, options) and
/// returns a per-command scripted result.
#[derive(Default)]
struct ScriptedRunner {
    calls: Mutex<Vec<Call>>,
    outcomes: Mutex<HashMap<&'static str, Option<Vec<u8>>>>,
}

impl ScriptedRunner {
    fn with_outcomes(outcomes: HashMap<&'static str, Option<Vec<u8>>>) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            outcomes: Mutex::new(outcomes),
        }
    }

    fn ok(command: &'static str, text: &str) -> (&'static str, Option<Vec<u8>>) {
        (command, Some(text.as_bytes().to_vec()))
    }

    fn fail(command: &'static str) -> (&'static str, Option<Vec<u8>>) {
        (command, None)
    }

    fn recorded(&self) -> Vec<Call> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl ClipboardCommandRunner for ScriptedRunner {
    fn run_clipboard_command<'a>(
        &'a self,
        command: &'a str,
        args: &'a [String],
        options: ClipboardCommandOptions,
    ) -> BoxFuture<'a, Option<Vec<u8>>> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Call {
                command: command.to_string(),
                args: args.to_vec(),
                input: options.input.clone(),
                timeout_ms: options.timeout_ms,
                max_buffer_bytes: options.max_buffer_bytes,
            });
        let outcome = self
            .outcomes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(command)
            .cloned()
            .flatten();
        Box::pin(async move { outcome })
    }
}

#[derive(Clone, Default)]
struct RecordingOsc52 {
    sequences: Arc<Mutex<Vec<String>>>,
}

impl RecordingOsc52 {
    fn captured(&self) -> Vec<String> {
        self.sequences
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl super::Osc52Writer for RecordingOsc52 {
    fn write_osc52(&self, sequence: &str) {
        self.sequences
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(sequence.to_string());
    }
}

fn env(platform: ClipboardPlatform, truthy: &[&str]) -> ClipboardEnv {
    let mut env = ClipboardEnv {
        platform,
        ..ClipboardEnv::default()
    };
    for name in truthy {
        match *name {
            "TERMUX_VERSION" => env.termux_version = true,
            "WAYLAND_DISPLAY" => env.wayland_display = true,
            "DISPLAY" => env.display = true,
            "SSH_CONNECTION" => env.ssh_connection = true,
            "SSH_CLIENT" => env.ssh_client = true,
            "MOSH_CONNECTION" => env.mosh_connection = true,
            other => panic!("unknown env probe {other}"),
        }
    }
    env
}

fn assert_read_options(call: &Call) {
    assert_eq!(call.input, None, "reads must not carry stdin input");
    assert_eq!(call.timeout_ms, Some(READ_CLIPBOARD_TIMEOUT_MS));
    assert_eq!(call.max_buffer_bytes, None);
}

fn assert_write_options(call: &Call, text: &str) {
    assert_eq!(call.input.as_deref(), Some(text));
    assert_eq!(call.timeout_ms, Some(WRITE_CLIPBOARD_TIMEOUT_MS));
    assert_eq!(call.max_buffer_bytes, None);
}

fn args(command: &Call) -> &[String] {
    &command.args
}

fn str_args(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

// ---------------------------------------------------------------------------
// Constants and helpers.
// ---------------------------------------------------------------------------

#[test]
fn constants_match_upstream() {
    assert_eq!(MAX_OSC52_ENCODED_LENGTH, 100_000);
    assert_eq!(DEFAULT_CLIPBOARD_TIMEOUT_MS, 3_000);
    assert_eq!(DEFAULT_CLIPBOARD_MAX_BUFFER_BYTES, 50 * 1024 * 1024);
    // clipboard.ts lines 30 and 69.
    assert_eq!(READ_CLIPBOARD_TIMEOUT_MS, 5_000);
    assert_eq!(WRITE_CLIPBOARD_TIMEOUT_MS, 5_000);
}

#[test]
fn emits_exact_osc52_bytes_for_hello() {
    let osc52 = RecordingOsc52::default();
    assert!(emit_osc52_with("hello", &osc52));
    assert_eq!(osc52.captured(), vec![OSC52_HELLO.to_string()]);
}

#[test]
fn rejects_oversized_osc52_payloads() {
    // Oracle `copy:darwin-remote-osc52-oversize`: 80_000 'x' → base64
    // 106_668 > 100_000; nothing written, generic error surfaces.
    let osc52 = RecordingOsc52::default();
    assert!(!emit_osc52_with(&"x".repeat(80_000), &osc52));
    assert!(osc52.captured().is_empty());
    // Oracle `copy:darwin-remote-osc52-boundary-over`: 75_001 'a' → 100_004.
    assert!(!emit_osc52_with(&"a".repeat(75_001), &osc52));
    assert!(osc52.captured().is_empty());
}

#[test]
fn accepts_osc52_payload_at_the_exact_boundary() {
    // Oracle `copy:darwin-remote-osc52-boundary-exact`: 75_000 'a' → base64
    // length exactly 100_000 → emitted; sequence is 7 (prefix) + 100_000 + 1
    // (BEL) bytes.
    let osc52 = RecordingOsc52::default();
    let text = "a".repeat(75_000);
    assert!(emit_osc52_with(&text, &osc52));
    let captured = osc52.captured();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].len(), 7 + 100_000 + 1);
    assert!(captured[0].starts_with("\x1b]52;c;YWFh"));
    assert!(captured[0].ends_with('\x07'));
    assert_eq!(&captured[0][7..7 + 100_000], base64_encode(text.as_bytes()));
    assert_eq!(base64_encode(text.as_bytes()).len(), 100_000);
}

#[test]
fn base64_encode_matches_node_buffer_semantics() {
    // `Buffer.from("hello").toString("base64")` from the oracle.
    assert_eq!(base64_encode(b"hello"), "aGVsbG8=");
    assert_eq!(base64_encode(b""), "");
    assert_eq!(base64_encode(b"a"), "YQ==");
    assert_eq!(base64_encode(b"ab"), "YWI=");
    assert_eq!(base64_encode(b"abc"), "YWJj");
    assert_eq!(base64_encode(&[0, 255, 10]), "AP8K");
}

#[test]
fn remote_session_truthiness() {
    // Oracle scenarios set SSH_CONNECTION/SSH_CLIENT/MOSH_CONNECTION; unset
    // (or empty, which `env_flag` folds to false like JS truthiness) means
    // local.
    assert!(is_remote_session(&env(
        ClipboardPlatform::Darwin,
        &["SSH_CONNECTION"]
    )));
    assert!(is_remote_session(&env(
        ClipboardPlatform::Darwin,
        &["SSH_CLIENT"]
    )));
    assert!(is_remote_session(&env(
        ClipboardPlatform::Darwin,
        &["MOSH_CONNECTION"]
    )));
    assert!(!is_remote_session(&env(ClipboardPlatform::Darwin, &[])));
}

#[test]
fn env_flag_folds_empty_strings_to_false() {
    // JS `Boolean(process.env.X)`: empty string is falsy. The probe variable
    // is unique to this test, so parallel tests cannot observe the mutation.
    std::env::set_var("PI_CLIPBOARD_TEST_FLAG", "");
    assert!(!env_flag("PI_CLIPBOARD_TEST_FLAG"));
    std::env::set_var("PI_CLIPBOARD_TEST_FLAG", "1");
    assert!(env_flag("PI_CLIPBOARD_TEST_FLAG"));
    std::env::remove_var("PI_CLIPBOARD_TEST_FLAG");
    assert!(!env_flag("PI_CLIPBOARD_TEST_FLAG"));
}

#[test]
fn system_env_reports_the_current_platform() {
    let system = ClipboardEnv::from_system();
    let expected = if cfg!(target_os = "linux") {
        ClipboardPlatform::Linux
    } else if cfg!(target_os = "macos") {
        ClipboardPlatform::Darwin
    } else if cfg!(windows) {
        ClipboardPlatform::Win32
    } else {
        ClipboardPlatform::Other
    };
    assert_eq!(system.platform, expected);
}

// ---------------------------------------------------------------------------
// readClipboardText flows (scripted runner, oracle-pinned).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn read_linux_termux_success_stops_fallback() {
    // Oracle `read:linux-termux-ok`.
    let runner = ScriptedRunner::with_outcomes(HashMap::from([ScriptedRunner::ok(
        "termux-clipboard-get",
        "clipboard text",
    )]));
    let text =
        read_clipboard_text_with(&env(ClipboardPlatform::Linux, &["TERMUX_VERSION"]), &runner)
            .await;
    assert_eq!(text.as_deref(), Some("clipboard text"));
    let calls = runner.recorded();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].command, "termux-clipboard-get");
    assert!(args(&calls[0]).is_empty());
    assert_read_options(&calls[0]);
}

#[tokio::test]
async fn read_linux_termux_empty_success_stops_without_native() {
    // Oracle `read:linux-termux-empty-stops`: empty content resolves null and
    // does NOT fall through (upstream `bytes.toString("utf8") || null`).
    let runner = ScriptedRunner::with_outcomes(HashMap::from([ScriptedRunner::ok(
        "termux-clipboard-get",
        "",
    )]));
    let text =
        read_clipboard_text_with(&env(ClipboardPlatform::Linux, &["TERMUX_VERSION"]), &runner)
            .await;
    assert_eq!(text, None);
    assert_eq!(runner.recorded().len(), 1);
}

#[tokio::test]
async fn read_linux_wayland_success_stops_fallback() {
    // Oracle `read:linux-wayland-ok`.
    let runner = ScriptedRunner::with_outcomes(HashMap::from([ScriptedRunner::ok(
        "wl-paste",
        "wayland text",
    )]));
    let text = read_clipboard_text_with(
        &env(ClipboardPlatform::Linux, &["WAYLAND_DISPLAY"]),
        &runner,
    )
    .await;
    assert_eq!(text.as_deref(), Some("wayland text"));
    let calls = runner.recorded();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].command, "wl-paste");
    assert_eq!(
        args(&calls[0]),
        str_args(&["--no-newline", "--type", "text"])
    );
    assert_read_options(&calls[0]);
}

#[tokio::test]
async fn read_linux_wayland_empty_does_not_fall_through_to_x11() {
    // Oracle `read:linux-wayland-empty-stops` (upstream regression #7248):
    // empty Wayland content must not return stale X11 clipboard contents.
    let runner = ScriptedRunner::with_outcomes(HashMap::from([
        ScriptedRunner::ok("wl-paste", ""),
        ScriptedRunner::ok("xclip", "stale x11"),
    ]));
    let text = read_clipboard_text_with(
        &env(ClipboardPlatform::Linux, &["WAYLAND_DISPLAY", "DISPLAY"]),
        &runner,
    )
    .await;
    assert_eq!(text, None);
    let calls = runner.recorded();
    assert_eq!(
        calls
            .iter()
            .map(|call| call.command.as_str())
            .collect::<Vec<_>>(),
        vec!["wl-paste"]
    );
}

#[tokio::test]
async fn read_linux_xclip_success_stops_fallback() {
    // Oracle `read:linux-xclip-ok`.
    let runner =
        ScriptedRunner::with_outcomes(HashMap::from([ScriptedRunner::ok("xclip", "x11 text")]));
    let text =
        read_clipboard_text_with(&env(ClipboardPlatform::Linux, &["DISPLAY"]), &runner).await;
    assert_eq!(text.as_deref(), Some("x11 text"));
    let calls = runner.recorded();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].command, "xclip");
    assert_eq!(
        args(&calls[0]),
        str_args(&["-selection", "clipboard", "-out"])
    );
}

#[tokio::test]
async fn read_linux_xclip_empty_stops_before_xsel() {
    // Oracle `read:linux-xclip-empty-stops`.
    let runner = ScriptedRunner::with_outcomes(HashMap::from([
        ScriptedRunner::ok("xclip", ""),
        ScriptedRunner::ok("xsel", "xsel text"),
    ]));
    let text =
        read_clipboard_text_with(&env(ClipboardPlatform::Linux, &["DISPLAY"]), &runner).await;
    assert_eq!(text, None);
    assert_eq!(
        runner
            .recorded()
            .iter()
            .map(|call| call.command.as_str())
            .collect::<Vec<_>>(),
        vec!["xclip"]
    );
}

#[tokio::test]
async fn read_linux_xclip_failure_falls_back_to_xsel() {
    // Oracle `read:linux-xclip-fail-xsel-ok`.
    let runner = ScriptedRunner::with_outcomes(HashMap::from([
        ScriptedRunner::fail("xclip"),
        ScriptedRunner::ok("xsel", "X11 text"),
    ]));
    let text =
        read_clipboard_text_with(&env(ClipboardPlatform::Linux, &["DISPLAY"]), &runner).await;
    assert_eq!(text.as_deref(), Some("X11 text"));
    let calls = runner.recorded();
    assert_eq!(
        calls
            .iter()
            .map(|call| call.command.as_str())
            .collect::<Vec<_>>(),
        vec!["xclip", "xsel"]
    );
    assert_eq!(args(&calls[1]), str_args(&["--clipboard", "--output"]));
    assert_read_options(&calls[1]);
}

#[tokio::test]
async fn read_linux_all_failures_resolve_null_without_native() {
    // Oracle `read:linux-all-fail-native-fallback` resolves upstream's native
    // X11 text; divergence 1c pins the port to null with the native layer
    // absent. The command chain itself matches: wl-paste, xclip, xsel.
    let runner = ScriptedRunner::with_outcomes(HashMap::from([
        ScriptedRunner::fail("wl-paste"),
        ScriptedRunner::fail("xclip"),
        ScriptedRunner::fail("xsel"),
    ]));
    let text = read_clipboard_text_with(
        &env(ClipboardPlatform::Linux, &["WAYLAND_DISPLAY", "DISPLAY"]),
        &runner,
    )
    .await;
    assert_eq!(text, None);
    assert_eq!(
        runner
            .recorded()
            .iter()
            .map(|call| call.command.as_str())
            .collect::<Vec<_>>(),
        vec!["wl-paste", "xclip", "xsel"]
    );
}

#[tokio::test]
async fn read_linux_without_display_probes_spawns_nothing() {
    // Oracle `read:linux-no-display-commands`.
    let runner = ScriptedRunner::with_outcomes(HashMap::new());
    let text = read_clipboard_text_with(&env(ClipboardPlatform::Linux, &[]), &runner).await;
    assert_eq!(text, None);
    assert!(runner.recorded().is_empty());
}

#[tokio::test]
async fn read_win32_is_not_implemented_divergence_1b() {
    // Oracle `read:win32-native-only` resolves the native text; the port has
    // no native layer on win32 and spawns nothing.
    let runner = ScriptedRunner::with_outcomes(HashMap::new());
    let text =
        read_clipboard_text_with(&env(ClipboardPlatform::Win32, &["DISPLAY"]), &runner).await;
    assert_eq!(text, None);
    assert!(runner.recorded().is_empty());
}

#[tokio::test]
async fn read_darwin_uses_the_pbpaste_supplement() {
    // Divergence 2 (additive; no upstream oracle): darwin reads pbpaste with
    // the Linux reads' 5s timeout and empty→null stop semantics.
    let runner = ScriptedRunner::with_outcomes(HashMap::from([ScriptedRunner::ok(
        "pbpaste",
        "pbpaste text",
    )]));
    let text = read_clipboard_text_with(&env(ClipboardPlatform::Darwin, &[]), &runner).await;
    assert_eq!(text.as_deref(), Some("pbpaste text"));
    let calls = runner.recorded();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].command, "pbpaste");
    assert!(args(&calls[0]).is_empty());
    assert_read_options(&calls[0]);
}

#[tokio::test]
async fn read_darwin_pbpaste_empty_and_failure_resolve_null() {
    let empty = ScriptedRunner::with_outcomes(HashMap::from([ScriptedRunner::ok("pbpaste", "")]));
    assert_eq!(
        read_clipboard_text_with(&env(ClipboardPlatform::Darwin, &[]), &empty).await,
        None
    );
    let failing = ScriptedRunner::with_outcomes(HashMap::from([ScriptedRunner::fail("pbpaste")]));
    assert_eq!(
        read_clipboard_text_with(&env(ClipboardPlatform::Darwin, &[]), &failing).await,
        None
    );
}

// ---------------------------------------------------------------------------
// copyToClipboard flows (scripted runner, oracle-pinned).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn copy_linux_xclip_success_stops_chain() {
    // Oracle `copy:linux-xclip-ok` (Linux skips the native writer upstream
    // too; `setText` never called — nativeCalls: []).
    let runner = ScriptedRunner::with_outcomes(HashMap::from([ScriptedRunner::ok("xclip", "")]));
    let osc52 = RecordingOsc52::default();
    let result = copy_to_clipboard_with(
        &env(ClipboardPlatform::Linux, &["DISPLAY"]),
        &runner,
        &osc52,
        "hello",
    )
    .await;
    assert_eq!(result, Ok(()));
    let calls = runner.recorded();
    assert_eq!(
        calls
            .iter()
            .map(|call| call.command.as_str())
            .collect::<Vec<_>>(),
        vec!["xclip"]
    );
    assert_eq!(args(&calls[0]), str_args(&["-selection", "clipboard"]));
    assert_write_options(&calls[0], "hello");
    assert!(osc52.captured().is_empty());
}

#[tokio::test]
async fn copy_linux_tries_xclip_and_xsel_after_wl_copy_fails() {
    // Oracle `copy:linux-wl-copy-fail-xsel-ok`.
    let runner = ScriptedRunner::with_outcomes(HashMap::from([
        ScriptedRunner::fail("wl-copy"),
        ScriptedRunner::fail("xclip"),
        ScriptedRunner::ok("xsel", ""),
    ]));
    let osc52 = RecordingOsc52::default();
    let result = copy_to_clipboard_with(
        &env(ClipboardPlatform::Linux, &["WAYLAND_DISPLAY", "DISPLAY"]),
        &runner,
        &osc52,
        "hello",
    )
    .await;
    assert_eq!(result, Ok(()));
    let calls = runner.recorded();
    assert_eq!(
        calls
            .iter()
            .map(|call| call.command.as_str())
            .collect::<Vec<_>>(),
        vec!["wl-copy", "xclip", "xsel"]
    );
    assert_eq!(args(&calls[2]), str_args(&["--clipboard", "--input"]));
    assert_write_options(&calls[2], "hello");
    assert!(osc52.captured().is_empty());
}

#[tokio::test]
async fn copy_linux_failure_reports_the_x11_tools() {
    // Oracle `copy:linux-fail-x11-error` (upstream #9618 regression: no
    // unverified OSC 52 on local failure).
    let runner = ScriptedRunner::with_outcomes(HashMap::from([
        ScriptedRunner::fail("xclip"),
        ScriptedRunner::fail("xsel"),
    ]));
    let osc52 = RecordingOsc52::default();
    let result = copy_to_clipboard_with(
        &env(ClipboardPlatform::Linux, &["DISPLAY"]),
        &runner,
        &osc52,
        "hello",
    )
    .await;
    assert_eq!(result.unwrap_err().message(), ERR_X11);
    assert_eq!(
        runner
            .recorded()
            .iter()
            .map(|call| call.command.as_str())
            .collect::<Vec<_>>(),
        vec!["xclip", "xsel"]
    );
    assert!(osc52.captured().is_empty());
}

#[tokio::test]
async fn copy_linux_failure_reports_the_wayland_tool() {
    // Oracle `copy:linux-fail-wayland-error`.
    let runner = ScriptedRunner::with_outcomes(HashMap::from([
        ScriptedRunner::fail("wl-copy"),
        ScriptedRunner::fail("xclip"),
        ScriptedRunner::fail("xsel"),
    ]));
    let result = copy_to_clipboard_with(
        &env(ClipboardPlatform::Linux, &["WAYLAND_DISPLAY", "DISPLAY"]),
        &runner,
        &RecordingOsc52::default(),
        "hello",
    )
    .await;
    assert_eq!(result.unwrap_err().message(), ERR_WAYLAND);
    assert_eq!(
        runner
            .recorded()
            .iter()
            .map(|call| call.command.as_str())
            .collect::<Vec<_>>(),
        vec!["wl-copy", "xclip", "xsel"]
    );
}

#[tokio::test]
async fn copy_linux_failure_reports_the_termux_app() {
    // Oracle `copy:linux-fail-termux-error`.
    let runner = ScriptedRunner::with_outcomes(HashMap::from([ScriptedRunner::fail(
        "termux-clipboard-set",
    )]));
    let result = copy_to_clipboard_with(
        &env(ClipboardPlatform::Linux, &["TERMUX_VERSION"]),
        &runner,
        &RecordingOsc52::default(),
        "hello",
    )
    .await;
    assert_eq!(result.unwrap_err().message(), ERR_TERMUX);
    assert_eq!(
        runner
            .recorded()
            .iter()
            .map(|call| call.command.as_str())
            .collect::<Vec<_>>(),
        vec!["termux-clipboard-set"]
    );
}

#[tokio::test]
async fn copy_linux_failure_without_displays_spawns_nothing() {
    // Oracle `copy:linux-fail-no-display-error`.
    let runner = ScriptedRunner::with_outcomes(HashMap::new());
    let result = copy_to_clipboard_with(
        &env(ClipboardPlatform::Linux, &[]),
        &runner,
        &RecordingOsc52::default(),
        "hello",
    )
    .await;
    assert_eq!(result.unwrap_err().message(), ERR_NO_DISPLAY);
    assert!(runner.recorded().is_empty());
}

#[tokio::test]
async fn copy_linux_failure_uses_osc52_in_remote_sessions() {
    // Oracle `copy:linux-fail-remote-osc52-saves`: byte-exact fallback write.
    let runner = ScriptedRunner::with_outcomes(HashMap::from([
        ScriptedRunner::fail("xclip"),
        ScriptedRunner::fail("xsel"),
    ]));
    let osc52 = RecordingOsc52::default();
    let result = copy_to_clipboard_with(
        &env(ClipboardPlatform::Linux, &["DISPLAY", "SSH_CONNECTION"]),
        &runner,
        &osc52,
        "hello",
    )
    .await;
    assert_eq!(result, Ok(()));
    assert_eq!(osc52.captured(), vec![OSC52_HELLO.to_string()]);
}

#[tokio::test]
async fn copy_success_still_emits_osc52_when_remote() {
    // Oracle `copy:darwin-native-ok-remote-osc52`: upstream emits OSC 52 even
    // after a successful write in a remote session.
    let runner = ScriptedRunner::with_outcomes(HashMap::from([ScriptedRunner::ok("pbcopy", "")]));
    let osc52 = RecordingOsc52::default();
    let result = copy_to_clipboard_with(
        &env(ClipboardPlatform::Darwin, &["SSH_CONNECTION"]),
        &runner,
        &osc52,
        "hello",
    )
    .await;
    assert_eq!(result, Ok(()));
    assert_eq!(osc52.captured(), vec![OSC52_HELLO.to_string()]);
}

#[tokio::test]
async fn copy_darwin_uses_pbcopy() {
    // Oracle `copy:darwin-native-fail-pbcopy-ok` (native failure fallthrough
    // is the port's only path — divergence 1a).
    let runner = ScriptedRunner::with_outcomes(HashMap::from([ScriptedRunner::ok("pbcopy", "")]));
    let osc52 = RecordingOsc52::default();
    let result = copy_to_clipboard_with(
        &env(ClipboardPlatform::Darwin, &[]),
        &runner,
        &osc52,
        "hello",
    )
    .await;
    assert_eq!(result, Ok(()));
    let calls = runner.recorded();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].command, "pbcopy");
    assert!(args(&calls[0]).is_empty());
    assert_write_options(&calls[0], "hello");
    assert!(osc52.captured().is_empty());
}

#[tokio::test]
async fn copy_darwin_failure_reports_generic_unavailable() {
    // Oracle `copy:darwin-native-fail-pbcopy-fail`.
    let runner = ScriptedRunner::with_outcomes(HashMap::from([ScriptedRunner::fail("pbcopy")]));
    let result = copy_to_clipboard_with(
        &env(ClipboardPlatform::Darwin, &[]),
        &runner,
        &RecordingOsc52::default(),
        "hello",
    )
    .await;
    assert_eq!(result.unwrap_err().message(), ERR_GENERIC);
}

#[tokio::test]
async fn copy_win32_uses_clip() {
    // Oracle `copy:win32-clip-ok`.
    let runner = ScriptedRunner::with_outcomes(HashMap::from([ScriptedRunner::ok("clip", "")]));
    let osc52 = RecordingOsc52::default();
    let result = copy_to_clipboard_with(
        &env(ClipboardPlatform::Win32, &[]),
        &runner,
        &osc52,
        "hello",
    )
    .await;
    assert_eq!(result, Ok(()));
    let calls = runner.recorded();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].command, "clip");
    assert!(args(&calls[0]).is_empty());
    assert_write_options(&calls[0], "hello");
    assert!(osc52.captured().is_empty());
}

#[tokio::test]
async fn copy_win32_failure_reports_generic_unavailable() {
    // Oracle `copy:win32-fail-remote-osc52` minus the remote session.
    let runner = ScriptedRunner::with_outcomes(HashMap::from([ScriptedRunner::fail("clip")]));
    let result = copy_to_clipboard_with(
        &env(ClipboardPlatform::Win32, &[]),
        &runner,
        &RecordingOsc52::default(),
        "hello",
    )
    .await;
    assert_eq!(result.unwrap_err().message(), ERR_GENERIC);
}

#[tokio::test]
async fn copy_other_platform_reports_generic_unavailable() {
    // Oracle `copy:other-platform-generic-error` (freebsd, no env probes).
    let runner = ScriptedRunner::with_outcomes(HashMap::new());
    let result = copy_to_clipboard_with(
        &env(ClipboardPlatform::Other, &[]),
        &runner,
        &RecordingOsc52::default(),
        "hello",
    )
    .await;
    assert_eq!(result.unwrap_err().message(), ERR_GENERIC);
    assert!(runner.recorded().is_empty());
}

#[tokio::test]
async fn copy_remote_does_not_emit_oversized_osc52_and_fails() {
    // Oracle `copy:darwin-remote-osc52-oversize`: 80_000 'x' → base64
    // 106_668 > 100_000 → nothing written → "Clipboard unavailable".
    let runner = ScriptedRunner::with_outcomes(HashMap::from([ScriptedRunner::fail("pbcopy")]));
    let osc52 = RecordingOsc52::default();
    let result = copy_to_clipboard_with(
        &env(ClipboardPlatform::Darwin, &["SSH_CONNECTION"]),
        &runner,
        &osc52,
        &"x".repeat(80_000),
    )
    .await;
    assert_eq!(result.unwrap_err().message(), ERR_GENERIC);
    assert!(osc52.captured().is_empty());
    // The failed write attempt still received the full payload on stdin
    // (oracle stdinEnds: len 80000).
    assert_eq!(
        runner.recorded()[0].input.as_deref().map(str::len),
        Some(80_000)
    );
}

#[tokio::test]
async fn copy_remote_boundary_exact_payload_is_emitted() {
    // Oracle `copy:darwin-remote-osc52-boundary-exact`.
    let runner = ScriptedRunner::with_outcomes(HashMap::from([ScriptedRunner::fail("pbcopy")]));
    let osc52 = RecordingOsc52::default();
    let result = copy_to_clipboard_with(
        &env(ClipboardPlatform::Darwin, &["SSH_CLIENT"]),
        &runner,
        &osc52,
        &"a".repeat(75_000),
    )
    .await;
    assert_eq!(result, Ok(()));
    let captured = osc52.captured();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].len(), 7 + 100_000 + 1);
}

#[tokio::test]
async fn copy_remote_boundary_over_payload_is_rejected() {
    // Oracle `copy:darwin-remote-osc52-boundary-over`: 75_001 'a' → 100_004.
    let runner = ScriptedRunner::with_outcomes(HashMap::from([ScriptedRunner::fail("pbcopy")]));
    let osc52 = RecordingOsc52::default();
    let result = copy_to_clipboard_with(
        &env(ClipboardPlatform::Darwin, &["MOSH_CONNECTION"]),
        &runner,
        &osc52,
        &"a".repeat(75_001),
    )
    .await;
    assert_eq!(result.unwrap_err().message(), ERR_GENERIC);
    assert!(osc52.captured().is_empty());
}

// ---------------------------------------------------------------------------
// RealClipboardCommandRunner: real-subprocess integration (upstream
// clipboard-command.test.ts fixtures, node -e).
// ---------------------------------------------------------------------------

fn real_runner() -> super::RealClipboardCommandRunner {
    super::RealClipboardCommandRunner
}

fn read_options(
    timeout_ms: Option<u64>,
    max_buffer_bytes: Option<usize>,
) -> ClipboardCommandOptions {
    ClipboardCommandOptions {
        input: None,
        timeout_ms,
        max_buffer_bytes,
    }
}

#[tokio::test]
async fn real_runner_preserves_binary_output() {
    // Oracle `run:binary-passthrough` (b64 AP8K).
    let output = real_runner()
        .run_clipboard_command(
            "node",
            &[
                "-e".to_string(),
                "process.stdout.write(Buffer.from([0, 255, 10]))".to_string(),
            ],
            read_options(None, None),
        )
        .await;
    assert_eq!(output, Some(vec![0, 255, 10]));
}

#[tokio::test]
async fn real_runner_empty_success_is_not_failure() {
    // Oracle `run:empty-success`.
    let output = real_runner()
        .run_clipboard_command(
            "node",
            &["-e".to_string(), String::new()],
            read_options(None, None),
        )
        .await;
    assert_eq!(output, Some(Vec::new()));
}

#[tokio::test]
async fn real_runner_nonzero_exit_is_failure() {
    // Oracle `run:nonzero-exit`.
    let output = real_runner()
        .run_clipboard_command(
            "node",
            &["-e".to_string(), "process.exit(1)".to_string()],
            read_options(None, None),
        )
        .await;
    assert_eq!(output, None);
}

#[tokio::test]
async fn real_runner_missing_program_is_failure() {
    // Oracle `run:missing-program` (spawn error event path).
    let output = real_runner()
        .run_clipboard_command(
            "pi-clipboard-command-does-not-exist",
            &[],
            read_options(None, None),
        )
        .await;
    assert_eq!(output, None);
}

#[tokio::test]
async fn real_runner_sends_unicode_input_to_writers() {
    // Upstream clipboard-command.test.ts "sends Unicode input to clipboard
    // writers" (oracle `run:unicode-input`): the child exits 0 only when the
    // UTF-8 round-trip is exact.
    let script = "let text = ''; process.stdin.setEncoding('utf8'); process.stdin.on('data', c => text += c); process.stdin.on('end', () => process.exit(text === 'café 日本語' ? 0 : 1));";
    let output = real_runner()
        .run_clipboard_command(
            "node",
            &["-e".to_string(), script.to_string()],
            ClipboardCommandOptions {
                input: Some("café 日本語".to_string()),
                ..ClipboardCommandOptions::default()
            },
        )
        .await;
    assert_eq!(output, Some(Vec::new()));
}

#[tokio::test]
async fn real_runner_ignores_stdout_when_input_is_given() {
    // Upstream stdio triple with input: stdout "ignore" (oracle
    // `run:stdout-ignored-when-input-given`).
    let output = real_runner()
        .run_clipboard_command(
            "node",
            &[
                "-e".to_string(),
                "process.stdout.write('ignored'); process.exit(0)".to_string(),
            ],
            ClipboardCommandOptions {
                input: Some("x".to_string()),
                ..ClipboardCommandOptions::default()
            },
        )
        .await;
    assert_eq!(output, Some(Vec::new()));
}

#[tokio::test]
async fn real_runner_times_out_with_explicit_timeout() {
    // Upstream "times out without blocking the event loop" (oracle
    // `run:timeout-explicit`, 211ms wall).
    let started = Instant::now();
    let output = real_runner()
        .run_clipboard_command(
            "node",
            &["-e".to_string(), "setInterval(() => {}, 1000)".to_string()],
            read_options(Some(200), None),
        )
        .await;
    assert_eq!(output, None);
    assert!(
        started.elapsed() < Duration::from_millis(5000),
        "timeout must abort promptly, took {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn real_runner_rejects_output_above_the_buffer_limit() {
    // Upstream "rejects output above the buffer limit" (oracle
    // `run:max-buffer`): 1024 bytes against a 16-byte cap.
    let output = real_runner()
        .run_clipboard_command(
            "node",
            &[
                "-e".to_string(),
                "process.stdout.write(Buffer.alloc(1024))".to_string(),
            ],
            read_options(None, Some(16)),
        )
        .await;
    assert_eq!(output, None);
}

#[tokio::test]
async fn real_runner_applies_the_default_timeout() {
    // clipboard-command.ts line 30 (`?? 3000`): a hung child with no explicit
    // timeout resolves undefined at the 3s default. The fixture is the same
    // bare node process as the explicit-timeout probe — a `cmd /c` wrapper is
    // deliberately avoided because the wrapped child survives the killed
    // wrapper as an orphan and makes the suite wait out its full lifetime.
    let started = Instant::now();
    let output = real_runner()
        .run_clipboard_command(
            "node",
            &["-e".to_string(), "setInterval(() => {}, 1000)".to_string()],
            read_options(None, None),
        )
        .await;
    assert_eq!(output, None);
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_millis(2500) && elapsed < Duration::from_millis(8000),
        "default timeout must be ~3s, took {elapsed:?}"
    );
}

#[tokio::test]
async fn real_runner_captures_shell_echo_probe() {
    // The M4 slice's `cmd /c echo` probe: stdout bytes survive the spawn
    // pipeline verbatim.
    #[cfg(windows)]
    let output = real_runner()
        .run_clipboard_command(
            "cmd",
            &["/c".to_string(), "echo hello".to_string()],
            read_options(None, None),
        )
        .await;
    #[cfg(unix)]
    let output = real_runner()
        .run_clipboard_command(
            "sh",
            &["-c".to_string(), "echo hello".to_string()],
            read_options(None, None),
        )
        .await;
    // cmd echoes CRLF on windows and sh echoes LF on unix; the probe asserts
    // the bytes survive the spawn pipeline verbatim on each platform.
    #[cfg(windows)]
    assert_eq!(output.as_deref(), Some("hello\r\n".as_bytes()));
    #[cfg(unix)]
    assert_eq!(output.as_deref(), Some("hello\n".as_bytes()));
}

#[cfg(windows)]
#[tokio::test]
async fn real_clip_write_round_trips_through_the_windows_clipboard() {
    // The M4 slice's sanctioned real `clip` write on this machine; the
    // content is verified through PowerShell's Get-Clipboard (ASCII-only to
    // stay independent of clip.exe's codepage handling of piped input).
    let text = "pi-rust clipboard probe 42";
    let result = copy_to_clipboard_with(
        &ClipboardEnv {
            platform: ClipboardPlatform::Win32,
            ..ClipboardEnv::default()
        },
        &real_runner(),
        &super::StdoutOsc52Writer,
        text,
    )
    .await;
    assert_eq!(result, Ok(()));
    let output = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", "Get-Clipboard"])
        .output()
        .expect("powershell Get-Clipboard");
    assert!(output.status.success(), "Get-Clipboard failed: {output:?}");
    let read_back = String::from_utf8_lossy(&output.stdout);
    assert_eq!(read_back.trim_end(), text);
}

#[cfg(windows)]
#[tokio::test]
async fn system_read_on_windows_is_none_divergence_1b() {
    // Real-environment confirmation of divergence 1b on this machine.
    assert_eq!(super::read_clipboard_text().await, None);
}
