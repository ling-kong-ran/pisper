//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Port of `modes/interactive/components/status-indicator.ts` (123 lines,
//! sha256 `1deb7f7ce7d05e4ab6ea8f088f36aac6a64273dfda8c1d46483c98297777cd2d`).
//!
//! `StatusIndicator extends Loader`; composition replaces inheritance and the
//! per-kind constructors become [`StatusIndicatorKind`]-typed helpers. The
//! retry countdown uses the ported
//! [`super::countdown_timer::CountdownTimer`] (explicit-tick seam): upstream's
//! `this.setMessage(...)` inside the interval callback records the message in
//! a shared slot that the owner applies right after the same
//! [`RetryStatusIndicator::tick_countdown`] call — the observable sequence is
//! identical because the tick is owner-driven.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use crate::coding_agent::modes::interactive::components::countdown_timer::{
    CountdownCallbacks, CountdownTimer,
};
use crate::coding_agent::modes::interactive::components::loader::{
    Loader, LoaderIndicatorOptions, LoaderStyle,
};
use crate::coding_agent::modes::interactive::components::model_selector::key_text;
use crate::coding_agent::modes::interactive::theme::Theme;
use crate::tui::component::Component;
use crate::tui::utils::truncate_to_width;

/// Upstream `StatusIndicatorKind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusIndicatorKind {
    Working,
    Retry,
    Compaction,
    BranchSummary,
}

/// Upstream `CompactionStatusReason`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompactionStatusReason {
    Manual,
    Threshold,
    Overflow,
}

/// Upstream `StatusIndicator`.
pub struct StatusIndicator {
    kind: StatusIndicatorKind,
    loader: Loader,
}

impl StatusIndicator {
    /// Upstream base constructor.
    pub fn new(
        kind: StatusIndicatorKind,
        spinner_color_fn: LoaderStyle,
        message_color_fn: LoaderStyle,
        message: impl Into<String>,
        indicator: Option<LoaderIndicatorOptions>,
    ) -> Self {
        Self {
            kind,
            loader: Loader::new(spinner_color_fn, message_color_fn, message, indicator),
        }
    }

    pub fn kind(&self) -> StatusIndicatorKind {
        self.kind
    }

    /// Upstream `renderInBorder`.
    pub fn render_in_border(&mut self, width: usize) -> String {
        let line = self
            .loader
            .render(width + 2)
            .get(1)
            .cloned()
            .unwrap_or_default();
        let trimmed = match line.strip_prefix(' ') {
            Some(stripped) => stripped.trim_end().to_string(),
            None => line.trim_end().to_string(),
        };
        truncate_to_width(&trimmed, width, "", false)
    }

    /// Upstream `renderSpinnerInBorder`.
    pub fn render_spinner_in_border(&mut self, width: usize) -> String {
        truncate_to_width(&self.loader.get_rendered_indicator(), width, "", false)
    }

    /// Upstream `dispose` (= `stop`).
    pub fn dispose(&mut self) {
        self.loader.stop();
    }

    pub fn set_message(&mut self, message: impl Into<String>) {
        self.loader.set_message(message);
    }

    pub fn message(&self) -> &str {
        self.loader.message()
    }

    /// One spinner-frame tick (upstream `setInterval`; timer seam).
    pub fn advance_frame(&mut self) {
        self.loader.advance_frame();
    }
}

impl Component for StatusIndicator {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.loader.render(width)
    }

    fn invalidate(&mut self) {
        self.loader.invalidate();
    }

    fn handle_input(&mut self, _data: &str) {}
}

fn shared_style(
    color_fn: Option<&LoaderStyle>,
    theme: &Theme,
    fallback_color: &'static str,
) -> LoaderStyle {
    match color_fn {
        Some(fn_) => Arc::clone(fn_),
        None => {
            let theme = theme.clone();
            Arc::new(move |s: &str| theme.fg(fallback_color, s).expect("theme fg color"))
        }
    }
}

/// Upstream `WorkingStatusIndicator`.
pub struct WorkingStatusIndicator {
    indicator: StatusIndicator,
}

impl WorkingStatusIndicator {
    pub fn new(
        theme: &Theme,
        message: impl Into<String>,
        indicator: Option<LoaderIndicatorOptions>,
        color_fn: Option<LoaderStyle>,
    ) -> Self {
        // (color_fn is shared between the spinner and message slots via Arc)
        let spinner_color_fn = shared_style(color_fn.as_ref(), theme, "accent");
        let message_color_fn = shared_style(color_fn.as_ref(), theme, "muted");
        Self {
            indicator: StatusIndicator::new(
                StatusIndicatorKind::Working,
                spinner_color_fn,
                message_color_fn,
                message,
                indicator,
            ),
        }
    }

    pub fn indicator(&mut self) -> &mut StatusIndicator {
        &mut self.indicator
    }
}

impl Component for WorkingStatusIndicator {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.indicator.render(width)
    }
    fn invalidate(&mut self) {
        self.indicator.invalidate();
    }
    fn handle_input(&mut self, data: &str) {
        self.indicator.handle_input(data);
    }
}

/// Upstream `RetryStatusIndicator` (countdown rewires the message).
pub struct RetryStatusIndicator {
    indicator: StatusIndicator,
    countdown: Option<CountdownTimer>,
    pending_message: Rc<RefCell<Option<String>>>,
}

pub fn retry_message(attempt: usize, max_attempts: usize, seconds: u64) -> String {
    format!(
        "Retrying ({attempt}/{max_attempts}) in {seconds}s... ({} to cancel)",
        key_text("app.interrupt")
    )
}

impl RetryStatusIndicator {
    pub fn new(theme: &Theme, attempt: usize, max_attempts: usize, delay_ms: u64) -> Self {
        let indicator = StatusIndicator::new(
            StatusIndicatorKind::Retry,
            Arc::new({
                let theme = theme.clone();
                move |s: &str| theme.fg("warning", s).expect("warning")
            }),
            Arc::new({
                let theme = theme.clone();
                move |s: &str| theme.fg("muted", s).expect("muted")
            }),
            retry_message(attempt, max_attempts, delay_ms.div_ceil(1000)),
            None,
        );
        let pending_message = Rc::new(RefCell::new(None));
        let countdown = CountdownTimer::new(
            delay_ms,
            CountdownCallbacks {
                on_tick: {
                    let pending_message = Rc::clone(&pending_message);
                    Box::new(move |seconds| {
                        *pending_message.borrow_mut() =
                            Some(retry_message(attempt, max_attempts, seconds));
                    })
                },
                on_expire: Box::new(|| {}),
            },
        );
        Self {
            indicator,
            countdown: Some(countdown),
            pending_message,
        }
    }

    /// Drive one countdown second; the pending message (upstream
    /// `this.setMessage(...)`) is applied within the same call.
    pub fn tick_countdown(&mut self) {
        if let Some(countdown) = self.countdown.as_mut() {
            countdown.tick();
            if let Some(message) = self.pending_message.borrow_mut().take() {
                self.indicator.set_message(message);
            }
            if !countdown.is_active() {
                self.countdown = None;
            }
        }
    }

    pub fn countdown_active(&self) -> bool {
        self.countdown
            .as_ref()
            .is_some_and(CountdownTimer::is_active)
    }

    pub fn message(&self) -> &str {
        self.indicator.message()
    }

    pub fn indicator(&mut self) -> &mut StatusIndicator {
        &mut self.indicator
    }
}

impl Component for RetryStatusIndicator {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.indicator.render(width)
    }
    fn invalidate(&mut self) {
        self.indicator.invalidate();
    }
    fn handle_input(&mut self, data: &str) {
        self.indicator.handle_input(data);
    }
}

/// Upstream `CompactionStatusIndicator` label builder.
pub fn compaction_status_label(reason: CompactionStatusReason) -> String {
    let cancel_hint = format!("({} to cancel)", key_text("app.interrupt"));
    match reason {
        CompactionStatusReason::Manual => format!("Compacting context... {cancel_hint}"),
        CompactionStatusReason::Overflow => {
            format!("Context overflow detected, Auto-compacting... {cancel_hint}")
        }
        CompactionStatusReason::Threshold => format!("Auto-compacting... {cancel_hint}"),
    }
}

/// Upstream `BranchSummaryStatusIndicator` label.
pub fn branch_summary_label() -> String {
    format!(
        "Summarizing branch... ({} to cancel)",
        key_text("app.interrupt")
    )
}

/// Upstream `IdleStatus`.
#[derive(Default)]
pub struct IdleStatus;

impl Component for IdleStatus {
    fn render(&mut self, width: usize) -> Vec<String> {
        let empty_line = " ".repeat(width);
        vec![empty_line.clone(), empty_line]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding_agent::modes::interactive::theme::{load_builtin_theme, ColorMode};
    use std::sync::Arc;

    fn dark() -> Arc<Theme> {
        Arc::new(load_builtin_theme("dark", Some(ColorMode::Truecolor)).expect("dark"))
    }

    /// Oracle: tests/fixtures/interactive_r19_oracle scenario `status_indicator` —
    /// labels, kinds and the border-rendered spinner bytes.
    #[test]
    fn status_indicator_matches_oracle() {
        let theme = dark();
        let mut working = WorkingStatusIndicator::new(&theme, "Running...", None, None);
        assert_eq!(working.indicator().kind(), StatusIndicatorKind::Working);
        assert_eq!(
            working.indicator().render_in_border(30),
            "\x1b[38;2;167;152;215m⠋\x1b[39m \x1b[38;2;157;165;169mRunning...\x1b[39m"
        );
        assert_eq!(
            working.indicator().render_spinner_in_border(30),
            "\x1b[38;2;167;152;215m⠋\x1b[39m"
        );
        // three frame ticks later the render shows ⠸ (oracle workingRender)
        for _ in 0..3 {
            working.indicator().advance_frame();
        }
        let render = working.indicator().render(30);
        assert_eq!(render[0], "");
        assert!(render[1].starts_with(
            " \x1b[38;2;167;152;215m⠸\x1b[39m \x1b[38;2;157;165;169mRunning...\x1b[39m"
        ));
        assert_eq!(crate::tui::utils::visible_width(&render[1]), 30);

        assert_eq!(
            retry_message(2, 5, 3),
            "Retrying (2/5) in 3s... (escape to cancel)"
        );
        assert_eq!(
            compaction_status_label(CompactionStatusReason::Manual),
            "Compacting context... (escape to cancel)"
        );
        assert_eq!(
            compaction_status_label(CompactionStatusReason::Overflow),
            "Context overflow detected, Auto-compacting... (escape to cancel)"
        );
        assert_eq!(
            compaction_status_label(CompactionStatusReason::Threshold),
            "Auto-compacting... (escape to cancel)"
        );
        assert_eq!(
            branch_summary_label(),
            "Summarizing branch... (escape to cancel)"
        );

        let mut idle = IdleStatus;
        assert_eq!(idle.render(10), vec!["          ", "          "]);
    }

    /// Retry countdown rewires the message each second and disposes on expiry
    /// (oracle: initial `Retrying (2/5) in 3s...`, after 3 ticks `0s`).
    #[test]
    fn retry_countdown_updates_message() {
        let theme = dark();
        let mut retry = RetryStatusIndicator::new(&theme, 2, 5, 2500);
        assert!(retry.countdown_active());
        assert_eq!(
            retry.message(),
            "Retrying (2/5) in 3s... (escape to cancel)"
        );
        retry.tick_countdown();
        assert_eq!(
            retry.message(),
            "Retrying (2/5) in 2s... (escape to cancel)"
        );
        retry.tick_countdown();
        retry.tick_countdown();
        assert_eq!(
            retry.message(),
            "Retrying (2/5) in 0s... (escape to cancel)"
        );
        assert!(
            !retry.countdown_active(),
            "2500ms → 3 ticks expire the countdown"
        );
    }
}
