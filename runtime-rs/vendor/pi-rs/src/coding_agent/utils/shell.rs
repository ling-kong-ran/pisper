//! Detached-child lifetime helpers from upstream `utils/shell.ts`.
//!
//! Shell discovery/environment/text helpers are separate migration slices.
//! Callers must track detached children at spawn and untrack them on exit;
//! this registry never scans or kills unrelated processes.

use std::sync::{Mutex, OnceLock};

#[derive(Default)]
pub struct DetachedChildren(Mutex<Vec<u32>>);
impl DetachedChildren {
    pub fn track(&self, pid: u32) {
        let mut pids = self.0.lock().expect("tracked children");
        if !pids.contains(&pid) {
            pids.push(pid);
        }
    }
    pub fn untrack(&self, pid: u32) {
        self.0
            .lock()
            .expect("tracked children")
            .retain(|item| *item != pid);
    }
    /// Snapshot without holding a mutex across process creation. A child added
    /// concurrently remains tracked for the next cleanup rather than being lost.
    pub fn kill_with(&self, mut kill: impl FnMut(u32)) {
        let pids = std::mem::take(&mut *self.0.lock().expect("tracked children"));
        for pid in pids {
            kill(pid);
        }
    }
}
fn tracked() -> &'static DetachedChildren {
    static TRACKED: OnceLock<DetachedChildren> = OnceLock::new();
    TRACKED.get_or_init(DetachedChildren::default)
}
pub fn track_detached_child_pid(pid: u32) {
    tracked().track(pid);
}
pub fn untrack_detached_child_pid(pid: u32) {
    tracked().untrack(pid);
}
pub fn kill_tracked_detached_children() {
    tracked().kill_with(kill_process_tree);
}

/// Best effort: group first on Unix, trusted System32 taskkill on Windows.
/// Invalid PIDs never mean "all processes" or "the calling process group".
pub fn kill_process_tree(pid: u32) {
    if pid == 0 || pid > i32::MAX as u32 {
        return;
    }
    #[cfg(unix)]
    {
        use rustix::process::{kill_process, kill_process_group, Pid, Signal};
        if let Some(pid) = Pid::from_raw(pid as i32) {
            if kill_process_group(pid, Signal::KILL).is_err() {
                let _ = kill_process(pid, Signal::KILL);
            }
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        let executable = std::path::PathBuf::from(root)
            .join("System32")
            .join("taskkill.exe");
        // CREATE_NO_WINDOW: cleanup must not flash a console window.
        let _ = std::process::Command::new(executable)
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(0x0800_0000)
            .spawn();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detached_children_are_set_ordered_and_clear_after_cleanup() {
        let children = DetachedChildren::default();
        children.track(31);
        children.track(12);
        children.track(31);
        children.untrack(31);
        children.track(31);
        let mut killed = vec![];
        children.kill_with(|pid| killed.push(pid));
        assert_eq!(killed, [12, 31]);
        children.kill_with(|_| panic!("child was killed twice"));
    }
    #[test]
    fn cleanup_does_not_drop_a_concurrently_registered_child() {
        let children = DetachedChildren::default();
        children.track(7);
        children.kill_with(|pid| {
            assert_eq!(pid, 7);
            children.track(9);
        });
        let mut remaining = vec![];
        children.kill_with(|pid| remaining.push(pid));
        assert_eq!(remaining, [9]);
    }
    #[test]
    fn invalid_process_ids_never_target_the_parent_group() {
        kill_process_tree(0);
        kill_process_tree(u32::MAX);
    }
}
