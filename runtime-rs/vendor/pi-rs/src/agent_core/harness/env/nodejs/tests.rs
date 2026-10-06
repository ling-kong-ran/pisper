//! Port of `packages/agent/test/harness/nodejs-env.test.ts` (the executable
//! spec for `NodeExecutionEnv`). Port deltas:
//! - Symlink fixtures are created opportunistically and the test returns
//!   early when the platform denies the privilege (Windows CI without
//!   developer mode), matching the M3b Task 3 precedent.
//! - The two oracle tests that monkeypatch `process.platform` (legacy-WSL
//!   stdin transport, taskkill spawn errors) are `#[cfg(unix)]`-gated or
//!   omitted: the port's platform gates are compile-time. The stdin-transport
//!   behavior itself is still exercised on unix (the legacy-WSL path shape is
//!   platform-independent in `getBashShellConfig`).
//! - `timeout: 0.01` becomes whole seconds (the port's timeout type; module
//!   docs), so the timeout oracle sleeps 1s instead of 10ms.
//! - The raw-bytes and large-output fixtures use `node -e` like upstream.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::*;
use crate::agent_core::harness::context::{background_context, with_abort_signal};
use crate::agent_core::harness::types::{
    Shell, ShellExecOptions, ShellOutputCaptureOptions, ShellOutputLimits, ShellOutputRetention,
    ShellOutputUpdate, ShellOutputView,
};
use crate::agent_core::harness::utils::output_capture::apply_shell_output_update;
use crate::agent_core::harness::utils::shell_output::execute_shell_with_capture;
use tokio_util::sync::CancellationToken;

fn temp_root() -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().expect("tempdir");
    // environment-anchored: both sides normalized. CI temp dirs can be 8.3
    // short paths (RUNNER~1) while spawned shells report the long form as
    // PWD; canonicalize the env root so both sides match on every machine.
    let path = canonical(&path_string(dir.path()));
    (dir, path)
}

fn env_at(root: &str) -> NodeExecutionEnv {
    NodeExecutionEnv::new(root)
}

fn ctx() -> Context {
    background_context()
}

/// `getOrThrow` in the oracle tests.
fn unwrap_ok<T>(result: Result<T, impl std::fmt::Display>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("expected ok result, got error: {error}"),
    }
}

fn canonical(path: &str) -> String {
    path_string(&std::fs::canonicalize(path).expect("canonicalize"))
}

/// The oracle's `join(root, ...)` expectations: platform-normalized join
/// (node normalizes separators; split the tail so each segment joins with
/// the platform separator).
fn joined(root: &str, tail: &str) -> String {
    let mut path = std::path::PathBuf::from(root);
    for segment in tail.split('/') {
        if segment.is_empty() {
            continue;
        }
        path.push(segment);
    }
    path_string(&path)
}

/// The oracle's `collectShellOutput` result pair.
type CollectedShellOutput = (
    Result<ShellExecResult, ExecutionError>,
    Option<ShellOutputView>,
);

/// The oracle's `collectShellOutput` (`nodejs-env.test.ts:36-54`).
async fn collect_shell_output(
    env: &NodeExecutionEnv,
    command: &str,
    options: Option<ShellExecOptions>,
    context: Context,
) -> CollectedShellOutput {
    let output: Arc<Mutex<Option<ShellOutputView>>> = Arc::new(Mutex::new(None));
    let writer = Arc::clone(&output);
    let mut exec_options = options.unwrap_or_default();
    exec_options.on_update = Some(Arc::new(move |update, _context| {
        let previous = writer.lock().unwrap().take();
        *writer.lock().unwrap() = Some(apply_shell_output_update(previous, update));
    }));
    let result = env.exec(command, Some(&exec_options), context).await;
    let view = output.lock().unwrap().clone();
    (result, view)
}

/// The oracle's `toBashSingleQuotedArg` (`nodejs-env.test.ts:56-58`).
// Only the windows-gated detached-descendant scenario consumes these helpers
// today; keep them out of the unix compilation to stay dead-code clean.
#[cfg(windows)]
fn to_bash_single_quoted_arg(value: &str) -> String {
    format!("'{}'", value.replace('\\', "/").replace('\'', "'\"'\"'"))
}

/// The oracle's `createInheritedStdioCommand` (`nodejs-env.test.ts:60-72`).
#[cfg(windows)]
fn create_inherited_stdio_command(pid_file: &str) -> String {
    format!(
        "node -e \"{}\" {}",
        "const fs=require('fs');\
         const {spawn}=require('child_process');\
         const child=spawn(process.execPath,['-e','setTimeout(()=>{},60000)'],{stdio:'inherit',detached:true});\
         fs.writeFileSync(process.argv[1], String(child.pid));\
         child.unref();\
         console.log('child-exiting');",
        to_bash_single_quoted_arg(pid_file)
    )
}

/// The oracle's `cleanupDetachedChild` (`nodejs-env.test.ts:74-81`).
#[cfg(windows)]
fn cleanup_detached_child(pid_file: &Path) {
    let Ok(text) = std::fs::read_to_string(pid_file) else {
        return;
    };
    let Ok(pid) = text.trim().parse::<u32>() else {
        return;
    };
    if pid == 0 {
        return;
    }
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    #[cfg(unix)]
    {
        let _ = std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .status();
    }
}

/// Oracle "reads, writes, lists, and removes files and directories"
/// (`nodejs-env.test.ts:105-138`).
#[tokio::test]
async fn reads_writes_lists_and_removes_files_and_directories() {
    let (_dir, root) = temp_root();
    let env = env_at(&root);
    assert_eq!(
        unwrap_ok(env.absolute_path("nested/child", ctx()).await),
        joined(&root, "nested/child")
    );
    assert_eq!(
        unwrap_ok(
            env.join_path(&[root.clone(), "nested".into(), "child".into()], ctx())
                .await
        ),
        joined(&root, "nested/child")
    );
    unwrap_ok(env.create_dir("nested/child", None, ctx()).await);
    unwrap_ok(
        env.write_file(
            "nested/child/file.txt",
            FileContent::Text("hel".into()),
            ctx(),
        )
        .await,
    );
    unwrap_ok(
        env.append_file(
            "nested/child/file.txt",
            FileContent::Text("lo".into()),
            ctx(),
        )
        .await,
    );
    assert_eq!(
        unwrap_ok(env.read_text_file("nested/child/file.txt", ctx()).await),
        "hello"
    );
    assert_eq!(
        unwrap_ok(
            env.read_text_lines(
                "nested/child/file.txt",
                Some(&ReadTextLinesOptions { max_lines: Some(1) }),
                ctx()
            )
            .await
        ),
        vec!["hello"]
    );
    assert_eq!(
        unwrap_ok(env.read_binary_file("nested/child/file.txt", ctx()).await),
        b"hello".to_vec()
    );

    let entries = unwrap_ok(env.list_dir("nested/child", ctx()).await);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "file.txt");
    assert_eq!(entries[0].path, joined(&root, "nested/child/file.txt"));
    assert_eq!(entries[0].kind, FileKind::File);
    assert_eq!(entries[0].size, 5);
    assert!(entries[0].mtime_ms > 0.0);

    assert!(unwrap_ok(env.exists("nested/child/file.txt", ctx()).await));
    unwrap_ok(env.remove("nested/child/file.txt", None, ctx()).await);
    assert!(!unwrap_ok(env.exists("nested/child/file.txt", ctx()).await));
}

/// Oracle "expands home-relative paths and file URLs"
/// (`nodejs-env.test.ts:140-148`).
#[tokio::test]
async fn expands_home_relative_paths_and_file_urls() {
    let (_dir, root) = temp_root();
    let env = env_at(&root);
    let home = dirs::home_dir().expect("homedir");
    let expected = {
        let mut joined = home.clone();
        joined.push("pi-node-env-test");
        path_string(&joined)
    };
    assert_eq!(
        unwrap_ok(env.absolute_path("~/pi-node-env-test", ctx()).await),
        expected
    );
    let file_path = join_raw(&root, "file with spaces.txt");
    let url = url::Url::from_file_path(&file_path)
        .expect("url")
        .to_string();
    assert_eq!(unwrap_ok(env.absolute_path(&url, ctx()).await), file_path);
}

fn create_symlink(original: &Path, link: &Path) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(original, link).is_ok()
    }
    #[cfg(windows)]
    {
        if original.is_dir() {
            std::os::windows::fs::symlink_dir(original, link).is_ok()
        } else {
            std::os::windows::fs::symlink_file(original, link).is_ok()
        }
    }
}

/// Oracle "returns fileInfo for files, directories, and symlinks without
/// following symlinks" (`nodejs-env.test.ts:150-182`).
#[tokio::test]
async fn returns_file_info_for_files_directories_and_symlinks_without_following() {
    let (_dir, root) = temp_root();
    let env = env_at(&root);
    unwrap_ok(
        env.create_dir(
            "dir",
            Some(&CreateDirOptions {
                recursive: Some(true),
            }),
            ctx(),
        )
        .await,
    );
    unwrap_ok(
        env.write_file("dir/file.txt", FileContent::Text("hello".into()), ctx())
            .await,
    );
    if !create_symlink(
        &Path::new(&root).join("dir/file.txt"),
        &Path::new(&root).join("file-link"),
    ) {
        eprintln!("skipping: symlink privilege unavailable");
        return;
    }
    assert!(create_symlink(
        &Path::new(&root).join("dir"),
        &Path::new(&root).join("dir-link")
    ));

    let dir_info = unwrap_ok(env.file_info("dir", ctx()).await);
    assert_eq!(dir_info.name, "dir");
    assert_eq!(dir_info.path, join_raw(&root, "dir"));
    assert_eq!(dir_info.kind, FileKind::Directory);
    let file_info = unwrap_ok(env.file_info("dir/file.txt", ctx()).await);
    assert_eq!(file_info.name, "file.txt");
    assert_eq!(
        file_info.path,
        join_raw(&join_raw(&root, "dir"), "file.txt")
    );
    assert_eq!(file_info.kind, FileKind::File);
    assert_eq!(file_info.size, 5);
    let link_info = unwrap_ok(env.file_info("file-link", ctx()).await);
    assert_eq!(link_info.name, "file-link");
    assert_eq!(link_info.kind, FileKind::Symlink);
    let dir_link_info = unwrap_ok(env.file_info("dir-link", ctx()).await);
    assert_eq!(dir_link_info.name, "dir-link");
    assert_eq!(dir_link_info.kind, FileKind::Symlink);
    assert_eq!(
        unwrap_ok(env.canonical_path("file-link", ctx()).await),
        canonical(&join_raw(&join_raw(&root, "dir"), "file.txt"))
    );
}

/// Oracle "lists symlinks as symlinks" (`nodejs-env.test.ts:184-197`).
#[tokio::test]
async fn lists_symlinks_as_symlinks() {
    let (_dir, root) = temp_root();
    let env = env_at(&root);
    unwrap_ok(
        env.write_file("target.txt", FileContent::Text("hello".into()), ctx())
            .await,
    );
    if !create_symlink(
        &Path::new(&root).join("target.txt"),
        &Path::new(&root).join("link.txt"),
    ) {
        eprintln!("skipping: symlink privilege unavailable");
        return;
    }

    let entries = unwrap_ok(env.list_dir(".", ctx()).await);
    let mut mapped: Vec<(String, FileKind)> = entries
        .iter()
        .map(|entry| (entry.name.clone(), entry.kind))
        .collect();
    mapped.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        mapped,
        vec![
            ("link.txt".to_string(), FileKind::Symlink),
            ("target.txt".to_string(), FileKind::File),
        ]
    );
}

/// Oracle "stops reading text lines at the requested limit"
/// (`nodejs-env.test.ts:199-204`).
#[tokio::test]
async fn stops_reading_text_lines_at_the_requested_limit() {
    let (_dir, root) = temp_root();
    let env = env_at(&root);
    unwrap_ok(
        env.write_file(
            "file.txt",
            FileContent::Text("one\ntwo\nthree".into()),
            ctx(),
        )
        .await,
    );
    assert_eq!(
        unwrap_ok(
            env.read_text_lines(
                "file.txt",
                Some(&ReadTextLinesOptions { max_lines: Some(1) }),
                ctx()
            )
            .await
        ),
        vec!["one"]
    );
}

/// Oracle "returns FileError for missing paths and keeps exists false for
/// missing paths" (`nodejs-env.test.ts:206-220`).
#[tokio::test]
async fn returns_file_error_for_missing_paths_and_keeps_exists_false() {
    let (_dir, root) = temp_root();
    let env = env_at(&root);
    let info = env.file_info("missing.txt", ctx()).await;
    let Err(error) = info else {
        panic!("expected error");
    };
    assert_eq!(error.code, FileErrorCode::NotFound);
    assert_eq!(
        error.path.as_deref(),
        Some(joined(&root, "missing.txt").as_str())
    );
    assert!(!unwrap_ok(env.exists("missing.txt", ctx()).await));
}

/// Oracle "returns FileError for listing non-directories"
/// (`nodejs-env.test.ts:222-232`).
#[tokio::test]
async fn returns_file_error_for_listing_non_directories() {
    let (_dir, root) = temp_root();
    let env = env_at(&root);
    unwrap_ok(
        env.write_file("file.txt", FileContent::Text("hello".into()), ctx())
            .await,
    );
    let result = env.list_dir("file.txt", ctx()).await;
    let Err(error) = result else {
        panic!("expected error");
    };
    assert_eq!(error.code, FileErrorCode::NotDirectory);
}

/// Oracle "appends to new files and creates parent directories"
/// (`nodejs-env.test.ts:234-240`).
#[tokio::test]
async fn appends_to_new_files_and_creates_parent_directories() {
    let (_dir, root) = temp_root();
    let env = env_at(&root);
    unwrap_ok(
        env.append_file("new/nested/file.txt", FileContent::Text("a".into()), ctx())
            .await,
    );
    unwrap_ok(
        env.append_file("new/nested/file.txt", FileContent::Text("b".into()), ctx())
            .await,
    );
    assert_eq!(
        unwrap_ok(env.read_text_file("new/nested/file.txt", ctx()).await),
        "ab"
    );
}

/// Oracle "atomically renames a file and replaces the destination"
/// (`nodejs-env.test.ts:242-252`).
#[tokio::test]
async fn atomically_renames_a_file_and_replaces_the_destination() {
    let (_dir, root) = temp_root();
    let env = env_at(&root);
    unwrap_ok(
        env.write_file("source.txt", FileContent::Text("new".into()), ctx())
            .await,
    );
    unwrap_ok(
        env.write_file("destination.txt", FileContent::Text("old".into()), ctx())
            .await,
    );
    unwrap_ok(
        env.rename_file("source.txt", "destination.txt", ctx())
            .await,
    );
    assert!(!unwrap_ok(env.exists("source.txt", ctx()).await));
    assert_eq!(
        unwrap_ok(env.read_text_file("destination.txt", ctx()).await),
        "new"
    );
}

/// Oracle "reports the source path when rename fails because the source is
/// missing" (`nodejs-env.test.ts:254-269`).
#[tokio::test]
async fn reports_the_source_path_when_rename_fails_because_the_source_is_missing() {
    let (_dir, root) = temp_root();
    let env = env_at(&root);
    unwrap_ok(
        env.write_file(
            "destination.txt",
            FileContent::Text("unchanged".into()),
            ctx(),
        )
        .await,
    );
    let result = env
        .rename_file("missing-source.txt", "destination.txt", ctx())
        .await;
    let Err(error) = result else {
        panic!("expected error");
    };
    assert_eq!(error.code, FileErrorCode::NotFound);
    assert_eq!(
        error.path.as_deref(),
        Some(joined(&root, "missing-source.txt").as_str())
    );
    assert_eq!(
        unwrap_ok(env.read_text_file("destination.txt", ctx()).await),
        "unchanged"
    );
}

/// Oracle "creates temporary directories and files"
/// (`nodejs-env.test.ts:271-279`).
#[tokio::test]
async fn creates_temporary_directories_and_files() {
    let (_dir, root) = temp_root();
    let env = env_at(&root);
    let temp_dir = unwrap_ok(env.create_temp_dir(Some("node-env-test-"), ctx()).await);
    assert!(Path::new(&temp_dir).is_dir());
    let temp_file = unwrap_ok(
        env.create_temp_file(
            Some(&TempFileOptions {
                prefix: Some("prefix-".into()),
                suffix: Some(".txt".into()),
            }),
            ctx(),
        )
        .await,
    );
    assert!(Path::new(&temp_file).is_file());
    assert!(temp_file.ends_with(".txt"));
}

/// Oracle "honors createDir recursive false and remove recursive/force
/// options" (`nodejs-env.test.ts:281-297`).
#[tokio::test]
async fn honors_create_dir_recursive_false_and_remove_recursive_force_options() {
    let (_dir, root) = temp_root();
    let env = env_at(&root);
    let create_result = env
        .create_dir(
            "missing/child",
            Some(&CreateDirOptions {
                recursive: Some(false),
            }),
            ctx(),
        )
        .await;
    let Err(error) = create_result else {
        panic!("expected error");
    };
    assert_eq!(error.code, FileErrorCode::NotFound);

    unwrap_ok(
        env.write_file(
            "dir/child/file.txt",
            FileContent::Text("hello".into()),
            ctx(),
        )
        .await,
    );
    let remove_directory = env
        .remove(
            "dir",
            Some(&RemoveOptions {
                recursive: Some(false),
                force: None,
            }),
            ctx(),
        )
        .await;
    assert!(remove_directory.is_err());
    unwrap_ok(
        env.remove(
            "dir",
            Some(&RemoveOptions {
                recursive: Some(true),
                force: None,
            }),
            ctx(),
        )
        .await,
    );
    assert!(!unwrap_ok(env.exists("dir", ctx()).await));

    let remove_missing = env
        .remove(
            "missing",
            Some(&RemoveOptions {
                recursive: None,
                force: Some(false),
            }),
            ctx(),
        )
        .await;
    assert!(remove_missing.is_err());
    unwrap_ok(
        env.remove(
            "missing",
            Some(&RemoveOptions {
                recursive: None,
                force: Some(true),
            }),
            ctx(),
        )
        .await,
    );
}

/// Oracle "returns aborted results for pre-aborted cancellable file
/// operations" (`nodejs-env.test.ts:299-319`).
#[tokio::test]
async fn returns_aborted_results_for_pre_aborted_cancellable_file_operations() {
    let (_dir, root) = temp_root();
    let env = env_at(&root);
    unwrap_ok(
        env.write_file("file.txt", FileContent::Text("hello".into()), ctx())
            .await,
    );
    let controller = CancellationToken::new();
    controller.cancel();
    let context = with_abort_signal(controller, ctx());

    let check = |name: &str, result: Result<(), FileError>| {
        let Err(error) = result else {
            panic!("{name}: expected aborted result");
        };
        assert_eq!(error.code, FileErrorCode::Aborted, "{name}");
    };
    check(
        "read_text_file",
        env.read_text_file("file.txt", context.clone())
            .await
            .map(|_| ()),
    );
    check(
        "read_text_lines",
        env.read_text_lines("file.txt", None, context.clone())
            .await
            .map(|_| ()),
    );
    check(
        "read_binary_file",
        env.read_binary_file("file.txt", context.clone())
            .await
            .map(|_| ()),
    );
    check(
        "write_file",
        env.write_file(
            "other.txt",
            FileContent::Text("hello".into()),
            context.clone(),
        )
        .await,
    );
    check(
        "rename_file",
        env.rename_file("file.txt", "renamed.txt", context.clone())
            .await,
    );
    check(
        "list_dir",
        env.list_dir(".", context.clone()).await.map(|_| ()),
    );
}

/// Oracle "cleanup is best-effort" (`nodejs-env.test.ts:321-325`).
#[tokio::test]
async fn cleanup_is_best_effort() {
    let (_dir, root) = temp_root();
    let env = env_at(&root);
    Shell::cleanup(&env, ctx()).await;
}

/// Oracle "executes commands in cwd with env overrides"
/// (`nodejs-env.test.ts:327-339`).
#[tokio::test]
async fn executes_commands_in_cwd_with_env_overrides() {
    let (_dir, root) = temp_root();
    let env = env_at(&root);
    let options = ShellExecOptions {
        env: Some(BTreeMap::from([(
            "NODE_ENV_TEST".to_string(),
            "ok".to_string(),
        )])),
        ..Default::default()
    };
    // Windows Git Bash maps %TEMP% to /tmp, so $PWD is the MSYS path; print
    // it through cygpath -m (mixed slashes) and compare against the mapped
    // temp root. The oracle's assertion intent — the command runs in the env
    // cwd and sees the override — is unchanged.
    let (command, expected_prefix) = if cfg!(windows) {
        (
            r#"printf '%s:%s' "$(cygpath -m "$PWD")" "$NODE_ENV_TEST""#.to_string(),
            canonical(&root).replace('\\', "/"),
        )
    } else {
        (
            r#"printf '%s:%s' "$PWD" "$NODE_ENV_TEST""#.to_string(),
            canonical(&root),
        )
    };
    let (result, output) = collect_shell_output(&env, &command, Some(options), ctx()).await;
    let result = unwrap_ok(result);
    assert_eq!(
        output.as_ref().map(|view| view.text.clone()).as_deref(),
        Some(format!("{expected_prefix}:ok").as_str()),
        "output: {:?}",
        output.as_ref().map(|view| view.text.clone())
    );
    assert_eq!(result.exit_code, 0);
}

/// Oracle "applies string shell environment overrides when ..."
/// (`nodejs-env.test.ts:341-370`).
#[tokio::test]
async fn applies_string_shell_environment_overrides() {
    let cases = vec![
        (
            "a missing override preserves the base value",
            None,
            "x:/stale/parent.jsonl",
        ),
        (
            "an empty override shadows the base value",
            Some(BTreeMap::from([(
                "PI_SESSION_FILE".to_string(),
                String::new(),
            )])),
            "x:",
        ),
        (
            "a string override replaces the base value",
            Some(BTreeMap::from([(
                "PI_SESSION_FILE".to_string(),
                "/sessions/current.jsonl".to_string(),
            )])),
            "x:/sessions/current.jsonl",
        ),
    ];
    for (_description, overrides, expected_session_file) in cases {
        let (_dir, root) = temp_root();
        let env = env_at(&root).with_shell_env(BTreeMap::from([
            (
                "PI_SESSION_FILE".to_string(),
                "/stale/parent.jsonl".to_string(),
            ),
            ("PI_CODING_AGENT".to_string(), "true".to_string()),
            (
                "PI_NODE_ENV_PRESERVED_TEST".to_string(),
                "preserved".to_string(),
            ),
        ]));
        let options = ShellExecOptions {
            env: overrides,
            ..Default::default()
        };
        let (result, output) = collect_shell_output(
            &env,
            r#"printf '%s:%s|%s|%s' "${PI_SESSION_FILE+x}" "${PI_SESSION_FILE-}" "$PI_CODING_AGENT" "$PI_NODE_ENV_PRESERVED_TEST""#,
            Some(options),
            ctx(),
        )
        .await;
        unwrap_ok(result);
        assert_eq!(
            output.as_ref().map(|view| view.text.clone()).as_deref(),
            Some(format!("{expected_session_file}|true|preserved").as_str()),
            "case: {_description}"
        );
    }
}

/// Oracle "can replace rather than inherit the default shell environment"
/// (`nodejs-env.test.ts:372-393`).
#[tokio::test]
async fn can_replace_rather_than_inherit_the_default_shell_environment() {
    let inherited_key = "PI_NODE_ENV_INHERITED_TEST";
    let configured_key = "PI_NODE_ENV_CONFIGURED_TEST";
    let explicit_key = "PI_NODE_ENV_EXPLICIT_TEST";
    // SAFETY-free test setup: single-threaded env mutation guarded by the
    // test runner's thread; restored below like the oracle's finally block.
    std::env::set_var(inherited_key, "host");
    let (_dir, root) = temp_root();
    let env = env_at(&root).with_shell_env(BTreeMap::from([(
        configured_key.to_string(),
        "configured".to_string(),
    )]));
    let options = ShellExecOptions {
        env: Some(BTreeMap::from([(
            explicit_key.to_string(),
            "explicit".to_string(),
        )])),
        inherit_env: Some(false),
        ..Default::default()
    };
    let (result, output) = collect_shell_output(
        &env,
        &format!(
            r#"printf '%s:%s:%s' "${{{inherited_key}-}}" "${{{configured_key}-}}" "${{{explicit_key}-}}""#
        ),
        Some(options),
        ctx(),
    )
    .await;
    unwrap_ok(result);
    assert_eq!(
        output.as_ref().map(|view| view.text.clone()).as_deref(),
        Some("::explicit")
    );
    std::env::remove_var(inherited_key);
}

/// Oracle "uses stdin command transport for legacy WSL bash paths"
/// (`nodejs-env.test.ts:395-439`). Upstream gates on win32 (the port's
/// platform gates are compile-time); the transport behavior is driven from
/// unix where the fake shell script is executable.
#[cfg(unix)]
#[tokio::test]
async fn uses_stdin_command_transport_for_legacy_wsl_bash_paths() {
    let (_dir, root) = temp_root();
    let shell_path = r"C:\Windows\System32\bash.exe";
    let env = env_at(&root);
    unwrap_ok(
        env.write_file(
            shell_path,
            FileContent::Text(
                "#!/bin/sh\nprintf 'args:%s\\n' \"$*\" >&2\nexec /bin/bash \"$@\"\n".into(),
            ),
            ctx(),
        )
        .await,
    );
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            Path::new(&root).join(shell_path),
            std::fs::Permissions::from_mode(0o755),
        )
        .expect("chmod");
    }
    // Upstream `process.chdir(root)` + PATH prepend (nodejs-env.test.ts:409):
    // the custom shell path is checked and spawned as given, so on unix it
    // must resolve against the fixture root, and since the backslash name
    // contains no `/` the spawn PATH-searches — find it via the prepended
    // root, exactly like upstream.
    struct RestoreProcessEnv {
        cwd: String,
        path_set: bool,
        path: String,
    }
    impl Drop for RestoreProcessEnv {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.cwd);
            if self.path_set {
                std::env::set_var("PATH", &self.path);
            } else {
                std::env::remove_var("PATH");
            }
        }
    }
    let _env_guard = RestoreProcessEnv {
        cwd: std::env::current_dir()
            .expect("cwd")
            .to_string_lossy()
            .into_owned(),
        path_set: std::env::var("PATH").is_ok(),
        path: std::env::var("PATH").unwrap_or_default(),
    };
    std::env::set_current_dir(&root).expect("chdir to fixture root");
    std::env::set_var(
        "PATH",
        format!("{root}:{}", std::env::var("PATH").unwrap_or_default()),
    );
    let wsl_env = env_at(&root).with_shell_path(shell_path);
    let (result, output) = collect_shell_output(
        &wsl_env,
        "name='World'; echo \"Hello, ${name}!\"",
        None,
        ctx(),
    )
    .await;
    let result = unwrap_ok(result);
    let text = output.expect("output").text;
    assert!(text.contains("Hello, World!"), "{text}");
    assert!(text.contains("args:-s"), "{text}");
    assert_eq!(result.exit_code, 0);
}

/// Oracle (win32) "settles after the shell exits when a detached descendant
/// retains inherited stdio" (`nodejs-env.test.ts:441-466`).
#[cfg(windows)]
#[tokio::test]
async fn settles_after_the_shell_exits_when_a_detached_descendant_retains_inherited_stdio() {
    let (_dir, root) = temp_root();
    let pid_file = Path::new(&root).join("grandchild.pid");
    let env = env_at(&root);
    let controller = CancellationToken::new();
    let context = with_abort_signal(controller.clone(), ctx());
    let command = create_inherited_stdio_command(&path_string(&pid_file));
    let collected = tokio::time::timeout(
        Duration::from_millis(3000),
        collect_shell_output(&env, &command, None, context),
    )
    .await;
    match collected {
        Ok((result, output)) => {
            unwrap_ok(result);
            let text = output.expect("output").text;
            assert!(text.contains("child-exiting"), "{text}");
        }
        Err(_) => {
            // `withTimeout(..., 3000, () => controller.abort())` — a timeout
            // rejects, failing the test.
            controller.cancel();
            cleanup_detached_child(&pid_file);
            panic!("settles timed out after 3000ms");
        }
    }
    cleanup_detached_child(&pid_file);
}

/// Oracle "cleanup terminates active shell processes"
/// (`nodejs-env.test.ts:468-478`).
#[tokio::test]
async fn cleanup_terminates_active_shell_processes() {
    let (_dir, root) = temp_root();
    let env = env_at(&root);
    // Upstream's `env.exec(...)` promise starts eagerly; Rust futures are
    // lazy, so spawn the run the way the real caller would.
    let exec_env = env.clone();
    let execution = tokio::spawn(async move {
        exec_env
            .exec("touch started; sleep 60", None, background_context())
            .await
    });
    let mut started = false;
    for _ in 0..200 {
        if unwrap_ok(env.exists("started", ctx()).await) {
            started = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    if !started {
        // Windows spawn latency can exceed the poll budget; surface the
        // exec's own view of the cwd before failing.
        let (probe_result, probe_view) =
            collect_shell_output(&env, "pwd; command -v touch; ls -la", None, ctx()).await;
        panic!(
            "started not seen; probe result: {probe_result:?}, probe output: {:?}",
            probe_view.map(|view| view.text)
        );
    }
    Shell::cleanup(&env, ctx()).await;
    let result = tokio::time::timeout(Duration::from_millis(3000), execution)
        .await
        .expect("exec resolves after cleanup");
    let exec_result = result.expect("exec succeeds after cleanup");
    unwrap_ok(exec_result);
}

/// Oracle "combines stdout and stderr into one bounded view"
/// (`nodejs-env.test.ts:480-501`).
#[tokio::test]
async fn combines_stdout_and_stderr_into_one_bounded_view() {
    let (_dir, root) = temp_root();
    let env = env_at(&root);
    let kinds: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let kinds_writer = Arc::clone(&kinds);
    let output: Arc<Mutex<Option<ShellOutputView>>> = Arc::new(Mutex::new(None));
    let output_writer = Arc::clone(&output);
    let options = ShellExecOptions {
        on_update: Some(Arc::new(move |update, _context| {
            kinds_writer.lock().unwrap().push(
                match update {
                    ShellOutputUpdate::Replace { .. } => "replace",
                    ShellOutputUpdate::Append { .. } => "append",
                    ShellOutputUpdate::Slide { .. } => "slide",
                    ShellOutputUpdate::Metadata { .. } => "metadata",
                }
                .to_string(),
            );
            let previous = output_writer.lock().unwrap().take();
            *output_writer.lock().unwrap() = Some(apply_shell_output_update(previous, update));
        })),
        ..Default::default()
    };
    let result = unwrap_ok(
        env.exec("printf out; printf err >&2", Some(&options), ctx())
            .await,
    );
    assert_eq!(result.exit_code, 0);
    let view = output.lock().unwrap().clone().expect("output");
    assert!(view.text.contains("out"), "{}", view.text);
    assert!(view.text.contains("err"), "{}", view.text);
    assert_eq!(kinds.lock().unwrap()[0], "replace");
}

/// Oracle "reports a missing working directory before spawning"
/// (`nodejs-env.test.ts:503-512`).
#[tokio::test]
async fn reports_a_missing_working_directory_before_spawning() {
    let (_dir, root) = temp_root();
    let env = env_at(&join_raw(&root, "missing"));
    let result = env.exec("printf ok", None, ctx()).await;
    let Err(error) = result else {
        panic!("expected error");
    };
    assert_eq!(error.code, ExecutionErrorCode::SpawnError);
    assert!(
        error.message.contains("Working directory does not exist"),
        "{}",
        error.message
    );
}

/// Oracle "returns non-zero command exit codes as successful execution
/// results" (`nodejs-env.test.ts:514-520`).
#[tokio::test]
async fn returns_non_zero_command_exit_codes_as_successful_execution_results() {
    let (_dir, root) = temp_root();
    let env = env_at(&root);
    let result = unwrap_ok(env.exec("exit 7", None, ctx()).await);
    assert_eq!(result.exit_code, 7);
    assert_eq!(result.metadata.truncation.total_bytes, 0);
}

/// Oracle (unix) "maps signal-killed processes to a non-zero exit code"
/// (`nodejs-env.test.ts:522-528`).
#[cfg(unix)]
#[tokio::test]
async fn maps_signal_killed_processes_to_a_non_zero_exit_code() {
    let (_dir, root) = temp_root();
    let env = env_at(&root);
    let result = unwrap_ok(env.exec("kill -9 $$", None, ctx()).await);
    assert_eq!(result.exit_code, 128 + 9);
}

/// Oracle "returns timeout errors for commands exceeding the timeout"
/// (`nodejs-env.test.ts:530-536`); the timeout is whole seconds in the port.
#[tokio::test]
async fn returns_timeout_errors_for_commands_exceeding_the_timeout() {
    let (_dir, root) = temp_root();
    let env = env_at(&root);
    let options = ShellExecOptions {
        timeout: Some(1.0),
        ..Default::default()
    };
    let result = env.exec("sleep 5", Some(&options), ctx()).await;
    let Err(error) = result else {
        panic!("expected error");
    };
    assert_eq!(error.code, ExecutionErrorCode::Timeout);
}

/// Oracle "returns callback errors from exec stream handlers"
/// (`nodejs-env.test.ts:538-552`). The port's callback throw channel is a
/// panic (see the output-capture module docs).
#[tokio::test]
async fn returns_callback_errors_from_exec_stream_handlers() {
    let (_dir, root) = temp_root();
    let env = env_at(&root);
    let options = ShellExecOptions {
        on_update: Some(Arc::new(|_update, _context| {
            panic!("callback failed");
        })),
        ..Default::default()
    };
    let result = env.exec("printf out", Some(&options), ctx()).await;
    let Err(error) = result else {
        panic!("expected error");
    };
    assert_eq!(error.code, ExecutionErrorCode::CallbackError);
    assert_eq!(error.message, "callback failed");
}

/// Oracle "returns shell unavailable and spawn errors"
/// (`nodejs-env.test.ts:554-568`).
#[tokio::test]
async fn returns_shell_unavailable_and_spawn_errors() {
    let (_dir, root) = temp_root();
    let missing_shell_env = env_at(&root).with_shell_path(join_raw(&root, "missing-shell"));
    let missing_shell = missing_shell_env.exec("printf ok", None, ctx()).await;
    let Err(error) = missing_shell else {
        panic!("expected error");
    };
    assert_eq!(error.code, ExecutionErrorCode::ShellUnavailable);

    let shell_path = join_raw(&root, "not-executable-shell");
    let env = env_at(&root);
    unwrap_ok(
        env.write_file(
            &shell_path,
            FileContent::Text("not executable".into()),
            ctx(),
        )
        .await,
    );
    let spawn_error_env = env_at(&root).with_shell_path(shell_path);
    let spawn_error = spawn_error_env.exec("printf ok", None, ctx()).await;
    let Err(error) = spawn_error else {
        panic!("expected error");
    };
    assert_eq!(error.code, ExecutionErrorCode::SpawnError);
}

/// Oracle "returns an aborted result for aborted commands"
/// (`nodejs-env.test.ts:570-579`).
#[tokio::test]
async fn returns_an_aborted_result_for_aborted_commands() {
    let (_dir, root) = temp_root();
    let env = env_at(&root);
    let controller = CancellationToken::new();
    let context = with_abort_signal(controller.clone(), ctx());
    let promise = env.exec("sleep 5", None, context);
    controller.cancel();
    let result = promise.await;
    let Err(error) = result else {
        panic!("expected error");
    };
    assert_eq!(error.code, ExecutionErrorCode::Aborted);
}

/// Oracle "does not create a spill before bounded output crosses its limits"
/// (`nodejs-env.test.ts:625-639`).
#[tokio::test]
async fn does_not_create_a_spill_before_bounded_output_crosses_its_limits() {
    let (_dir, root) = temp_root();
    let env = env_at(&root);
    let options = ShellExecOptions {
        capture: Some(ShellOutputCaptureOptions {
            limits: ShellOutputLimits {
                max_bytes: 100,
                max_lines: 10,
                retain: Some(ShellOutputRetention::Tail),
            },
            spill: Some(true),
        }),
        on_update: Some(Arc::new(|_update, _context| {})),
        ..Default::default()
    };
    let result = unwrap_ok(env.exec("printf short", Some(&options), ctx()).await);
    assert_eq!(result.metadata.spill_path, None);
}

/// Oracle "preserves exact raw bytes in the spill while decoding a bounded
/// text view" (`nodejs-env.test.ts:641-657`).
#[tokio::test]
async fn preserves_exact_raw_bytes_in_the_spill_while_decoding_a_bounded_text_view() {
    let (_dir, root) = temp_root();
    let env = env_at(&root);
    let options = ShellExecOptions {
        capture: Some(ShellOutputCaptureOptions {
            limits: ShellOutputLimits {
                max_bytes: 1,
                max_lines: 10,
                retain: Some(ShellOutputRetention::Tail),
            },
            spill: Some(true),
        }),
        on_update: Some(Arc::new(|_update, _context| {})),
        ..Default::default()
    };
    let result = unwrap_ok(
        env.exec(
            r#"node -e "process.stdout.write(Buffer.from([102,128,0,111]))""#,
            Some(&options),
            ctx(),
        )
        .await,
    );
    let spill_path = result.metadata.spill_path.expect("spill path");
    let bytes = unwrap_ok(env.read_binary_file(&spill_path, ctx()).await);
    assert_eq!(bytes, vec![0x66, 0x80, 0x00, 0x6f]);
}

/// The oracle's `FailingSpillExecutionEnv` subclass
/// (`nodejs-env.test.ts:83-93`): overrides `createTempFile` for the spill
/// prefix so the spill write lands in a missing directory. The port mirrors
/// the JS virtual dispatch by running `exec_via` against `self`.
#[derive(Clone)]
struct FailingSpillExecutionEnv {
    inner: NodeExecutionEnv,
}

impl FailingSpillExecutionEnv {
    fn new(root: &str) -> Self {
        FailingSpillExecutionEnv {
            inner: NodeExecutionEnv::new(root),
        }
    }
}

impl FileSystem for FailingSpillExecutionEnv {
    fn cwd(&self) -> &str {
        self.inner.cwd()
    }
    fn absolute_path<'a>(
        &'a self,
        path: &str,
        c: Context,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        self.inner.absolute_path(path, c)
    }
    fn join_path<'a>(
        &'a self,
        parts: &[String],
        c: Context,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        self.inner.join_path(parts, c)
    }
    fn read_text_file<'a>(
        &'a self,
        path: &str,
        c: Context,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        self.inner.read_text_file(path, c)
    }
    fn open_text_line_reader<'a>(
        &'a self,
        path: &str,
        c: Context,
    ) -> BoxFuture<'a, Result<Arc<dyn TextLineReader>, FileError>> {
        self.inner.open_text_line_reader(path, c)
    }
    fn read_text_lines<'a>(
        &'a self,
        path: &str,
        options: Option<&ReadTextLinesOptions>,
        c: Context,
    ) -> BoxFuture<'a, Result<Vec<String>, FileError>> {
        self.inner.read_text_lines(path, options, c)
    }
    fn read_binary_file<'a>(
        &'a self,
        path: &str,
        c: Context,
    ) -> BoxFuture<'a, Result<Vec<u8>, FileError>> {
        self.inner.read_binary_file(path, c)
    }
    fn write_file<'a>(
        &'a self,
        path: &str,
        content: FileContent,
        c: Context,
    ) -> BoxFuture<'a, Result<(), FileError>> {
        self.inner.write_file(path, content, c)
    }
    fn append_file<'a>(
        &'a self,
        path: &str,
        content: FileContent,
        c: Context,
    ) -> BoxFuture<'a, Result<(), FileError>> {
        self.inner.append_file(path, content, c)
    }
    fn rename_file<'a>(
        &'a self,
        source_path: &str,
        destination_path: &str,
        c: Context,
    ) -> BoxFuture<'a, Result<(), FileError>> {
        self.inner.rename_file(source_path, destination_path, c)
    }
    fn file_info<'a>(
        &'a self,
        path: &str,
        c: Context,
    ) -> BoxFuture<'a, Result<FileInfo, FileError>> {
        self.inner.file_info(path, c)
    }
    fn list_dir<'a>(
        &'a self,
        path: &str,
        c: Context,
    ) -> BoxFuture<'a, Result<Vec<FileInfo>, FileError>> {
        self.inner.list_dir(path, c)
    }
    fn canonical_path<'a>(
        &'a self,
        path: &str,
        c: Context,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        self.inner.canonical_path(path, c)
    }
    fn exists<'a>(&'a self, path: &str, c: Context) -> BoxFuture<'a, Result<bool, FileError>> {
        self.inner.exists(path, c)
    }
    fn create_dir<'a>(
        &'a self,
        path: &str,
        options: Option<&CreateDirOptions>,
        c: Context,
    ) -> BoxFuture<'a, Result<(), FileError>> {
        self.inner.create_dir(path, options, c)
    }
    fn remove<'a>(
        &'a self,
        path: &str,
        options: Option<&RemoveOptions>,
        c: Context,
    ) -> BoxFuture<'a, Result<(), FileError>> {
        self.inner.remove(path, options, c)
    }
    fn create_temp_dir<'a>(
        &'a self,
        prefix: Option<&str>,
        c: Context,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        self.inner.create_temp_dir(prefix, c)
    }
    /// The override: the spill prefix gets a path in a missing directory.
    fn create_temp_file<'a>(
        &'a self,
        options: Option<&TempFileOptions>,
        _context: Context,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        let options = options.cloned();
        let is_spill = options
            .as_ref()
            .and_then(|options| options.prefix.as_deref())
            == Some("pi-output-");
        Box::pin(async move {
            if is_spill {
                return Ok(join_raw(&join_raw(self.cwd(), "missing"), "spill.log"));
            }
            self.inner
                .create_temp_file(options.as_ref(), background_context())
                .await
        })
    }
    fn cleanup<'a>(&'a self, c: Context) -> BoxFuture<'a, ()> {
        Shell::cleanup(&self.inner, c)
    }
}

impl Shell for FailingSpillExecutionEnv {
    fn exec<'a>(
        &'a self,
        command: &str,
        options: Option<&ShellExecOptions>,
        context: Context,
    ) -> BoxFuture<'a, Result<ShellExecResult, ExecutionError>> {
        let env: Arc<dyn ExecutionEnv> = Arc::new(self.clone());
        let command = command.to_string();
        let options = options.cloned();
        Box::pin(async move {
            exec_via(
                env,
                self.inner.runtime.clone(),
                &command,
                options.as_ref(),
                context,
            )
            .await
        })
    }
    fn cleanup<'a>(&'a self, c: Context) -> BoxFuture<'a, ()> {
        Shell::cleanup(&self.inner, c)
    }
}

impl ExecutionEnv for FailingSpillExecutionEnv {}

/// Oracle "fails rather than silently losing a requested spill"
/// (`nodejs-env.test.ts:659-674`).
#[tokio::test]
async fn fails_rather_than_silently_losing_a_requested_spill() {
    let (_dir, root) = temp_root();
    let env = FailingSpillExecutionEnv::new(&root);
    let options = ShellExecOptions {
        capture: Some(ShellOutputCaptureOptions {
            limits: ShellOutputLimits {
                max_bytes: 10,
                max_lines: 10,
                retain: Some(ShellOutputRetention::Tail),
            },
            spill: Some(true),
        }),
        on_update: Some(Arc::new(|_update, _context| {})),
        ..Default::default()
    };
    let result = env
        .exec("printf 12345678901234567890", Some(&options), ctx())
        .await;
    let Err(error) = result else {
        panic!("expected error");
    };
    assert_eq!(error.code, ExecutionErrorCode::Unknown);
    assert!(
        error
            .message
            .contains("Failed to preserve complete shell output"),
        "{}",
        error.message
    );
}

/// Oracle "preserves complete output when spill-stream backpressure pauses a
/// process that exits quickly" (`nodejs-env.test.ts:676-692`).
#[tokio::test]
async fn preserves_complete_output_when_spill_stream_backpressure_pauses_a_process_that_exits_quickly(
) {
    let (_dir, root) = temp_root();
    let env = env_at(&root);
    let options = ShellExecOptions {
        capture: Some(ShellOutputCaptureOptions {
            limits: ShellOutputLimits {
                max_bytes: 10,
                max_lines: 10,
                retain: Some(ShellOutputRetention::Tail),
            },
            spill: Some(true),
        }),
        on_update: Some(Arc::new(|_update, _context| {})),
        ..Default::default()
    };
    let result = unwrap_ok(
        env.exec(
            r#"node -e "process.stdout.write('x'.repeat(500000))""#,
            Some(&options),
            ctx(),
        )
        .await,
    );
    let spill_path = result.metadata.spill_path.expect("spill path");
    let text = unwrap_ok(env.read_text_file(&spill_path, ctx()).await);
    assert_eq!(text.chars().count(), 500_000);
}

/// Oracle "captures large shell output to a full output file through the
/// execution env" (`nodejs-env.test.ts:694-705`).
#[tokio::test]
async fn captures_large_shell_output_to_a_full_output_file_through_the_execution_env() {
    let (_dir, root) = temp_root();
    let env = env_at(&root);
    let result =
        unwrap_ok(execute_shell_with_capture(&env, "yes line | head -n 15000", None, ctx()).await);
    assert!(result.truncated);
    let full_output_path = result.full_output_path.expect("full output path");
    let full_output = unwrap_ok(env.read_text_file(&full_output_path, ctx()).await);
    assert!(full_output.split('\n').count() > 10_000);
    assert!(result.output.len() < full_output.len());
}

/// A spill-start gate like the oracle's `FailingSpillExecutionEnv` subclass:
/// `create_temp_file` announces the spill-start window and only resolves when
/// the test releases it, so a follow-up chunk is guaranteed to arrive while
/// the spill is still starting (upstream `startSpill` queues it into
/// `spillQueue`; nodejs.ts:559-565).
#[derive(Clone)]
struct GatedSpillExecutionEnv {
    inner: NodeExecutionEnv,
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    /// When set, `create_temp_file` returns this exact (pre-created) path
    /// after the gate instead of delegating to the inner env.
    fixed_spill_path: Option<String>,
}

impl GatedSpillExecutionEnv {
    fn new(root: &str) -> Self {
        GatedSpillExecutionEnv::new_with_spill_path(root, None)
    }

    fn new_with_spill_path(root: &str, fixed_spill_path: Option<String>) -> Self {
        GatedSpillExecutionEnv {
            inner: NodeExecutionEnv::new(root),
            entered: Arc::new(tokio::sync::Notify::new()),
            release: Arc::new(tokio::sync::Notify::new()),
            fixed_spill_path,
        }
    }
}

impl FileSystem for GatedSpillExecutionEnv {
    fn cwd(&self) -> &str {
        self.inner.cwd()
    }
    fn absolute_path<'a>(
        &'a self,
        path: &str,
        c: Context,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        self.inner.absolute_path(path, c)
    }
    fn join_path<'a>(
        &'a self,
        parts: &[String],
        c: Context,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        self.inner.join_path(parts, c)
    }
    fn read_text_file<'a>(
        &'a self,
        path: &str,
        c: Context,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        self.inner.read_text_file(path, c)
    }
    fn open_text_line_reader<'a>(
        &'a self,
        path: &str,
        c: Context,
    ) -> BoxFuture<'a, Result<Arc<dyn TextLineReader>, FileError>> {
        self.inner.open_text_line_reader(path, c)
    }
    fn read_text_lines<'a>(
        &'a self,
        path: &str,
        options: Option<&ReadTextLinesOptions>,
        c: Context,
    ) -> BoxFuture<'a, Result<Vec<String>, FileError>> {
        self.inner.read_text_lines(path, options, c)
    }
    fn read_binary_file<'a>(
        &'a self,
        path: &str,
        c: Context,
    ) -> BoxFuture<'a, Result<Vec<u8>, FileError>> {
        self.inner.read_binary_file(path, c)
    }
    fn write_file<'a>(
        &'a self,
        path: &str,
        content: FileContent,
        c: Context,
    ) -> BoxFuture<'a, Result<(), FileError>> {
        self.inner.write_file(path, content, c)
    }
    fn append_file<'a>(
        &'a self,
        path: &str,
        content: FileContent,
        c: Context,
    ) -> BoxFuture<'a, Result<(), FileError>> {
        self.inner.append_file(path, content, c)
    }
    fn rename_file<'a>(
        &'a self,
        source_path: &str,
        destination_path: &str,
        c: Context,
    ) -> BoxFuture<'a, Result<(), FileError>> {
        self.inner.rename_file(source_path, destination_path, c)
    }
    fn file_info<'a>(
        &'a self,
        path: &str,
        c: Context,
    ) -> BoxFuture<'a, Result<FileInfo, FileError>> {
        self.inner.file_info(path, c)
    }
    fn list_dir<'a>(
        &'a self,
        path: &str,
        c: Context,
    ) -> BoxFuture<'a, Result<Vec<FileInfo>, FileError>> {
        self.inner.list_dir(path, c)
    }
    fn canonical_path<'a>(
        &'a self,
        path: &str,
        c: Context,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        self.inner.canonical_path(path, c)
    }
    fn exists<'a>(&'a self, path: &str, c: Context) -> BoxFuture<'a, Result<bool, FileError>> {
        self.inner.exists(path, c)
    }
    fn create_dir<'a>(
        &'a self,
        path: &str,
        options: Option<&CreateDirOptions>,
        c: Context,
    ) -> BoxFuture<'a, Result<(), FileError>> {
        self.inner.create_dir(path, options, c)
    }
    fn remove<'a>(
        &'a self,
        path: &str,
        options: Option<&RemoveOptions>,
        c: Context,
    ) -> BoxFuture<'a, Result<(), FileError>> {
        self.inner.remove(path, options, c)
    }
    fn create_temp_dir<'a>(
        &'a self,
        prefix: Option<&str>,
        c: Context,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        self.inner.create_temp_dir(prefix, c)
    }
    fn create_temp_file<'a>(
        &'a self,
        options: Option<&TempFileOptions>,
        _context: Context,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        let options = options.cloned();
        let is_spill = options
            .as_ref()
            .and_then(|options| options.prefix.as_deref())
            == Some("pi-output-");
        let entered = Arc::clone(&self.entered);
        let release = Arc::clone(&self.release);
        let fixed_spill_path = self.fixed_spill_path.clone();
        Box::pin(async move {
            if is_spill {
                // Announce the spill-start window and hold it open.
                entered.notify_one();
                release.notified().await;
                if let Some(fixed) = fixed_spill_path {
                    return Ok(fixed);
                }
            }
            self.inner
                .create_temp_file(options.as_ref(), background_context())
                .await
        })
    }
    fn cleanup<'a>(&'a self, c: Context) -> BoxFuture<'a, ()> {
        Shell::cleanup(&self.inner, c)
    }
}

impl Shell for GatedSpillExecutionEnv {
    fn exec<'a>(
        &'a self,
        command: &str,
        options: Option<&ShellExecOptions>,
        context: Context,
    ) -> BoxFuture<'a, Result<ShellExecResult, ExecutionError>> {
        let env: Arc<dyn ExecutionEnv> = Arc::new(self.clone());
        let command = command.to_string();
        let options = options.cloned();
        Box::pin(async move {
            exec_via(
                env,
                self.inner.runtime.clone(),
                &command,
                options.as_ref(),
                context,
            )
            .await
        })
    }
    fn cleanup<'a>(&'a self, c: Context) -> BoxFuture<'a, ()> {
        Shell::cleanup(&self.inner, c)
    }
}

impl ExecutionEnv for GatedSpillExecutionEnv {}

/// Regression test for the spill-start window: a chunk that arrives while
/// `create_temp_file` is still awaited (was_truncated already true, spill not
/// open) must reach the spill file, not be dropped. Upstream queues it into
/// `spillQueue` and drains after the temp file resolves.
#[tokio::test]
async fn queues_chunks_that_arrive_during_the_spill_start_window() {
    let (_dir, root) = temp_root();
    let env = GatedSpillExecutionEnv::new(&root);
    let mut options = ShellExecOptions {
        capture: Some(ShellOutputCaptureOptions {
            limits: ShellOutputLimits {
                max_bytes: 10,
                max_lines: 10,
                retain: Some(ShellOutputRetention::Tail),
            },
            spill: Some(true),
        }),
        on_update: Some(Arc::new(|_update, _context| {})),
        ..Default::default()
    };
    let _ = &mut options;
    let stdout_run = "A".repeat(100);
    let stderr_run = "B".repeat(100);
    let command = format!("printf '{stdout_run}'; printf '{stderr_run}' >&2");

    let execution = {
        let env = env.clone();
        tokio::spawn(async move { env.exec(&command, Some(&options), ctx()).await })
    };

    // Wait for the spill-start window (create_temp_file entered), then give
    // the stderr reader time to route its chunk through the window.
    tokio::time::timeout(Duration::from_secs(5), env.entered.notified())
        .await
        .expect("spill start entered the gate");
    tokio::time::sleep(Duration::from_millis(200)).await;
    env.release.notify_one();

    let result = tokio::time::timeout(Duration::from_secs(10), execution)
        .await
        .expect("exec resolves");
    let result = unwrap_ok(result.expect("join"));
    let spill_path = result.metadata.spill_path.expect("spill path");
    let bytes = unwrap_ok(env.read_binary_file(&spill_path, ctx()).await);

    assert!(
        bytes
            .windows(100)
            .any(|window| window == stdout_run.as_bytes()),
        "stdout run missing from spill: {} bytes",
        bytes.len()
    );
    assert!(
        bytes
            .windows(100)
            .any(|window| window == stderr_run.as_bytes()),
        "stderr run missing from spill (dropped in the spill-start window?): {} bytes",
        bytes.len()
    );
    assert_eq!(bytes.len(), 200, "spill must hold the complete output");
}

/// Regression test for the spill drain/writer lock cycle: with a deep
/// queue and a writer whose appends fail, the exec must settle with the
/// spill error (killing the child) instead of parking forever. The queue
/// goes deep before the spill ever starts: the byte limit exceeds the pipe
/// chunk size, so every pre-truncation chunk queues as a prefix (~30+ chunks
/// for 2.7MB of output against 64KB reads) until the crossing drives the
/// start with the queue already past the channel capacity (8). The writer's
/// appends fail because the test holds a mandatory byte-range lock over the
/// spill file (fs2/LockFileEx on Windows).
#[cfg(windows)]
#[tokio::test]
async fn settles_with_the_spill_error_when_the_writer_fails_on_a_deep_queue() {
    use fs2::FileExt;

    let (_dir, root) = temp_root();
    let spill_target = joined(&root, "locked-spill.log");
    let lock_handle = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&spill_target)
        .expect("create spill target");
    lock_handle.lock_exclusive().expect("lock spill target");

    let env = GatedSpillExecutionEnv::new_with_spill_path(&root, Some(spill_target.clone()));
    // Release the gate up front: the accumulation comes from the byte limit,
    // not from gating the start.
    env.release.notify_one();
    let options = ShellExecOptions {
        capture: Some(ShellOutputCaptureOptions {
            limits: ShellOutputLimits {
                max_bytes: 2_000_000,
                max_lines: 1_000_000,
                retain: Some(ShellOutputRetention::Tail),
            },
            spill: Some(true),
        }),
        on_update: Some(Arc::new(|_update, _context| {})),
        ..Default::default()
    };
    // ~2.7MB of stderr.
    let command = "printf 'line-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\n%.0s' $(seq 1 75000) >&2";

    let exec_env = env.clone();
    let execution = tokio::spawn(async move {
        exec_env
            .exec(command, Some(&options), background_context())
            .await
    });

    let settled = tokio::time::timeout(Duration::from_secs(15), execution).await;
    let joined = settled.expect("exec must settle instead of deadlocking");
    let result = joined.expect("exec task joins");
    let Err(error) = result else {
        panic!("expected the spill error");
    };
    assert_eq!(error.code, ExecutionErrorCode::Unknown);
    assert!(
        error
            .message
            .contains("Failed to preserve complete shell output"),
        "{}",
        error.message
    );
    drop(lock_handle);
}
