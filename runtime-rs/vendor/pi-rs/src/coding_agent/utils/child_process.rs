//! Port of upstream `coding-agent/src/utils/child-process.ts`.
//!
//! Process spawning helpers. Upstream routes `spawn`/`spawnSync` through
//! `cross-spawn` on win32 (so `.cmd`/`.bat` shims resolve) and node's
//! `child_process` elsewhere; the Rust port uses [`std::process`], whose
//! CreateProcess-based spawn resolves `.cmd`/`.bat` shims on Windows the
//! same way (disclosed divergence: cross-spawn's custom cmd-escaping quirks
//! are replaced by std's argument quoting).
//!
//! `waitForChildProcess` reproduces the upstream settle policy
//! (earendil-works/pi#5303): after the child `exit`s, do not finalize on a
//! fixed deadline — instead wait for the stdio pipes to fall idle. The grace
//! timer is (re-)armed on every chunk, so an actively writing descendant
//! keeps us reading, while a quiet inherited handle releases us after
//! [`EXIT_STDIO_GRACE_MS`]. A reader blocked on an abandoned inherited pipe
//! is detached (the moral equivalent of upstream's `stream.destroy()`), so
//! one blocked OS thread can outlive the call in that scenario.

use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Upstream `EXIT_STDIO_GRACE_MS`: how long a post-exit pipe may stay quiet
/// before the wait finalizes.
pub const EXIT_STDIO_GRACE_MS: u64 = 100;

/// Upstream `spawnSync` result with `encoding: "utf-8"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncOutput {
    /// Exit code, `None` when the process was killed by a signal.
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// Upstream `spawnProcess`: spawn `command` with `args` (win32 routes through
/// a cmd-shim-capable spawn, matching cross-spawn's role).
pub fn spawn_process(command: &str, args: &[&str]) -> std::process::Command {
    let mut cmd = std::process::Command::new(command);
    cmd.args(args);
    cmd
}

/// Upstream `spawnProcessSync(command, args, { encoding: "utf-8" })`.
pub fn spawn_process_sync(command: &str, args: &[&str]) -> std::io::Result<SyncOutput> {
    let output = spawn_process(command, args).output()?;
    Ok(SyncOutput {
        status: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

/// Upstream `spawnProcessSync(command, args, { encoding: "utf-8",
/// stdio: "ignore" })`: waits for completion and discards the output.
pub fn spawn_process_sync_discarding_output(
    command: &str,
    args: &[&str],
) -> std::io::Result<Option<i32>> {
    use std::process::Stdio;
    let status = spawn_process(command, args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    Ok(status.code())
}

/// Receiver for stdout/stderr chunks while waiting (upstream callers attach
/// `data` listeners to the child streams).
pub type ChunkSink = Box<dyn FnMut(&[u8]) + Send>;

struct StreamState {
    ended: AtomicBool,
    last_chunk: Mutex<Instant>,
}

impl StreamState {
    fn started() -> Self {
        Self {
            ended: AtomicBool::new(false),
            last_chunk: Mutex::new(Instant::now()),
        }
    }

    fn touch(&self) {
        *self
            .last_chunk
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Instant::now();
    }

    fn idle_ms(&self) -> u128 {
        self.last_chunk
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .elapsed()
            .as_millis()
    }
}

fn spawn_reader<R: Read + Send + 'static>(
    mut stream: R,
    mut sink: ChunkSink,
    state: Arc<StreamState>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        loop {
            match stream.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    sink(&buffer[..read]);
                    state.touch();
                }
                Err(_) => break,
            }
        }
        state.ended.store(true, Ordering::SeqCst);
    })
}

/// Wait for a child process to terminate without hanging on inherited stdio
/// handles (upstream `waitForChildProcess`).
///
/// `on_stdout` / `on_stderr` receive each chunk as it arrives (upstream
/// `data` events). Returns the exit code (`None` when the child died to a
/// signal, mirroring node's `exit` code `null`).
pub fn wait_for_child_process(
    child: &mut std::process::Child,
    on_stdout: ChunkSink,
    on_stderr: ChunkSink,
) -> std::io::Result<Option<i32>> {
    // A null stream starts out "ended", like upstream's
    // `stdoutEnded = child.stdout === null`.
    let states: Vec<Arc<StreamState>> = vec![
        Arc::new(StreamState::started()),
        Arc::new(StreamState::started()),
    ];
    let mut handles = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        handles.push(spawn_reader(stdout, on_stdout, Arc::clone(&states[0])));
    } else {
        states[0].ended.store(true, Ordering::SeqCst);
    }
    if let Some(stderr) = child.stderr.take() {
        handles.push(spawn_reader(stderr, on_stderr, Arc::clone(&states[1])));
    } else {
        states[1].ended.store(true, Ordering::SeqCst);
    }

    // Upstream waits for the `exit` event first; std's `wait()` is the
    // equivalent (it only waits for the process, not the pipes).
    let exit_code = child.wait()?.code();

    // The post-exit grace timer arms now and re-arms on every chunk.
    for state in &states {
        state.touch();
    }

    loop {
        if states
            .iter()
            .all(|state| state.ended.load(Ordering::SeqCst))
        {
            break;
        }
        let min_idle_ms = states
            .iter()
            .filter(|state| !state.ended.load(Ordering::SeqCst))
            .map(|state| state.idle_ms())
            .min()
            .unwrap_or(u128::MAX);
        if min_idle_ms >= u128::from(EXIT_STDIO_GRACE_MS) {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    // Readers that observed EOF are joined; a reader still blocked on an
    // abandoned inherited pipe is detached (upstream destroys the stream).
    for handle in handles {
        if !handle.is_finished() {
            continue;
        }
        let _ = handle.join();
    }

    Ok(exit_code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    fn collect() -> (ChunkSink, Arc<StdMutex<Vec<u8>>>) {
        let shared: Arc<StdMutex<Vec<u8>>> = Arc::new(StdMutex::new(Vec::new()));
        let sink_ref = Arc::clone(&shared);
        let sink: ChunkSink = Box::new(move |chunk: &[u8]| {
            sink_ref
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .extend_from_slice(chunk);
        });
        (sink, shared)
    }

    #[cfg(windows)]
    fn echo_child() -> std::process::Child {
        spawn_process("cmd", &["/c", "echo done"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn")
    }

    #[cfg(unix)]
    fn echo_child() -> std::process::Child {
        spawn_process("sh", &["-c", "echo done"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn")
    }

    #[test]
    fn waits_for_a_quick_child_and_drains_output() {
        let mut child = echo_child();
        let (stdout_sink, stdout_data) = collect();
        let (stderr_sink, _stderr_data) = collect();
        let code = wait_for_child_process(&mut child, stdout_sink, stderr_sink).expect("wait");
        assert_eq!(code, Some(0));
        let output = String::from_utf8_lossy(
            &stdout_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
        .trim()
        .to_string();
        assert_eq!(output, "done");
    }

    #[test]
    fn waits_for_a_child_with_null_stdio() {
        let mut child = spawn_process("cmd", &["/c", "exit 3"])
            .spawn()
            .or_else(|_| spawn_process("sh", &["-c", "exit 3"]).spawn())
            .expect("spawn");
        let code =
            wait_for_child_process(&mut child, Box::new(|_| {}), Box::new(|_| {})).expect("wait");
        assert_eq!(code, Some(3));
    }

    #[cfg(unix)]
    #[test]
    fn releases_when_a_descendant_keeps_the_pipe_open() {
        // The shell exits immediately but the backgrounded `sleep` inherits
        // stdout and keeps it open for 5s; the idle grace must release us
        // long before that (pi#5303).
        let start = Instant::now();
        let mut child = spawn_process("sh", &["-c", "echo done; sleep 5 &"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn");
        let (stdout_sink, stdout_data) = collect();
        let code = wait_for_child_process(&mut child, stdout_sink, Box::new(|_| {})).expect("wait");
        assert_eq!(code, Some(0));
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "should not wait for the descendant's EOF, took {:?}",
            start.elapsed()
        );
        let stdout_guard = stdout_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let output = String::from_utf8_lossy(&stdout_guard);
        assert!(
            output.contains("done"),
            "output must still be drained: {output:?}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn releases_when_a_descendant_keeps_the_pipe_open() {
        // `waitfor` blocks silently for 30s with the inherited pipe open;
        // the idle grace must release us right after `done` (pi#5303).
        let start = Instant::now();
        let mut child = spawn_process(
            "cmd",
            &["/c", "echo done & start /b cmd /c waitfor neverthing /t 30"],
        )
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn");
        let (stdout_sink, stdout_data) = collect();
        let code = wait_for_child_process(&mut child, stdout_sink, Box::new(|_| {})).expect("wait");
        assert_eq!(code, Some(0));
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "should not wait for the descendant's EOF, took {:?}",
            start.elapsed()
        );
        let output = String::from_utf8_lossy(
            &stdout_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
        .trim()
        .to_string();
        assert!(
            output.contains("done"),
            "output must still be drained: {output:?}"
        );
    }

    #[test]
    fn spawn_process_sync_captures_utf8_output_and_status() {
        let output = if cfg!(windows) {
            spawn_process_sync("cmd", &["/c", "echo hello"]).expect("sync")
        } else {
            spawn_process_sync("sh", &["-c", "echo hello"]).expect("sync")
        };
        assert_eq!(output.status, Some(0));
        assert_eq!(output.stdout.trim(), "hello");
        assert_eq!(output.stderr, "");
    }

    #[test]
    fn spawn_process_sync_reports_missing_program_as_error() {
        let result = spawn_process_sync("no-such-program-xyz", &[]);
        assert!(result.is_err());
    }

    #[test]
    fn exit_stdio_grace_matches_upstream_constant() {
        assert_eq!(EXIT_STDIO_GRACE_MS, 100);
    }
}
