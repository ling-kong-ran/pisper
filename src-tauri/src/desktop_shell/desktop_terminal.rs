use portable_pty::{native_pty_system, Child, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::{
    collections::HashMap,
    io::{Read, Write},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Condvar, Mutex,
    },
    thread,
    time::Duration,
};
use tauri::{ipc::Channel, State, WebviewWindow};

const MAX_TERMINALS: usize = 12;
const MAX_INPUT_BYTES: usize = 256 * 1024;
const CLOSE_TIMEOUT: Duration = Duration::from_secs(10);
static NEXT_TERMINAL_INSTANCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Default)]
pub struct DesktopTerminalState(Arc<Mutex<TerminalRegistry>>);

#[derive(Default)]
struct TerminalRegistry {
    terminals: HashMap<String, ManagedTerminal>,
    shutting_down: bool,
}

#[derive(Clone)]
struct ManagedTerminal {
    instance_id: u64,
    master: Arc<Mutex<Option<Box<dyn MasterPty + Send>>>>,
    writer: Arc<Mutex<Option<Box<dyn Write + Send>>>>,
    control: Arc<TerminalControl>,
    completion: Arc<TerminalCompletion>,
}

struct TerminalControl {
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    tree: ProcessTree,
    stopping: AtomicBool,
}

impl TerminalControl {
    fn stop(&self) -> Result<(), String> {
        if self.stopping.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let result = self.tree.terminate();
        if let Ok(mut killer) = self.killer.lock() {
            // portable-pty 0.9's Windows killer incorrectly reports an error on a
            // successful TerminateProcess. The job/process-group result owns the
            // tree result; the child handle remains an additional fallback.
            let _ = killer.kill();
        }
        if result.is_err() {
            self.stopping.store(false, Ordering::Release);
        }
        result.map_err(|error| format!("Failed to stop terminal processes: {error}"))
    }
}

#[derive(Default)]
struct TerminalCompletion {
    finished: Mutex<bool>,
    changed: Condvar,
}

impl TerminalCompletion {
    fn finish(&self) {
        if let Ok(mut finished) = self.finished.lock() {
            *finished = true;
            self.changed.notify_all();
        }
    }

    fn wait(&self) -> Result<(), String> {
        let finished = self
            .finished
            .lock()
            .map_err(|_| "Terminal cleanup is unavailable.".to_string())?;
        let (finished, _) = self
            .changed
            .wait_timeout_while(finished, CLOSE_TIMEOUT, |finished| !*finished)
            .map_err(|_| "Terminal cleanup is unavailable.".to_string())?;
        if *finished {
            Ok(())
        } else {
            Err("Terminal processes did not finish closing.".into())
        }
    }
}

type EventSink = Arc<dyn Fn(TerminalEvent) -> Result<(), String> + Send + Sync>;

#[cfg(windows)]
struct ProcessTree(std::os::windows::io::OwnedHandle);

#[cfg(windows)]
impl ProcessTree {
    fn new() -> std::io::Result<Self> {
        use std::os::windows::io::{AsRawHandle, FromRawHandle};
        use windows_sys::Win32::System::JobObjects::{
            CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        let job = Self(unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(handle) });
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if unsafe {
            SetInformationJobObject(
                job.0.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&limits) as u32,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        Ok(job)
    }

    fn attach(&self, child: &dyn Child) -> std::io::Result<()> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
        let handle = child
            .as_raw_handle()
            .ok_or_else(|| std::io::Error::other("The shell process handle is unavailable."))?;
        if unsafe { AssignProcessToJobObject(self.0.as_raw_handle(), handle) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    fn terminate(&self) -> std::io::Result<()> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;
        if unsafe { TerminateJobObject(self.0.as_raw_handle(), 1) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(unix)]
struct ProcessTree(std::sync::atomic::AtomicI32);

#[cfg(unix)]
impl ProcessTree {
    fn new() -> std::io::Result<Self> {
        Ok(Self(std::sync::atomic::AtomicI32::new(0)))
    }

    fn attach(&self, child: &dyn Child) -> std::io::Result<()> {
        let pid = child
            .process_id()
            .and_then(|pid| i32::try_from(pid).ok())
            .ok_or_else(|| std::io::Error::other("The shell process is unavailable."))?;
        // portable-pty calls setsid before exec: the initial shell owns this group.
        self.0.store(pid, Ordering::Release);
        Ok(())
    }

    fn terminate(&self) -> std::io::Result<()> {
        let group = self.0.load(Ordering::Acquire);
        if group > 0 && unsafe { libc::kill(-group, libc::SIGKILL) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(error);
            }
        }
        Ok(())
    }
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
    create_terminal(
        &state,
        input,
        Arc::new(move |event| on_event.send(event).map_err(|error| error.to_string())),
    )
}

fn create_terminal(
    state: &DesktopTerminalState,
    input: TerminalCreateInput,
    on_event: EventSink,
) -> Result<TerminalCreated, String> {
    let profile = shell_profiles()
        .into_iter()
        .find(|profile| profile.id == input.profile_id)
        .ok_or_else(|| "Unknown terminal profile.".to_string())?;
    create_terminal_with_profile(state, input, profile, on_event)
}

fn create_terminal_with_profile(
    state: &DesktopTerminalState,
    input: TerminalCreateInput,
    profile: ShellProfile,
    on_event: EventSink,
) -> Result<TerminalCreated, String> {
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
    let cwd = checked_cwd(&cwd)?;
    // Serialize admission and shutdown through spawn and registration. No child
    // can start after shutdown has acquired this lock and closed admission.
    let mut registry = state
        .0
        .lock()
        .map_err(|_| "Terminal state is unavailable.".to_string())?;
    if registry.shutting_down {
        return Err("The desktop terminal is shutting down.".into());
    }
    if registry.terminals.contains_key(&terminal_id) {
        return Err("A terminal with this identifier already exists.".into());
    }
    if registry.terminals.len() >= MAX_TERMINALS {
        return Err(format!(
            "No more than {MAX_TERMINALS} terminals may run at once."
        ));
    }

    let pair = native_pty_system()
        .openpty(terminal_size(cols, rows))
        .map_err(|error| format!("Failed to open terminal: {error}"))?;
    let mut command = CommandBuilder::new(&profile.program);
    command.args(&profile.args);
    command.cwd(&cwd);
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    // Prepare every fallible pipe/job resource before creating the shell.
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| format!("Failed to read terminal output: {error}"))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|error| format!("Failed to open terminal input: {error}"))?;
    let tree = ProcessTree::new()
        .map_err(|error| format!("Failed to prepare terminal process ownership: {error}"))?;
    let mut child = pair
        .slave
        .spawn_command(command)
        .map_err(|error| format!("Failed to start shell: {error}"))?;
    drop(pair.slave);
    if let Err(error) = tree.attach(child.as_ref()) {
        // ClosePseudoConsole can wait for output to drain. Keep a reader alive
        // during failed initialization, terminate and reap before returning.
        let drain = thread::spawn(move || {
            let _ = std::io::copy(&mut reader, &mut std::io::sink());
        });
        let _ = tree.terminate();
        let _ = child.kill();
        drop(writer);
        drop(pair.master);
        let _ = child.wait();
        let _ = drain.join();
        return Err(format!("Failed to own terminal processes: {error}"));
    }
    let instance_id = NEXT_TERMINAL_INSTANCE.fetch_add(1, Ordering::Relaxed);
    let terminal = ManagedTerminal {
        instance_id,
        master: Arc::new(Mutex::new(Some(pair.master))),
        writer: Arc::new(Mutex::new(Some(writer))),
        control: Arc::new(TerminalControl {
            killer: Mutex::new(child.clone_killer()),
            tree,
            stopping: AtomicBool::new(false),
        }),
        completion: Arc::new(TerminalCompletion::default()),
    };
    registry
        .terminals
        .insert(terminal_id.clone(), terminal.clone());
    drop(registry);

    let event_terminal_id = terminal_id.clone();
    let output_channel = on_event.clone();
    let output_control = terminal.control.clone();
    let output_thread = thread::spawn(move || {
        let mut buffer = vec![0_u8; 16 * 1024];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    if output_channel(TerminalEvent::Output {
                        terminal_id: event_terminal_id.clone(),
                        data: buffer[..count].to_vec(),
                    })
                    .is_err()
                    {
                        let _ = output_control.stop();
                        // The master still owns its original read handle. Drain
                        // shutdown output even when the renderer is gone, so
                        // pre-24H2 ClosePseudoConsole cannot block on a full pipe.
                        let _ = std::io::copy(&mut reader, &mut std::io::sink());
                        break;
                    }
                }
                Err(error) => {
                    #[cfg(unix)]
                    if error.raw_os_error() == Some(libc::EIO) {
                        // PTY readers report EIO when the final slave closes.
                        break;
                    }
                    if !output_control.stopping.load(Ordering::Acquire) {
                        let _ = output_channel(TerminalEvent::Error {
                            terminal_id: event_terminal_id.clone(),
                            message: error.to_string(),
                        });
                    }
                    let _ = output_control.stop();
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
        let _ = terminal.control.stop();
        close_terminal_io(&terminal);
        // All bytes precede the final event, including short-lived shells.
        let _ = output_thread.join();
        let _ = on_event(event);
        if let Ok(mut registry) = terminal_registry.lock() {
            if registry
                .terminals
                .get(&exit_terminal_id)
                .is_some_and(|terminal| terminal.instance_id == instance_id)
            {
                registry.terminals.remove(&exit_terminal_id);
            }
        }
        terminal.completion.finish();
    });

    Ok(TerminalCreated {
        terminal_id,
        profile_id,
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
    write_terminal(&state, &terminal_id, &data)
}

fn write_terminal(
    state: &DesktopTerminalState,
    terminal_id: &str,
    data: &[u8],
) -> Result<(), String> {
    if data.len() > MAX_INPUT_BYTES {
        return Err("Terminal input is too large.".into());
    }
    let writer = state
        .0
        .lock()
        .map_err(|_| "Terminal state is unavailable.".to_string())?
        .terminals
        .get(terminal_id)
        .map(|terminal| Arc::clone(&terminal.writer))
        .ok_or_else(|| "Terminal not found.".to_string())?;
    let mut writer = writer
        .lock()
        .map_err(|_| "Terminal input is unavailable.".to_string())?;
    let writer = writer
        .as_mut()
        .ok_or_else(|| "Terminal not found.".to_string())?;
    writer.write_all(data).map_err(|error| error.to_string())?;
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
    resize_terminal(&state, &terminal_id, cols, rows)
}

fn resize_terminal(
    state: &DesktopTerminalState,
    terminal_id: &str,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let master = state
        .0
        .lock()
        .map_err(|_| "Terminal state is unavailable.".to_string())?
        .terminals
        .get(terminal_id)
        .map(|terminal| terminal.master.clone())
        .ok_or_else(|| "Terminal not found.".to_string())?;
    let master = master
        .lock()
        .map_err(|_| "Terminal output is unavailable.".to_string())?;
    master
        .as_ref()
        .ok_or_else(|| "Terminal not found.".to_string())?
        .resize(terminal_size(cols, rows))
        .map_err(|error| error.to_string())
}

fn close_terminal_io(terminal: &ManagedTerminal) {
    if let Ok(mut master) = terminal.master.lock() {
        master.take();
    }
    // Closing the pseudo-console also releases a blocked input write. Do not
    // wait for the writer mutex before closing the console.
    if let Ok(mut writer) = terminal.writer.lock() {
        writer.take();
    }
}

fn stop_terminal(terminal: ManagedTerminal) -> Result<(), String> {
    let stopped = terminal.control.stop();
    close_terminal_io(&terminal);
    terminal.completion.wait()?;
    stopped
}

#[tauri::command]
pub fn desktop_terminal_close(
    window: WebviewWindow,
    state: State<'_, DesktopTerminalState>,
    terminal_id: String,
) -> Result<bool, String> {
    ensure_main(&window)?;
    close_terminal(&state, &terminal_id)
}

fn close_terminal(state: &DesktopTerminalState, terminal_id: &str) -> Result<bool, String> {
    let terminal = state
        .0
        .lock()
        .map_err(|_| "Terminal state is unavailable.".to_string())?
        .terminals
        .remove(terminal_id);
    if let Some(terminal) = terminal {
        stop_terminal(terminal)?;
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
    close_registry(&state, false)
}

/// Close admission permanently before GUI teardown. Ordinary panel close-all
/// keeps admission open so the user can create another terminal later.
pub fn shutdown(state: &DesktopTerminalState) -> usize {
    close_registry(state, true).unwrap_or_else(|error| {
        eprintln!("{error}");
        0
    })
}

fn close_registry(state: &DesktopTerminalState, shutting_down: bool) -> Result<usize, String> {
    let terminals = match state.0.lock() {
        Ok(mut registry) => {
            registry.shutting_down |= shutting_down;
            registry
                .terminals
                .drain()
                .map(|(_, terminal)| terminal)
                .collect::<Vec<_>>()
        }
        Err(_) => return Err("Terminal state is unavailable.".into()),
    };
    let count = terminals.len();
    // Signal every job before waiting for any one child, outside the registry
    // lock. The waiters must acquire that lock to retire their own instances.
    let mut first_error = None;
    for terminal in &terminals {
        if let Err(error) = terminal.control.stop() {
            first_error.get_or_insert(error);
        }
    }
    for terminal in &terminals {
        close_terminal_io(terminal);
    }
    for terminal in &terminals {
        if let Err(error) = terminal.completion.wait() {
            first_error.get_or_insert(error);
        }
    }
    first_error.map_or(Ok(count), Err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "pisper-native-terminal-{}-{}",
                std::process::id(),
                NEXT_TERMINAL_INSTANCE.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct Terminals(DesktopTerminalState);

    impl Terminals {
        fn new() -> Self {
            Self(DesktopTerminalState::default())
        }
    }

    impl Drop for Terminals {
        fn drop(&mut self) {
            shutdown(&self.0);
        }
    }

    #[derive(Default)]
    struct Events {
        values: Mutex<Vec<TerminalEvent>>,
        changed: Condvar,
    }

    impl Events {
        fn sink(self: &Arc<Self>) -> EventSink {
            let events = self.clone();
            Arc::new(move |event| {
                events.values.lock().unwrap().push(event);
                events.changed.notify_all();
                Ok(())
            })
        }

        fn wait(&self, condition: impl Fn(&[TerminalEvent]) -> bool) {
            let values = self.values.lock().unwrap();
            let (values, timeout) = self
                .changed
                .wait_timeout_while(values, CLOSE_TIMEOUT, |values| !condition(values))
                .unwrap();
            let satisfied = condition(&values);
            let output = String::from_utf8_lossy(&Self::output(&values)).into_owned();
            // An assertion while holding this lock poisons both native event
            // threads and prevents RAII cleanup from observing their finish.
            drop(values);
            assert!(
                satisfied,
                "Native PTY condition timed out: {}; actual synthetic output: {output:?}",
                timeout.timed_out(),
            );
        }

        fn output(values: &[TerminalEvent]) -> Vec<u8> {
            values
                .iter()
                .filter_map(|event| match event {
                    TerminalEvent::Output { data, .. } => Some(data.as_slice()),
                    _ => None,
                })
                .flatten()
                .copied()
                .collect()
        }
    }

    fn fixture_profile() -> ShellProfile {
        #[cfg(windows)]
        {
            let mut profile = shell_profiles()
                .into_iter()
                .find(|profile| profile.id == "powershell" || profile.id == "pwsh")
                .expect("Windows PowerShell is required for the ConPTY fixture");
            // The application continues to load user profiles. Tests explicitly
            // suppress them to avoid executing personal configuration.
            profile.args = vec!["-NoLogo".into(), "-NoProfile".into()];
            profile
        }
        #[cfg(unix)]
        {
            ShellProfile {
                id: "sh".into(),
                label: "Fixture shell".into(),
                program: PathBuf::from("/bin/sh"),
                args: vec![],
            }
        }
    }

    fn input(fixture: &Fixture, id: &str) -> TerminalCreateInput {
        TerminalCreateInput {
            terminal_id: id.into(),
            profile_id: fixture_profile().id,
            cwd: fixture.0.to_string_lossy().into_owned(),
            cols: 100,
            rows: 24,
        }
    }

    fn create_fixture(state: &DesktopTerminalState, fixture: &Fixture, id: &str, sink: EventSink) {
        create_terminal_with_profile(state, input(fixture, id), fixture_profile(), sink)
            .unwrap_or_else(|error| panic!("Native PTY spawn failed: {error}"));
    }

    fn create_interactive_fixture(
        state: &DesktopTerminalState,
        fixture: &Fixture,
        id: &str,
        events: &Arc<Events>,
    ) {
        #[cfg(windows)]
        let sink: EventSink = {
            let state = state.clone();
            let id = id.to_string();
            let events = events.sink();
            let pending = Mutex::new(Vec::<u8>::new());
            Arc::new(move |event| {
                if let TerminalEvent::Output { data, .. } = &event {
                    let mut pending = pending.lock().unwrap();
                    pending.extend_from_slice(data);
                    let requests = pending
                        .windows(4)
                        .filter(|bytes| *bytes == b"\x1b[6n")
                        .count();
                    let tail = pending[pending.len().saturating_sub(3)..].to_vec();
                    *pending = tail;
                    drop(pending);
                    // ConPTY uses INHERIT_CURSOR. A headless PTY consumer must
                    // reply to its real VT query, as xterm does in production.
                    for _ in 0..requests {
                        write_terminal(&state, &id, b"\x1b[1;1R")?;
                    }
                }
                events(event)
            })
        };
        #[cfg(unix)]
        let sink = events.sink();
        create_fixture(state, fixture, id, sink);
        #[cfg(windows)]
        events.wait(|events| String::from_utf8_lossy(&Events::output(events)).contains("> "));
    }

    fn has_red_unicode_then_reset_and_cwd(output: &str, cwd: &str) -> bool {
        let mut chars = output.chars().peekable();
        let mut foreground = None;
        let mut red_unicode = false;
        let mut reset_after_unicode = false;
        let mut after_reset = String::new();
        while let Some(character) = chars.next() {
            if character == '\x1b' && chars.peek() == Some(&'[') {
                chars.next();
                let mut parameters = String::new();
                let mut final_byte = None;
                for character in chars.by_ref() {
                    if ('@'..='~').contains(&character) {
                        final_byte = Some(character);
                        break;
                    }
                    parameters.push(character);
                }
                // CR/LF and cursor movement/visibility do not change SGR color.
                if final_byte != Some('m') {
                    continue;
                }
                if !parameters
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || byte == b';')
                {
                    continue;
                }
                let parameters = parameters
                    .split(';')
                    .map(|value| {
                        if value.is_empty() {
                            Some(0)
                        } else {
                            value.parse::<u16>().ok()
                        }
                    })
                    .collect::<Option<Vec<_>>>();
                let Some(parameters) = parameters else {
                    continue;
                };
                let mut index = 0;
                while index < parameters.len() {
                    match parameters[index] {
                        0 | 39 => {
                            foreground = None;
                            if red_unicode {
                                reset_after_unicode = true;
                            }
                        }
                        color @ (30..=37 | 90..=97) => foreground = Some(color),
                        color @ (38 | 48 | 58) => {
                            if color == 38 {
                                foreground = Some(38);
                            }
                            // RGB/palette values are not subsequent SGR codes;
                            // e.g. 38;2;31;0;0 must not become foreground red 31.
                            index += match parameters.get(index + 1) {
                                Some(2) => 4,
                                Some(5) => 2,
                                _ => parameters.len(),
                            };
                        }
                        _ => {}
                    }
                    index += 1;
                }
                continue;
            }
            if character == '中' && foreground == Some(31) {
                red_unicode = true;
            }
            if reset_after_unicode {
                after_reset.push(character);
            }
        }
        red_unicode && reset_after_unicode && after_reset.contains(cwd)
    }

    #[test]
    fn native_pty_output_utf8_cwd_resize_input_and_exit_are_real() {
        assert!(has_red_unicode_then_reset_and_cwd(
            "\x1b[31m\r\n中\x1b[?25h\x1b[mfixture-cwd",
            "fixture-cwd"
        ));
        assert!(has_red_unicode_then_reset_and_cwd(
            "\x1b[31m中\x1b[39mfixture-cwd",
            "fixture-cwd"
        ));
        assert!(!has_red_unicode_then_reset_and_cwd(
            "\x1b[32m中\x1b[mfixture-cwd",
            "fixture-cwd"
        ));
        assert!(!has_red_unicode_then_reset_and_cwd(
            "\x1b[31m中fixture-cwd",
            "fixture-cwd"
        ));
        assert!(!has_red_unicode_then_reset_and_cwd(
            "\x1b[38;2;31;0;0m中\x1b[mfixture-cwd",
            "fixture-cwd"
        ));
        let fixture = Fixture::new();
        let terminals = Terminals::new();
        let events = Arc::new(Events::default());
        create_interactive_fixture(&terminals.0, &fixture, "io", &events);
        resize_terminal(&terminals.0, "io", 132, 37).unwrap();
        let master = terminals.0 .0.lock().unwrap().terminals["io"]
            .master
            .clone();
        let size = master.lock().unwrap().as_ref().unwrap().get_size().unwrap();
        assert_eq!((size.cols, size.rows), (132, 37));
        assert!(write_terminal(&terminals.0, "io", &vec![0; MAX_INPUT_BYTES + 1]).is_err());
        #[cfg(windows)]
        let command = "[Console]::Write(([char]27).ToString()+'[31m'+[char]0x4e2d+([char]27)+'[0m'); [Console]::WriteLine((Get-Location).Path); exit 7\r";
        #[cfg(unix)]
        let command = "printf '\\033[31m\\344\\270\\255\\033[0m'; pwd; exit 7\n";
        write_terminal(&terminals.0, "io", command.as_bytes()).unwrap();
        events.wait(|events| {
            events
                .iter()
                .any(|event| matches!(event, TerminalEvent::Exit { .. }))
        });
        let values = events.values.lock().unwrap().clone();
        let output = String::from_utf8_lossy(&Events::output(&values)).into_owned();
        assert!(
            has_red_unicode_then_reset_and_cwd(&output, &fixture.0.to_string_lossy()),
            "Actual ConPTY/PTY UTF-8/ANSI output: {output:?}"
        );
        assert!(
            output.contains(&fixture.0.to_string_lossy().to_string()),
            "The real shell did not use its synthetic cwd: {output:?}"
        );
        assert!(matches!(
            values.last(),
            Some(TerminalEvent::Exit { code: Some(7), .. })
        ));
        assert!(write_terminal(&terminals.0, "missing", b"echo ignored").is_err());
    }

    #[test]
    fn native_pty_close_all_reaps_then_reopens_and_shutdown_blocks_late_spawn() {
        let fixture = Fixture::new();
        let terminals = Terminals::new();
        let events = Arc::new(Events::default());
        create_fixture(&terminals.0, &fixture, "first", events.sink());
        create_fixture(&terminals.0, &fixture, "second", events.sink());
        let completions = terminals
            .0
             .0
            .lock()
            .unwrap()
            .terminals
            .values()
            .map(|terminal| terminal.completion.clone())
            .collect::<Vec<_>>();
        assert_eq!(close_registry(&terminals.0, false).unwrap(), 2);
        assert!(completions
            .iter()
            .all(|completion| *completion.finished.lock().unwrap()));
        assert!(terminals.0 .0.lock().unwrap().terminals.is_empty());
        create_fixture(&terminals.0, &fixture, "first", events.sink());
        assert_eq!(shutdown(&terminals.0), 1);
        let error = create_terminal_with_profile(
            &terminals.0,
            input(&fixture, "late"),
            fixture_profile(),
            events.sink(),
        )
        .err()
        .unwrap();
        assert!(error.contains("shutting down"));
        assert!(terminals.0 .0.lock().unwrap().terminals.is_empty());
    }

    #[test]
    fn native_pty_disconnected_event_channel_kills_and_reaps_shell() {
        let fixture = Fixture::new();
        let terminals = Terminals::new();
        let exit_attempts = Arc::new(AtomicU64::new(0));
        let attempts = exit_attempts.clone();
        let sink: EventSink = Arc::new(move |event| {
            if matches!(event, TerminalEvent::Exit { .. }) {
                attempts.fetch_add(1, Ordering::Release);
            }
            Err("The isolated renderer has disconnected.".into())
        });
        create_fixture(&terminals.0, &fixture, "disconnected", sink);
        // Request real output on quiet POSIX shells; Windows startup already
        // produces output. It may have exited before this write is attempted.
        let _ = write_terminal(
            &terminals.0,
            "disconnected",
            b"echo renderer-disconnected\r\n",
        );
        let deadline = Instant::now() + CLOSE_TIMEOUT;
        while Instant::now() < deadline {
            if exit_attempts.load(Ordering::Acquire) == 1
                && terminals.0 .0.lock().unwrap().terminals.is_empty()
            {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("The real child wait/output cleanup did not finish after channel disconnection");
    }

    #[test]
    fn native_pty_invalid_spawn_and_duplicate_admission_leave_no_extra_child() {
        let fixture = Fixture::new();
        let terminals = Terminals::new();
        let events = Arc::new(Events::default());
        let mut profile = fixture_profile();
        profile.program = fixture.0.join("missing-shell-executable");
        assert!(create_terminal_with_profile(
            &terminals.0,
            input(&fixture, "failed"),
            profile,
            events.sink()
        )
        .is_err());
        assert!(terminals.0 .0.lock().unwrap().terminals.is_empty());
        create_fixture(&terminals.0, &fixture, "unique", events.sink());
        let error = create_terminal_with_profile(
            &terminals.0,
            input(&fixture, "unique"),
            fixture_profile(),
            events.sink(),
        )
        .err()
        .unwrap();
        assert!(error.contains("already exists"));
        assert_eq!(terminals.0 .0.lock().unwrap().terminals.len(), 1);
        assert!(close_terminal(&terminals.0, "unique").unwrap());
        assert!(!close_terminal(&terminals.0, "unique").unwrap());
    }

    #[cfg(windows)]
    #[test]
    fn native_pty_windows_job_close_reaps_spawned_descendant() {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use windows_sys::Win32::{
            Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT},
            System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE},
        };
        let fixture = Fixture::new();
        let terminals = Terminals::new();
        let events = Arc::new(Events::default());
        create_interactive_fixture(&terminals.0, &fixture, "tree", &events);
        let pid_file = fixture.0.join("child.pid");
        let program = fixture_profile()
            .program
            .to_string_lossy()
            .replace('\'', "''");
        let path = pid_file.to_string_lossy().replace('\'', "''");
        let command = format!("$child = Start-Process -FilePath '{program}' -ArgumentList '-NoLogo','-NoProfile','-Command','Start-Sleep 120' -WindowStyle Hidden -PassThru; [IO.File]::WriteAllText('{path}', $child.Id.ToString()); [Console]::WriteLine(('ch'+'ild-owned'))\r");
        write_terminal(&terminals.0, "tree", command.as_bytes()).unwrap();
        events.wait(|events| {
            String::from_utf8_lossy(&Events::output(events)).contains("child-owned")
        });
        let pid: u32 = std::fs::read_to_string(pid_file).unwrap().parse().unwrap();
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        assert!(!handle.is_null(), "Synthetic child was not running");
        let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
        assert_eq!(
            unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) },
            WAIT_TIMEOUT
        );
        close_terminal(&terminals.0, "tree").unwrap();
        assert_eq!(
            unsafe { WaitForSingleObject(handle.as_raw_handle(), 1_000) },
            WAIT_OBJECT_0,
            "The owned Windows job left a live descendant"
        );
    }

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
