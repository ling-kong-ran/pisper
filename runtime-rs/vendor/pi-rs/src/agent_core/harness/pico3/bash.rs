//! Port of `packages/agent/src/harness/pico3/bash.ts` (29 lines): the bash
//! tool declaration. Output is piped to the kernel; bounds come from
//! `output`.
//!
//! Disclosed substitution: upstream spawns `bash -c` with
//! `child.kill("SIGKILL")` on abort (`bash.ts:17-18`); the port spawns the
//! platform `bash` through `tokio::process` and kills the child on abort.

use std::sync::Arc;
use std::time::Instant;

use futures::future::BoxFuture;
use serde_json::{json, Value};

use crate::agent_core::chord_support::Context;

use super::runtime::{OutputBounds, StreamChunk, ToolApi, ToolDeclaration, ToolResult};

/// Upstream `parameters` (`bash.ts:5`): `{ command: string, cwd?: string }`.
fn parameters() -> Value {
    json!({
        "type": "object",
        "properties": {
            "command": { "type": "string" },
            "cwd": { "type": "string" },
        },
        "required": ["command"],
    })
}

/// Upstream `bashTool(output)` (`bash.ts:8-29`).
pub fn bash_tool(output: Option<OutputBounds>) -> ToolDeclaration {
    ToolDeclaration {
        name: "bash".to_owned(),
        description: "Run a shell command".to_owned(),
        parameters: parameters(),
        replay: Some("unsafe".to_owned()),
        output,
        execute: Arc::new(execute),
    }
}

fn execute(
    args: Value,
    api: ToolApi,
    ctx: Context,
) -> BoxFuture<'static, anyhow::Result<ToolResult>> {
    Box::pin(async move {
        let started = Instant::now();
        let command = args
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let cwd = args.get("cwd").and_then(Value::as_str).map(str::to_owned);
        let mut command_builder = tokio::process::Command::new("bash");
        command_builder
            .arg("-c")
            .arg(&command)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        if let Some(dir) = &cwd {
            command_builder.current_dir(dir);
        }
        let mut child = command_builder
            .spawn()
            .map_err(|error| anyhow::anyhow!("failed to spawn bash: {error}"))?;
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");

        // `child.stdout.on("data", c => api.stream(c))` (`bash.ts:20-21`).
        async fn pump(
            mut reader: impl tokio::io::AsyncRead + Unpin + Send + 'static,
            api: ToolApi,
        ) -> anyhow::Result<()> {
            let mut buffer = vec![0u8; 8192];
            loop {
                match tokio::io::AsyncReadExt::read(&mut reader, &mut buffer).await {
                    Ok(0) | Err(_) => break,
                    Ok(read) => api.stream(StreamChunk::Bytes(buffer[..read].to_vec()))?,
                }
            }
            Ok(())
        }
        let stdout_task = tokio::spawn(pump(stdout, api.clone()));
        let stderr_task = tokio::spawn(pump(stderr, api));

        // Abort: `child.kill("SIGKILL")` (`bash.ts:18-19`).
        let abort_signal = ctx.abort_signal();
        let status = if let Some(signal) = &abort_signal {
            tokio::select! {
                _ = signal.cancelled() => {
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    anyhow::bail!("aborted");
                }
                status = child.wait() => status.map_err(|error| anyhow::anyhow!("{error}"))?,
            }
        } else {
            child
                .wait()
                .await
                .map_err(|error| anyhow::anyhow!("{error}"))?
        };
        let (stdout_result, stderr_result) = tokio::join!(stdout_task, stderr_task);
        stdout_result??;
        stderr_result??;

        let code = status.code();
        Ok(ToolResult {
            content: None,
            is_error: Some(code != Some(0)),
            details: Some(json!({
                "exitCode": code.map(|code| code as i64),
                "signal": Option::<String>::None,
                "ms": started.elapsed().as_millis() as i64,
            })),
            diagnostics: None,
            control: None,
        })
    })
}
