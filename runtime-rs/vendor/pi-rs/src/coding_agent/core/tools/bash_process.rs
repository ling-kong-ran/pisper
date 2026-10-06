//! Native process backend shared by bash and PowerShell tools. A child exit
//! starts a 100ms *idle* drain, rearmed for every stdout/stderr chunk; a
//! descendant holding a quiet pipe cannot hang the command forever.
use super::ToolFuture;
use crate::coding_agent::{
    extensions::types::AbortSignal,
    utils::{
        child_process::EXIT_STDIO_GRACE_MS,
        shell::{kill_process_tree, track_detached_child_pid, untrack_detached_child_pid},
        shell_config::{
            get_powershell_config, get_shell_config, get_shell_env, CommandTransport, ShellConfig,
            ShellEnvironment,
        },
    },
};
use crate::serde_support::js_number_string;
use futures::future::BoxFuture;
use std::{process::Stdio, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    time::Instant,
};

pub type ShellDataCallback = Arc<dyn Fn(&[u8]) -> Result<(), String> + Send + Sync>;
pub type ShellConfigResolver =
    Arc<dyn Fn() -> BoxFuture<'static, Result<ShellConfig, String>> + Send + Sync>;
#[derive(Clone)]
pub struct ShellExecOptions {
    pub on_data: ShellDataCallback,
    pub signal: Option<Arc<AbortSignal>>,
    pub timeout: Option<f64>,
    pub env: Option<ShellEnvironment>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellExit {
    pub exit_code: Option<i32>,
}
pub type ShellExecute =
    Arc<dyn Fn(String, String, ShellExecOptions) -> ToolFuture<ShellExit> + Send + Sync>;
#[derive(Clone)]
pub struct ShellOperations {
    pub exec: ShellExecute,
}

pub fn resolve_timeout_ms(timeout: Option<f64>) -> Result<Option<f64>, String> {
    let Some(seconds) = timeout else {
        return Ok(None);
    };
    if !seconds.is_finite() || seconds <= 0.0 {
        return Err("Invalid timeout: must be a finite number of seconds".into());
    }
    let milliseconds = seconds * 1000.0;
    if milliseconds > 2_147_483_647.0 {
        return Err("Invalid timeout: maximum is 2147483.647 seconds".into());
    }
    Ok(Some(milliseconds))
}
struct ChildLifetime {
    pid: Option<u32>,
    completed: bool,
}
impl ChildLifetime {
    fn new(pid: Option<u32>) -> Self {
        if let Some(pid) = pid {
            track_detached_child_pid(pid);
        }
        Self {
            pid,
            completed: false,
        }
    }
}
impl Drop for ChildLifetime {
    fn drop(&mut self) {
        if let Some(pid) = self.pid {
            if !self.completed {
                kill_process_tree(pid);
            }
            untrack_detached_child_pid(pid);
        }
    }
}
async fn stop_tree(pid: u32) -> bool {
    #[cfg(windows)]
    {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        let executable = std::path::PathBuf::from(root)
            .join("System32")
            .join("taskkill.exe");
        let mut kill = tokio::process::Command::new(executable);
        kill.args(["/F", "/T", "/PID", &pid.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x0800_0000);
        kill.status().await.is_ok_and(|s| s.success())
    }
    #[cfg(not(windows))]
    {
        kill_process_tree(pid);
        true
    }
}
pub fn create_local_shell_operations(
    shell_name: &str,
    resolve: ShellConfigResolver,
) -> ShellOperations {
    let shell_name = shell_name.to_owned();
    ShellOperations {
        exec: Arc::new(move |command, cwd, options| {
            let resolve = resolve.clone();
            let shell_name = shell_name.clone();
            Box::pin(async move {
                let timeout = resolve_timeout_ms(options.timeout)?;
                if options.signal.as_ref().is_some_and(|s| s.is_aborted()) {
                    return Err("aborted".into());
                }
                let config = resolve().await?;
                if tokio::fs::metadata(&cwd).await.is_err() {
                    return Err(format!("Working directory does not exist: {cwd}\nCannot execute {shell_name} commands."));
                }
                let stdin_transport = config.command_transport == Some(CommandTransport::Stdin);
                let mut spawn = tokio::process::Command::new(&config.shell);
                spawn
                    .args(&config.args)
                    .current_dir(&cwd)
                    .env_clear()
                    .envs(options.env.unwrap_or_else(get_shell_env))
                    .stdin(if stdin_transport {
                        Stdio::piped()
                    } else {
                        Stdio::null()
                    })
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped());
                if !stdin_transport {
                    spawn.arg(&command);
                }
                #[cfg(windows)]
                spawn.creation_flags(0x0800_0000);
                #[cfg(unix)]
                {
                    use std::os::unix::process::CommandExt;
                    // Safe std process-group creation permits tree SIGKILL without
                    // a pre_exec/unsafe block. This is not a new Unix login session.
                    spawn.as_std_mut().process_group(0);
                }
                let mut child = spawn.spawn().map_err(|e| {
                    if e.kind() == std::io::ErrorKind::NotFound {
                        format!("spawn {} ENOENT", config.shell)
                    } else {
                        e.to_string()
                    }
                })?;
                let mut lifetime = ChildLifetime::new(child.id());
                let mut stdout = child.stdout.take().expect("piped stdout");
                let mut stderr = child.stderr.take().expect("piped stderr");
                let input = child.stdin.take();
                let write_input = async move {
                    if let Some(mut input) = input {
                        let _ = input.write_all(command.as_bytes()).await;
                        let _ = input.shutdown().await;
                    }
                };
                tokio::pin!(write_input);
                let cancelled = async {
                    match &options.signal {
                        Some(signal) => signal.cancelled().await,
                        None => std::future::pending::<()>().await,
                    }
                };
                tokio::pin!(cancelled);
                let deadline = timeout
                    .map(|ms| Instant::now() + Duration::from_millis(ms.trunc().max(1.0) as u64));
                let mut idle_deadline = None;
                let mut status = None;
                let mut stdout_open = true;
                let mut stderr_open = true;
                let mut input_done = false;
                let mut killed = false;
                let mut timed_out = false;
                let mut stdout_buffer = [0u8; 8192];
                let mut stderr_buffer = [0u8; 8192];
                loop {
                    if status.is_some() && !stdout_open && !stderr_open {
                        break;
                    }
                    tokio::select! {biased;
                        _=&mut cancelled,if !killed=>{
                            killed=true;
                            if let Some(pid)=lifetime.pid {if !stop_tree(pid).await {let _=child.start_kill();}}
                        }
                        _=wait_until(deadline),if !killed=>{
                            killed=true;timed_out=true;
                            if let Some(pid)=lifetime.pid {if !stop_tree(pid).await {let _=child.start_kill();}}
                        }
                        read=stdout.read(&mut stdout_buffer),if stdout_open=>{
                            let count=read.map_err(|e|e.to_string())?;
                            if count==0 {stdout_open=false;}else{
                                (options.on_data)(&stdout_buffer[..count])?;
                                if status.is_some(){idle_deadline=Some(Instant::now()+Duration::from_millis(EXIT_STDIO_GRACE_MS));}
                            }
                        }
                        read=stderr.read(&mut stderr_buffer),if stderr_open=>{
                            let count=read.map_err(|e|e.to_string())?;
                            if count==0 {stderr_open=false;}else{
                                (options.on_data)(&stderr_buffer[..count])?;
                                if status.is_some(){idle_deadline=Some(Instant::now()+Duration::from_millis(EXIT_STDIO_GRACE_MS));}
                            }
                        }
                        exit=child.wait(),if status.is_none()=>{
                            status=Some(exit.map_err(|e|e.to_string())?);
                            idle_deadline=Some(Instant::now()+Duration::from_millis(EXIT_STDIO_GRACE_MS));
                        }
                        _=&mut write_input,if !input_done=>{input_done=true;}
                        _=wait_until(idle_deadline)=>{break;}
                    }
                }
                lifetime.completed = true;
                if options.signal.as_ref().is_some_and(|s| s.is_aborted()) {
                    return Err("aborted".into());
                }
                if timed_out {
                    return Err(format!(
                        "timeout:{}",
                        js_number_string(options.timeout.expect("timeout configured"))
                    ));
                }
                let status = status.expect("exit before idle/EOF completion");
                #[cfg(unix)]
                let code = {
                    use std::os::unix::process::ExitStatusExt;
                    status
                        .code()
                        .or_else(|| status.signal().map(|signal| 128 + signal))
                        .unwrap_or(1)
                };
                #[cfg(not(unix))]
                let code = status.code().unwrap_or(1);
                Ok(ShellExit {
                    exit_code: Some(code),
                })
            })
        }),
    }
}
async fn wait_until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending::<()>().await,
    }
}
pub fn create_local_bash_operations(shell_path: Option<String>) -> ShellOperations {
    create_local_shell_operations(
        "bash",
        Arc::new(move || {
            let path = shell_path.clone();
            Box::pin(async move { get_shell_config(path.as_deref()).await })
        }),
    )
}
pub const POWERSHELL_UTF8_PREFIX: &str =
    "try { [Console]::OutputEncoding=[System.Text.Encoding]::UTF8 } catch {}\n";
pub fn create_local_powershell_operations() -> ShellOperations {
    let ops =
        create_local_shell_operations("PowerShell", Arc::new(|| Box::pin(get_powershell_config())));
    ShellOperations {
        exec: Arc::new(move |command, cwd, options| {
            (ops.exec)(format!("{POWERSHELL_UTF8_PREFIX}{command}"), cwd, options)
        }),
    }
}
#[cfg(test)]
#[path = "bash_process_tests.rs"]
mod tests;
