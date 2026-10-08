//! Port of upstream `coding-agent` clipboard helpers — the subprocess route of
//! the M4 native-clipboard evaluation (`docs/migration/MIGRATION_STATUS.md`,
//! entry after line 440).
//!
//! Provenance map (upstream file → this module), upstream SHA256 at migration
//! time:
//!
//! | upstream                | surface ported                                        | sha256 |
//! |---|---|---|
//! | `src/utils/clipboard.ts` | [`read_clipboard_text_with`], [`copy_to_clipboard_with`], [`emit_osc52_with`], [`is_remote_session`], [`MAX_OSC52_ENCODED_LENGTH`] | `892f019d5701c733e046306062d597698395592e093117d3541134985bdb3ec6` |
//! | `src/utils/clipboard-command.ts` | [`ClipboardCommandRunner`] / [`RealClipboardCommandRunner`], [`DEFAULT_CLIPBOARD_TIMEOUT_MS`], [`DEFAULT_CLIPBOARD_MAX_BUFFER_BYTES`] | `9ec41f5195b08929e697b3ec718f72e99852ab19f125aed330f23cf78d961d33` |
//!
//! `clipboard-image.ts` (`d38796e1f442…`) is **not** ported (divergence 5).
//!
//! Behavior was captured from the vendored, byte-identical upstream sources
//! under node (`--experimental-strip-types`) in `tests/fixtures/clipboard_oracle/`:
//! the recorder preload scripts `child_process.spawn` before the ESM graph
//! loads (so argv sequences and `{stdio, windowsHide}` options are recorded
//! per scenario), `process.platform` is faked per scenario, the native module
//! is stubbed to absent (the same condition the port is in), and OSC 52
//! stdout writes are captured byte-exactly. The captured values are pinned by
//! [`tests`] here.
//!
//! ## Platform matrix (text clipboard)
//!
//! | platform | read | write |
//! |---|---|---|
//! | linux (Termux env) | `termux-clipboard-get` | `termux-clipboard-set` |
//! | linux (Wayland env) | `wl-paste --no-newline --type text` | `wl-copy` |
//! | linux (X11 env) | `xclip -selection clipboard -out`, then `xsel --clipboard --output` | `xclip -selection clipboard`, then `xsel --clipboard --input` |
//! | darwin | `pbpaste` (supplement, divergence 2) | `pbcopy` |
//! | win32 | not implemented (upstream native-only; divergence 1b) | `clip` |
//! | any + remote session | — | OSC 52 fallback (`\x1b]52;c;<base64>\x07` on stdout, 100k encoded cap) |
//!
//! ## Seams
//!
//! - **Child process** ([`ClipboardCommandRunner`] / [`RealClipboardCommandRunner`]):
//!   mirrors upstream `runClipboardCommand` (`None` = failed, `Some(empty)` =
//!   success); tests inject scripted runners exactly like the upstream suite
//!   mocks `runClipboardCommand` (W3.9 [`crate::coding_agent::package_manager::CommandRunner`]
//!   precedent). Argv fidelity: the recorded upstream argv IS what a real
//!   runner spawns (std `Command`'s array contract).
//! - **Environment/platform** ([`ClipboardEnv`]): upstream reads
//!   `process.env` / `platform()` directly; the port snapshots them into an
//!   injectable struct ([`ClipboardEnv::from_system`]).
//! - **OSC 52 stdout** ([`Osc52Writer`] / [`StdoutOsc52Writer`]): upstream
//!   `process.stdout.write`; tests capture the sequences.
//! - **Host integration**: `ShellPlatform` (`modes/interactive`, S7) exposes
//!   `copy_to_clipboard`/`read_clipboard_text`; a real host impl delegates to
//!   [`copy_to_clipboard`] / [`read_clipboard_text`] one-to-one
//!   (`read_clipboard_image` stays served by the seam, divergence 5). The
//!   TUI-side sequence builder (`tui::component_clipboard::osc52_sequence`)
//!   already covers the alt-screen component layer and is untouched; this
//!   module keeps the coding-agent layer's own local encoder, exactly like
//!   upstream keeps `emitOsc52` local to `clipboard.ts`.
//!
//! ## Divergences
//!
//! 1. **Native N-API layer absent.** Upstream's primary backend
//!    (`getNativeClipboard()` from `@earendil-works/pi-tui`: prebuilt
//!    `win32-platform.node` / `darwin-platform.node` / `linux-platform-x11.node`)
//!    has no dependency-free Rust equivalent, so the port is the subprocess
//!    route only. Observable effects: (a) win32/darwin writes skip the
//!    native-first attempt — upstream catches a native failure and proceeds to
//!    the same command list, so the fallthrough is behavior-identical when
//!    native is unavailable, but upstream succeeds where an OS-API write works
//!    and the port's `clip`/`pbcopy` is missing; (b) **Windows text read
//!    returns `None`** (upstream is native-only there); (c) after Linux
//!    command failures the port returns `None` where upstream still consults
//!    the native X11 clipboard; (d) win/mac image reads are native-only
//!    upstream and are absent here.
//! 2. **macOS text read supplement.** Upstream reads text on darwin only via
//!    the native module; per the M4 evaluation ("macOS 补 `pbpaste` 读") the
//!    port reads `pbpaste` with the same 5s timeout and empty→`null`
//!    stop-fallback semantics as the Linux command reads. Additive; no
//!    upstream oracle.
//! 3. **Spawn layer.** node/libuv `spawn` becomes tokio's `std::process`
//!    wrapper: `windowsHide: true` maps to `CREATE_NO_WINDOW` (0x08000000),
//!    argv passes through std's argument quoting (same disclosure as
//!    [`crate::coding_agent::utils::child_process`]), and the timeout abort
//!    (`kill("SIGKILL")` + stream destroy) maps to `kill_on_drop` plus an
//!    explicit kill on the max-buffer path.
//! 4. **OSC 52 write timing.** `process.stdout.write` is a buffered async
//!    stream write in node; the port writes+synchronizes synchronously. Same
//!    bytes, no interleaving guarantees beyond upstream's.
//! 5. **`readClipboardImage` not ported** (linux `wl-paste --list-types` /
//!    `xclip -t TARGETS` probing, Photon PNG conversion, WSL PowerShell
//!    fallback). `ShellPlatform::read_clipboard_image` consumers remain on
//!    their seam doubles.

use std::fmt;
use std::time::Duration;

use futures::future::BoxFuture;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

/// Upstream `MAX_OSC52_ENCODED_LENGTH`: base64 payloads above this are not
/// emitted (`clipboard.ts` line 5).
pub const MAX_OSC52_ENCODED_LENGTH: usize = 100_000;

/// Upstream `runClipboardCommand` default `timeoutMs` (`clipboard-command.ts`
/// line 30, `?? 3000`).
pub const DEFAULT_CLIPBOARD_TIMEOUT_MS: u64 = 3000;

/// Upstream `runClipboardCommand` default `maxBufferBytes`: 50 MiB
/// (`clipboard-command.ts` line 38).
pub const DEFAULT_CLIPBOARD_MAX_BUFFER_BYTES: usize = 50 * 1024 * 1024;

/// The 5000 ms timeout every upstream clipboard read command uses
/// (`clipboard.ts` line 30).
pub const READ_CLIPBOARD_TIMEOUT_MS: u64 = 5000;

/// The 5000 ms timeout every upstream clipboard write command uses
/// (`clipboard.ts` line 69).
pub const WRITE_CLIPBOARD_TIMEOUT_MS: u64 = 5000;

/// Platform snapshot of upstream `platform()` (faked per oracle scenario).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ClipboardPlatform {
    Linux,
    Darwin,
    Win32,
    #[default]
    Other,
}

/// Snapshot of the environment probes upstream performs on `process.env`.
/// A variable counts only when set to a non-empty string (JS truthiness).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClipboardEnv {
    pub platform: ClipboardPlatform,
    pub termux_version: bool,
    pub wayland_display: bool,
    pub display: bool,
    pub ssh_connection: bool,
    pub ssh_client: bool,
    pub mosh_connection: bool,
}

impl ClipboardEnv {
    /// Snapshot the real `platform()` and `process.env` probes.
    pub fn from_system() -> Self {
        let platform = if cfg!(target_os = "linux") {
            ClipboardPlatform::Linux
        } else if cfg!(target_os = "macos") {
            ClipboardPlatform::Darwin
        } else if cfg!(windows) {
            ClipboardPlatform::Win32
        } else {
            ClipboardPlatform::Other
        };
        Self {
            platform,
            termux_version: env_flag("TERMUX_VERSION"),
            wayland_display: env_flag("WAYLAND_DISPLAY"),
            display: env_flag("DISPLAY"),
            ssh_connection: env_flag("SSH_CONNECTION"),
            ssh_client: env_flag("SSH_CLIENT"),
            mosh_connection: env_flag("MOSH_CONNECTION"),
        }
    }
}

fn env_flag(key: &str) -> bool {
    std::env::var(key).is_ok_and(|value| !value.is_empty())
}

/// Upstream `isRemoteSession` (`clipboard.ts` lines 7-9).
pub fn is_remote_session(env: &ClipboardEnv) -> bool {
    env.ssh_connection || env.ssh_client || env.mosh_connection
}

/// Upstream `runClipboardCommand` options (`clipboard-command.ts` line 7).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClipboardCommandOptions {
    /// When `Some`, stdin carries the text and the child's stdout is
    /// discarded (`stdio: ["pipe", "ignore", "ignore"]`).
    pub input: Option<String>,
    /// Defaults to [`DEFAULT_CLIPBOARD_TIMEOUT_MS`].
    pub timeout_ms: Option<u64>,
    /// Defaults to [`DEFAULT_CLIPBOARD_MAX_BUFFER_BYTES`].
    pub max_buffer_bytes: Option<usize>,
}

/// Seam over upstream `runClipboardCommand`: `None` means the command failed;
/// an empty buffer is a successful result (`clipboard-command.ts` line 3).
pub trait ClipboardCommandRunner: Send + Sync {
    fn run_clipboard_command<'a>(
        &'a self,
        command: &'a str,
        args: &'a [String],
        options: ClipboardCommandOptions,
    ) -> BoxFuture<'a, Option<Vec<u8>>>;
}

/// Default [`ClipboardCommandRunner`] over `std::process::Command` via tokio
/// (divergence 3).
pub struct RealClipboardCommandRunner;

impl ClipboardCommandRunner for RealClipboardCommandRunner {
    fn run_clipboard_command<'a>(
        &'a self,
        command: &'a str,
        args: &'a [String],
        options: ClipboardCommandOptions,
    ) -> BoxFuture<'a, Option<Vec<u8>>> {
        Box::pin(run_real_clipboard_command(command, args, options))
    }
}

async fn run_real_clipboard_command(
    command: &str,
    args: &[String],
    options: ClipboardCommandOptions,
) -> Option<Vec<u8>> {
    let timeout_ms = options.timeout_ms.unwrap_or(DEFAULT_CLIPBOARD_TIMEOUT_MS);
    let max_buffer_bytes = options
        .max_buffer_bytes
        .unwrap_or(DEFAULT_CLIPBOARD_MAX_BUFFER_BYTES);

    // Clipboard writers can daemonize. Do not give them output pipes to
    // retain: stdin always piped, stdout piped only for readers, stderr
    // ignored (upstream `stdio` triple, oracle-confirmed).
    let mut cmd = Command::new(command);
    cmd.args(args);
    cmd.stdin(std::process::Stdio::piped());
    if options.input.is_some() {
        cmd.stdout(std::process::Stdio::null());
    } else {
        cmd.stdout(std::process::Stdio::piped());
    }
    cmd.stderr(std::process::Stdio::null());
    // Upstream abort() SIGKILLs the child; a future dropped by the timeout
    // must not leave it behind.
    cmd.kill_on_drop(true);
    #[cfg(windows)]
    {
        // Upstream `windowsHide: true`.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().ok()?;

    match options.input {
        Some(input) => {
            if let Some(mut stdin) = child.stdin.take() {
                // A writer may exit before consuming all input; stdin errors
                // are swallowed (upstream `child.stdin?.on("error", ...)`).
                tokio::spawn(async move {
                    let _ = stdin.write_all(input.as_bytes()).await;
                    let _ = stdin.shutdown().await;
                });
            }
        }
        None => {
            // Close stdin immediately so readers see EOF (upstream pipes
            // stdin but never writes; `end(undefined)` closes it).
            drop(child.stdin.take());
        }
    }

    let collect = async move {
        let mut child = child;
        let mut data = Vec::new();
        if let Some(stdout) = child.stdout.as_mut() {
            let mut chunk = [0u8; 64 * 1024];
            loop {
                match stdout.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        if data.len() + read > max_buffer_bytes {
                            // Upstream abort(): kill and resolve undefined
                            // even if the child would still exit 0.
                            let _ = child.kill().await;
                            return None;
                        }
                        data.extend_from_slice(&chunk[..read]);
                    }
                }
            }
        }
        let status = child.wait().await.ok()?;
        if status.success() {
            Some(data)
        } else {
            None
        }
    };
    // Upstream's abort timer resolves undefined on expiry.
    tokio::time::timeout(Duration::from_millis(timeout_ms), collect)
        .await
        .unwrap_or_default()
}

/// Sink for upstream `process.stdout.write` of the OSC 52 escape sequence.
pub trait Osc52Writer: Send + Sync {
    fn write_osc52(&self, sequence: &str);
}

/// Real sink: writes the sequence bytes to the process stdout.
pub struct StdoutOsc52Writer;

impl Osc52Writer for StdoutOsc52Writer {
    fn write_osc52(&self, sequence: &str) {
        use std::io::Write;
        let stdout = std::io::stdout();
        let mut lock = stdout.lock();
        let _ = lock.write_all(sequence.as_bytes());
        let _ = lock.flush();
    }
}

/// Upstream `emitOsc52` (`clipboard.ts` lines 11-18): returns false when the
/// base64 payload exceeds [`MAX_OSC52_ENCODED_LENGTH`] (nothing is written).
pub fn emit_osc52_with(text: &str, writer: &dyn Osc52Writer) -> bool {
    let encoded = base64_encode(text.as_bytes());
    if encoded.len() > MAX_OSC52_ENCODED_LENGTH {
        return false;
    }
    writer.write_osc52(&format!("\x1b]52;c;{encoded}\x07"));
    true
}

/// Standard base64 with padding over the raw UTF-8 bytes, local to the
/// terminal protocol (same shape as `tui::component_clipboard::osc52_sequence`).
fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let a = chunk[0];
        let b = chunk.get(1).copied().unwrap_or(0);
        let c = chunk.get(2).copied().unwrap_or(0);
        out.push(ALPHABET[usize::from(a >> 2)] as char);
        out.push(ALPHABET[usize::from(((a & 3) << 4) | (b >> 4))] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[usize::from(((b & 15) << 2) | (c >> 6))] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[usize::from(c & 63)] as char
        } else {
            '='
        });
    }
    out
}

/// Upstream `readClipboardText` (`clipboard.ts` lines 21-39), with the native
/// terminal state replaced per divergences 1 and 2.
pub async fn read_clipboard_text_with(
    env: &ClipboardEnv,
    runner: &dyn ClipboardCommandRunner,
) -> Option<String> {
    let commands: Vec<(String, Vec<String>)> = match env.platform {
        ClipboardPlatform::Linux => {
            let mut commands = Vec::new();
            if env.termux_version {
                commands.push(("termux-clipboard-get".to_string(), Vec::new()));
            }
            if env.wayland_display {
                commands.push((
                    "wl-paste".to_string(),
                    vec![
                        "--no-newline".to_string(),
                        "--type".to_string(),
                        "text".to_string(),
                    ],
                ));
            }
            if env.display {
                commands.push((
                    "xclip".to_string(),
                    vec![
                        "-selection".to_string(),
                        "clipboard".to_string(),
                        "-out".to_string(),
                    ],
                ));
                commands.push((
                    "xsel".to_string(),
                    vec!["--clipboard".to_string(), "--output".to_string()],
                ));
            }
            commands
        }
        ClipboardPlatform::Darwin => {
            // Divergence 2: upstream uses the native module here; the port
            // reads pbpaste with the Linux reads' timeout semantics.
            vec![("pbpaste".to_string(), Vec::new())]
        }
        // Divergence 1b: upstream is native-only on win32 (and "other").
        ClipboardPlatform::Win32 | ClipboardPlatform::Other => return None,
    };

    for (command, args) in &commands {
        let options = ClipboardCommandOptions {
            timeout_ms: Some(READ_CLIPBOARD_TIMEOUT_MS),
            ..ClipboardCommandOptions::default()
        };
        if let Some(bytes) = runner.run_clipboard_command(command, args, options).await {
            // `bytes.toString("utf8") || null` (upstream #7248): an empty
            // success stops the fallback chain and resolves null instead of
            // consulting the remaining commands or the native clipboard.
            return if bytes.is_empty() {
                None
            } else {
                Some(String::from_utf8_lossy(&bytes).into_owned())
            };
        }
    }
    // Divergence 1c: upstream's final `getNativeClipboard()?.getText() || null`
    // resolves null with the native layer absent.
    None
}

/// Upstream `copyToClipboard` (`clipboard.ts` lines 41-91), with the native
/// first write skipped per divergence 1a.
pub async fn copy_to_clipboard_with(
    env: &ClipboardEnv,
    runner: &dyn ClipboardCommandRunner,
    osc52: &dyn Osc52Writer,
    text: &str,
) -> Result<(), ClipboardError> {
    let mut copied = false;

    let commands: Vec<(String, Vec<String>)> = match env.platform {
        ClipboardPlatform::Darwin => vec![("pbcopy".to_string(), Vec::new())],
        ClipboardPlatform::Win32 => vec![("clip".to_string(), Vec::new())],
        ClipboardPlatform::Linux | ClipboardPlatform::Other => {
            let mut commands = Vec::new();
            if env.termux_version {
                commands.push(("termux-clipboard-set".to_string(), Vec::new()));
            }
            if env.wayland_display {
                commands.push(("wl-copy".to_string(), Vec::new()));
            }
            if env.display {
                commands.push((
                    "xclip".to_string(),
                    vec!["-selection".to_string(), "clipboard".to_string()],
                ));
                commands.push((
                    "xsel".to_string(),
                    vec!["--clipboard".to_string(), "--input".to_string()],
                ));
            }
            commands
        }
    };
    for (command, args) in &commands {
        let options = ClipboardCommandOptions {
            input: Some(text.to_string()),
            timeout_ms: Some(WRITE_CLIPBOARD_TIMEOUT_MS),
            ..ClipboardCommandOptions::default()
        };
        if runner
            .run_clipboard_command(command, args, options)
            .await
            .is_some()
        {
            copied = true;
            break;
        }
    }

    // The remote-session fallback runs regardless of `copied` (upstream even
    // emits OSC 52 after a successful native write, oracle scenario
    // `copy:darwin-native-ok-remote-osc52`).
    if is_remote_session(env) {
        copied = emit_osc52_with(text, osc52) || copied;
    }

    if !copied {
        return Err(ClipboardError::new(unavailable_message(env)));
    }
    Ok(())
}

/// Upstream error texts (`clipboard.ts` lines 77-89), byte-identical.
fn unavailable_message(env: &ClipboardEnv) -> &'static str {
    if env.platform == ClipboardPlatform::Linux {
        if env.termux_version {
            "Clipboard unavailable: install the Termux:API app and `termux-api` package"
        } else if env.wayland_display {
            "Clipboard unavailable: install `wl-clipboard` (`wl-copy`) or check Wayland access"
        } else if env.display {
            "Clipboard unavailable: install `xclip` or `xsel`, or check X11 access"
        } else {
            "Clipboard unavailable: no Wayland or X11 display detected"
        }
    } else {
        "Clipboard unavailable"
    }
}

/// Upstream thrown `Error` with its exact message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardError {
    message: String,
}

impl ClipboardError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ClipboardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ClipboardError {}

/// System convenience over [`ClipboardEnv::from_system`],
/// [`RealClipboardCommandRunner`] and [`StdoutOsc52Writer`]. Shape-matches
/// `ShellPlatform::copy_to_clipboard` (map [`ClipboardError::message`] into
/// the host's `String` error).
pub async fn copy_to_clipboard(text: &str) -> Result<(), ClipboardError> {
    copy_to_clipboard_with(
        &ClipboardEnv::from_system(),
        &RealClipboardCommandRunner,
        &StdoutOsc52Writer,
        text,
    )
    .await
}

/// System convenience over [`ClipboardEnv::from_system`] and
/// [`RealClipboardCommandRunner`]. Shape-matches
/// `ShellPlatform::read_clipboard_text`.
pub async fn read_clipboard_text() -> Option<String> {
    read_clipboard_text_with(&ClipboardEnv::from_system(), &RealClipboardCommandRunner).await
}

#[cfg(test)]
#[path = "clipboard_tests.rs"]
mod tests;
