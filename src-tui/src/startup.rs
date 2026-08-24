//! TUI 进入备用屏前的启动进度指示器。
//!
//! Runtime 冷启动和首次会话扫描都可能持续数秒；在普通终端中用后台线程刷新
//! 单行 spinner，重定向输出时保持静默，避免污染脚本消费的 stdout/stderr。

use std::{
    io::{self, IsTerminal, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

const FRAMES: [&str; 4] = ["|", "/", "-", "\\"];
const FRAME_INTERVAL: Duration = Duration::from_millis(100);

struct IndicatorState {
    message: String,
    changed_at: Instant,
}

/// 启动阶段单行动画；Drop 会停止线程并清理终端行。
pub struct StartupIndicator {
    state: Arc<Mutex<IndicatorState>>,
    stopped: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl StartupIndicator {
    /// 仅在 stderr 连接真实终端时启动动画，重定向环境下保持无输出。
    pub fn start(message: impl Into<String>) -> Self {
        let state = Arc::new(Mutex::new(IndicatorState {
            message: message.into(),
            changed_at: Instant::now(),
        }));
        let stopped = Arc::new(AtomicBool::new(false));
        let worker = io::stderr().is_terminal().then(|| {
            let state = Arc::clone(&state);
            let stopped = Arc::clone(&stopped);
            thread::spawn(move || run_indicator(state, stopped))
        });
        Self {
            state,
            stopped,
            worker,
        }
    }

    /// 切换启动阶段并重置该阶段的耗时显示。
    pub fn set_message(&self, message: impl Into<String>) {
        if let Ok(mut state) = self.state.lock() {
            state.message = message.into();
            state.changed_at = Instant::now();
        }
    }

    /// 主动结束动画；Drop 中仍会兜底执行相同清理。
    pub fn finish(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for StartupIndicator {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run_indicator(state: Arc<Mutex<IndicatorState>>, stopped: Arc<AtomicBool>) {
    let mut frame = 0usize;
    let mut rendered_width = 0usize;
    while !stopped.load(Ordering::Acquire) {
        let line = state
            .lock()
            .map(|state| indicator_line(FRAMES[frame % FRAMES.len()], &state))
            .unwrap_or_default();
        rendered_width = rendered_width.max(line.len());
        render_line(&line, rendered_width);
        frame += 1;
        thread::sleep(FRAME_INTERVAL);
    }
    clear_line(rendered_width);
}

fn indicator_line(frame: &str, state: &IndicatorState) -> String {
    let elapsed = state.changed_at.elapsed().as_secs();
    if elapsed == 0 {
        format!("{frame} {}", state.message)
    } else {
        format!("{frame} {} ({elapsed}s)", state.message)
    }
}

fn render_line(line: &str, width: usize) {
    let mut stderr = io::stderr();
    let _ = write!(stderr, "\r{line:<width$}", width = width);
    let _ = stderr.flush();
}

fn clear_line(width: usize) {
    if width == 0 {
        return;
    }
    let mut stderr = io::stderr();
    let _ = write!(stderr, "\r{:width$}\r", "", width = width);
    let _ = stderr.flush();
}

#[cfg(test)]
mod tests {
    use super::{indicator_line, IndicatorState};
    use std::time::Instant;

    /// 验证启动行同时包含动画帧和当前阶段，便于用户区分等待位置。
    #[test]
    fn indicator_line_contains_frame_and_stage() {
        let state = IndicatorState {
            message: "Loading conversations".to_owned(),
            changed_at: Instant::now(),
        };
        let line = indicator_line("|", &state);
        assert!(line.starts_with("| Loading conversations"));
    }
}
