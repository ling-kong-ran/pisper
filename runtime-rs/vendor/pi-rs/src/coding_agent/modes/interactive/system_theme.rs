//! Port of upstream
//! `coding-agent/src/modes/interactive/theme/system-theme.ts` — the `system`
//! theme: pi's colors derived from the terminal's own theme.
//!
//! Every token belongs to a color family (its hue) and has contrast rules: it
//! must reach a contrast level on the background and on the panels it is
//! drawn on. Hue and saturation come from the terminal's palette color for
//! the family's ANSI slot, or from the family's own hue when the terminal
//! reports no palette. Lightness comes from the rules alone. Colors are
//! built in OKHSL, whose saturation is relative to the sRGB gamut, and fade
//! toward gray near black and white.
//!
//! The generation runs on the unrounded float sRGB pipeline
//! ([`crate::tui::oklab::okhsl_to_rgb_f64`]): upstream `okhslColor` keeps
//! 0-255 floats and only `hexOf` rounds, so the solver's lightness binary
//! searches must not see quantized colors.
//!
//! Oracle: `tests/fixtures/coding_agent_theme_delta_oracle/oracle/` runs the
//! verbatim upstream module under node (`capture.mjs`, 42 generation
//! scenarios + appearance/luminance/contrast grids); the tests here replay
//! it.

use std::collections::HashMap;

use crate::tui::colors::{color_to_oklch, color_to_rgb, okhsl_color, oklch_color};
use crate::tui::oklab::{oklab_to_okhsl_lightness, rgb_to_okhsl, rgb_to_oklab};
use crate::tui::terminal_colors::RgbColor;

pub const SYSTEM_THEME_NAME: &str = "system";

/// An sRGB color on upstream's quantized pipeline: `linearSrgbToRgb`
/// `Math.round`s channels to integers (oklab.ts), so every color the solver
/// sees or produces is an integer RGB triple.
pub type Rgb = RgbColor;

/// Upstream `ThemeAppearance`.
pub type ThemeAppearance = &'static str;

// ============================================================================
// Recipe: color families and their tokens
// ============================================================================

/// A family's OKHSL hue and saturation: `max` at mid lightness, falling
/// toward `min` at black and white.
#[derive(Clone, Copy)]
struct Family {
    hue: f64,
    sat_min: f64,
    sat_max: f64,
    /// ANSI palette slot the family takes its hue and saturation from.
    slot: u8,
}

const fn family(hue: f64, min: f64, max: f64, slot: u8) -> Family {
    Family {
        hue,
        sat_min: min,
        sat_max: max,
        slot,
    }
}

/// Family table in upstream literal order (`Object.keys(FAMILIES)`), keyed by
/// name through [`family_named`].
const FAMILIES: &[(&str, Family)] = &[
    ("neutral", family(231.49, 0.02, 0.08, 8)),
    ("blue", family(231.49, 0.1, 0.68, 4)),
    ("green", family(158.68, 0.1, 0.76, 2)),
    ("red", family(20.0, 0.1, 0.92, 1)),
    ("yellow", family(82.36, 0.5, 1.0, 3)),
    ("orange", family(52.0, 0.12, 0.85, 3)),
    ("violet", family(295.0, 0.2, 0.6, 5)),
    ("calamine", family(202.43, 0.1, 0.74, 6)),
    ("thinkingSlate", family(231.49, 0.08, 0.2, 4)),
    ("thinkingBlue", family(231.49, 0.2, 0.45, 4)),
    ("thinkingPeriwinkle", family(263.25, 0.3, 0.6, 6)),
    ("thinkingViolet", family(295.0, 0.4, 0.75, 5)),
    ("thinkingMagenta", family(337.5, 0.5, 0.85, 13)),
    ("thinkingRed", family(20.0, 0.95, 1.0, 1)),
];

fn family_named(name: &str) -> &'static Family {
    FAMILIES
        .iter()
        .find(|(key, _)| *key == name)
        .map(|(_, family)| family)
        .unwrap_or_else(|| panic!("unknown family {name}"))
}

/// `TOKEN_FAMILIES`: every theme token, in upstream literal order (the
/// result map's insertion order), mapped to its family.
const TOKEN_FAMILIES: &[(&str, &str)] = &[
    ("selectedBg", "blue"),
    ("searchMatchBg", "orange"),
    ("userMessageBg", "blue"),
    ("customMessageBg", "violet"),
    ("toolPendingBg", "neutral"),
    ("toolSuccessBg", "green"),
    ("toolErrorBg", "red"),
    ("text", "neutral"),
    ("userMessageText", "neutral"),
    ("customMessageText", "neutral"),
    ("toolTitle", "neutral"),
    ("syntaxOperator", "neutral"),
    ("syntaxPunctuation", "neutral"),
    ("muted", "neutral"),
    ("dim", "neutral"),
    ("thinkingText", "neutral"),
    ("toolOutput", "neutral"),
    ("mdLinkUrl", "neutral"),
    ("mdQuote", "neutral"),
    ("mdQuoteBorder", "neutral"),
    ("mdHr", "neutral"),
    ("mdCodeBlockBorder", "neutral"),
    ("toolDiffContext", "neutral"),
    ("syntaxComment", "neutral"),
    ("scrollbarTrack", "neutral"),
    ("scrollbarThumb", "neutral"),
    ("searchMatchText", "neutral"),
    ("borderMuted", "neutral"),
    ("accent", "violet"),
    ("borderAccent", "violet"),
    ("customMessageLabel", "violet"),
    ("mdCode", "violet"),
    ("mdListBullet", "violet"),
    ("syntaxType", "violet"),
    ("border", "blue"),
    ("mdLink", "blue"),
    ("syntaxKeyword", "blue"),
    ("syntaxVariable", "calamine"),
    ("success", "green"),
    ("mdCodeBlock", "green"),
    ("toolDiffAdded", "green"),
    ("bashMode", "green"),
    ("syntaxNumber", "green"),
    ("error", "red"),
    ("toolDiffRemoved", "red"),
    ("warning", "yellow"),
    ("mdHeading", "yellow"),
    ("syntaxFunction", "yellow"),
    ("syntaxString", "orange"),
    ("thinkingOff", "neutral"),
    ("thinkingMinimal", "thinkingSlate"),
    ("thinkingLow", "thinkingBlue"),
    ("thinkingMedium", "thinkingPeriwinkle"),
    ("thinkingHigh", "thinkingViolet"),
    ("thinkingXhigh", "thinkingMagenta"),
    ("thinkingMax", "thinkingRed"),
];

/// Palette slots for tokens that would otherwise share a hue with a similar
/// token.
const TOKEN_SLOTS: &[(&str, u8)] = &[
    ("syntaxString", 2),
    ("syntaxNumber", 5),
    ("searchMatchBg", 3),
];

fn token_slot(token: &str) -> Option<u8> {
    TOKEN_SLOTS
        .iter()
        .find(|(name, _)| *name == token)
        .map(|(_, slot)| *slot)
}

const PANELS: &[&str] = &[
    "userMessageBg",
    "toolPendingBg",
    "toolSuccessBg",
    "toolErrorBg",
    "selectedBg",
    "searchMatchBg",
    "customMessageBg",
];

fn is_panel(token: &str) -> bool {
    PANELS.contains(&token)
}

// ============================================================================
// Contrast levels and rules
// ============================================================================

/// Target-lightness curve: a polynomial in the surface's OKLab lightness
/// giving the OKLab lightness a token needs on it. `reachable` is the range
/// of surface lightness where the level can be reached; beyond it the level
/// is relaxed.
#[derive(Clone, Copy)]
struct Curve {
    coefficients: &'static [f64],
    reachable: [f64; 2],
}

macro_rules! curves {
    ($($level:ident => dark: [$($dc:expr),*] reachable [$dlo:expr, $dhi:expr],
        light: [$($lc:expr),*] reachable [$llo:expr, $lhi:expr];)*) => {
        const LEVELS: &[(&str, [Curve; 2])] = &[
            $(
                (
                    stringify!($level),
                    [
                        Curve { coefficients: &[$($dc),*], reachable: [$dlo, $dhi] },
                        Curve { coefficients: &[$($lc),*], reachable: [$llo, $lhi] },
                    ],
                )
            ),*
        ];
    };
}

// Appearance index: 0 = dark, 1 = light.
curves! {
    panel => dark: [0.29131, -0.39746, 2.33185, -0.85524, -1.2076, 0.86276] reachable [0.0, 0.979],
        light: [-3.74073, 27.94549, -78.44258, 112.6798, -79.60015, 22.11277] reachable [0.348, 1.0];
    track => dark: [0.39028, -0.23015, 0.83573, 2.43829, -4.38292, 2.01582] reachable [0.0, 0.946],
        light: [-5.24921, 38.37322, -107.28833, 152.10005, -106.17127, 29.18061] reachable [0.368, 1.0];
    thinking0 => dark: [0.52988, -0.05809, -0.30924, 4.63567, -6.52933, 2.89108] reachable [0.0, 0.873],
        light: [-28.27749, 182.85284, -469.62416, 603.15916, -384.59976, 97.35147] reachable [0.51, 1.0];
    thinking1 => dark: [0.55278, -0.03667, -0.45659, 4.95347, -6.90265, 3.0706] reachable [0.0, 0.858],
        light: [-37.10484, 235.86282, -596.62344, 754.3633, -474.00763, 118.3551] reachable [0.535, 1.0];
    thinking2 => dark: [0.57486, -0.01765, -0.58987, 5.25227, -7.27175, 3.25532] reachable [0.0, 0.842],
        light: [-59.89653, 377.05024, -945.07843, 1182.03145, -734.96375, 181.68658] reachable [0.556, 1.0];
    thinking3 => dark: [0.59621, -0.00062, -0.71148, 5.53588, -7.6392, 3.44606] reachable [0.0, 0.827],
        light: [-72.07122, 445.84082, -1099.57352, 1353.88793, -829.53392, 202.26164] reachable [0.58, 1.0];
    thinking4 => dark: [0.61691, 0.01462, -0.82288, 5.80651, -8.00641, 3.64333] reachable [0.0, 0.811],
        light: [-110.14338, 674.21488, -1645.75941, 2004.32367, -1215.15899, 293.3183] reachable [0.6, 1.0];
    thinking5 => dark: [0.63702, 0.02826, -0.92498, 6.06465, -8.37246, 3.84651] reachable [0.0, 0.795],
        light: [-175.47701, 1063.54495, -2570.70594, 3098.80776, -1860.15527, 444.76392] reachable [0.62, 1.0];
    thinking6 => dark: [0.65658, 0.04044, -1.01835, 6.30989, -8.73529, 4.05439] reachable [0.0, 0.779],
        light: [-183.81712, 1094.70055, -2602.68539, 3088.71276, -1826.91131, 430.75931] reachable [0.643, 1.0];
    subtle => dark: [0.56762, -0.02475, -0.5383, 5.12628, -7.10931, 3.17324] reachable [0.0, 0.848],
        light: [-232.85459, 1376.54473, -3249.11801, 3827.91186, -2248.29472, 526.55751] reachable [0.657, 1.0];
    thumb => dark: [0.60323, 0.00278, -0.73328, 5.57157, -7.68067, 3.46933] reachable [0.0, 0.823],
        light: [-82.89897, 511.01355, -1255.98095, 1540.76821, -940.68087, 228.58523] reachable [0.586, 1.0];
    readable => dark: [0.66937, 0.04704, -1.06871, 6.43941, -8.9332, 4.17229] reachable [0.0, 0.77],
        light: [-1554.52576, 8733.56817, -19604.93507, 21977.72696, -12300.99599, 2749.81288] reachable [0.751, 1.0];
    emphasis => dark: [0.7303, 0.07695, -1.31626, 7.1681, -10.14436, 4.92846] reachable [0.0, 0.712],
        light: [-4948.31942, 26870.91986, -58334.48399, 63280.17197, -34298.01053, 7430.30146] reachable [0.811, 1.0];
    textOnPanel => dark: [0.86713, 0.05232, -0.89428, 4.79014, -5.5432, 1.75023] reachable [0.0, 0.542],
        light: [-8570.89457, 43954.60805, -90084.00702, 92220.6791, -47152.15802, 9632.27113] reachable [0.867, 1.0];
    text => dark: [0.89242, 0.02311, -0.44862, 2.34417, -0.06084, -2.63844] reachable [0.0, 0.5],
        light: [-2004.67048, 6664.47299, -6060.70202, -1792.61209, 5133.82359, -1939.85583] reachable [0.894, 1.0];
}

type Surface<'s> = &'s str;

/// One contrast rule: the token must reach `level` on every surface in `on`.
struct Rule {
    token: &'static str,
    on: &'static [&'static str],
    level: &'static str,
}

const fn make_rule(token: &'static str, on: &'static [&'static str], level: &'static str) -> Rule {
    Rule { token, on, level }
}

const TOOL_PANELS: &[&str] = &["toolPendingBg", "toolSuccessBg", "toolErrorBg"];
/// The full rule list, in upstream order. `each(...)` spreads become
/// concatenated slices.
const RULES: &[Rule] = &[
    // ...each(PANELS, ["background"], "panel")
    make_rule("userMessageBg", &["background"], "panel"),
    make_rule("toolPendingBg", &["background"], "panel"),
    make_rule("toolSuccessBg", &["background"], "panel"),
    make_rule("toolErrorBg", &["background"], "panel"),
    make_rule("selectedBg", &["background"], "panel"),
    make_rule("searchMatchBg", &["background"], "panel"),
    make_rule("customMessageBg", &["background"], "panel"),
    make_rule("text", &["background"], "text"),
    make_rule("text", &["selectedBg"], "textOnPanel"),
    make_rule("userMessageText", &["userMessageBg"], "textOnPanel"),
    make_rule("toolTitle", TOOL_PANELS, "textOnPanel"),
    // ...each(["accent","success","error","warning"], ["background","selectedBg",...TOOL_PANELS], "readable")
    make_rule(
        "accent",
        &[
            "background",
            "selectedBg",
            "toolPendingBg",
            "toolSuccessBg",
            "toolErrorBg",
        ],
        "readable",
    ),
    make_rule(
        "success",
        &[
            "background",
            "selectedBg",
            "toolPendingBg",
            "toolSuccessBg",
            "toolErrorBg",
        ],
        "readable",
    ),
    make_rule(
        "error",
        &[
            "background",
            "selectedBg",
            "toolPendingBg",
            "toolSuccessBg",
            "toolErrorBg",
        ],
        "readable",
    ),
    make_rule(
        "warning",
        &[
            "background",
            "selectedBg",
            "toolPendingBg",
            "toolSuccessBg",
            "toolErrorBg",
        ],
        "readable",
    ),
    make_rule(
        "muted",
        &[
            "background",
            "selectedBg",
            "customMessageBg",
            "toolPendingBg",
            "toolSuccessBg",
            "toolErrorBg",
        ],
        "readable",
    ),
    make_rule(
        "dim",
        &[
            "background",
            "selectedBg",
            "customMessageBg",
            "toolPendingBg",
            "toolSuccessBg",
            "toolErrorBg",
        ],
        "subtle",
    ),
    make_rule("thinkingText", &["background"], "readable"),
    make_rule(
        "customMessageText",
        &[
            "customMessageBg",
            "toolPendingBg",
            "toolSuccessBg",
            "toolErrorBg",
        ],
        "readable",
    ),
    make_rule(
        "customMessageLabel",
        &[
            "background",
            "customMessageBg",
            "selectedBg",
            "toolPendingBg",
            "toolSuccessBg",
            "toolErrorBg",
        ],
        "readable",
    ),
    make_rule(
        "toolOutput",
        &[
            "background",
            "toolPendingBg",
            "toolSuccessBg",
            "toolErrorBg",
        ],
        "readable",
    ),
    make_rule(
        "mdHeading",
        &["background", "userMessageBg", "customMessageBg"],
        "readable",
    ),
    make_rule(
        "mdLink",
        &["background", "userMessageBg", "customMessageBg"],
        "readable",
    ),
    make_rule(
        "mdLinkUrl",
        &["background", "userMessageBg", "customMessageBg"],
        "readable",
    ),
    make_rule(
        "mdCode",
        &["background", "userMessageBg", "customMessageBg"],
        "readable",
    ),
    make_rule(
        "mdQuote",
        &["background", "userMessageBg", "customMessageBg"],
        "readable",
    ),
    make_rule(
        "mdCodeBlockBorder",
        &["background", "userMessageBg", "customMessageBg"],
        "readable",
    ),
    make_rule(
        "mdListBullet",
        &["background", "userMessageBg", "customMessageBg"],
        "readable",
    ),
    make_rule(
        "mdCodeBlock",
        &[
            "background",
            "userMessageBg",
            "customMessageBg",
            "toolPendingBg",
            "toolSuccessBg",
            "toolErrorBg",
        ],
        "readable",
    ),
    make_rule(
        "toolDiffAdded",
        &[
            "background",
            "toolPendingBg",
            "toolSuccessBg",
            "toolErrorBg",
        ],
        "readable",
    ),
    make_rule(
        "toolDiffRemoved",
        &[
            "background",
            "toolPendingBg",
            "toolSuccessBg",
            "toolErrorBg",
        ],
        "readable",
    ),
    make_rule(
        "toolDiffContext",
        &[
            "background",
            "toolPendingBg",
            "toolSuccessBg",
            "toolErrorBg",
        ],
        "readable",
    ),
    make_rule(
        "syntaxComment",
        &[
            "background",
            "userMessageBg",
            "customMessageBg",
            "toolPendingBg",
            "toolSuccessBg",
            "toolErrorBg",
        ],
        "readable",
    ),
    make_rule(
        "syntaxKeyword",
        &[
            "background",
            "userMessageBg",
            "customMessageBg",
            "toolPendingBg",
            "toolSuccessBg",
            "toolErrorBg",
        ],
        "readable",
    ),
    make_rule(
        "syntaxFunction",
        &[
            "background",
            "userMessageBg",
            "customMessageBg",
            "toolPendingBg",
            "toolSuccessBg",
            "toolErrorBg",
        ],
        "readable",
    ),
    make_rule(
        "syntaxVariable",
        &[
            "background",
            "userMessageBg",
            "customMessageBg",
            "toolPendingBg",
            "toolSuccessBg",
            "toolErrorBg",
        ],
        "readable",
    ),
    make_rule(
        "syntaxString",
        &[
            "background",
            "userMessageBg",
            "customMessageBg",
            "toolPendingBg",
            "toolSuccessBg",
            "toolErrorBg",
        ],
        "readable",
    ),
    make_rule(
        "syntaxNumber",
        &[
            "background",
            "userMessageBg",
            "customMessageBg",
            "toolPendingBg",
            "toolSuccessBg",
            "toolErrorBg",
        ],
        "readable",
    ),
    make_rule(
        "syntaxType",
        &[
            "background",
            "userMessageBg",
            "customMessageBg",
            "toolPendingBg",
            "toolSuccessBg",
            "toolErrorBg",
        ],
        "readable",
    ),
    make_rule(
        "syntaxOperator",
        &[
            "background",
            "userMessageBg",
            "customMessageBg",
            "toolPendingBg",
            "toolSuccessBg",
            "toolErrorBg",
        ],
        "readable",
    ),
    make_rule(
        "syntaxPunctuation",
        &[
            "background",
            "userMessageBg",
            "customMessageBg",
            "toolPendingBg",
            "toolSuccessBg",
            "toolErrorBg",
        ],
        "readable",
    ),
    make_rule("searchMatchText", &["searchMatchBg"], "readable"),
    make_rule("bashMode", &["background"], "readable"),
    make_rule("border", &["background"], "readable"),
    make_rule("borderAccent", &["background"], "readable"),
    make_rule("borderMuted", &["background"], "subtle"),
    make_rule(
        "mdQuoteBorder",
        &[
            "background",
            "userMessageBg",
            "customMessageBg",
            "toolPendingBg",
            "toolSuccessBg",
            "toolErrorBg",
        ],
        "readable",
    ),
    make_rule(
        "mdHr",
        &[
            "background",
            "userMessageBg",
            "customMessageBg",
            "toolPendingBg",
            "toolSuccessBg",
            "toolErrorBg",
        ],
        "readable",
    ),
    make_rule("scrollbarTrack", &["background"], "track"),
    make_rule("scrollbarThumb", &["scrollbarTrack"], "thumb"),
    make_rule("thinkingOff", &["background"], "thinking0"),
    make_rule("thinkingMinimal", &["background"], "thinking1"),
    make_rule("thinkingLow", &["background"], "thinking2"),
    make_rule("thinkingMedium", &["background"], "thinking3"),
    make_rule("thinkingHigh", &["background"], "thinking4"),
    make_rule("thinkingXhigh", &["background"], "thinking5"),
    make_rule("thinkingMax", &["background"], "thinking6"),
];

/// Relaxation compresses levels stronger than this one toward it before
/// weakening all levels.
fn readable_floor(appearance: ThemeAppearance) -> &'static str {
    if appearance == "dark" {
        "readable"
    } else {
        "subtle"
    }
}

/// Body text uses the terminal's foreground when it reaches this level, which
/// is clearly stronger than muted.
const FOREGROUND_LEVEL: &str = "emphasis";

/// Text-level tokens that take the terminal's foreground.
const FOREGROUND_TOKENS: &[&str] = &["text", "userMessageText", "toolTitle"];

/// WCAG 2 contrast ratio that body text must reach on the surfaces it is
/// drawn on.
const TEXT_MINIMUM_WCAG_CONTRAST: f64 = 4.5;

/// Tokens in dependency order: every surface before the tokens drawn on it.
fn solve_order() -> Vec<&'static str> {
    let mut order: Vec<&'static str> = Vec::new();
    fn visit(token: &'static str, order: &mut Vec<&'static str>) {
        if order.contains(&token) {
            return;
        }
        for surface in RULES
            .iter()
            .filter(|rule| rule.token == token)
            .flat_map(|rule| rule.on.iter())
            .copied()
        {
            if surface != "background" {
                visit_static(surface, order);
            }
        }
        order.push(token);
    }
    fn visit_static(surface: &'static str, order: &mut Vec<&'static str>) {
        visit(surface, order);
    }
    for rule in RULES {
        visit(rule.token, &mut order);
    }
    order
}

// ============================================================================
// Public API
// ============================================================================

/// Upstream `SystemThemeInput`.
#[derive(Clone, Debug, Default)]
pub struct SystemThemeInput {
    pub foreground: Option<Rgb>,
    pub background: Option<Rgb>,
    /// ANSI colors 0-15.
    pub palette: Option<Vec<Rgb>>,
    /// Saturation multiplier from 0 (grayscale) to 1. The first frame renders
    /// in grayscale until colors arrive.
    pub saturation: Option<f64>,
    /// Appearance when the terminal did not report its background.
    pub appearance_hint: Option<ThemeAppearance>,
}

/// One solved token color: a hex string, an ANSI palette index, or "" for the
/// terminal default.
#[derive(Clone, Debug, PartialEq)]
pub enum TokenColor {
    Hex(String),
    Index(u8),
    TerminalDefault,
}

/// Upstream `SystemThemeColors`.
#[derive(Clone, Debug)]
pub struct SystemThemeColors {
    /// In `TOKEN_FAMILIES` (upstream object literal) order.
    pub colors: Vec<(&'static str, TokenColor)>,
    /// Foreground tokens rendered faint (SGR 2), for terminals that did not
    /// report colors.
    pub dim: Vec<&'static str>,
    pub appearance: Option<ThemeAppearance>,
}

/// Upstream `okhslColor` on the quantized pipeline.
fn okhsl(hue: f64, saturation: f64, lightness: f64) -> Rgb {
    let color = okhsl_color(hue, saturation, lightness).expect("okhsl arguments are in range");
    let [r, g, b] = color_to_rgb(color);
    RgbColor {
        r: r.round() as u8,
        g: g.round() as u8,
        b: b.round() as u8,
    }
}

/// OKLab lightness of an sRGB color, 0-1.
fn oklab_lightness(color: Rgb) -> f64 {
    rgb_to_oklab([f64::from(color.r), f64::from(color.g), f64::from(color.b)])[0]
}

/// WCAG 2 relative luminance.
pub fn relative_luminance(color: Rgb) -> f64 {
    let linear = |channel: f64| -> f64 {
        let value = channel / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(f64::from(color.r))
        + 0.7152 * linear(f64::from(color.g))
        + 0.0722 * linear(f64::from(color.b))
}

/// WCAG 2 contrast ratio, 1-21.
pub fn wcag_contrast(first: Rgb, second: Rgb) -> f64 {
    let a = relative_luminance(first);
    let b = relative_luminance(second);
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// Whether a terminal is dark or light, from its reported colors.
pub fn terminal_appearance(background: Rgb, foreground: Option<Rgb>) -> ThemeAppearance {
    let white: Rgb = RgbColor {
        r: 255,
        g: 255,
        b: 255,
    };
    let black: Rgb = RgbColor { r: 0, g: 0, b: 0 };
    let white_contrast = wcag_contrast(white, background);
    let black_contrast = wcag_contrast(black, background);
    if let Some(foreground) = foreground {
        let foreground_l = oklab_lightness(foreground);
        let background_l = oklab_lightness(background);
        if (foreground_l - background_l).abs() > 0.05 {
            let appearance: ThemeAppearance = if foreground_l > background_l {
                "dark"
            } else {
                "light"
            };
            let best = if appearance == "dark" {
                white_contrast
            } else {
                black_contrast
            };
            if best >= TEXT_MINIMUM_WCAG_CONTRAST {
                return appearance;
            }
        }
    }
    if white_contrast >= black_contrast {
        "dark"
    } else {
        "light"
    }
}

// ============================================================================
// Generation
// ============================================================================

fn hex_of(color: Rgb) -> String {
    // Upstream `Math.round(channel)` on channels that the pipeline already
    // rounded to integers: identity.
    format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b)
}

/// Saturation weight at a lightness: a Gaussian (center 0.5, sigma 0.25), 0
/// at black and white, 1 in the middle.
fn bell_weight(lightness: f64) -> f64 {
    let gaussian = |x: f64| f64::exp(-(x - 0.5).powi(2) / (2.0 * 0.25f64.powi(2)));
    (gaussian(lightness) - gaussian(0.0)) / (1.0 - gaussian(0.0))
}

/// A family's saturation curve relative to its maximum: 1 at mid lightness,
/// `min / max` at black and white.
fn saturation_curve(family: &Family, lightness: f64) -> f64 {
    let floor = if family.sat_max > 0.0 {
        family.sat_min / family.sat_max
    } else {
        1.0
    };
    floor + (1.0 - floor) * bell_weight(lightness)
}

/// The target lightness for a level on a surface, or None where the level
/// cannot be reached.
fn level_target(level: &str, appearance: ThemeAppearance, surface_l: f64) -> Option<f64> {
    let curve = &LEVELS
        .iter()
        .find(|(name, _)| *name == level)
        .map(|(_, pair)| pair)
        .unwrap_or_else(|| panic!("unknown level {level}"))
        [if appearance == "dark" { 0 } else { 1 }];
    if surface_l < curve.reachable[0] || surface_l > curve.reachable[1] {
        return None;
    }
    Some(
        curve
            .coefficients
            .iter()
            .enumerate()
            .map(|(power, coefficient)| coefficient * surface_l.powf(power as f64))
            .sum(),
    )
}

fn okhsl_of(color: Rgb) -> [f64; 3] {
    rgb_to_okhsl([f64::from(color.r), f64::from(color.g), f64::from(color.b)])
}

/// A terminal color's OKHSL channels and its OKLCH chroma (upstream
/// `SourceColor`, v1.0.0).
#[derive(Debug, Clone, Copy)]
struct SourceColor {
    h: f64,
    s: f64,
    l: f64,
    chroma: f64,
}

/// Upstream `sourceOf` (v1.0.0): the OKHSL channels plus the color's OKLCH
/// chroma, so [`anchored`] can also cap chroma at the source's.
fn source_of(color: Rgb) -> SourceColor {
    let [h, s, l] = okhsl_of(color);
    let chroma = color_to_oklch(crate::tui::colors::Color::Rgb {
        r: f64::from(color.r),
        g: f64::from(color.g),
        b: f64::from(color.b),
    })
    .c;
    SourceColor { h, s, l, chroma }
}

/// A source color's hue at another OKHSL lightness. Its saturation applies at
/// its own lightness and falls off toward black and white along the family's
/// saturation curve, never rising above it.
///
/// v1.0.0: OKHSL saturation is relative to the most chroma sRGB allows at a
/// lightness, so the same saturation can mean more chroma elsewhere
/// (Catppuccin Frappe's pink #f4b8e4, chroma 0.089, would become #eb76d1,
/// 0.180, at the lightness the accent needs). Chroma is therefore also capped
/// at the source's, with the same falloff.
fn anchored(source: SourceColor, family: &Family, lightness: f64, saturation: f64) -> Rgb {
    let anchor = saturation_curve(family, source.l);
    let falloff = if anchor > 0.0 {
        (saturation_curve(family, lightness) / anchor).min(1.0)
    } else {
        1.0
    };
    let color = okhsl(source.h, source.s * falloff * saturation, lightness);
    let cap = source.chroma * falloff * saturation;
    let channels = color_to_oklch(crate::tui::colors::Color::Rgb {
        r: f64::from(color.r),
        g: f64::from(color.g),
        b: f64::from(color.b),
    });
    if channels.c <= cap {
        color
    } else {
        let oklch = oklch_color(channels.l, cap, source.h).expect("oklch arguments are in range");
        let [r, g, b] = color_to_rgb(oklch);
        RgbColor {
            r: r.round() as u8,
            g: g.round() as u8,
            b: b.round() as u8,
        }
    }
}

/// Move a text color toward white or black until it reaches the WCAG minimum
/// on every surface.
fn with_text_contrast(color: Rgb, surfaces: &[Rgb], lighter: bool) -> Rgb {
    let meets = |candidate: Rgb| -> bool {
        surfaces
            .iter()
            .all(|surface| wcag_contrast(candidate, *surface) >= TEXT_MINIMUM_WCAG_CONTRAST)
    };
    if meets(color) {
        return color;
    }
    let [h, s, l] = okhsl_of(color);
    let at = |lightness: f64| okhsl(h, s, lightness);
    let extreme = if lighter { 1.0 } else { 0.0 };
    if !meets(at(extreme)) {
        return at(extreme);
    }
    let mut low = l;
    let mut high = extreme;
    for _ in 0..20 {
        let middle = (low + high) / 2.0;
        if meets(at(middle)) {
            high = middle;
        } else {
            low = middle;
        }
    }
    at(high)
}

/// Colors for terminals that reported nothing: the terminal renders ANSI
/// indices 0-15 and the default colors with its own theme, so they fit any
/// background. Neutral tokens below body text are faint (SGR 2) instead of
/// bright black, which some themes make nearly invisible. Panels have no
/// background.
fn indexed_colors(saturation: f64, appearance: Option<ThemeAppearance>) -> SystemThemeColors {
    let mut colors = Vec::new();
    let mut dim = Vec::new();
    for &(token, family_name) in TOKEN_FAMILIES {
        if is_panel(token) {
            colors.push((token, TokenColor::TerminalDefault));
            continue;
        }
        let neutral = family_name == "neutral";
        let value = if !neutral && saturation > 0.0 {
            TokenColor::Index(token_slot(token).unwrap_or_else(|| family_named(family_name).slot))
        } else {
            TokenColor::TerminalDefault
        };
        colors.push((token, value));
        if neutral && !FOREGROUND_TOKENS.contains(&token) {
            dim.push(token);
        }
    }
    SystemThemeColors {
        colors,
        dim,
        appearance,
    }
}

/// Generate the system theme's colors from the terminal's reported colors.
pub fn generate_system_theme_colors(input: &SystemThemeInput) -> SystemThemeColors {
    let saturation = input.saturation.unwrap_or(1.0).clamp(0.0, 1.0);
    let Some(background) = input.background else {
        return indexed_colors(saturation, input.appearance_hint);
    };
    let palette = input
        .palette
        .as_ref()
        .filter(|palette| palette.len() == 16)
        .map(|palette| {
            palette
                .iter()
                .map(|color| source_of(*color))
                .collect::<Vec<_>>()
        });

    let appearance = terminal_appearance(background, input.foreground);
    let lighter = appearance == "dark";
    let extreme = if lighter { 1.0 } else { 0.0 };
    let background_l = oklab_lightness(background);

    // A token's color at an OKLab lightness. With a palette, the palette
    // color's saturation applies at its own lightness and falls off toward
    // black and white along the family's curve, never rising above it.
    let paint = |token: &'static str, oklab_l: f64| -> Rgb {
        let lightness = oklab_to_okhsl_lightness(oklab_l);
        let family = family_named(
            TOKEN_FAMILIES
                .iter()
                .find(|(name, _)| *name == token)
                .map(|(_, family)| *family)
                .unwrap_or("neutral"),
        );
        if let Some(palette) = &palette {
            let slot = token_slot(token).unwrap_or(family.slot);
            return anchored(palette[slot as usize], family, lightness, saturation);
        }
        let min = family.sat_min;
        let max = family.sat_max;
        okhsl(
            family.hue,
            (min + (max - min) * bell_weight(lightness)) * saturation,
            lightness,
        )
    };

    // The lightness a rule needs on a surface, relaxed by `t`: from 0 to 1,
    // levels stronger than the readable floor move toward it; from 1 to 2,
    // all levels move toward the surface itself.
    let target = |level: &str, surface_l: f64, t: f64| -> Option<f64> {
        let reached = level_target(level, appearance, surface_l);
        if reached.is_none() && t == 0.0 {
            return None;
        }
        let distance = reached.unwrap_or(extreme) - surface_l;
        let floor = (level_target(readable_floor(appearance), appearance, surface_l)
            .unwrap_or(extreme))
            - surface_l;
        let compressed = if distance.abs() > floor.abs() {
            distance - (distance - floor) * t.min(1.0)
        } else {
            distance
        };
        Some(surface_l + compressed * (1.0 - (t - 1.0).max(0.0)))
    };

    // Keep a panel light enough (or dark enough) that white (or black) text
    // still reaches the body text minimum on it.
    let extreme_text: Rgb = if lighter {
        RgbColor {
            r: 255,
            g: 255,
            b: 255,
        }
    } else {
        RgbColor { r: 0, g: 0, b: 0 }
    };
    let readable = |color: Rgb| wcag_contrast(extreme_text, color) >= TEXT_MINIMUM_WCAG_CONTRAST;
    let limit_panel = |token: &'static str, l: f64| -> Rgb {
        let color = paint(token, l);
        if readable(color) {
            return color;
        }
        let mut low = background_l;
        let mut high = l;
        for _ in 0..20 {
            let middle = (low + high) / 2.0;
            if readable(paint(token, middle)) {
                low = middle;
            } else {
                high = middle;
            }
        }
        paint(token, low)
    };

    type Solved<'s> = HashMap<Surface<'s>, Rgb>;

    let solve = |t: f64| -> Option<Solved<'static>> {
        let mut colors: Solved<'static> = HashMap::new();
        colors.insert("background", background);
        for token in solve_order() {
            let mut targets: Vec<f64> = Vec::new();
            for surface in RULES
                .iter()
                .filter(|rule| rule.token == token)
                .flat_map(|rule| rule.on.iter())
                .copied()
            {
                let value = target(
                    RULES
                        .iter()
                        .find(|rule| rule.token == token && rule.on.contains(&surface))
                        .map(|rule| rule.level)
                        .unwrap_or("readable"),
                    oklab_lightness(*colors.get(surface).unwrap_or(&background)),
                    t,
                );
                let value = value?;
                if !(0.0..=1.0).contains(&value) {
                    return None;
                }
                targets.push(value);
            }
            let l = if lighter {
                targets.iter().copied().fold(f64::NEG_INFINITY, f64::max)
            } else {
                targets.iter().copied().fold(f64::INFINITY, f64::min)
            };
            let color = if is_panel(token) {
                limit_panel(token, l)
            } else {
                paint(token, l)
            };
            colors.insert(token, color);
        }
        Some(colors)
    };

    let mut relaxation = 0.0;
    let mut colors = solve(0.0);
    if colors.is_none() {
        // Mid-gray backgrounds cannot fit every level: relax as little as
        // possible. Full relaxation always fits.
        let mut low = 0.0;
        let mut high = 2.0;
        colors = solve(high);
        for _ in 0..20 {
            let middle = (low + high) / 2.0;
            match solve(middle) {
                Some(attempt) => {
                    high = middle;
                    colors = Some(attempt);
                }
                None => low = middle,
            }
        }
        relaxation = high;
    }
    let solved = colors.unwrap_or_default();
    let surfaces_of = |token: &'static str| -> Vec<Rgb> {
        RULES
            .iter()
            .filter(|rule| rule.token == token)
            .flat_map(|rule| rule.on.iter())
            .map(|surface| *solved.get(surface).unwrap_or(&background))
            .collect()
    };

    let mut result: Vec<(&'static str, TokenColor)> = Vec::new();
    let mut colors_out: HashMap<&'static str, TokenColor> = HashMap::new();
    for &(token, _) in TOKEN_FAMILIES {
        let value = match solved.get(token) {
            Some(color) => TokenColor::Hex(hex_of(*color)),
            None => TokenColor::TerminalDefault,
        };
        colors_out.insert(token, value.clone());
        result.push((token, value));
    }

    for token in FOREGROUND_TOKENS {
        let surfaces = surfaces_of(token);
        // Body text uses the terminal's own foreground where it is clearly
        // stronger than muted text; otherwise the foreground's hue at just
        // enough lightness.
        let mut text = solved.get(token).copied();
        if let Some(foreground) = input.foreground {
            let targets: Vec<Option<f64>> = surfaces
                .iter()
                .map(|surface| target(FOREGROUND_LEVEL, oklab_lightness(*surface), relaxation))
                .collect();
            if targets
                .iter()
                .all(|value| matches!(value, Some(value) if (0.0..=1.0).contains(value)))
            {
                let needed = if lighter {
                    targets
                        .iter()
                        .copied()
                        .flatten()
                        .fold(f64::NEG_INFINITY, f64::max)
                } else {
                    targets
                        .iter()
                        .copied()
                        .flatten()
                        .fold(f64::INFINITY, f64::min)
                };
                let foreground_l = oklab_lightness(foreground);
                if (lighter && foreground_l >= needed) || (!lighter && foreground_l <= needed) {
                    if let Some(entry) = result.iter_mut().find(|(name, _)| name == token) {
                        entry.1 = TokenColor::TerminalDefault;
                    }
                    continue;
                }
                text = Some(anchored(
                    source_of(foreground),
                    family_named("neutral"),
                    oklab_to_okhsl_lightness(needed),
                    saturation,
                ));
            }
        }
        // Body text keeps at least 4.5:1 on the surfaces it is drawn on, even
        // on relaxed mid-gray backgrounds.
        if let Some(text) = text {
            let value = TokenColor::Hex(hex_of(with_text_contrast(text, &surfaces, lighter)));
            if let Some(entry) = result.iter_mut().find(|(name, _)| name == token) {
                entry.1 = value;
            }
        }
    }

    SystemThemeColors {
        colors: result,
        dim: Vec::new(),
        appearance: Some(appearance),
    }
}
