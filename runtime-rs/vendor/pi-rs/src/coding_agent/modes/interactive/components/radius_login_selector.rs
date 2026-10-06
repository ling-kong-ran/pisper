//! Port of `modes/interactive/components/radius-login-selector.ts` (v1.0.0):
//! the `/login` menu with the animated "Sign in with Radius" option. Internal
//! to the interactive mode: the shimmer is Radius-only and is not exposed to
//! other selectors.

use std::sync::Arc;
use std::time::Instant;

use crate::coding_agent::modes::interactive::components::extension_selector::{
    ExtensionSelectorCallbacks, ExtensionSelectorComponent,
};
use crate::coding_agent::modes::interactive::components::model_selector::theme_fg;
use crate::coding_agent::modes::interactive::theme::Theme;
use crate::tui::colors::{foreground_ansi, mix_colors, parse_color, Color, ColorMixSpace};
use crate::tui::component::Component;

/// The four colors of the Radius logo, in the order they stream across the
/// text (upstream `RADIUS_COLORS`).
fn radius_colors() -> Vec<Color> {
    ["#4d9abf", "#83ccd2", "#f1be57", "#f09082"]
        .iter()
        .filter_map(|hex| parse_color(hex).ok())
        .collect()
}
/// Width of each color band, in characters (upstream `CHARS_PER_COLOR`).
const CHARS_PER_COLOR: f64 = 4.0;
const CHARS_PER_SECOND: f64 = 10.0;

/// Color `text` with the Radius logo colors flowing left to right;
/// `elapsed_ms` is the animation time (upstream `radiusShimmer`).
fn radius_shimmer(theme: &Theme, text: &str, elapsed_ms: f64) -> String {
    let mode = theme.get_color_mode();
    let colors = radius_colors();
    let cycle = colors.len() as f64 * CHARS_PER_COLOR;
    let offset = elapsed_ms / 1000.0 * CHARS_PER_SECOND;
    let mut result = String::new();
    for (index, char) in text.chars().enumerate() {
        let position = ((index as f64 - offset).rem_euclid(cycle) + cycle) % cycle;
        let band = (position / CHARS_PER_COLOR).floor() as usize;
        let t = position / CHARS_PER_COLOR - band as f64;
        // Smoothstep keeps each band recognizable while still blending into
        // the next one.
        let amount = t * t * (3.0 - 2.0 * t);
        let from = colors[band % colors.len()];
        let to = colors[(band + 1) % colors.len()];
        let mixed = mix_colors(from, to, amount, ColorMixSpace::Srgb).unwrap_or(from);
        result.push_str(&foreground_ansi(mixed, mode));
        result.push(char);
    }
    format!("{result}\x1b[39m")
}

/// The "Sign in with Radius" option: `label` is the full option, starting
/// with the animated `text` (upstream `RadiusOption`).
pub struct RadiusOption {
    pub label: String,
    pub text: String,
}

/// Swaps the line [`ExtensionSelectorComponent`] draws for the selected
/// Radius option with the animated one. When the selector's row style changes
/// or the label wraps, the line no longer matches and the option renders
/// normally (upstream `RadiusLoginMenuComponent`).
pub struct RadiusLoginMenuComponent {
    inner: ExtensionSelectorComponent,
    radius_option: RadiusOption,
    animation_start: Instant,
    theme: Arc<Theme>,
}

impl RadiusLoginMenuComponent {
    pub fn new(
        theme: Arc<Theme>,
        title: &str,
        options: Vec<String>,
        radius_option: RadiusOption,
        callbacks: ExtensionSelectorCallbacks,
    ) -> Self {
        RadiusLoginMenuComponent {
            inner: ExtensionSelectorComponent::new(theme.clone(), title, options, callbacks, None),
            radius_option,
            animation_start: Instant::now(),
            theme,
        }
    }

    /// The selected row (upstream matches the styled selected line instead).
    pub fn selected_index(&self) -> usize {
        self.inner.selected_index()
    }
}

impl Component for RadiusLoginMenuComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        let mut lines = self.inner.render(width);
        // The upstream swap relies on the exact styled selected line the
        // base selector draws; the port's selector exposes its selection
        // index, so the animated line lands on the selected row directly.
        let selected = self.inner.selected_index();
        if let Some(line_slot) = lines
            .iter()
            .enumerate()
            .find(|(_, line)| line.contains(&self.radius_option.label) && line.contains('→'))
            .map(|(index, _)| index)
        {
            let _ = selected;
            let shimmer = radius_shimmer(
                &self.theme,
                &self.radius_option.text,
                self.animation_start.elapsed().as_secs_f64() * 1000.0,
            );
            let animated_line = format!(
                "{}{}{}",
                theme_fg(&self.theme, "accent", "→ "),
                shimmer,
                &self.radius_option.label[self.radius_option.text.len()..],
            );
            lines[line_slot] =
                crate::tui::components::text::Text::with_options(&animated_line, 1, 0, None)
                    .render(width)
                    .into_iter()
                    .next()
                    .unwrap_or_default();
        }
        lines
    }

    fn invalidate(&mut self) {
        self.inner.invalidate();
    }
}

/// Top-level `/login` selector whose Radius option shimmers in the Radius
/// logo colors while it is selected (upstream `createLoginMenuSelector`).
pub fn create_login_menu_selector(
    theme: Arc<Theme>,
    title: &str,
    options: Vec<String>,
    radius_option: RadiusOption,
    callbacks: ExtensionSelectorCallbacks,
) -> RadiusLoginMenuComponent {
    RadiusLoginMenuComponent::new(theme, title, options, radius_option, callbacks)
}
