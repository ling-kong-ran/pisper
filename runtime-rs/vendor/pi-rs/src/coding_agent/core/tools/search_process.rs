//! Shared streaming process adapter for fd/rg. Commands are spawned directly,
//! without a shell. Dropping an in-flight future kills its child via Tokio.
use super::ToolFuture;
use std::sync::Arc;
use tokio::io::AsyncReadExt;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchCommand {
    pub program: String,
    pub args: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchExit {
    pub code: Option<i32>,
    pub stderr: String,
}
/// Return false after a result limit; remaining output is drained but not used.
pub type SearchLineCallback = Arc<dyn Fn(String) -> bool + Send + Sync>;
pub type SearchRunner =
    Arc<dyn Fn(SearchCommand, SearchLineCallback) -> ToolFuture<SearchExit> + Send + Sync>;
pub type EnsureSearchTool = Arc<dyn Fn(&'static str) -> ToolFuture<Option<String>> + Send + Sync>;
pub fn native_ensure_tool() -> EnsureSearchTool {
    Arc::new(|name| {
        Box::pin(async move {
            Ok(crate::coding_agent::utils::tools_manager::ensure_tool(name, None).await)
        })
    })
}
#[derive(Default)]
struct Readline {
    bytes: Vec<u8>,
    last_cr: Option<tokio::time::Instant>,
}
impl Readline {
    fn push(&mut self, chunk: &[u8], callback: &SearchLineCallback) -> bool {
        for byte in chunk {
            if *byte == b'\n'
                && self
                    .last_cr
                    .take()
                    .is_some_and(|at| at.elapsed() <= std::time::Duration::from_millis(100))
            {
                continue;
            }
            self.last_cr = None;
            if matches!(byte, b'\r' | b'\n') {
                let line = String::from_utf8_lossy(&self.bytes).into_owned();
                self.bytes.clear();
                if *byte == b'\r' {
                    self.last_cr = Some(tokio::time::Instant::now());
                }
                if !callback(line) {
                    return false;
                }
            } else {
                self.bytes.push(*byte);
            }
        }
        true
    }
    fn finish(&mut self, callback: &SearchLineCallback) -> bool {
        if self.bytes.is_empty() {
            true
        } else {
            let line = String::from_utf8_lossy(&self.bytes).into_owned();
            self.bytes.clear();
            callback(line)
        }
    }
}
pub fn native_runner() -> SearchRunner {
    Arc::new(|command, on_line| {
        Box::pin(async move {
            let mut process = tokio::process::Command::new(&command.program);
            process
                .args(&command.args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true);
            #[cfg(windows)]
            process.creation_flags(0x08000000);
            let mut child = process
                .spawn()
                .map_err(|error| format!("spawn {} {}", command.program, super::io_code(&error)))?;
            let mut stdout = child.stdout.take().expect("piped stdout");
            let mut stderr = child.stderr.take().expect("piped stderr");
            let mut output = [0u8; 8192];
            let mut errors = [0u8; 8192];
            let mut stdout_open = true;
            let mut stderr_open = true;
            let mut exit = None;
            let mut stopped = false;
            let mut decoder = Readline::default();
            let mut error_text = String::new();
            loop {
                tokio::select! {
                    result=stdout.read(&mut output),if stdout_open=>{
                        let count=result.map_err(|e|e.to_string())?;
                        if count==0{stdout_open=false;if !stopped&&!decoder.finish(&on_line){stopped=true;let _=child.start_kill();}}
                        else if !stopped&&!decoder.push(&output[..count],&on_line){stopped=true;let _=child.start_kill();}
                    },
                    result=stderr.read(&mut errors),if stderr_open=>{
                        let count=result.map_err(|e|e.to_string())?;if count==0{stderr_open=false;}else{error_text.push_str(&String::from_utf8_lossy(&errors[..count]));}
                    },
                    result=child.wait(),if exit.is_none()=>{exit=Some(result.map_err(|e|e.to_string())?.code());},
                }
                if !stdout_open && !stderr_open && exit.is_some() {
                    break;
                }
            }
            Ok(SearchExit {
                code: exit.flatten(),
                stderr: error_text,
            })
        })
    })
}
#[cfg(test)]
#[path = "search_process_tests.rs"]
mod tests;
