use super::*;
use std::{
    io::{Read, Write},
    sync::Mutex,
};
use tokio::sync::Notify;
const HELPER: &str = "coding_agent::core::tools::bash_process::tests::native_shell_child_helper";
fn helper_environment(mode: &str) -> ShellEnvironment {
    let mut env: ShellEnvironment = std::env::vars()
        .filter(|(k, _)| !k.starts_with("PI_RUST_SHELL_TEST_"))
        .collect();
    env.push(("PI_RUST_SHELL_TEST_MODE".into(), mode.into()));
    env
}
fn helper_operations(stdin: bool) -> ShellOperations {
    let config = ShellConfig {
        shell: std::env::current_exe()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
        args: if stdin {
            vec!["--exact".into(), HELPER.into(), "--nocapture".into()]
        } else {
            vec!["--exact".into(), HELPER.into()]
        },
        command_transport: stdin.then_some(CommandTransport::Stdin),
    };
    create_local_shell_operations(
        "helper",
        Arc::new(move || {
            let config = config.clone();
            Box::pin(async move { Ok(config) })
        }),
    )
}
fn capture_options(mode: &str) -> (ShellExecOptions, Arc<Mutex<Vec<u8>>>) {
    let bytes = Arc::new(Mutex::new(vec![]));
    (
        ShellExecOptions {
            on_data: {
                let bytes = bytes.clone();
                Arc::new(move |b| {
                    bytes.lock().unwrap().extend_from_slice(b);
                    Ok(())
                })
            },
            signal: None,
            timeout: Some(5.0),
            env: Some(helper_environment(mode)),
        },
        bytes,
    )
}
fn helper_child(mode: &str) -> std::process::Child {
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", HELPER, "--nocapture"])
        .env("PI_RUST_SHELL_TEST_MODE", mode)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command.spawn().unwrap()
}
/// Only becomes a helper inside a specifically spawned subprocess. The normal
/// all-targets test invocation has no PI_RUST_SHELL_TEST_MODE and does nothing.
// Deliberately orphan grandchildren to exercise inherited pipe handles.
// A blocking wait here would invalidate the parent-exit regression.
#[allow(clippy::zombie_processes)]
#[test]
fn native_shell_child_helper() {
    let Ok(mode) = std::env::var("PI_RUST_SHELL_TEST_MODE") else {
        return;
    };
    match mode.as_str() {
        "stream" => {
            std::io::stdout().write_all(b"STDOUT_MARKER\n").unwrap();
            std::io::stderr().write_all(b"STDERR_MARKER\n").unwrap();
            println!("ENV={}", std::env::var("PI_RUST_SHELL_TEST_VALUE").unwrap());
            std::process::exit(7);
        }
        "stdin" => {
            let mut bytes = vec![];
            std::io::stdin().read_to_end(&mut bytes).unwrap();
            println!("STDIN_BYTES={}", bytes.len());
            assert!(String::from_utf8(bytes).unwrap().ends_with("\nsnow=雪🙂\n"));
        }
        "hang" => {
            println!("SHELL_READY");
            std::io::stdout().flush().unwrap();
            std::thread::sleep(Duration::from_secs(12));
        }
        "drop" => {
            println!("SHELL_READY");
            std::io::stdout().flush().unwrap();
            std::thread::sleep(Duration::from_millis(900));
            std::fs::write(
                std::env::var("PI_RUST_SHELL_TEST_MARKER").unwrap(),
                b"survived",
            )
            .unwrap();
        }
        "active-parent" => {
            let child = helper_child("active-descendant");
            println!("CHILD_PID={}", child.id());
        }
        "active-descendant" => {
            for i in 0..8 {
                println!("ACTIVE_{i}");
                std::io::stdout().flush().unwrap();
                std::thread::sleep(Duration::from_millis(40));
            }
            println!("ACTIVE_END");
        }
        "quiet-parent" => {
            let child = helper_child("quiet-descendant");
            println!("CHILD_PID={}", child.id());
        }
        "quiet-descendant" => {
            println!("QUIET_READY");
            std::io::stdout().flush().unwrap();
            std::thread::sleep(Duration::from_secs(2));
        }
        other => panic!("unknown helper mode {other}"),
    }
}
#[tokio::test]
async fn native_process_preserves_stdout_stderr_env_and_nonzero_exit() {
    let dir = tempfile::tempdir().unwrap();
    let (mut options, bytes) = capture_options("stream");
    options
        .env
        .as_mut()
        .unwrap()
        .push(("PI_RUST_SHELL_TEST_VALUE".into(), "雪🙂".into()));
    let result = (helper_operations(false).exec)(
        "--nocapture".into(),
        dir.path().to_string_lossy().into_owned(),
        options,
    )
    .await
    .unwrap();
    assert_eq!(result.exit_code, Some(7));
    let text = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
    assert!(
        text.contains("STDOUT_MARKER")
            && text.contains("STDERR_MARKER")
            && text.contains("ENV=雪🙂"),
        "{text}"
    );
}
#[tokio::test]
async fn native_process_stdin_transport_delivers_large_unicode_script_and_eof() {
    let dir = tempfile::tempdir().unwrap();
    let (options, bytes) = capture_options("stdin");
    let command = "x".repeat(150_000) + "\nsnow=雪🙂\n";
    let expected = format!("STDIN_BYTES={}", command.len());
    let result =
        (helper_operations(true).exec)(command, dir.path().to_string_lossy().into_owned(), options)
            .await
            .unwrap();
    assert_eq!(result.exit_code, Some(0));
    assert!(String::from_utf8_lossy(&bytes.lock().unwrap()).contains(&expected));
}
#[tokio::test]
async fn native_process_preflight_orders_timeout_abort_discovery_and_cwd() {
    let ops = create_local_shell_operations(
        "bash",
        Arc::new(|| Box::pin(async { Err("discovery".into()) })),
    );
    let (mut options, _) = capture_options("hang");
    let abort = Arc::new(AbortSignal::new());
    abort.abort();
    options.signal = Some(abort);
    options.timeout = Some(-1.0);
    assert_eq!(
        (ops.exec)("command".into(), "missing".into(), options.clone())
            .await
            .unwrap_err(),
        "Invalid timeout: must be a finite number of seconds"
    );
    options.timeout = None;
    assert_eq!(
        (ops.exec)("command".into(), "missing".into(), options.clone())
            .await
            .unwrap_err(),
        "aborted"
    );
    options.signal = None;
    assert_eq!(
        (ops.exec)("command".into(), "missing".into(), options.clone())
            .await
            .unwrap_err(),
        "discovery"
    );
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("absent").to_string_lossy().into_owned();
    assert_eq!(
        (helper_operations(false).exec)("--nocapture".into(), missing.clone(), options)
            .await
            .unwrap_err(),
        format!("Working directory does not exist: {missing}\nCannot execute helper commands.")
    );
}
#[tokio::test]
async fn native_process_timeout_and_abort_kill_child_and_preserve_output() {
    let dir = tempfile::tempdir().unwrap();
    let (mut options, _) = capture_options("hang");
    options.timeout = Some(0.1);
    let result = tokio::time::timeout(
        Duration::from_secs(6),
        (helper_operations(false).exec)(
            "--nocapture".into(),
            dir.path().to_string_lossy().into_owned(),
            options,
        ),
    )
    .await
    .unwrap();
    assert_eq!(result.unwrap_err(), "timeout:0.1");
    let (mut options, bytes) = capture_options("hang");
    let signal = Arc::new(AbortSignal::new());
    options.signal = Some(signal.clone());
    options.timeout = None;
    let ready = Arc::new(Notify::new());
    let original = options.on_data.clone();
    options.on_data = {
        let ready = ready.clone();
        let bytes = bytes.clone();
        Arc::new(move |data| {
            original(data)?;
            if String::from_utf8_lossy(&bytes.lock().unwrap()).contains("SHELL_READY") {
                ready.notify_one();
            }
            Ok(())
        })
    };
    let execution = (helper_operations(false).exec)(
        "--nocapture".into(),
        dir.path().to_string_lossy().into_owned(),
        options,
    );
    let cancel = async {
        tokio::time::timeout(Duration::from_secs(5), ready.notified())
            .await
            .unwrap();
        signal.abort();
    };
    let (result, ()) = tokio::join!(execution, cancel);
    assert_eq!(result.unwrap_err(), "aborted");
    assert!(String::from_utf8_lossy(&bytes.lock().unwrap()).contains("SHELL_READY"));
}
#[tokio::test]
async fn native_process_idle_grace_keeps_active_descendant_but_releases_quiet_pipe() {
    let dir = tempfile::tempdir().unwrap();
    for (mode, active) in [("active-parent", true), ("quiet-parent", false)] {
        let (options, bytes) = capture_options(mode);
        let start = Instant::now();
        let result = (helper_operations(false).exec)(
            "--nocapture".into(),
            dir.path().to_string_lossy().into_owned(),
            options,
        )
        .await
        .unwrap();
        let text = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
        let pid = text
            .lines()
            .find_map(|line| line.strip_prefix("CHILD_PID="))
            .unwrap()
            .parse::<u32>()
            .unwrap();
        if !active {
            kill_process_tree(pid);
        }
        assert_eq!(result.exit_code, Some(0));
        if active {
            assert!(text.contains("ACTIVE_END"), "{text}");
            assert!(start.elapsed() >= Duration::from_millis(250));
        } else {
            assert!(
                start.elapsed() < Duration::from_millis(1500),
                "quiet descendant held pipe too long: {:?}",
                start.elapsed()
            );
        }
    }
}
#[tokio::test]
async fn dropping_native_exec_terminates_child_even_while_stdin_write_is_pending() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("should-not-exist");
    let (mut options, bytes) = capture_options("drop");
    options.env.as_mut().unwrap().push((
        "PI_RUST_SHELL_TEST_MARKER".into(),
        marker.to_string_lossy().into_owned(),
    ));
    let ready = Arc::new(Notify::new());
    let original = options.on_data.clone();
    options.on_data = {
        let ready = ready.clone();
        Arc::new(move |data| {
            original(data)?;
            if String::from_utf8_lossy(&bytes.lock().unwrap()).contains("SHELL_READY") {
                ready.notify_one();
            }
            Ok(())
        })
    };
    let mut execution = (helper_operations(true).exec)(
        "x".repeat(4_000_000),
        dir.path().to_string_lossy().into_owned(),
        options,
    );
    tokio::select! {result=&mut execution=>panic!("helper unexpectedly completed: {result:?}"),r=tokio::time::timeout(Duration::from_secs(5),ready.notified())=>r.unwrap()};
    drop(execution);
    tokio::time::sleep(Duration::from_millis(1300)).await;
    assert!(!marker.exists(), "dropped future left helper running");
}
#[cfg(windows)]
#[tokio::test]
async fn real_powershell_wrapper_sets_utf8_and_executes_unicode() {
    let dir = tempfile::tempdir().unwrap();
    let (mut options, bytes) = capture_options("unused");
    // environment-anchored: cold PowerShell startup on CI runners can exceed
    // a tight timeout (Defender scanning); the pinned intent — UTF-8 output
    // and unicode round-trip — is unchanged by a generous ceiling.
    options.timeout = Some(120.0);
    let result = (create_local_powershell_operations().exec)(
        "[Console]::Write('雪🙂')".into(),
        dir.path().to_string_lossy().into_owned(),
        options,
    )
    .await
    .unwrap();
    assert_eq!(result.exit_code, Some(0));
    assert_eq!(&*bytes.lock().unwrap(), "雪🙂".as_bytes());
}
#[cfg(unix)]
#[tokio::test]
async fn unix_process_signal_exit_is_not_reported_as_success() {
    let dir = tempfile::tempdir().unwrap();
    let (options, _) = capture_options("unused");
    let result = (create_local_bash_operations(Some("/bin/sh".into())).exec)(
        "kill -TERM $$".into(),
        dir.path().to_string_lossy().into_owned(),
        options,
    )
    .await
    .unwrap();
    assert_eq!(result.exit_code, Some(143));
}
