//! Port of `modes/interactive/components/themed-text.ts` (32 lines): text
//! whose content applies theme colors.
//!
//! Plain `Text` keeps the colors its string was built with, so a theme change,
//! or the system theme receiving the terminal's colors, would leave it stale.
//! This rebuilds the string from `build` after every invalidation, which the
//! UI performs on theme changes.
//!
//! `build` must return the same content each time, apart from colors.
//! Snapshot changing data before creating the component, or call
//! `invalidate()` after changing state that `build` reads.

use std::sync::Arc;

use crate::tui::component::Component;
use crate::tui::components::text::{BgFn, Text};

/// The `build` closure (upstream `() => string`).
pub type ThemedTextBuilder = Arc<dyn Fn() -> String + Send + Sync>;

/// Upstream `ThemedText`.
pub struct ThemedText {
    inner: Text,
    build: ThemedTextBuilder,
    stale: bool,
}

impl ThemedText {
    /// Upstream constructor (`build`, `paddingX = 1`, `paddingY = 1`).
    pub fn new(build: ThemedTextBuilder) -> Self {
        Self::with_options(build, 1, 1)
    }

    /// Upstream constructor with explicit padding.
    pub fn with_options(build: ThemedTextBuilder, padding_x: usize, padding_y: usize) -> Self {
        Self {
            inner: Text::with_options("", padding_x, padding_y, None),
            build,
            stale: true,
        }
    }

    /// `Text.setCustomBgFn` passthrough.
    pub fn set_custom_bg_fn(&mut self, custom_bg_fn: Option<BgFn>) {
        self.inner.set_custom_bg_fn(custom_bg_fn);
    }
}

impl Component for ThemedText {
    fn render(&mut self, width: usize) -> Vec<String> {
        if self.stale {
            self.stale = false;
            let text = (self.build)();
            self.inner.set_text(&text);
        }
        self.inner.render(width)
    }

    fn invalidate(&mut self) {
        self.stale = true;
        self.inner.invalidate();
    }
}
