use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::{
    collections::HashMap,
    io::{Read, Write},
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread,
};
use tauri::{ipc::Channel, State, WebviewWindow};

const MAX_TERMINALS: usize = 12;
const MAX_INPUT_BYTES: usize = 256 * 1024;
static NEXT_TERMINAL_INSTANCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Default)]
pub struct DesktopTerminalState(Arc<Mutex<HashMap<String, ManagedTerminal>>>);

struct ManagedTerminal {
    instance_id: u64,
    master: Box<dyn MasterPty + Send>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    killer: Box<dyn ChildKiller + Send + Sync>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalProfile {
    id: String,
    label: String,
    default: bool,
}

#[derive(Clone, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum TerminalEvent {
    Output {
        terminal_id: String,
        data: Vec<u8>,
    },
    Exit {
        terminal_id: String,
        code: Option<u32>,
    },
    Error {
        terminal_id: String,
        message: String,
    },
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalCreateInput {
    terminal_id: String,
    profile_id: String,
    cwd: String,
    cols: u16,
    rows: u16,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalCreated {
    terminal_id: String,
    profile_id: String,
    cwd: String,
}

#[derive(Clone)]
struct ShellProfile {
    id: String,
    label: String,
    program: PathBuf,
    args: Vec<String>,
}

fn ensure_main(window: &WebviewWindow) -> Result<(), String> {
    if window.label() == "main" {
        Ok(())
    } else {
        Err("The terminal is available only in the main desktop window.".into())
    }
}

fn profile_if_available(
    id: &'static str,
    label: &'static str,
    program: impl Into<PathBuf>,
    args: &'static [&'static str],
) -> Option<ShellProfile> {
    let program = program.into();
    if program.is_absolute() && !program.is_file() {
        return None;
    }
    Some(ShellProfile {
        id: id.into(),
        label: label.into(),
        program,
        args: args.iter().map(|value| (*value).into()).collect(),
    })
}

fn shell_profiles() -> Vec<ShellProfile> {
    #[cfg(windows)]
    {
        let mut profiles = Vec::new();
        if let Some(program_files) = std::env::var_os("ProgramFiles") {
            if let Some(profile) = profile_if_available(
                "pwsh",
                "PowerShell",
                PathBuf::from(program_files)
                    .join("PowerShell")
                    .join("7")
                    .join("pwsh.exe"),
                &["-NoLogo"],
            ) {
                profiles.push(profile);
            }
        }
        let system_root = std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        if let Some(profile) = profile_if_available(
            "powershell",
            "Windows PowerShell",
            system_root
                .join("System32")
                .join("WindowsPowerShell")
                .join("v1.0")
                .join("powershell.exe"),
            &["-NoLogo"],
        ) {
            profiles.push(profile);
        }
        if let Some(profile) = profile_if_available(
            "cmd",
            "Command Prompt",
            system_root.join("System32").join("cmd.exe"),
            &[],
        ) {
            profiles.push(profile);
        }
        profiles
    }
    #[cfg(not(windows))]
    {
        let mut profiles = Vec::new();
        let configured = std::env::var_os("SHELL")
            .map(PathBuf::from)
            .filter(|path| path.is_file());
        if let Some(program) = configured {
            let label = program
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("Shell")
                .to_string();
            profiles.push(ShellProfile {
                id: "default".into(),
                label,
                program,
                args: vec!["-l".into()],
            });
        }
        for (id, label, program) in [
            ("zsh", "Zsh", "/bin/zsh"),
            ("bash", "Bash", "/bin/bash"),
            ("sh", "Shell", "/bin/sh"),
        ] {
            if profiles
                .iter()
                .any(|profile| profile.program == std::path::Path::new(program))
            {
                continue;
            }
            if let Some(profile) = profile_if_available(id, label, program, &["-l"]) {
                profiles.push(profile);
            }
        }
        profiles
    }
}

fn terminal_size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows: rows.clamp(2, 500),
        cols: cols.clamp(2, 500),
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn checked_cwd(cwd: &str) -> Result<PathBuf, String> {
    let path = if cwd.trim().is_empty() {
        std::env::current_dir().map_err(|error| error.to_string())?
    } else {
        PathBuf::from(cwd.trim())
    };
    if !path.is_dir() {
        return Err("The terminal working directory does not exist.".into());
    }
    Ok(path)
}

#[tauri::command]
pub fn desktop_terminal_profiles(window: WebviewWindow) -> Result<Vec<TerminalProfile>, String> {
    ensure_main(&window)?;
    Ok(shell_profiles()
        .into_iter()
        .enumerate()
        .map(|(index, profile)| TerminalProfile {
            id: profile.id,
            label: profile.label,
            default: index == 0,
        })
        .collect())
}

#[tauri::command]
pub fn desktop_terminal_create(
    window: WebviewWindow,
    state: State<'_, DesktopTerminalState>,
    input: TerminalCreateInput,
    on_event: Channel<TerminalEvent>,
) -> Result<TerminalCreated, String> {
    ensure_main(&window)?;
    let TerminalCreateInput {
        terminal_id,
        profile_id,
        cwd,
        cols,
        rows,
    } = input;
    if terminal_id.is_empty() || terminal_id.len() > 100 {
        return Err("Invalid terminal identifier.".into());
    }
    let profiles = shell_profiles();
    let profile = profiles
        .into_iter()
        .find(|profile| profile.id == profile_id)
        .ok_or_else(|| "Unknown terminal profile.".to_string())?;
    let cwd = checked_cwd(&cwd)?;

    {
        let terminals = state
            .0
            .lock()
            .map_err(|_| "Terminal state is unavailable.".to_string())?;
        if terminals.contains_key(&terminal_id) {
            return Err("A terminal with this identifier already exists.".into());
        }
        if terminals.len() >= MAX_TERMINALS {
            return Err(format!(
                "No more than {MAX_TERMINALS} terminals may run at once."
            ));
        }
    }

    let pair = native_pty_system()
        .openpty(terminal_size(cols, rows))
        .map_err(|error| format!("Failed to open terminal: {error}"))?;
    let mut command = CommandBuilder::new(&profile.program);
    command.args(profile.args);
    command.cwd(&cwd);
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    let mut child = pair
        .slave
        .spawn_command(command)
        .map_err(|error| format!("Failed to start shell: {error}"))?;
    drop(pair.slave);
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| format!("Failed to read terminal output: {error}"))?;
    let writer =
        Arc::new(Mutex::new(pair.master.take_writer().map_err(|error| {
            format!("Failed to open terminal input: {error}")
        })?));
    let killer = child.clone_killer();
    let instance_id = NEXT_TERMINAL_INSTANCE.fetch_add(1, Ordering::Relaxed);
    {
        let mut terminals = state
            .0
            .lock()
            .map_err(|_| "Terminal state is unavailable.".to_string())?;
        if terminals.contains_key(&terminal_id) || terminals.len() >= MAX_TERMINALS {
            thread::spawn(move || {
                let mut killer = killer;
                let _ = killer.kill();
                let _ = child.wait();
            });
            return Err(if terminals.contains_key(&terminal_id) {
                "A terminal with this identifier already exists.".into()
            } else {
                format!("No more than {MAX_TERMINALS} terminals may run at once.")
            });
        }
        terminals.insert(
            terminal_id.clone(),
            ManagedTerminal {
                instance_id,
                master: pair.master,
                writer,
                killer,
            },
        );
    }

    let event_terminal_id = terminal_id.clone();
    let output_channel = on_event.clone();
    thread::spawn(move || {
        let mut buffer = vec![0_u8; 16 * 1024];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    if output_channel
                        .send(TerminalEvent::Output {
                            terminal_id: event_terminal_id.clone(),
                            data: buffer[..count].to_vec(),
                        })
                        .is_err()
                    {
                        break;
                    }
                }
                Err(error) => {
                    let _ = output_channel.send(TerminalEvent::Error {
                        terminal_id: event_terminal_id.clone(),
                        message: error.to_string(),
                    });
                    break;
                }
            }
        }
    });
    let exit_terminal_id = terminal_id.clone();
    let terminal_registry = Arc::clone(&state.0);
    thread::spawn(move || {
        let event = match child.wait() {
            Ok(status) => TerminalEvent::Exit {
                terminal_id: exit_terminal_id.clone(),
                code: Some(status.exit_code()),
            },
            Err(error) => TerminalEvent::Error {
                terminal_id: exit_terminal_id.clone(),
                message: error.to_string(),
            },
        };
        let _ = on_event.send(event);
        if let Ok(mut terminals) = terminal_registry.lock() {
            if terminals
                .get(&exit_terminal_id)
                .is_some_and(|terminal| terminal.instance_id == instance_id)
            {
                terminals.remove(&exit_terminal_id);
            }
        }
    });

    Ok(TerminalCreated {
        terminal_id,
        profile_id: profile.id,
        cwd: cwd.to_string_lossy().into_owned(),
    })
}

#[tauri::command]
pub fn desktop_terminal_write(
    window: WebviewWindow,
    state: State<'_, DesktopTerminalState>,
    terminal_id: String,
    data: Vec<u8>,
) -> Result<(), String> {
    ensure_main(&window)?;
    if data.len() > MAX_INPUT_BYTES {
        return Err("Terminal input is too large.".into());
    }
    let writer = state
        .0
        .lock()
        .map_err(|_| "Terminal state is unavailable.".to_string())?
        .get(&terminal_id)
        .map(|terminal| Arc::clone(&terminal.writer))
        .ok_or_else(|| "Terminal not found.".to_string())?;
    let mut writer = writer
        .lock()
        .map_err(|_| "Terminal input is unavailable.".to_string())?;
    writer.write_all(&data).map_err(|error| error.to_string())?;
    writer.flush().map_err(|error| error.to_string())
}

#[tauri::command]
pub fn desktop_terminal_resize(
    window: WebviewWindow,
    state: State<'_, DesktopTerminalState>,
    terminal_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    ensure_main(&window)?;
    let terminals = state
        .0
        .lock()
        .map_err(|_| "Terminal state is unavailable.".to_string())?;
    terminals
        .get(&terminal_id)
        .ok_or_else(|| "Terminal not found.".to_string())?
        .master
        .resize(terminal_size(cols, rows))
        .map_err(|error| error.to_string())
}

fn stop_terminal(mut terminal: ManagedTerminal) {
    thread::spawn(move || {
        let _ = terminal.killer.kill();
    });
}

#[tauri::command]
pub fn desktop_terminal_close(
    window: WebviewWindow,
    state: State<'_, DesktopTerminalState>,
    terminal_id: String,
) -> Result<bool, String> {
    ensure_main(&window)?;
    let terminal = state
        .0
        .lock()
        .map_err(|_| "Terminal state is unavailable.".to_string())?
        .remove(&terminal_id);
    if let Some(terminal) = terminal {
        stop_terminal(terminal);
        Ok(true)
    } else {
        Ok(false)
    }
}

#[tauri::command]
pub fn desktop_terminal_close_all(
    window: WebviewWindow,
    state: State<'_, DesktopTerminalState>,
) -> Result<usize, String> {
    ensure_main(&window)?;
    Ok(close_all(&state))
}

pub fn close_all(state: &DesktopTerminalState) -> usize {
    let terminals = match state.0.lock() {
        Ok(mut terminals) => terminals
            .drain()
            .map(|(_, terminal)| terminal)
            .collect::<Vec<_>>(),
        Err(_) => return 0,
    };
    let count = terminals.len();
    for terminal in terminals {
        stop_terminal(terminal);
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_size_is_bounded() {
        assert_eq!(terminal_size(0, 0).cols, 2);
        assert_eq!(terminal_size(u16::MAX, u16::MAX).rows, 500);
    }

    #[test]
    fn profiles_do_not_expose_arbitrary_programs() {
        let profiles = shell_profiles();
        assert!(!profiles.is_empty());
        assert!(profiles.iter().all(|profile| !profile.id.is_empty()));
        let public = TerminalProfile {
            id: profiles[0].id.clone(),
            label: profiles[0].label.clone(),
            default: true,
        };
        let serialized = serde_json::to_value(public).unwrap();
        assert_eq!(serialized["id"], profiles[0].id);
        assert!(serialized.get("program").is_none());
    }
}
