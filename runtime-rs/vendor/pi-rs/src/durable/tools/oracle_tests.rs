//! Byte-oracle tests against `tests/fixtures/durable_oracle/durable_oracle.json`
//! for the tools slice (`tools_decl_and_exec`, `diff_surface`,
//! `image_detect`, `path_utils`), captured by
//! `capture_durable_oracle.mjs` from the read-only upstream sources. Execute
//! records and declaration surfaces are compared in the capture's canonical
//! form (object keys sorted, `undefined` → `null`), so construction-order
//! divergence cannot mask value divergence.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::agent_core::chord_support::context::Context;
use crate::durable::env::node::{NodeExecutionEnv, NodeExecutionEnvOptions};
use crate::durable::env::{
    CreateDirOptions, ExecutionEnv, ExecutionError, ExecutionErrorCode, FileContent, FileError,
    FileInfo, OnOutput, RemoveOptions, Shell, ShellExecOptions, ShellExecResult, ShellSpillOptions,
    TempFileOptions, TextLineReader, TextLinesOptions,
};
use crate::durable::errors::PlainError;
use crate::durable::harness::types::{
    ToolDiagnostic, ToolExecutionApiLike, ToolExecutionResult, ToolRegistration,
};

use super::{
    create_bash_tool, create_edit_tool, create_read_tool, create_write_tool, BashToolOptions,
};

fn oracle() -> Value {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let path = manifest_dir.join("tests/fixtures/durable_oracle/durable_oracle.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

use std::path::PathBuf;

/// Canonical form shared with the capture: object keys sorted recursively.
fn canonical(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<(String, Value)> = map.into_iter().collect();
            keys.sort_by(|left, right| left.0.cmp(&right.0));
            let mut sorted = serde_json::Map::new();
            for (key, value) in keys {
                sorted.insert(key, canonical(value));
            }
            Value::Object(sorted)
        }
        Value::Array(items) => Value::Array(items.into_iter().map(canonical).collect()),
        other => other,
    }
}

fn wire(value: &Value) -> String {
    serde_json::to_string(value).unwrap()
}

fn expected_tools() -> Value {
    oracle()["tools_decl_and_exec"].clone()
}

/// The tool result wire form: upstream result literals omit absent keys, so
/// the port's `null` fields drop the same way.
fn wire_result(result: &ToolExecutionResult) -> Value {
    let mut value = serde_json::to_value(result).unwrap();
    if let Value::Object(map) = &mut value {
        map.retain(|_, field| !field.is_null());
    }
    value
}

// ─── The fake invocation api ────────────────────────────────────────────────

struct FakeApi {
    env: Option<Arc<dyn ExecutionEnv>>,
    outputs: Mutex<Vec<String>>,
    diagnostics: Mutex<Vec<ToolDiagnostic>>,
    ended: AtomicBool,
}

impl FakeApi {
    fn new(env: Option<Arc<dyn ExecutionEnv>>) -> Arc<Self> {
        Arc::new(FakeApi {
            env,
            outputs: Mutex::new(Vec::new()),
            diagnostics: Mutex::new(Vec::new()),
            ended: AtomicBool::new(false),
        })
    }

    fn record(&self) -> (Vec<String>, Vec<ToolDiagnostic>) {
        (
            self.outputs
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone(),
            self.diagnostics
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone(),
        )
    }
}

impl ToolExecutionApiLike for FakeApi {
    fn task_id(&self) -> i64 {
        7
    }

    fn conversation_id(&self) -> i64 {
        1
    }

    fn call_id(&self) -> &str {
        "call-1"
    }

    fn env(&self) -> Option<Arc<dyn ExecutionEnv>> {
        self.env.clone()
    }

    fn output(&self, chunk: &[u8]) -> Result<(), PlainError> {
        if self.ended.load(Ordering::SeqCst) {
            return Err(PlainError::new("The tool call has settled"));
        }
        self.outputs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(String::from_utf8_lossy(chunk).into_owned());
        Ok(())
    }

    fn diagnostic(&self, diagnostic: ToolDiagnostic) -> Result<(), PlainError> {
        if self.ended.load(Ordering::SeqCst) {
            return Err(PlainError::new("The tool call has settled"));
        }
        self.diagnostics
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(diagnostic);
        Ok(())
    }

    fn details(
        &self,
        _value: Value,
        _context: Context,
    ) -> crate::durable::harness::types::ApiFuture<()> {
        unreachable!("the oracle tools never call api.details")
    }

    fn commit(
        &self,
        _change: crate::durable::harness::types::ErasedCommitChange,
        _context: Context,
    ) -> crate::durable::harness::types::ApiFuture<Value> {
        unreachable!("the oracle tools never call api.commit")
    }

    fn memo(
        &self,
        _name: &str,
        _candidate: Option<Value>,
        _context: Context,
    ) -> crate::durable::harness::types::ApiFuture<Option<Value>> {
        unreachable!("the oracle tools never call api.memo")
    }

    fn create_task(
        &self,
        _task: crate::durable::tasks::TaskToken,
        _input: Value,
        _options: crate::durable::types::TaskOptions,
        _context: Context,
    ) -> crate::durable::harness::types::ApiFuture<i64> {
        unreachable!("the oracle tools never call api.createTask")
    }

    fn get_task(
        &self,
        _id: i64,
        _context: Context,
    ) -> crate::durable::harness::types::ApiFuture<Option<crate::durable::types::TaskRecord>> {
        unreachable!("the oracle tools never call api.getTask")
    }

    fn wait_for_task(
        &self,
        _id: i64,
        _context: Context,
    ) -> crate::durable::harness::types::ApiFuture<crate::durable::types::TaskRecord> {
        unreachable!("the oracle tools never call api.waitForTask")
    }

    fn conversation(
        &self,
        _id: i64,
        _context: Context,
    ) -> crate::durable::harness::types::ApiFuture<
        Option<crate::durable::harness::types::ConversationHandle>,
    > {
        unreachable!("the oracle tools never call api.conversation")
    }
}

/// `thrown_name` reproduces the upstream error `name` per construction site
/// (D4): plain `new Error` throws record `"Error"`, re-thrown
/// `ExecutionError` / `FileError` objects record `null` (no `name` property).
async fn run_tool_execute(
    tool: &ToolRegistration,
    args: Value,
    env: Option<Arc<dyn ExecutionEnv>>,
    context: Context,
    thrown_name: Value,
) -> Value {
    let api = FakeApi::new(env);
    let outcome = (tool.execute)(
        args.as_object().cloned().unwrap_or_default(),
        api.clone(),
        context,
    )
    .await;
    let (result, thrown) = match outcome {
        Ok(result) => (wire_result(&result), Value::Null),
        Err(error) => (
            Value::Null,
            json!({ "message": error.message, "name": thrown_name }),
        ),
    };
    api.ended.store(true, Ordering::SeqCst);
    let (outputs, diagnostics) = api.record();
    canonical(json!({
        "result": result,
        "thrown": thrown,
        "outputs": outputs,
        "diagnostics": diagnostics,
    }))
}

// ─── The deterministic exec stub ────────────────────────────────────────────

/// The scenario directory, scrubbed out of recorded values exactly like the
/// capture (`<name>/` for path separators, `<name>` otherwise).
fn scrub(value: Value, dir: &str, name: &str) -> Value {
    let with_sep = format!("{dir}\\");
    let slash = format!("<{name}>/");
    let plain = format!("<{name}>");
    fn scrub_text(text: &str, dir: &str, with_sep: &str, slash: &str, plain: &str) -> String {
        text.replace(with_sep, slash).replace(dir, plain)
    }
    match value {
        Value::String(text) => Value::String(scrub_text(&text, dir, &with_sep, &slash, &plain)),
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|item| scrub(item, dir, name))
                .collect(),
        ),
        Value::Object(map) => {
            let mut scrubbed = serde_json::Map::new();
            for (key, item) in map {
                scrubbed.insert(key, scrub(item, dir, name));
            }
            Value::Object(scrubbed)
        }
        other => other,
    }
}

struct OracleEnv {
    base: NodeExecutionEnv,
    exec_log: Mutex<Vec<Value>>,
}

impl OracleEnv {
    fn new(dir: &Path) -> Arc<Self> {
        Arc::new(OracleEnv {
            base: NodeExecutionEnv::new(NodeExecutionEnvOptions {
                cwd: dir.to_string_lossy().into_owned(),
                shell_path: None,
                shell_env: None,
            }),
            exec_log: Mutex::new(Vec::new()),
        })
    }

    fn exec_log(&self) -> Vec<Value> {
        self.exec_log
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl crate::durable::env::FileSystem for OracleEnv {
    fn cwd(&self) -> &str {
        self.base.cwd()
    }

    fn absolute_path(&self, path: &str, context: &Context) -> Result<String, FileError> {
        self.base.absolute_path(path, context)
    }

    fn join_path(&self, parts: &[&str], context: &Context) -> Result<String, FileError> {
        self.base.join_path(parts, context)
    }

    fn read_text_file(&self, path: &str, context: &Context) -> Result<String, FileError> {
        self.base.read_text_file(path, context)
    }

    fn open_text_line_reader(
        &self,
        path: &str,
        context: &Context,
    ) -> Result<Box<dyn TextLineReader>, FileError> {
        self.base.open_text_line_reader(path, context)
    }

    fn read_text_lines(
        &self,
        path: &str,
        options: Option<TextLinesOptions>,
        context: &Context,
    ) -> Result<Vec<String>, FileError> {
        self.base.read_text_lines(path, options, context)
    }

    fn read_binary_file(&self, path: &str, context: &Context) -> Result<Vec<u8>, FileError> {
        self.base.read_binary_file(path, context)
    }

    fn write_file(
        &self,
        path: &str,
        content: FileContent<'_>,
        context: &Context,
    ) -> Result<(), FileError> {
        self.base.write_file(path, content, context)
    }

    fn append_file(
        &self,
        path: &str,
        content: FileContent<'_>,
        context: &Context,
    ) -> Result<(), FileError> {
        self.base.append_file(path, content, context)
    }

    fn truncate_file(&self, path: &str, size: u64, context: &Context) -> Result<(), FileError> {
        self.base.truncate_file(path, size, context)
    }

    fn flush_file(&self, path: &str, context: &Context) -> Result<(), FileError> {
        self.base.flush_file(path, context)
    }

    fn rename_file(
        &self,
        source_path: &str,
        destination_path: &str,
        context: &Context,
    ) -> Result<(), FileError> {
        self.base
            .rename_file(source_path, destination_path, context)
    }

    fn file_info(&self, path: &str, context: &Context) -> Result<FileInfo, FileError> {
        self.base.file_info(path, context)
    }

    fn list_dir(&self, path: &str, context: &Context) -> Result<Vec<FileInfo>, FileError> {
        self.base.list_dir(path, context)
    }

    fn canonical_path(&self, path: &str, context: &Context) -> Result<String, FileError> {
        self.base.canonical_path(path, context)
    }

    fn exists(&self, path: &str, context: &Context) -> Result<bool, FileError> {
        self.base.exists(path, context)
    }

    fn create_dir(
        &self,
        path: &str,
        options: Option<CreateDirOptions>,
        context: &Context,
    ) -> Result<(), FileError> {
        self.base.create_dir(path, options, context)
    }

    fn remove(
        &self,
        path: &str,
        options: Option<RemoveOptions>,
        context: &Context,
    ) -> Result<(), FileError> {
        self.base.remove(path, options, context)
    }

    fn create_temp_dir(
        &self,
        prefix: Option<&str>,
        context: &Context,
    ) -> Result<String, FileError> {
        self.base.create_temp_dir(prefix, context)
    }

    fn create_temp_file(
        &self,
        options: Option<TempFileOptions>,
        context: &Context,
    ) -> Result<String, FileError> {
        self.base.create_temp_file(options, context)
    }

    fn cleanup(&self) {
        crate::durable::env::FileSystem::cleanup(&self.base);
    }
}

impl Shell for OracleEnv {
    fn exec(
        &self,
        command: &str,
        options: Option<ShellExecOptions>,
        _context: &Context,
    ) -> Result<ShellExecResult, ExecutionError> {
        // The deterministic exec table, keyed by command text (never spawns a
        // shell); every call's options are recorded like the capture.
        let (cwd, env_keys, has_on_output) = match &options {
            Some(options) => (
                options.cwd.clone().unwrap_or_default(),
                options
                    .env
                    .as_ref()
                    .map(|pairs| pairs.iter().map(|(key, _)| key.clone()).collect::<Vec<_>>())
                    .unwrap_or_default(),
                options.on_output.is_some(),
            ),
            None => (String::new(), Vec::new(), false),
        };
        // JS numbers serialize without a trailing `.0`; match that on the log.
        let timeout = options.as_ref().and_then(|options| options.timeout);
        let timeout_json = match timeout {
            Some(value) if value.fract() == 0.0 => json!(value as i64),
            Some(value) => json!(value),
            None => Value::Null,
        };
        let spill = options.as_ref().map(|options| {
            let spill = options.spill.unwrap_or(ShellSpillOptions {
                after_bytes: 0,
                after_lines: 0,
            });
            json!({ "afterBytes": spill.after_bytes, "afterLines": spill.after_lines })
        });
        let inherit_env = options.as_ref().and_then(|options| options.inherit_env);
        self.exec_log
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(canonical(json!({
                "command": command,
                "cwd": cwd,
                "envKeys": env_keys,
                "hasOnOutput": has_on_output,
                "inheritEnv": inherit_env,
                "spill": spill,
                "timeout": timeout_json,
            })));

        type ExecOutcome = (
            Vec<&'static str>,
            i32,
            Option<String>,
            Option<(ExecutionErrorCode, &'static str)>,
        );
        let (chunks, exit_code, spill_path, error): ExecOutcome = match command {
            "echo oracle-ok" => (vec!["out1\n", "err1\n"], 0, None, None),
            "echo oracle-spill" => (
                vec!["chunk-a\n", "chunk-b\n"],
                0,
                Some(String::from("/tmp/durable-oracle-spill.txt")),
                None,
            ),
            "exit 3" => (vec!["boom\n"], 3, None, None),
            "sleep oracle-timeout" => (
                vec!["partial\n"],
                0,
                None,
                Some((ExecutionErrorCode::Timeout, "timed out")),
            ),
            "sleep oracle-abort" => (
                vec![],
                0,
                None,
                Some((ExecutionErrorCode::Aborted, "aborted")),
            ),
            "bad oracle-spawn" => (
                vec![],
                0,
                None,
                Some((ExecutionErrorCode::SpawnError, "spawn failed")),
            ),
            _ => (vec![], 0, None, None),
        };
        if let Some(options) = &options {
            if let Some(on_output) = &options.on_output {
                for chunk in chunks {
                    let callback: OnOutput = Arc::clone(on_output);
                    let mut callback = callback.lock().unwrap();
                    callback(chunk);
                }
            }
        }
        if let Some((code, message)) = error {
            return Err(ExecutionError::new(code, message));
        }
        Ok(ShellExecResult {
            exit_code,
            spill_path,
        })
    }

    fn cleanup(&self) {
        crate::durable::env::FileSystem::cleanup(&self.base);
    }
}

impl ExecutionEnv for OracleEnv {}

fn canceled_signal() -> tokio_util::sync::CancellationToken {
    let token = tokio_util::sync::CancellationToken::new();
    token.cancel();
    token
}

// ─── Scenario 9a: tool declarations ─────────────────────────────────────────

#[test]
fn oracle_tools_decl() {
    use crate::durable::harness::types::OutputRetain;
    let expected = &expected_tools()["tools_decl"];
    let bash = create_bash_tool(BashToolOptions::default());
    let read = create_read_tool();
    let write = create_write_tool();
    let edit = create_edit_tool();
    let declarations: Vec<Value> = vec![&bash, &read, &write, &edit]
        .into_iter()
        .map(|tool| {
            canonical(json!({
                "name": tool.tool.name,
                "description": tool.tool.description,
                "parameters": tool.tool.parameters,
                "outputLimits": tool.output_limits.map(|limits| {
                    let mut map = serde_json::Map::new();
                    if let Some(max_bytes) = limits.max_bytes {
                        map.insert(String::from("maxBytes"), json!(max_bytes));
                    }
                    if let Some(max_lines) = limits.max_lines {
                        map.insert(String::from("maxLines"), json!(max_lines));
                    }
                    if let Some(retain) = limits.retain {
                        map.insert(
                            String::from("retain"),
                            json!(match retain {
                                OutputRetain::Head => "head",
                                OutputRetain::Tail => "tail",
                            }),
                        );
                    }
                    Value::Object(map)
                }),
            }))
        })
        .collect();
    assert_eq!(
        wire(&Value::Array(declarations)),
        wire(expected),
        "tool declarations"
    );
}

// ─── Scenario 9b: execute shapes ────────────────────────────────────────────

#[tokio::test]
async fn oracle_tools_execute() {
    let expected = &expected_tools()["tools_execute"];
    let dir = tempfile::tempdir().unwrap();
    let dir_text = dir.path().to_string_lossy().into_owned();
    let oracle_env = OracleEnv::new(dir.path());
    let env: Arc<dyn ExecutionEnv> = oracle_env.clone();

    // requireEnv without an environment.
    let no_env_api = FakeApi::new(None);
    let require_env_error = match super::env::require_env(no_env_api.as_ref()) {
        Err(error) => json!({ "message": error.message, "name": "Error" }),
        Ok(_) => Value::Null,
    };

    let bash = create_bash_tool(BashToolOptions::default());
    let dir_for_prepare = dir_text.clone();
    let bash_prefix = create_bash_tool(BashToolOptions {
        command_prefix: Some(String::from("set -e")),
        prepare: Some(Arc::new(
            move |execution: &mut super::BashExecution,
                  _api: Arc<dyn ToolExecutionApiLike>,
                  _context: Context| {
                execution.cwd = format!("{dir_for_prepare}\\prepared");
                execution
                    .env
                    .push((String::from("PREPARED"), String::from("1")));
                Box::pin(async move { Ok(()) })
            },
        )),
    });

    async fn run(env: &Arc<dyn ExecutionEnv>, tool: &ToolRegistration, args: Value) -> Value {
        run_tool_execute(
            tool,
            args,
            Some(env.clone()),
            Context::background(),
            json!("Error"),
        )
        .await
    }

    let aborted_context = Context::background().with_value(
        crate::agent_core::chord_support::context::abort_signal_key(),
        Some(canceled_signal()),
    );

    let bash_ok = run(&env, &bash, json!({ "command": "echo oracle-ok" })).await;
    let bash_spill = run(&env, &bash, json!({ "command": "echo oracle-spill" })).await;
    let bash_nonzero = run(&env, &bash, json!({ "command": "exit 3" })).await;
    let bash_timeout = run(
        &env,
        &bash,
        json!({ "command": "sleep oracle-timeout", "timeout": 5 }),
    )
    .await;
    let bash_unknown_error = run_tool_execute(
        &bash,
        json!({ "command": "bad oracle-spawn" }),
        Some(env.clone()),
        Context::background(),
        Value::Null,
    )
    .await;
    let bash_prefix_prepare = run(&env, &bash_prefix, json!({ "command": "echo oracle-ok" })).await;
    let bash_timeout_invalid = run(&env, &bash, json!({ "command": "echo x", "timeout": 0 })).await;
    let bash_timeout_too_large = run(
        &env,
        &bash,
        json!({ "command": "echo x", "timeout": 2147483647 }),
    )
    .await;
    let bash_aborted_with_signal = run_tool_execute(
        &bash,
        json!({ "command": "sleep oracle-abort" }),
        Some(env.clone()),
        aborted_context,
        Value::Null,
    )
    .await;
    let bash_aborted_without_signal =
        run(&env, &bash, json!({ "command": "sleep oracle-abort" })).await;

    let bash_records = json!({
        "requireEnvError": require_env_error,
        "bashOk": bash_ok,
        "bashSpill": bash_spill,
        "bashNonzero": bash_nonzero,
        "bashTimeout": bash_timeout,
        "bashUnknownError": bash_unknown_error,
        "bashPrefixPrepare": bash_prefix_prepare,
        "bashTimeoutInvalid": bash_timeout_invalid,
        "bashTimeoutTooLarge": bash_timeout_too_large,
        "bashAbortedWithSignal": bash_aborted_with_signal,
        "bashAbortedWithoutSignal": bash_aborted_without_signal,
    });
    for (name, record) in bash_records.as_object().unwrap() {
        assert_eq!(
            wire(&scrub(record.clone(), &dir_text, "scenario9")),
            wire(&expected[name]),
            "bash record {name}"
        );
    }

    // Fixture files, mirroring the capture.
    std::fs::write(dir.path().join("one-two-three.txt"), "one\ntwo\nthree").unwrap();
    let mut many_lines = String::new();
    for index in 0..2500 {
        many_lines.push_str(&format!("line-{index}\n"));
    }
    std::fs::write(dir.path().join("many-lines.txt"), many_lines).unwrap();
    std::fs::write(
        dir.path().join("huge-line.txt"),
        format!("{}\nsecond\n", "x".repeat(60000)),
    )
    .unwrap();
    std::fs::write(
        dir.path().join("picture.png"),
        [
            0x89u8, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13, 73, 72, 68, 82,
        ],
    )
    .unwrap();

    let read = create_read_tool();
    let read_basic = run(&env, &read, json!({ "path": "one-two-three.txt" })).await;
    let read_offset_limit = run(
        &env,
        &read,
        json!({ "path": "one-two-three.txt", "offset": 2, "limit": 1 }),
    )
    .await;
    let read_offset_beyond = run(
        &env,
        &read,
        json!({ "path": "one-two-three.txt", "offset": 99 }),
    )
    .await;
    let read_truncated_lines = run(&env, &read, json!({ "path": "many-lines.txt" })).await;
    let read_huge_line = run(&env, &read, json!({ "path": "huge-line.txt" })).await;
    let read_unsupported_image = run(&env, &read, json!({ "path": "picture.png" })).await;
    let read_records = json!({
        "readBasic": read_basic,
        "readOffsetLimit": read_offset_limit,
        "readOffsetBeyond": read_offset_beyond,
        "readTruncatedLines": read_truncated_lines,
        "readHugeLine": read_huge_line,
        "readUnsupportedImage": read_unsupported_image,
    });
    for (name, record) in read_records.as_object().unwrap() {
        assert_eq!(
            wire(&scrub(record.clone(), &dir_text, "scenario9")),
            wire(&expected[name]),
            "read record {name}"
        );
    }

    let write = create_write_tool();
    let write_basic = run(
        &env,
        &write,
        json!({ "path": "out/write-target.txt", "content": "written-1\n" }),
    )
    .await;
    let write_bytes_1 = std::fs::read_to_string(dir.path().join("out/write-target.txt")).unwrap();
    let write_overwrite = run(
        &env,
        &write,
        json!({ "path": "out/write-target.txt", "content": "written-2" }),
    )
    .await;
    let write_bytes_2 = std::fs::read_to_string(dir.path().join("out/write-target.txt")).unwrap();
    let write_records = json!({
        "writeBasic": write_basic,
        "writeFileBytes1": Value::String(write_bytes_1),
        "writeOverwrite": write_overwrite,
        "writeFileBytes2": Value::String(write_bytes_2),
    });
    for (name, record) in write_records.as_object().unwrap() {
        assert_eq!(
            wire(&scrub(record.clone(), &dir_text, "scenario9")),
            wire(&expected[name]),
            "write record {name}"
        );
    }

    let edit = create_edit_tool();
    std::fs::write(
        dir.path().join("edit-me.txt"),
        "alpha\nbeta\ngamma\nbeta again\n",
    )
    .unwrap();
    let edit_two_blocks = run(
        &env,
        &edit,
        json!({
            "path": "edit-me.txt",
            "edits": [
                { "oldText": "alpha\nbeta", "newText": "ALPHA\nBETA" },
                { "oldText": "gamma", "newText": "GAMMA" },
            ],
        }),
    )
    .await;
    let edit_me_bytes = std::fs::read_to_string(dir.path().join("edit-me.txt")).unwrap();
    std::fs::write(
        dir.path().join("fuzzy.txt"),
        "value = \u{201c}quoted\u{201d} end\n",
    )
    .unwrap();
    let edit_fuzzy = run(
        &env,
        &edit,
        json!({ "path": "fuzzy.txt", "edits": [{ "oldText": "\"quoted\"", "newText": "\"QUOTED\"" }] }),
    )
    .await;
    let fuzzy_bytes = std::fs::read_to_string(dir.path().join("fuzzy.txt")).unwrap();
    std::fs::write(dir.path().join("crlf.txt"), "one\r\ntwo\r\nthree\r\n").unwrap();
    std::fs::write(dir.path().join("dup.txt"), "two\nmore two\n").unwrap();
    std::fs::write(dir.path().join("nochange.txt"), "two\n").unwrap();
    let edit_duplicate = run(
        &env,
        &edit,
        json!({ "path": "dup.txt", "edits": [{ "oldText": "two", "newText": "x" }] }),
    )
    .await;
    let edit_no_change = run(
        &env,
        &edit,
        json!({ "path": "nochange.txt", "edits": [{ "oldText": "two", "newText": "two" }] }),
    )
    .await;
    let edit_crlf = run(
        &env,
        &edit,
        json!({ "path": "crlf.txt", "edits": [{ "oldText": "two", "newText": "TWO" }] }),
    )
    .await;
    let crlf_bytes = std::fs::read_to_string(dir.path().join("crlf.txt")).unwrap();
    let edit_not_found = run(
        &env,
        &edit,
        json!({ "path": "crlf.txt", "edits": [{ "oldText": "absent", "newText": "x" }] }),
    )
    .await;
    let edit_empty_old = run(
        &env,
        &edit,
        json!({ "path": "crlf.txt", "edits": [{ "oldText": "", "newText": "x" }] }),
    )
    .await;
    let edit_missing_file = run(
        &env,
        &edit,
        json!({ "path": "absent-file.txt", "edits": [{ "oldText": "a", "newText": "b" }] }),
    )
    .await;
    let edit_directory = run(
        &env,
        &edit,
        json!({ "path": "out", "edits": [{ "oldText": "a", "newText": "b" }] }),
    )
    .await;
    let edit_records = json!({
        "editTwoBlocks": edit_two_blocks,
        "editMeBytes": Value::String(edit_me_bytes),
        "editFuzzy": edit_fuzzy,
        "fuzzyBytes": Value::String(fuzzy_bytes),
        "editCrlf": edit_crlf,
        "crlfBytes": Value::String(crlf_bytes),
        "editNotFound": edit_not_found,
        "editDuplicate": edit_duplicate,
        "editEmptyOld": edit_empty_old,
        "editNoChange": edit_no_change,
        "editMissingFile": edit_missing_file,
        "editDirectory": edit_directory,
    });
    for (name, record) in edit_records.as_object().unwrap() {
        assert_eq!(
            wire(&scrub(record.clone(), &dir_text, "scenario9")),
            wire(&expected[name]),
            "edit record {name}"
        );
    }

    // The exec log, recorded in call order across every bash run.
    assert_eq!(
        wire(&scrub(
            Value::Array(oracle_env.exec_log()),
            &dir_text,
            "scenario9"
        )),
        wire(&expected["execLog"]),
        "exec log"
    );
}

// ─── Scenario 10: edit-diff / image / path-utils surfaces ───────────────────

#[test]
fn oracle_diff_surface() {
    use super::edit_diff::{
        apply_edits_to_normalized_content, detect_line_ending, fuzzy_find_text,
        generate_diff_string, generate_unified_patch, normalize_for_fuzzy_match, normalize_to_lf,
        restore_line_endings, strip_bom, Edit,
    };
    let expected = oracle()["diff_surface"].clone();

    let detect = json!([
        detect_line_ending("a\r\nb\n"),
        detect_line_ending("a\nb\r\n"),
        detect_line_ending("no endings"),
        detect_line_ending("lone \r cr"),
    ]);
    assert_eq!(
        wire(&canonical(detect)),
        wire(&expected["detectLineEnding"])
    );
    assert_eq!(
        canonical(json!(normalize_to_lf("a\r\nb\rc\nd"))),
        expected["normalizeToLf"]
    );
    assert_eq!(
        canonical(json!(restore_line_endings("a\nb", "\r\n"))),
        expected["restoreLineEndings"]
    );
    assert_eq!(
        canonical(json!(normalize_for_fuzzy_match(
            "\u{201c}quoted\u{201d} \u{a0} en\u{2013}dash \u{2014} em  \n z\u{205f}w"
        ))),
        expected["normalizeForFuzzyMatch"]
    );
    let (bom, text) = strip_bom("\u{feff}body");
    assert_eq!(
        canonical(json!({ "bom": bom, "text": text })),
        expected["stripBom"]
    );

    let fuzzy_json = |result: &super::edit_diff::FuzzyMatchResult| {
        canonical(json!({
            "found": result.found,
            "index": result.index,
            "matchLength": result.match_length,
            "usedFuzzyMatch": result.used_fuzzy_match,
            "contentForReplacement": result.content_for_replacement,
        }))
    };
    assert_eq!(
        fuzzy_json(&fuzzy_find_text("abc def", "def")),
        expected["fuzzyExact"]
    );
    assert_eq!(
        fuzzy_json(&fuzzy_find_text("x = \u{201c}q\u{201d};", "\"q\"")),
        expected["fuzzyFuzzy"]
    );
    assert_eq!(
        fuzzy_json(&fuzzy_find_text("abc", "zzz")),
        expected["fuzzyMiss"]
    );

    let applied = apply_edits_to_normalized_content(
        "one\ntwo\nthree\ntwo again\n",
        &[
            Edit {
                old_text: String::from("one\ntwo"),
                new_text: String::from("ONE\nTWO"),
            },
            Edit {
                old_text: String::from("three"),
                new_text: String::from("THREE"),
            },
        ],
        "f.txt",
    )
    .unwrap();
    assert_eq!(
        canonical(
            json!({ "baseContent": applied.base_content, "newContent": applied.new_content })
        ),
        expected["applyTwoEdits"]
    );
    let overlay = apply_edits_to_normalized_content(
        "x = \u{201c}v\u{201d} ;  \ny\n",
        &[Edit {
            old_text: String::from("\"v\""),
            new_text: String::from("\"W\""),
        }],
        "f.txt",
    )
    .unwrap();
    assert_eq!(
        canonical(
            json!({ "baseContent": overlay.base_content, "newContent": overlay.new_content })
        ),
        expected["applyFuzzyOverlay"]
    );

    assert_eq!(
        json!(generate_unified_patch(
            "f.txt",
            "keep\nchange-from\nkeep2\nkeep3\nkeep4\nkeep5\nchange-to\nkeep6\n",
            "keep\nchange-from-edited\nkeep2\nkeep3\nkeep4\nkeep5\nchange-to\nkeep6\n",
            4,
        )),
        expected["unifiedPatch"]
    );
    assert_eq!(
        json!(generate_unified_patch("g.txt", "a\nb", "a\nc", 4)),
        expected["unifiedPatchNoNewline"]
    );
    let display = generate_diff_string(
        "l0\nl1\nl2\nl3\nl4\nold\nl6\nl7\nl8\nl9\nl10\nnew-tail\n",
        "l0\nl1\nl2\nl3\nl4\nNEW\nl6\nl7\nl8\nl9\nl10\nNEW-TAIL\n",
        4,
    );
    assert_eq!(
        canonical(json!({
            "diff": display.diff,
            "firstChangedLine": display.first_changed_line,
        })),
        expected["diffString"]
    );
}

#[test]
fn oracle_image_detect() {
    use super::image::detect_supported_image_mime_type;
    let expected = oracle()["image_detect"].clone();
    let rows = [
        ("jpeg", vec![0xff, 0xd8, 0xff, 0xe0, 0, 5]),
        ("jpeg-lossless", vec![0xff, 0xd8, 0xff, 0xf7]),
        (
            "png",
            vec![
                0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13, 73, 72, 68, 82,
            ],
        ),
        ("png-short", vec![0x89, 0x50, 0x4e, 0x47]),
        (
            "png-bad-ihdr",
            vec![
                0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 12, 73, 72, 68, 82,
            ],
        ),
        (
            "png-animated",
            vec![
                0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 1, 97, 99, 84, 76, 1,
            ],
        ),
        (
            "png-idat-first",
            vec![
                0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0,
                0, 5, 73, 68, 65, 84,
            ],
        ),
        ("gif87a", b"GIF87a".to_vec()),
        ("gif89a", b"GIF89a".to_vec()),
        ("gif88a", b"GIF88a".to_vec()),
        ("webp", b"RIFF0000WEBPVP8 ".to_vec()),
        (
            "bmp-24",
            vec![
                0x42, 0x4d, 46, 0, 0, 0, 0, 0, 0, 0, 54, 0, 0, 0, 40, 0, 0, 0, 1, 0, 1, 0, 1, 0,
                24, 0, 0, 0,
            ],
        ),
        (
            "bmp-bad-planes",
            vec![
                0x42, 0x4d, 46, 0, 0, 0, 0, 0, 0, 0, 54, 0, 0, 0, 40, 0, 0, 0, 1, 0, 1, 0, 2, 0,
                24, 0, 0, 0,
            ],
        ),
        (
            "bmp-core",
            vec![
                0x42, 0x4d, 0, 0, 0, 0, 0, 0, 0, 0, 26, 0, 0, 0, 12, 0, 0, 0, 1, 0, 1, 0, 1, 0, 8,
                0,
            ],
        ),
        ("bmp-truncated", vec![0x42, 0x4d]),
        ("empty", vec![]),
        ("text", b"plain text".to_vec()),
    ];
    let computed: Vec<Value> = rows
        .into_iter()
        .map(|(name, bytes)| {
            canonical(json!({
                "name": name,
                "bytes": bytes,
                "mime": detect_supported_image_mime_type(&bytes),
            }))
        })
        .collect();
    assert_eq!(
        wire(&Value::Array(computed)),
        wire(&expected),
        "image detection table"
    );
}

#[test]
fn oracle_path_utils() {
    use super::path_utils::{resolve_read_tool_path, resolve_tool_path};
    let expected = oracle()["path_utils"].clone();
    let dir = tempfile::tempdir().unwrap();
    let dir_text = dir.path().to_string_lossy().into_owned();
    let env = OracleEnv::new(dir.path());
    std::fs::write(dir.path().join("re ad.txt"), "spacey").unwrap();
    std::fs::write(dir.path().join("photo\u{202f}AM.txt"), "narrow").unwrap();
    std::fs::write(dir.path().join("apostrophe.txt"), "quote").unwrap();
    let context = Context::background();

    let resolved = |path: &str| -> Value {
        resolve_tool_path(env.as_ref(), path, &context)
            .map(Value::String)
            .unwrap_or(Value::Null)
    };
    let resolved_read = |path: &str| -> Value {
        resolve_read_tool_path(env.as_ref(), path, &context)
            .map(Value::String)
            .unwrap_or(Value::Null)
    };
    let computed = json!({
        "resolveToolPath": [
            resolved("re ad.txt"),
            resolved("@re ad.txt"),
            resolved("re\u{a0}ad.txt"),
        ],
        "resolveReadToolPath": [
            resolved_read("re ad.txt"),
            resolved_read("photo AM.txt"),
            resolved_read("apostrophe.txt"),
            resolved_read("missing.txt"),
        ],
    });
    assert_eq!(
        wire(&scrub(canonical(computed), &dir_text, "scenario10")),
        wire(&expected),
        "path utils"
    );
}
