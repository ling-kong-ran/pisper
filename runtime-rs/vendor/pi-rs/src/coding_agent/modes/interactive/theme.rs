//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Port of upstream `coding-agent/src/modes/interactive/theme/theme.ts`
//! (1159 lines at v0.99.1 HEAD `2bbfcca43`, sha256
//! `082e9706529d468cfd3315f1d4c865a60ecd463e9fdd207b0b43607998a58ddc`) — the
//! theme core routed through the tui color pipeline.
//!
//! Oracle: `tests/fixtures/theme_delta_oracle/` runs the verbatim HEAD
//! `theme.ts` (plus `system-theme.ts`) under node — the `@earendil-works/pi-tui`
//! imports resolve to the SHA-pinned tui sources in that directory — and
//! captures `theme_delta_oracle.json`; the tests in `interactive_tests.rs`
//! compare this module's output against it byte for byte. The private
//! hex/256 quantizer of the r17 baseline (hexToRgb/rgbTo256/hexTo256/
//! ansi256ToHex) is upstream-deleted and gone here; color encoding, mixing,
//! and parsing route through [`crate::tui::colors`].
//!
//! Presentation seams (see `interactive/mod.rs`): the fs watcher, the
//! `globalThis` singleton (replaced by the [`theme_store`] mutex), chalk's
//! env-dependent color level, `getTerminalColorMode`'s capability probe
//! (explicit modes; the default is the port's 256-color conservative pick),
//! `process.env` COLORFGBG fallbacks (callers pass the value), and the
//! typebox-backed validator (the no-validator default is the ported
//! [`parse_theme_json_content`] path).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::Value;

use super::system_theme::{
    generate_system_theme_colors, terminal_appearance, SystemThemeInput, TokenColor,
};
use super::theme_json::ThemeJson;
use crate::coding_agent::utils::text::strip_bom;
use crate::tui::colors::{
    background_ansi, color_to_hex, color_to_oklch, foreground_ansi, indexed_color, mix_colors,
    parse_color, rgb_color, style_text_with_ansi, Color, ColorMixSpace, TerminalColorMode,
    TextAttributes,
};
use crate::tui::terminal_colors::{RgbColor, TerminalColors};

/// Upstream `ColorMode` became the tui `TerminalColorMode` in the delta.
pub type ColorMode = TerminalColorMode;

/// Upstream re-exports `SYSTEM_THEME_NAME` from `system-theme.ts`.
pub use super::system_theme::SYSTEM_THEME_NAME;

/// Upstream `ThemeAppearance` (the background a theme is designed for).
pub type ThemeAppearance = &'static str;

/// Upstream `ThemeColorValue`: a hex string (or OKLCH/OKHSL literal or var ref
/// / empty), or a 256-color index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColorValue {
    Str(String),
    Index(u8),
}

impl ColorValue {
    /// The resolved form of a color value after var expansion.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            ColorValue::Str(s) => Some(s),
            ColorValue::Index(_) => None,
        }
    }
}

/// A resolved color: hex string, empty string, or 256 index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedColor {
    Str(String),
    Index(u8),
}

impl ResolvedColor {
    /// Upstream `addToken`'s `parseColor(value)`: a 256 index parses through
    /// the numeric form, a string through [`parse_color`].
    fn to_color(&self) -> Result<Color, String> {
        match self {
            ResolvedColor::Index(index) => indexed_color(f64::from(*index)),
            ResolvedColor::Str(value) => parse_color(value),
        }
    }
}

/// Upstream `TerminalTheme`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalTheme {
    Light,
    Dark,
}

impl TerminalTheme {
    pub fn as_str(self) -> &'static str {
        match self {
            TerminalTheme::Light => "light",
            TerminalTheme::Dark => "dark",
        }
    }

    /// Upstream `"dark" | "light"` values (from the tui color-scheme report).
    pub fn from_scheme(scheme: crate::tui::terminal_colors::TerminalColorScheme) -> Self {
        match scheme {
            crate::tui::terminal_colors::TerminalColorScheme::Dark => TerminalTheme::Dark,
            crate::tui::terminal_colors::TerminalColorScheme::Light => TerminalTheme::Light,
        }
    }
}

// ============================================================================
// Color Utilities (verbatim upstream semantics)
// ============================================================================

/// Whether a string is an OKLCH/OKHSL literal (`/^ok(lch|hsl)\(/i`), which
/// `resolveVarRefs` treats as a color, not a variable reference.
fn is_ok_color_literal(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.starts_with("oklch(") || lower.starts_with("okhsl(")
}

/// Upstream `resolveVarRefs`.
pub fn resolve_var_refs(
    value: &ColorValue,
    vars: &BTreeMap<String, ColorValue>,
) -> Result<ResolvedColor, String> {
    resolve_var_refs_inner(value, vars, &mut Vec::new())
}

fn resolve_var_refs_inner(
    value: &ColorValue,
    vars: &BTreeMap<String, ColorValue>,
    visited: &mut Vec<String>,
) -> Result<ResolvedColor, String> {
    match value {
        ColorValue::Index(i) => Ok(ResolvedColor::Index(*i)),
        ColorValue::Str(s) if s.is_empty() || s.starts_with('#') || is_ok_color_literal(s) => {
            Ok(ResolvedColor::Str(s.clone()))
        }
        ColorValue::Str(s) => {
            if visited.contains(s) {
                return Err(format!("Circular variable reference detected: {s}"));
            }
            let Some(next) = vars.get(s) else {
                return Err(format!("Variable reference not found: {s}"));
            };
            visited.push(s.clone());
            resolve_var_refs_inner(next, vars, visited)
        }
    }
}

/// Upstream `resolveThemeColors`.
pub fn resolve_theme_colors(
    colors: &BTreeMap<String, ColorValue>,
    vars: &BTreeMap<String, ColorValue>,
) -> Result<BTreeMap<String, ResolvedColor>, String> {
    let mut resolved = BTreeMap::new();
    for (key, value) in colors {
        resolved.insert(key.clone(), resolve_var_refs(value, vars)?);
    }
    Ok(resolved)
}

fn fallback(colors: &mut BTreeMap<String, ColorValue>, key: &str, fallback_key: &str) {
    if !colors.contains_key(key) {
        let value = colors
            .get(fallback_key)
            .cloned()
            .expect("fallback key present");
        colors.insert(key.to_string(), value);
    }
}

/// Upstream `withThemeColorFallbacks`.
pub fn with_theme_color_fallbacks(
    colors: &BTreeMap<String, ColorValue>,
) -> BTreeMap<String, ColorValue> {
    let mut computed = colors.clone();
    fallback(&mut computed, "scrollbarTrack", "muted");
    fallback(&mut computed, "scrollbarThumb", "text");
    fallback(&mut computed, "thinkingMax", "thinkingXhigh");
    fallback(&mut computed, "searchMatchBg", "selectedBg");
    fallback(&mut computed, "searchMatchText", "text");
    computed
}

// ============================================================================
// Appearance & Terminal Default Colors
// ============================================================================

/// The terminal's reported colors plus its last light/dark report. Replaced
/// (never mutated) on update, so themes can cache resolved colors by
/// [`Arc`] identity — the module state of upstream `theme.ts`.
struct TerminalRuntimeState {
    colors: Arc<TerminalColors>,
    /// While the terminal color query is in flight, the system theme renders
    /// in grayscale.
    pending: bool,
    /// The terminal's last light/dark report (mode 2031). Only used while it
    /// has not reported a background.
    scheme: Option<TerminalTheme>,
}

fn terminal_state() -> &'static Mutex<TerminalRuntimeState> {
    static STATE: OnceLock<Mutex<TerminalRuntimeState>> = OnceLock::new();
    STATE.get_or_init(|| {
        Mutex::new(TerminalRuntimeState {
            colors: Arc::new(TerminalColors::default()),
            pending: false,
            scheme: None,
        })
    })
}

/// The current terminal colors snapshot (identity changes on every
/// [`set_terminal_colors`] call, matching upstream's replace-not-mutate).
pub fn get_terminal_colors() -> Arc<TerminalColors> {
    Arc::clone(&terminal_state().lock().expect("terminal state").colors)
}

/// Upstream `setTerminalColors`: record the terminal's reported colors. Themes
/// use the default colors for tokens set to "" (terminal default); the system
/// theme is generated from all of them. Ends the pending state.
pub fn set_terminal_colors(colors: TerminalColors) {
    let mut state = terminal_state().lock().expect("terminal state");
    state.colors = Arc::new(colors);
    state.pending = false;
}

/// Upstream `setTerminalColorScheme`: record the terminal's light/dark report,
/// the fallback for terminals that do not report their background.
pub fn set_terminal_color_scheme(scheme: Option<TerminalTheme>) {
    terminal_state().lock().expect("terminal state").scheme = scheme;
}

/// Upstream `markTerminalColorsPending`: render the system theme in grayscale
/// until [`set_terminal_colors`] reports the terminal's colors.
pub fn mark_terminal_colors_pending() {
    terminal_state().lock().expect("terminal state").pending = true;
}

/// Assumed terminal default colors when the terminal does not report them
/// (upstream `GUESSED_DEFAULT_COLORS`, per appearance: fg, bg).
fn guessed_default_colors(appearance: ThemeAppearance) -> (Color, Color) {
    let parse = |hex| parse_color(hex).expect("guessed default colors are valid hex");
    match appearance {
        "dark" => (parse("#e5e5e7"), parse("#000000")),
        "light" => (parse("#000000"), parse("#ffffff")),
        other => unreachable!("unknown appearance: {other}"),
    }
}

/// Upstream `averageLightness`. Palette colors 0-15 follow the user's terminal
/// palette, so they say nothing about the theme.
fn average_lightness(colors: &[Color]) -> Option<f64> {
    let fixed: Vec<Color> = colors
        .iter()
        .copied()
        .filter(|color| !matches!(color, Color::Indexed(indexed) if indexed.index < 16))
        .collect();
    if fixed.is_empty() {
        return None;
    }
    let sum: f64 = fixed.iter().map(|color| color_to_oklch(*color).l).sum();
    Some(sum / fixed.len() as f64)
}

/// Upstream `detectAppearance`: detect the background a theme is designed for
/// from the lightness of its own colors.
fn detect_appearance(foregrounds: &[Color], backgrounds: &[Color]) -> Option<ThemeAppearance> {
    let fg = average_lightness(foregrounds);
    let bg = average_lightness(backgrounds);
    if let (Some(fg), Some(bg)) = (fg, bg) {
        return Some(if bg < fg { "dark" } else { "light" });
    }
    if let Some(bg) = bg {
        return Some(if bg < 0.5 { "dark" } else { "light" });
    }
    if let Some(fg) = fg {
        return Some(if fg > 0.5 { "dark" } else { "light" });
    }
    None
}

// ============================================================================
// Theme (upstream Theme class)
// ============================================================================

/// Upstream `ThemeStyle`'s fg/bg: a token name or a raw color.
#[derive(Clone, Debug, PartialEq)]
pub enum StyleColor {
    Token(String),
    Color(Color),
}

/// Upstream `ThemeStyle` (`TextAttributes` plus fg/bg).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ThemeStyle {
    pub fg: Option<StyleColor>,
    pub bg: Option<StyleColor>,
    pub attributes: TextAttributes,
}

/// Upstream constructor `options`: name, source path, declared appearance, and
/// the foreground tokens to render faint (SGR 2).
#[derive(Clone, Debug, Default)]
pub struct ThemeOptions {
    pub name: Option<String>,
    pub source_path: Option<String>,
    pub appearance: Option<ThemeAppearance>,
    pub dim: Vec<String>,
}

/// The resolved-colors cache entry, keyed by terminal-colors identity.
#[derive(Clone)]
struct ResolvedColors {
    terminal: Arc<TerminalColors>,
    colors: BTreeMap<String, Color>,
}

/// Upstream `SourceInfo`-style provenance is carried as plain strings.
pub struct Theme {
    pub name: Option<String>,
    pub source_path: Option<String>,
    mode: TerminalColorMode,
    // Precomputed escape sequences keep fg()/bg() on the render hot path to a
    // lookup and concat.
    fg_ansi: BTreeMap<String, String>,
    bg_ansi: BTreeMap<String, String>,
    // Tokens set to "" have no color of their own; `colors()` fills them from
    // the terminal defaults.
    concrete_colors: BTreeMap<String, Color>,
    default_foreground_tokens: Vec<String>,
    default_background_tokens: Vec<String>,
    // Foreground tokens rendered faint (SGR 2) on top of their color.
    dim_tokens: BTreeSet<String>,
    own_appearance: Option<ThemeAppearance>,
    resolved_colors: Mutex<Option<ResolvedColors>>,
}

impl Clone for Theme {
    /// A clone shares nothing mutable: the resolved-colors cache deep-copies
    /// (the entry itself is an immutable snapshot).
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            source_path: self.source_path.clone(),
            mode: self.mode,
            fg_ansi: self.fg_ansi.clone(),
            bg_ansi: self.bg_ansi.clone(),
            concrete_colors: self.concrete_colors.clone(),
            default_foreground_tokens: self.default_foreground_tokens.clone(),
            default_background_tokens: self.default_background_tokens.clone(),
            dim_tokens: self.dim_tokens.clone(),
            own_appearance: self.own_appearance,
            resolved_colors: Mutex::new(
                self.resolved_colors
                    .lock()
                    .expect("theme colors cache")
                    .clone(),
            ),
        }
    }
}

/// Upstream `addToken`: the escape sequence for the token's own slot. ""
/// tokens register as terminal-default; everything else parses to a
/// [`Color`] and renders through the tui ANSI encoders.
#[allow(clippy::too_many_arguments)]
fn add_token(
    token: &str,
    value: &ResolvedColor,
    is_background: bool,
    mode: TerminalColorMode,
    concrete_colors: &mut BTreeMap<String, Color>,
    concrete_foregrounds: &mut Vec<Color>,
    concrete_backgrounds: &mut Vec<Color>,
    default_foreground_tokens: &mut Vec<String>,
    default_background_tokens: &mut Vec<String>,
) -> Result<String, String> {
    match value {
        ResolvedColor::Str(s) if s.is_empty() => {
            if is_background {
                default_background_tokens.push(token.to_string());
            } else {
                default_foreground_tokens.push(token.to_string());
            }
            Ok(if is_background {
                "\x1b[49m"
            } else {
                "\x1b[39m"
            }
            .to_string())
        }
        value => {
            let color = value.to_color()?;
            concrete_colors.insert(token.to_string(), color);
            if is_background {
                concrete_backgrounds.push(color);
            } else {
                concrete_foregrounds.push(color);
            }
            Ok(if is_background {
                background_ansi(color, mode)
            } else {
                foreground_ansi(color, mode)
            })
        }
    }
}

impl Theme {
    /// Upstream `new Theme(fgColors, bgColors, mode, options)` with the same
    /// optional-key fallbacks applied in the constructor.
    pub fn new(
        fg_colors: &BTreeMap<String, ResolvedColor>,
        bg_colors: &BTreeMap<String, ResolvedColor>,
        mode: TerminalColorMode,
        options: ThemeOptions,
    ) -> Result<Self, String> {
        let mut foregrounds = fg_colors.clone();
        // Constructor fallbacks (verbatim upstream expressions).
        fallback_resolved(&mut foregrounds, "scrollbarTrack", "muted")?;
        fallback_resolved(&mut foregrounds, "scrollbarThumb", "text")?;
        fallback_resolved(&mut foregrounds, "thinkingMax", "thinkingXhigh")?;
        fallback_resolved(&mut foregrounds, "searchMatchText", "text")?;
        let mut backgrounds = bg_colors.clone();
        fallback_resolved(&mut backgrounds, "searchMatchBg", "selectedBg")?;

        let mut fg_ansi = BTreeMap::new();
        let mut bg_ansi = BTreeMap::new();
        let mut concrete_colors = BTreeMap::new();
        let mut concrete_foregrounds = Vec::new();
        let mut concrete_backgrounds = Vec::new();
        let mut default_foreground_tokens = Vec::new();
        let mut default_background_tokens = Vec::new();
        for (token, value) in &foregrounds {
            let ansi = add_token(
                token,
                value,
                false,
                mode,
                &mut concrete_colors,
                &mut concrete_foregrounds,
                &mut concrete_backgrounds,
                &mut default_foreground_tokens,
                &mut default_background_tokens,
            )?;
            fg_ansi.insert(token.clone(), ansi);
        }
        for (token, value) in &backgrounds {
            let ansi = add_token(
                token,
                value,
                true,
                mode,
                &mut concrete_colors,
                &mut concrete_foregrounds,
                &mut concrete_backgrounds,
                &mut default_foreground_tokens,
                &mut default_background_tokens,
            )?;
            bg_ansi.insert(token.clone(), ansi);
        }

        Ok(Self {
            name: options.name,
            source_path: options.source_path,
            mode,
            fg_ansi,
            bg_ansi,
            concrete_colors,
            default_foreground_tokens,
            default_background_tokens,
            dim_tokens: options.dim.into_iter().collect(),
            own_appearance: options
                .appearance
                .or_else(|| detect_appearance(&concrete_foregrounds, &concrete_backgrounds)),
            resolved_colors: Mutex::new(None),
        })
    }

    /// Upstream `get appearance`: the background the theme is designed for —
    /// declared in the theme JSON, detected from its colors, or, for themes
    /// without usable colors, the terminal's appearance.
    pub fn appearance(&self) -> ThemeAppearance {
        self.own_appearance
            .unwrap_or_else(|| get_terminal_theme().as_str())
    }

    /// Upstream `get colors`: concrete colors for all tokens. Tokens set to ""
    /// (terminal default) use the terminal's reported default colors, or a
    /// guess based on [`Theme::appearance`] when the terminal did not report
    /// them. Faint tokens are approximated by mixing their color toward the
    /// background. Cached by terminal-colors identity.
    pub fn colors(&self) -> BTreeMap<String, Color> {
        let terminal = get_terminal_colors();
        let mut cache = self.resolved_colors.lock().expect("theme colors cache");
        let fresh = !matches!(&*cache, Some(cached) if Arc::ptr_eq(&cached.terminal, &terminal));
        if fresh {
            let guess = guessed_default_colors(self.appearance());
            let to_color = |rgb: Option<RgbColor>, fallback: Color| {
                rgb.map(|rgb| {
                    rgb_color(f64::from(rgb.r), f64::from(rgb.g), f64::from(rgb.b))
                        .expect("reported terminal channels are in range")
                })
                .unwrap_or(fallback)
            };
            let foreground = to_color(terminal.foreground, guess.0);
            let background = to_color(terminal.background, guess.1);
            let mut colors = self.concrete_colors.clone();
            for token in &self.default_foreground_tokens {
                colors.insert(token.clone(), foreground);
            }
            for token in &self.default_background_tokens {
                colors.insert(token.clone(), background);
            }
            for token in &self.dim_tokens {
                if let Some(color) = colors.get(token).copied() {
                    colors.insert(
                        token.clone(),
                        mix_colors(color, background, 0.4, ColorMixSpace::Oklch)
                            .expect("0.4 is a valid mix amount"),
                    );
                }
            }
            *cache = Some(ResolvedColors {
                terminal: Arc::clone(&terminal),
                colors,
            });
        }
        cache
            .as_ref()
            .expect("cache was just populated")
            .colors
            .clone()
    }

    /// Upstream `Theme.style`.
    pub fn style(&self, text: &str, options: &ThemeStyle) -> Result<String, String> {
        let mut attributes = options.attributes;
        if let Some(StyleColor::Token(fg)) = &options.fg {
            if self.dim_tokens.contains(fg) {
                attributes.dim = Some(true);
            }
        }
        let fg_ansi = match &options.fg {
            None => None,
            Some(StyleColor::Token(token)) => Some(self.token_ansi(&self.fg_ansi, token)?),
            Some(StyleColor::Color(color)) => Some(foreground_ansi(*color, self.mode)),
        };
        let bg_ansi = match &options.bg {
            None => None,
            Some(StyleColor::Token(token)) => Some(self.token_ansi(&self.bg_ansi, token)?),
            Some(StyleColor::Color(color)) => Some(background_ansi(*color, self.mode)),
        };
        Ok(style_text_with_ansi(text, fg_ansi, bg_ansi, &attributes))
    }

    /// Upstream `Theme.fg`: colorize with a foreground reset only. Faint
    /// tokens wrap the text in SGR 2 / SGR 22.
    pub fn fg(&self, color: &str, text: &str) -> Result<String, String> {
        let ansi = self.token_ansi(&self.fg_ansi, color)?;
        if self.dim_tokens.contains(color) {
            return Ok(format!("{ansi}\x1b[2m{text}\x1b[22;39m"));
        }
        Ok(format!("{ansi}{text}\x1b[39m"))
    }

    /// Upstream `Theme.bg`: colorize with a background reset only.
    pub fn bg(&self, color: &str, text: &str) -> Result<String, String> {
        let ansi = self.token_ansi(&self.bg_ansi, color)?;
        Ok(format!("{ansi}{text}\x1b[49m"))
    }

    fn token_ansi(&self, ansi: &BTreeMap<String, String>, token: &str) -> Result<String, String> {
        ansi.get(token)
            .cloned()
            .ok_or_else(|| format!("Unknown theme color: {token}"))
    }

    /// Upstream `Theme.bold`. Chalk seam: the enabled-level ANSI codes are
    /// fixed here instead of depending on process env (D2 in mod.rs).
    pub fn bold(&self, text: &str) -> String {
        format!("\x1b[1m{text}\x1b[22m")
    }

    /// Upstream `Theme.italic`.
    pub fn italic(&self, text: &str) -> String {
        format!("\x1b[3m{text}\x1b[23m")
    }

    /// Upstream `Theme.underline`.
    pub fn underline(&self, text: &str) -> String {
        format!("\x1b[4m{text}\x1b[24m")
    }

    /// Upstream `Theme.inverse`.
    pub fn inverse(&self, text: &str) -> String {
        format!("\x1b[7m{text}\x1b[27m")
    }

    /// Upstream `Theme.strikethrough`.
    pub fn strikethrough(&self, text: &str) -> String {
        format!("\x1b[9m{text}\x1b[29m")
    }

    /// Upstream `getFgAnsi`. Faint tokens include SGR 2, which `\x1b[22m` closes.
    pub fn get_fg_ansi(&self, color: &str) -> Result<String, String> {
        let ansi = self.token_ansi(&self.fg_ansi, color)?;
        Ok(if self.dim_tokens.contains(color) {
            format!("{ansi}\x1b[2m")
        } else {
            ansi
        })
    }

    /// Upstream `getBgAnsi`.
    pub fn get_bg_ansi(&self, color: &str) -> Result<String, String> {
        self.token_ansi(&self.bg_ansi, color)
    }

    /// Upstream `getColorMode`.
    pub fn get_color_mode(&self) -> TerminalColorMode {
        self.mode
    }

    /// Upstream `getThinkingBorderColor`, applied to a string immediately.
    pub fn get_thinking_border_color(
        &self,
        level: ThinkingLevel,
        text: &str,
    ) -> Result<String, String> {
        let key = match level {
            ThinkingLevel::Off => "thinkingOff",
            ThinkingLevel::Minimal => "thinkingMinimal",
            ThinkingLevel::Low => "thinkingLow",
            ThinkingLevel::Medium => "thinkingMedium",
            ThinkingLevel::High => "thinkingHigh",
            ThinkingLevel::Xhigh => "thinkingXhigh",
            ThinkingLevel::Max => "thinkingMax",
        };
        self.fg(key, text)
    }

    /// Upstream `getBashModeBorderColor`, applied to a string immediately.
    pub fn get_bash_mode_border_color(&self, text: &str) -> Result<String, String> {
        self.fg("bashMode", text)
    }
}

/// Constructor-level `?? ` fallbacks over already-resolved colors.
fn fallback_resolved(
    colors: &mut BTreeMap<String, ResolvedColor>,
    key: &str,
    fallback_key: &str,
) -> Result<(), String> {
    if !colors.contains_key(key) {
        let value = colors
            .get(fallback_key)
            .cloned()
            .ok_or_else(|| format!("missing fallback color: {fallback_key}"))?;
        colors.insert(key.to_string(), value);
    }
    Ok(())
}

/// Upstream `ThinkingLevel` (from `@earendil-works/pi-agent-core`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingLevel {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

// ============================================================================
// Theme Loading (deterministic core over explicit JSON documents)
// ============================================================================

/// The byte-identical built-in theme documents (`theme/dark.json`,
/// `theme/light.json`, sha256-matched to upstream dark.json / light.json).
pub const BUILTIN_THEME_DARK_JSON: &str = include_str!("theme/dark.json");
pub const BUILTIN_THEME_LIGHT_JSON: &str = include_str!("theme/light.json");

/// Upstream `getBuiltinThemes` (the `{ dark, light }` literal, dedeterminized
/// from fs+config indirection to byte-identical embedded documents).
pub fn builtin_theme_names() -> [&'static str; 2] {
    ["dark", "light"]
}

pub fn get_builtin_theme_json(name: &str) -> Option<ThemeJson> {
    let content = match name {
        "dark" => BUILTIN_THEME_DARK_JSON,
        "light" => BUILTIN_THEME_LIGHT_JSON,
        _ => return None,
    };
    Some(ThemeJson::parse(strip_bom(content)).expect("built-in theme JSON parses"))
}

/// Upstream `loadThemeJson` restricted to the built-in registry; custom-dir
/// and registered-theme lookups are the caller's registry concern.
pub fn load_builtin_theme(name: &str, mode: Option<TerminalColorMode>) -> Result<Theme, String> {
    let theme_json =
        get_builtin_theme_json(name).ok_or_else(|| format!("Theme not found: {name}"))?;
    create_theme(&theme_json, mode, None)
}

/// Keys routed to the background table by upstream `splitThemeColors`.
const BACKGROUND_TOKENS: [&str; 7] = [
    "selectedBg",
    "searchMatchBg",
    "userMessageBg",
    "customMessageBg",
    "toolPendingBg",
    "toolSuccessBg",
    "toolErrorBg",
];

/// Upstream `splitThemeColors`.
fn split_theme_colors(
    colors: &BTreeMap<String, ResolvedColor>,
) -> (
    BTreeMap<String, ResolvedColor>,
    BTreeMap<String, ResolvedColor>,
) {
    let mut fg_colors = BTreeMap::new();
    let mut bg_colors = BTreeMap::new();
    for (key, value) in colors {
        if BACKGROUND_TOKENS.contains(&key.as_str()) {
            bg_colors.insert(key.clone(), value.clone());
        } else {
            fg_colors.insert(key.clone(), value.clone());
        }
    }
    (fg_colors, bg_colors)
}

/// Upstream `createTheme` (`mode` defaults to the terminal capability probe;
/// the probe is presentation, so callers pass an explicit mode or [`None`] to
/// get the port's 256-color conservative default via [`ColorMode`]).
pub fn create_theme(
    theme_json: &ThemeJson,
    mode: Option<TerminalColorMode>,
    source_path: Option<String>,
) -> Result<Theme, String> {
    let color_mode = mode.unwrap_or(TerminalColorMode::Color256);
    let resolved_colors = resolve_theme_colors(
        &with_theme_color_fallbacks(&theme_json.colors),
        theme_json.vars.as_ref().unwrap_or(&BTreeMap::new()),
    )?;
    let (fg_colors, bg_colors) = split_theme_colors(&resolved_colors);
    Theme::new(
        &fg_colors,
        &bg_colors,
        color_mode,
        ThemeOptions {
            name: Some(theme_json.name.clone()),
            source_path,
            appearance: theme_json.appearance(),
            dim: Vec::new(),
        },
    )
}

/// Upstream `createSystemTheme`: generate the system theme from the terminal's
/// reported colors (grayscale while they are pending).
pub fn create_system_theme(mode: Option<TerminalColorMode>) -> Result<Theme, String> {
    let (colors, pending) = {
        let state = terminal_state().lock().expect("terminal state");
        (Arc::clone(&state.colors), state.pending)
    };
    let generated = generate_system_theme_colors(&SystemThemeInput {
        foreground: colors.foreground,
        background: colors.background,
        palette: colors.palette.clone(),
        saturation: Some(if pending { 0.0 } else { 1.0 }),
        appearance_hint: Some(get_terminal_theme().as_str()),
    });
    let mut resolved = BTreeMap::new();
    for (token, color) in &generated.colors {
        let resolved_color = match color {
            TokenColor::TerminalDefault => ResolvedColor::Str(String::new()),
            TokenColor::Hex(hex) => ResolvedColor::Str(hex.clone()),
            TokenColor::Index(index) => ResolvedColor::Index(*index),
        };
        resolved.insert((*token).to_string(), resolved_color);
    }
    let (fg_colors, bg_colors) = split_theme_colors(&resolved);
    Theme::new(
        &fg_colors,
        &bg_colors,
        mode.unwrap_or(TerminalColorMode::Color256),
        ThemeOptions {
            name: Some(SYSTEM_THEME_NAME.to_string()),
            source_path: None,
            appearance: generated.appearance,
            dim: generated
                .dim
                .iter()
                .map(|token| token.to_string())
                .collect(),
        },
    )
}

/// Upstream `loadThemeFromPath` core: parse + build from file content.
pub fn load_theme_from_content(
    content: &str,
    mode: Option<TerminalColorMode>,
    source_path: String,
) -> Result<Theme, String> {
    let theme_json = parse_theme_json_content(&source_path, content)?;
    create_theme(&theme_json, mode, Some(source_path))
}

/// Upstream `parseThemeJsonContent` over the no-validator default
/// (`parseThemeJson` accepts any object with a `colors` map; the typebox
/// validator is installed by interactive-mode and lives in
/// [`super::theme_json::validate_theme_json`]).
pub fn parse_theme_json_content(label: &str, content: &str) -> Result<ThemeJson, String> {
    let json: Value = serde_json::from_str(strip_bom(content))
        .map_err(|error| format!("Failed to parse theme {label}: {error}"))?;
    parse_theme_json_value(label, &json)
}

/// Upstream `parseThemeJson` (no validator installed).
fn parse_theme_json_value(label: &str, json: &Value) -> Result<ThemeJson, String> {
    if !json.is_object() || json.get("colors").is_none() {
        return Err(format!(
            "Invalid theme \"{label}\": expected an object with a \"colors\" map."
        ));
    }
    // (The no-validator path casts the document; the shape parse still
    // requires a `name`, which every real theme document carries.)
    ThemeJson::parse(&serde_json::to_string(json).expect("round-trip"))
}

// ============================================================================
// Global Theme Instance (upstream globalThis store; the watcher is D1)
// ============================================================================

/// Upstream `globalThis` theme sharing: the installed theme, the current theme
/// name, and the registered-themes map.
struct ThemeStoreState {
    theme: Option<Arc<Theme>>,
    current_theme_name: Option<String>,
    registered: BTreeMap<String, Arc<Theme>>,
}

fn theme_store() -> &'static Mutex<ThemeStoreState> {
    static STORE: OnceLock<Mutex<ThemeStoreState>> = OnceLock::new();
    STORE.get_or_init(|| {
        Mutex::new(ThemeStoreState {
            theme: None,
            current_theme_name: None,
            registered: BTreeMap::new(),
        })
    })
}

/// Upstream `theme` proxy (throws before `initTheme()`; the port panics with
/// the same message).
pub fn theme() -> Arc<Theme> {
    theme_store()
        .lock()
        .expect("theme store")
        .theme
        .clone()
        .expect("Theme not initialized. Call initTheme() first.")
}

/// Upstream `currentThemeName` read.
pub fn get_current_theme_name() -> Option<String> {
    theme_store()
        .lock()
        .expect("theme store")
        .current_theme_name
        .clone()
}

fn set_global_theme(t: Theme) {
    theme_store().lock().expect("theme store").theme = Some(Arc::new(t));
}

/// Upstream `assertThemeNameIsValid`.
pub fn assert_theme_name_is_valid(name: &str) -> Result<(), String> {
    if name.contains('/') {
        return Err(format!(
            "Invalid theme name \"{name}\": theme names cannot contain \"/\" because it is reserved for automatic light/dark theme settings."
        ));
    }
    Ok(())
}

/// Upstream `setRegisteredThemes`.
pub fn set_registered_themes(themes: Vec<Theme>) {
    let mut store = theme_store().lock().expect("theme store");
    store.registered.clear();
    for theme in themes {
        if let Some(name) = theme.name.clone() {
            assert_theme_name_is_valid(&name).expect("registered theme name is valid");
            store.registered.insert(name, Arc::new(theme));
        }
    }
}

/// Upstream `loadTheme`: the system theme name is reserved (it takes
/// precedence over custom themes of the same name), then registered themes,
/// then the built-in documents.
pub fn load_theme(name: &str) -> Result<Theme, String> {
    if name == SYSTEM_THEME_NAME {
        return create_system_theme(None);
    }
    if let Some(registered) = theme_store()
        .lock()
        .expect("theme store")
        .registered
        .get(name)
    {
        return Ok((**registered).clone());
    }
    let theme_json =
        get_builtin_theme_json(name).ok_or_else(|| format!("Theme not found: {name}"))?;
    create_theme(&theme_json, None, None)
}

/// Upstream `initTheme` (`enableWatcher` is the D1 presentation seam and is
/// accepted for signature parity only).
pub fn init_theme(theme_name: Option<&str>) {
    let name = theme_name.unwrap_or(SYSTEM_THEME_NAME).to_string();
    theme_store()
        .lock()
        .expect("theme store")
        .current_theme_name = Some(name.clone());
    match load_theme(&name) {
        Ok(theme) => set_global_theme(theme),
        Err(_) => {
            // Theme is invalid - fall back to the system theme silently
            theme_store()
                .lock()
                .expect("theme store")
                .current_theme_name = Some(SYSTEM_THEME_NAME.to_string());
            set_global_theme(create_system_theme(None).expect("system theme generates"));
        }
    }
}

/// Upstream `setTheme` result shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeResult {
    pub success: bool,
    pub error: Option<String>,
}

/// Upstream `setTheme` (`enableWatcher` is the D1 presentation seam).
pub fn set_theme(name: &str) -> ThemeResult {
    theme_store()
        .lock()
        .expect("theme store")
        .current_theme_name = Some(name.to_string());
    match load_theme(name) {
        Ok(theme) => {
            set_global_theme(theme);
            ThemeResult {
                success: true,
                error: None,
            }
        }
        Err(error) => {
            // Theme is invalid - fall back to the system theme
            theme_store()
                .lock()
                .expect("theme store")
                .current_theme_name = Some(SYSTEM_THEME_NAME.to_string());
            set_global_theme(create_system_theme(None).expect("system theme generates"));
            ThemeResult {
                success: false,
                error: Some(error),
            }
        }
    }
}

/// Upstream `setThemeInstance`.
pub fn set_theme_instance(theme_instance: Theme) {
    set_global_theme(theme_instance);
    theme_store()
        .lock()
        .expect("theme store")
        .current_theme_name = Some("<in-memory>".to_string());
}

/// Upstream `getThemeByName`.
pub fn get_theme_by_name(name: &str) -> Option<Theme> {
    load_theme(name).ok()
}

/// Upstream `getAvailableThemesWithPaths` ordering: the system theme first
/// (it is the default and adapts to every terminal), then everything else in
/// name order. `localeCompare` is ported as byte-wise ordering (identical for
/// the ASCII names the oracle covers; D4 in mod.rs).
pub fn sort_theme_infos(mut infos: Vec<(String, Option<String>)>) -> Vec<(String, Option<String>)> {
    let mut seen = std::collections::BTreeSet::new();
    infos.retain(|(name, _)| seen.insert(name.clone()));
    infos.sort_by(|a, b| a.0.cmp(&b.0));
    if let Some(position) = infos.iter().position(|(name, _)| name == SYSTEM_THEME_NAME) {
        let entry = infos.remove(position);
        infos.insert(0, entry);
    }
    infos
}

// ============================================================================
// Theme Setting Helpers (verbatim upstream semantics)
// ============================================================================

/// Upstream `parseAutoThemeSetting` -> `(lightTheme, darkTheme)`.
pub fn parse_auto_theme_setting(theme_setting: Option<&str>) -> Option<(String, String)> {
    let theme_setting = theme_setting?;
    let slash_index = theme_setting.find('/')?;
    if theme_setting[slash_index + 1..].contains('/') {
        return None;
    }

    let light_theme = theme_setting[..slash_index].trim();
    let dark_theme = theme_setting[slash_index + 1..].trim();
    if light_theme.is_empty() || dark_theme.is_empty() {
        return None;
    }
    Some((light_theme.to_string(), dark_theme.to_string()))
}

/// Upstream `resolveThemeSetting`.
pub fn resolve_theme_setting(
    theme_setting: Option<&str>,
    terminal_theme: TerminalTheme,
) -> Option<String> {
    if let Some((light, dark)) = parse_auto_theme_setting(theme_setting) {
        return Some(if terminal_theme == TerminalTheme::Light {
            light
        } else {
            dark
        });
    }
    let theme_setting = theme_setting?;
    if theme_setting.contains('/') {
        return None;
    }
    Some(theme_setting.to_string())
}

// ============================================================================
// Terminal Theme Detection (verbatim upstream semantics)
// ============================================================================

/// Upstream `detectColorFgBgTheme`: dark or light from the `COLORFGBG`
/// environment variable some terminals set, or [`None`] without a usable
/// background index. The value is `fg;bg` or `fg;xpm;bg` (rxvt), where a field
/// is an ANSI color index or `default`. The index refers to the terminal's own
/// palette, whose colors are unknown here, so it is classified by index like
/// Vim does: 0-6 and 8 (bright black, e.g. Solarized Dark's background) are
/// dark, 7 and 9-15 are light.
pub fn detect_color_fg_bg_theme(colorfgbg: Option<&str>) -> Option<TerminalTheme> {
    let colorfgbg = colorfgbg?;
    let bg = colorfgbg.split(';').next_back()?.trim();
    if !is_one_or_two_digits(bg) {
        return None;
    }
    let index: usize = bg.parse().expect("digits parse");
    if index > 15 {
        return None;
    }
    Some(if index <= 6 || index == 8 {
        TerminalTheme::Dark
    } else {
        TerminalTheme::Light
    })
}

/// `/^\d{1,2}$/`.
fn is_one_or_two_digits(value: &str) -> bool {
    !value.is_empty() && value.len() <= 2 && value.bytes().all(|byte| byte.is_ascii_digit())
}

/// Upstream `detectTerminalTheme`: whether the terminal is dark or light. The
/// background it renders decides, classified the same way the system theme
/// does. Without a reported background: the terminal's light/dark report, then
/// COLORFGBG, then dark. (`env` is passed explicitly; the process-env read is
/// the caller's seam.)
pub fn detect_terminal_theme(
    colors: &TerminalColors,
    reported_scheme: Option<TerminalTheme>,
    colorfgbg: Option<&str>,
) -> TerminalTheme {
    if let Some(background) = colors.background {
        return theme_from_str(terminal_appearance(background, colors.foreground));
    }
    reported_scheme
        .or_else(|| detect_color_fg_bg_theme(colorfgbg))
        .unwrap_or(TerminalTheme::Dark)
}

fn theme_from_str(value: ThemeAppearance) -> TerminalTheme {
    if value == "light" {
        TerminalTheme::Light
    } else {
        TerminalTheme::Dark
    }
}

/// The `COLORFGBG` fallback of [`get_terminal_theme`] reads the process env
/// (upstream `process.env`); explicit-argument callers use
/// [`detect_terminal_theme`].
fn process_colorfgbg() -> Option<String> {
    std::env::var("COLORFGBG").ok()
}

/// Upstream `getTerminalTheme`: whether the terminal is dark or light, from
/// everything it reported so far. See [`detect_terminal_theme`].
pub fn get_terminal_theme() -> TerminalTheme {
    let state = terminal_state().lock().expect("terminal state");
    detect_terminal_theme(&state.colors, state.scheme, process_colorfgbg().as_deref())
}

/// Test isolation: fresh module state (upstream tests get a fresh module
/// instance per file; the Rust mutex state is reset explicitly). Also clears
/// `COLORFGBG` so the process-env fallback matches the capture harness.
#[cfg(test)]
pub fn reset_state_for_tests() {
    {
        let mut state = terminal_state().lock().expect("terminal state");
        state.colors = Arc::new(TerminalColors::default());
        state.pending = false;
        state.scheme = None;
    }
    {
        let mut store = theme_store().lock().expect("theme store");
        store.theme = None;
        store.current_theme_name = None;
        store.registered.clear();
    }
    std::env::remove_var("COLORFGBG");
}

// ============================================================================
// HTML Export Helpers (verbatim upstream semantics over the theme store)
// ============================================================================

/// Upstream `getResolvedThemeColors`: resolved theme colors as CSS-compatible
/// hex strings. The by-name fs lookup runs through [`load_theme`].
pub fn get_resolved_theme_colors(
    theme_name: Option<&str>,
) -> Result<BTreeMap<String, String>, String> {
    let name = match theme_name {
        Some(name) => name.to_string(),
        None => get_current_theme_name().unwrap_or_else(|| SYSTEM_THEME_NAME.to_string()),
    };
    let theme = load_theme(&name)?;
    Ok(theme
        .colors()
        .into_iter()
        .map(|(token, color)| (token, color_to_hex(color)))
        .collect())
}

/// Upstream `isLightTheme`: whether a theme is a "light" theme (for CSS that
/// needs light/dark variants).
pub fn is_light_theme(theme_name: Option<&str>) -> Result<bool, String> {
    let name = match theme_name {
        Some(name) => name.to_string(),
        None => get_current_theme_name().unwrap_or_else(|| SYSTEM_THEME_NAME.to_string()),
    };
    Ok(load_theme(&name)?.appearance() == "light")
}

/// Upstream `getThemeExportColors` return shape (`None` = not specified).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThemeExportColors {
    pub page_bg: Option<String>,
    pub card_bg: Option<String>,
    pub info_bg: Option<String>,
}

/// Upstream `getThemeExportColors`: explicit export colors from theme JSON, if
/// specified. Returns `None` for each color that isn't explicitly set. Load
/// errors yield the empty result (upstream `catch { return {} }`).
pub fn get_theme_export_colors(theme_name: Option<&str>) -> ThemeExportColors {
    let empty = ThemeExportColors::default();
    let name = match theme_name {
        Some(name) => name.to_string(),
        None => get_current_theme_name().unwrap_or_else(|| SYSTEM_THEME_NAME.to_string()),
    };
    if name == SYSTEM_THEME_NAME {
        return empty;
    }
    let Ok(theme_json) = load_theme_json(&name) else {
        return empty;
    };
    let Some(export) = &theme_json.export else {
        return empty;
    };

    let empty_vars = BTreeMap::new();
    let vars = theme_json.vars.as_ref().unwrap_or(&empty_vars);
    // Export colors end up in CSS, which understands hex and oklch() values
    // directly but not okhsl().
    let resolve = |value: Option<&ColorValue>| -> Option<String> {
        let value = value?;
        match resolve_var_refs(value, vars).ok()? {
            ResolvedColor::Index(index) => Some(color_to_hex(
                indexed_color(f64::from(index)).expect("u8 index"),
            )),
            ResolvedColor::Str(s) if s.is_empty() => None,
            ResolvedColor::Str(s) if s.to_ascii_lowercase().starts_with("okhsl(") => {
                parse_color(&s).ok().map(color_to_hex)
            }
            ResolvedColor::Str(s) => Some(s),
        }
    };

    ThemeExportColors {
        page_bg: resolve(export.page_bg.as_ref()),
        card_bg: resolve(export.card_bg.as_ref()),
        info_bg: resolve(export.info_bg.as_ref()),
    }
}

/// Upstream `loadThemeJson` over the built-in registry (registered-theme and
/// custom-dir fs lookups are the D1 caller concern).
fn load_theme_json(name: &str) -> Result<ThemeJson, String> {
    get_builtin_theme_json(name).ok_or_else(|| format!("Theme not found: {name}"))
}

// ============================================================================
// Language Table (verbatim upstream semantics)
// ============================================================================

/// Upstream `getLanguageFromPath`. `filePath.split(".").pop()` takes the
/// substring after the last "." of the WHOLE path (so "dir.d/file" looks up
/// "d/file" and misses), lowercased; the empty extension also misses.
pub fn get_language_from_path(file_path: &str) -> Option<&'static str> {
    let ext = match file_path.rfind('.') {
        Some(index) => file_path[index + 1..].to_ascii_lowercase(),
        None => file_path.to_ascii_lowercase(),
    };
    EXT_TO_LANG
        .iter()
        .find(|(candidate, _)| *candidate == ext)
        .map(|(_, language)| *language)
}

/// Upstream `extToLang`.
const EXT_TO_LANG: &[(&str, &str)] = &[
    ("ts", "typescript"),
    ("tsx", "typescript"),
    ("js", "javascript"),
    ("jsx", "javascript"),
    ("mjs", "javascript"),
    ("cjs", "javascript"),
    ("py", "python"),
    ("rb", "ruby"),
    ("rs", "rust"),
    ("go", "go"),
    ("java", "java"),
    ("kt", "kotlin"),
    ("swift", "swift"),
    ("c", "c"),
    ("h", "c"),
    ("cpp", "cpp"),
    ("cc", "cpp"),
    ("cxx", "cpp"),
    ("hpp", "cpp"),
    ("cs", "csharp"),
    ("php", "php"),
    ("sh", "bash"),
    ("bash", "bash"),
    ("zsh", "bash"),
    ("fish", "fish"),
    ("ps1", "powershell"),
    ("sql", "sql"),
    ("html", "html"),
    ("htm", "html"),
    ("css", "css"),
    ("scss", "scss"),
    ("sass", "sass"),
    ("less", "less"),
    ("json", "json"),
    ("yaml", "yaml"),
    ("yml", "yaml"),
    ("toml", "toml"),
    ("xml", "xml"),
    ("md", "markdown"),
    ("markdown", "markdown"),
    ("dockerfile", "dockerfile"),
    ("makefile", "makefile"),
    ("cmake", "cmake"),
    ("lua", "lua"),
    ("perl", "perl"),
    ("r", "r"),
    ("scala", "scala"),
    ("clj", "clojure"),
    ("ex", "elixir"),
    ("exs", "elixir"),
    ("erl", "erlang"),
    ("hs", "haskell"),
    ("ml", "ocaml"),
    ("vim", "vim"),
    ("graphql", "graphql"),
    ("proto", "protobuf"),
    ("tf", "hcl"),
    ("hcl", "hcl"),
];

pub mod theme_controller;
