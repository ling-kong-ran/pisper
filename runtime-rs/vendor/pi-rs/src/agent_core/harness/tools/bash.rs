//! Upstream tools/bash.ts: bounded output, spill notices and replay checkpoints.
use super::super::utils::truncate::format_size;
use super::super::utils::{apply_shell_output_update, DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES};
use super::super::{
    AgentHarnessTool, AgentHarnessToolUpdateOptions, Context, ExecutionErrorCode, ShellExecOptions,
    ShellOutputCaptureOptions, ShellOutputLimits, ShellOutputRetention, ShellOutputTruncation,
    ShellOutputView, TruncatedBy,
};
use super::{text_result, ExecutionToolContext, HasExecutionEnv};
use crate::agent_core::types::AgentToolResult;
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use tokio::time::Instant;

const MAX_TIMEOUT_SECONDS: f64 = 2_147_483_647.0 / 1000.0;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BashToolInput {
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<f64>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BashToolDetails {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncation: Option<ShellOutputTruncation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_output_path: Option<String>,
}
#[derive(Debug, Clone)]
pub struct BashExecution {
    pub command: String,
    pub cwd: String,
    pub env: BTreeMap<String, String>,
    pub inherit_env: bool,
}
pub type BashPrepare<T = ExecutionToolContext> = dyn for<'a> Fn(&'a mut BashExecution, &'a T, Context) -> BoxFuture<'a, anyhow::Result<()>>
    + Send
    + Sync;
pub struct BashToolOptions<T: HasExecutionEnv = ExecutionToolContext> {
    pub command_prefix: Option<String>,
    pub prepare: Option<Arc<BashPrepare<T>>>,
}
impl<T: HasExecutionEnv> Default for BashToolOptions<T> {
    fn default() -> Self {
        Self {
            command_prefix: None,
            prepare: None,
        }
    }
}
impl<T: HasExecutionEnv> Clone for BashToolOptions<T> {
    fn clone(&self) -> Self {
        Self {
            command_prefix: self.command_prefix.clone(),
            prepare: self.prepare.clone(),
        }
    }
}
fn validate_timeout(timeout: Option<f64>) -> anyhow::Result<()> {
    if let Some(timeout) = timeout {
        if !timeout.is_finite() || timeout <= 0.0 {
            anyhow::bail!("Invalid timeout: must be a finite number of seconds");
        }
        if timeout > MAX_TIMEOUT_SECONDS {
            anyhow::bail!("Invalid timeout: maximum is {MAX_TIMEOUT_SECONDS} seconds");
        }
    }
    Ok(())
}
struct Updates {
    view: Option<ShellOutputView>,
    last_checkpoint_at: Instant,
    last_checkpoint: Option<String>,
    accepting: bool,
}

pub fn create_bash_tool(options: BashToolOptions) -> AgentHarnessTool<ExecutionToolContext> {
    create_bash_tool_for(options)
}
pub fn create_bash_tool_for<T: HasExecutionEnv>(
    options: BashToolOptions<T>,
) -> AgentHarnessTool<T> {
    AgentHarnessTool {
        name:"bash".into(),label:"bash".into(),
        description:format!("Execute a bash command in the current working directory. Returns combined stdout and stderr. Output is truncated to last {DEFAULT_MAX_LINES} lines or {}KB (whichever is hit first). If truncated, full output is saved to a temp file. Optionally provide a timeout in seconds.", DEFAULT_MAX_BYTES/1024),
        parameters:json!({"type":"object","properties":{"command":{"type":"string","description":"Bash command to execute"},"timeout":{"type":"number","description":"Timeout in seconds (optional, no default timeout)"}},"required":["command"]}),
        constrained_sampling:None,prepare_arguments:None,replay:None,execution_mode:None,
        execute:Arc::new(move |_, input, on_update, tool_context:T, _, context| {
            let options=options.clone();
            Box::pin(async move {
                let input:BashToolInput=serde_json::from_value(input)?;
                validate_timeout(input.timeout)?;
                let env=tool_context.execution_env();
                let mut execution=BashExecution {
                    command:match options.command_prefix.filter(|p|!p.is_empty()) {Some(prefix)=>format!("{prefix}\n{}",input.command),None=>input.command},
                    cwd:env.cwd().to_string(),env:BTreeMap::new(),inherit_env:true,
                };
                if let Some(prepare)=options.prepare {prepare(&mut execution,&tool_context,context.clone()).await?;}
                let updates=Arc::new(Mutex::new(Updates {view:None,last_checkpoint_at:Instant::now(),last_checkpoint:None,accepting:true}));
                on_update(&AgentToolResult::default(),AgentHarnessToolUpdateOptions::default());
                let callback_state=updates.clone();
                let exec_options=ShellExecOptions {
                    cwd:Some(execution.cwd),env:Some(execution.env),inherit_env:Some(execution.inherit_env),timeout:input.timeout,
                    capture:Some(ShellOutputCaptureOptions {limits:ShellOutputLimits {max_bytes:DEFAULT_MAX_BYTES,max_lines:DEFAULT_MAX_LINES,retain:Some(ShellOutputRetention::Tail)},spill:Some(true)}),
                    on_update:Some(Arc::new(move |update,_| {
                        let (snapshot,checkpoint)={
                            let mut state=callback_state.lock().unwrap();
                            if !state.accepting {return;}
                            let view=apply_shell_output_update(state.view.take(),update);
                            let mut snapshot=text_result(view.text.clone());
                            snapshot.details=Some(json!(BashToolDetails {truncation:view.metadata.truncation.truncated.then_some(view.metadata.truncation),full_output_path:view.metadata.spill_path.clone()}));
                            state.view=Some(view);
                            let now=Instant::now();
                            let encoded=serde_json::to_string(&snapshot).expect("tool result is JSON");
                            let checkpoint=now.duration_since(state.last_checkpoint_at).as_millis()>=2000 && state.last_checkpoint.as_ref()!=Some(&encoded);
                            if checkpoint {state.last_checkpoint_at=now;state.last_checkpoint=Some(encoded);}
                            (snapshot,checkpoint)
                        };
                        on_update(&snapshot,AgentHarnessToolUpdateOptions {checkpoint});
                    })),
                };
                let result=env.exec(&execution.command,Some(&exec_options),context).await;
                let view={let mut state=updates.lock().unwrap();state.accepting=false;state.view.take()};
                let mut output=view.as_ref().map(|v|v.text.clone()).unwrap_or_default();
                let capture=result.as_ref().ok().map(|r|r.metadata.clone()).or_else(||view.map(|v|v.metadata));
                let mut details=None;
                if let Some(capture)=capture.filter(|v|v.truncation.truncated) {
                    let tr=capture.truncation;
                    details=Some(json!(BashToolDetails {truncation:Some(tr),full_output_path:capture.spill_path.clone()}));
                    let start=tr.total_lines-tr.output_lines+1;
                    let end=tr.total_lines;
                    let full=capture.spill_path.as_deref().unwrap_or("undefined");
                    if tr.last_line_partial {output+=&format!("\n\n[Showing last {} of line {end} (line is {}). Full output: {full}]",format_size(tr.output_bytes),format_size(capture.last_line_bytes.unwrap_or(tr.output_bytes)));}
                    else if tr.truncated_by==Some(TruncatedBy::Lines) {output+=&format!("\n\n[Showing lines {start}-{end} of {}. Full output: {full}]",tr.total_lines);}
                    else {output+=&format!("\n\n[Showing lines {start}-{end} of {} ({} limit). Full output: {full}]",tr.total_lines,format_size(DEFAULT_MAX_BYTES));}
                }
                match result {
                    Err(error)=>{
                        let status=match error.code {
                            ExecutionErrorCode::Timeout=>format!("Command timed out after {} seconds",input.timeout.map(|t|t.to_string()).unwrap_or_else(||"undefined".into())),
                            ExecutionErrorCode::Aborted=>"Command aborted".to_string(),
                            _=>error.message.clone(),
                        };
                        let message=if output.is_empty() {status} else {format!("{output}\n\n{status}")};
                        Err(anyhow::Error::new(error).context(message))
                    }
                    Ok(result) if result.exit_code!=0=>anyhow::bail!("{}Command exited with code {}",if output.is_empty() {String::new()} else {format!("{output}\n\n")},result.exit_code),
                    Ok(_)=>{let mut result=text_result(if output.is_empty() {"(no output)".to_string()} else {output});result.details=details;Ok(result)}
                }
            })
        }),
    }
}
