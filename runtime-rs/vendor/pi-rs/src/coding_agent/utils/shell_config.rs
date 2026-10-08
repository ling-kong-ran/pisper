//! Shell discovery and environment helpers from upstream `utils/shell.ts`.
//! Native lookup is asynchronous but retains its 5s process deadline and
//! Windows first-match existence check. Environment/IO can be injected.
use crate::coding_agent::utils::{node_path, text::trim_js_whitespace};
use crate::tui::utf16::Utf16Text;
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::{path::Path, process::Stdio, sync::Arc, time::Duration};

pub type ShellEnvironment = Vec<(String, String)>;
pub type ExecutableLookup = Arc<dyn Fn(String) -> BoxFuture<'static, Option<String>> + Send + Sync>;
#[derive(Clone)]
pub struct ShellDiscovery {
    pub windows: bool,
    pub environment: ShellEnvironment,
    pub exists: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    /// Successful where/which stdout; None includes timeout, error/nonzero exit.
    pub lookup: ExecutableLookup,
}
impl Default for ShellDiscovery {
    fn default() -> Self {
        Self {
            windows: cfg!(windows),
            environment: std::env::vars().collect(),
            exists: Arc::new(|path| Path::new(path).exists()),
            lookup: Arc::new(|name| {
                Box::pin(async move {
                    let mut command =
                        tokio::process::Command::new(if cfg!(windows) { "where" } else { "which" });
                    command
                        .arg(name)
                        .stdin(Stdio::null())
                        .stderr(Stdio::null())
                        .kill_on_drop(true);
                    #[cfg(windows)]
                    command.creation_flags(0x0800_0000);
                    let output = tokio::time::timeout(Duration::from_secs(5), command.output())
                        .await
                        .ok()?
                        .ok()?;
                    output
                        .status
                        .success()
                        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
                })
            }),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommandTransport {
    Argv,
    Stdin,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellConfig {
    pub shell: String,
    pub args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command_transport: Option<CommandTransport>,
}
pub fn is_legacy_wsl_bash_path(path: &str) -> bool {
    let normalized = path.replace('/', "\\").to_lowercase();
    let bytes = normalized.as_bytes();
    bytes.len() > 2
        && bytes[0].is_ascii_alphabetic()
        && matches!(
            &normalized[1..],
            ":\\windows\\system32\\bash.exe" | ":\\windows\\sysnative\\bash.exe"
        )
}
fn bash_config(shell: &str) -> ShellConfig {
    let stdin = is_legacy_wsl_bash_path(shell);
    ShellConfig {
        shell: shell.into(),
        args: vec![if stdin { "-s" } else { "-c" }.into()],
        command_transport: stdin.then_some(CommandTransport::Stdin),
    }
}
async fn find_executable(executable: &str, host: &ShellDiscovery) -> Option<String> {
    let output = (host.lookup)(executable.into()).await?;
    let output = trim_js_whitespace(&output);
    let first = output
        .split_once('\n')
        .map(|(head, _)| head.strip_suffix('\r').unwrap_or(head))
        .unwrap_or(output);
    if first.is_empty() || (host.windows && !(host.exists)(first)) {
        None
    } else {
        Some(first.into())
    }
}
pub async fn get_shell_config(custom_shell_path: Option<&str>) -> Result<ShellConfig, String> {
    get_shell_config_with(custom_shell_path, &ShellDiscovery::default()).await
}
pub async fn get_shell_config_with(
    custom_shell_path: Option<&str>,
    host: &ShellDiscovery,
) -> Result<ShellConfig, String> {
    if let Some(custom) = custom_shell_path.filter(|s| !s.is_empty()) {
        return if (host.exists)(custom) {
            Ok(bash_config(custom))
        } else {
            Err(format!("Custom shell path not found: {custom}"))
        };
    }
    if host.windows {
        let paths = ["ProgramFiles", "ProgramFiles(x86)"]
            .into_iter()
            .filter_map(|key| {
                host.environment
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(key))
                    .map(|(_, v)| v)
                    .filter(|v| !v.is_empty())
            })
            .map(|path| format!("{path}\\Git\\bin\\bash.exe"))
            .collect::<Vec<_>>();
        for path in &paths {
            if (host.exists)(path) {
                return Ok(bash_config(path));
            }
        }
        if let Some(shell) = find_executable("bash.exe", host).await {
            return Ok(bash_config(&shell));
        }
        return Err(format!("No bash shell found. Options:\n  1. Install Git for Windows: https://git-scm.com/download/win\n  2. Add your bash to PATH (Cygwin, MSYS2, etc.)\n  3. Set shellPath in settings.json\n\nSearched Git Bash in:\n{}",paths.iter().map(|p|format!("  {p}")).collect::<Vec<_>>().join("\n")));
    }
    if (host.exists)("/bin/bash") {
        return Ok(bash_config("/bin/bash"));
    }
    if let Some(shell) = find_executable("bash", host).await {
        return Ok(bash_config(&shell));
    }
    Ok(ShellConfig {
        shell: "sh".into(),
        args: vec!["-c".into()],
        command_transport: None,
    })
}
pub const POWERSHELL_ARGS: &[&str] = &[
    "-NoProfile",
    "-NonInteractive",
    "-ExecutionPolicy",
    "Bypass",
    "-Command",
];
pub async fn get_powershell_config() -> Result<ShellConfig, String> {
    get_powershell_config_with(&ShellDiscovery::default()).await
}
pub async fn get_powershell_config_with(host: &ShellDiscovery) -> Result<ShellConfig, String> {
    if !host.windows {
        return Err("The powershell tool is only available on Windows.".into());
    }
    let shell = match find_executable("pwsh.exe", host).await {
        Some(shell) => shell,
        None => {
            match find_executable("powershell.exe", host).await {
                Some(shell) => shell,
                None => {
                    // Well-known install locations as a last resort: when
                    // tests (or any cargo-spawned process) run, cargo
                    // prepends `target\debug\...` build-output dirs to PATH,
                    // and a `where` scan across those thousands of
                    // C-artifact files (Defender-amplified) blows past the
                    // discovery timeout on dev machines. Upstream's lookup
                    // trace above is untouched; these documented install
                    // paths only fire after both lookups miss.
                    for known in [
                        r"C:\Program Files\PowerShell\7\pwsh.exe",
                        r"C:\Program Files (x86)\PowerShell\7\pwsh.exe",
                        r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe",
                    ] {
                        if (host.exists)(known) {
                            return Ok(ShellConfig {
                                shell: known.to_string(),
                                args: POWERSHELL_ARGS.iter().map(|s| (*s).into()).collect(),
                                command_transport: None,
                            });
                        }
                    }
                    return Err("No PowerShell executable found. Install PowerShell or add powershell.exe/pwsh.exe to PATH.".into());
                }
            }
        }
    };
    Ok(ShellConfig {
        shell,
        args: POWERSHELL_ARGS.iter().map(|s| (*s).into()).collect(),
        command_transport: None,
    })
}
pub fn shell_environment_with(
    environment: ShellEnvironment,
    bin_dir: &str,
    windows: bool,
) -> ShellEnvironment {
    let mut environment = environment;
    let delimiter = if windows { ';' } else { ':' };
    let key = environment
        .iter()
        .position(|(k, _)| k.to_lowercase() == "path");
    let current = key.map(|i| environment[i].1.as_str()).unwrap_or("");
    let has_bin = current
        .split(delimiter)
        .filter(|s| !s.is_empty())
        .any(|s| s == bin_dir);
    let value = if has_bin {
        current.to_owned()
    } else {
        [bin_dir, current]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(&delimiter.to_string())
    };
    if let Some(index) = key {
        environment[index].1 = value;
    } else {
        environment.push(("PATH".into(), value));
    }
    environment
}
pub fn get_shell_env() -> ShellEnvironment {
    let agent = crate::coding_agent::core::get_agent_dir();
    let bin = if cfg!(windows) {
        node_path::win32_join(&[&agent, "bin"])
    } else {
        node_path::posix_join(&[&agent, "bin"])
    };
    shell_environment_with(std::env::vars().collect(), &bin, cfg!(windows))
}
/// The actual upstream predicate only removes C0 (except TAB/LF/CR) and
/// U+FFF9..U+FFFB. Its comment claims more; other format chars are preserved.
pub fn sanitize_binary_output(text: &str) -> String {
    text.chars()
        .filter(|&c| {
            matches!(c, '\t' | '\n' | '\r')
                || (c > '\u{1f}' && !('\u{fff9}'..='\u{fffb}').contains(&c))
        })
        .collect()
}
pub fn sanitize_binary_output_utf16(text: &Utf16Text) -> Utf16Text {
    Utf16Text::from_units(
        text.as_ref()
            .iter()
            .copied()
            .filter(|&c| matches!(c, 9 | 10 | 13) || (c > 31 && !(0xfff9..=0xfffb).contains(&c)))
            .collect(),
    )
}
#[cfg(test)]
#[path = "shell_config_tests.rs"]
mod tests;
