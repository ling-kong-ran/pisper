//! Port of upstream `packages/tui/src/colors.ts`: concrete colors, parsing,
//! ANSI styling, and color math over [`crate::tui::oklab`].
//!
//! Upstream constructors throw on invalid input; the Rust ports return
//! `Result` with the exact upstream error strings (JS `Number` formatting is
//! reproduced by [`js_number_to_string`], covering the ECMAScript
//! `Number::toString` fixed/exponential switch at 1e21).

use std::sync::OnceLock;

use regex::Regex;

use crate::tui::oklab::{
    linear_srgb_to_rgb, okhsl_to_rgb, oklab_to_linear_srgb, rgb_to_okhsl, rgb_to_oklab,
};
use crate::tui::terminal_colors::RgbColor;

/// Upstream `IndexedColor`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IndexedColor {
    pub index: u8,
}

/// A concrete color. Every color can be converted to sRGB, so color math never
/// fails.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Color {
    Indexed(IndexedColor),
    /// Upstream `RgbColorValue`: channels stay JS numbers (fractions allowed).
    Rgb {
        r: f64,
        g: f64,
        b: f64,
    },
    Oklch {
        l: f64,
        c: f64,
        h: f64,
    },
}

/// Upstream `TerminalColorMode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalColorMode {
    Color256,
    Truecolor,
}
impl TerminalColorMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Color256 => "256color",
            Self::Truecolor => "truecolor",
        }
    }
}

/// Upstream `ColorMixSpace`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorMixSpace {
    Oklch,
    Srgb,
}

/// Upstream `OklchChannels`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OklchChannels {
    pub l: f64,
    pub c: f64,
    pub h: f64,
}

/// Upstream `OkhslChannels`: hue in degrees, saturation and lightness 0-1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OkhslChannels {
    pub h: f64,
    pub s: f64,
    pub l: f64,
}

/// Upstream `TextAttributes`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextAttributes {
    pub bold: Option<bool>,
    pub dim: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub inverse: Option<bool>,
    pub strikethrough: Option<bool>,
}

/// Upstream `TextStyle` (attributes plus foreground/background).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TextStyle {
    pub bold: Option<bool>,
    pub dim: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub inverse: Option<bool>,
    pub strikethrough: Option<bool>,
    pub fg: Option<Color>,
    pub bg: Option<Color>,
}

impl TextStyle {
    fn attributes(&self) -> TextAttributes {
        TextAttributes {
            bold: self.bold,
            dim: self.dim,
            italic: self.italic,
            underline: self.underline,
            inverse: self.inverse,
            strikethrough: self.strikethrough,
        }
    }
}

/// ECMAScript `Number::toString`: fixed notation for -6 < n <= 21 decimal
/// points, otherwise exponent notation with an explicit sign.
pub fn js_number_to_string(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_string();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    if value == 0.0 {
        return "0".to_string();
    }
    let negative = value < 0.0;
    // `{:e}` yields Rust's shortest-round-trip digits: "d[.ddd]e<exp>".
    let scientific = format!("{:e}", value.abs());
    let (mantissa, exponent) = scientific.split_once('e').expect("LowerExp format");
    let exponent: i32 = exponent.parse().expect("exponent digits");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let digits = digits.trim_end_matches('0');
    let digits = if digits.is_empty() { "0" } else { digits };
    let n = exponent + 1; // position of the decimal point
    let sign = if negative { "-" } else { "" };
    if n > -6 && n <= 21 {
        if n <= 0 {
            format!("{sign}0.{}{digits}", "0".repeat((-n) as usize))
        } else if (n as usize) >= digits.len() {
            format!("{sign}{}{}", digits, "0".repeat(n as usize - digits.len()))
        } else {
            format!("{sign}{}.{}", &digits[..n as usize], &digits[n as usize..])
        }
    } else {
        let mantissa = if digits.len() == 1 {
            digits.to_string()
        } else {
            format!("{}.{}", &digits[..1], &digits[1..])
        };
        format!(
            "{sign}{mantissa}e{}{exponent}",
            if exponent > 0 { "+" } else { "" }
        )
    }
}

fn require_finite(value: f64, name: &str) -> Result<(), String> {
    if !value.is_finite() {
        return Err(format!("{name} must be finite"));
    }
    Ok(())
}

/// Upstream `indexedColor`. `index` is a JS number: -0 and 12.0 are integers.
pub fn indexed_color(index: f64) -> Result<Color, String> {
    if !is_js_integer(index) || !(0.0..=255.0).contains(&index) {
        return Err(format!(
            "ANSI color index must be an integer from 0 to 255: {}",
            js_number_to_string(index)
        ));
    }
    Ok(Color::Indexed(IndexedColor { index: index as u8 }))
}

fn is_js_integer(value: f64) -> bool {
    // Number.isInteger: finite with an empty fraction. -0 passes (== 0).
    value.is_finite() && value.fract() == 0.0
}

/// Upstream `rgbColor`.
pub fn rgb_color(r: f64, g: f64, b: f64) -> Result<Color, String> {
    for (name, value) in [("r", r), ("g", g), ("b", b)] {
        require_finite(value, name)?;
        if !(0.0..=255.0).contains(&value) {
            return Err(format!(
                "{name} must be between 0 and 255: {}",
                js_number_to_string(value)
            ));
        }
    }
    Ok(Color::Rgb { r, g, b })
}

/// Upstream `oklchColor`.
pub fn oklch_color(l: f64, c: f64, h: f64) -> Result<Color, String> {
    require_finite(l, "l")?;
    require_finite(c, "c")?;
    require_finite(h, "h")?;
    if !(0.0..=1.0).contains(&l) {
        return Err(format!(
            "l must be between 0 and 1: {}",
            js_number_to_string(l)
        ));
    }
    if c < 0.0 {
        return Err(format!(
            "c must not be negative: {}",
            js_number_to_string(c)
        ));
    }
    Ok(Color::Oklch {
        l,
        c,
        h: ((h % 360.0) + 360.0) % 360.0,
    })
}

/// An OKHSL color, converted to sRGB. Saturation is relative to the sRGB gamut
/// at the hue and lightness, so equal saturation looks equally colorful across
/// hues and lightness.
pub fn okhsl_color(h: f64, s: f64, l: f64) -> Result<Color, String> {
    require_finite(h, "h")?;
    require_finite(s, "s")?;
    require_finite(l, "l")?;
    if !(0.0..=1.0).contains(&s) {
        return Err(format!(
            "s must be between 0 and 1: {}",
            js_number_to_string(s)
        ));
    }
    if !(0.0..=1.0).contains(&l) {
        return Err(format!(
            "l must be between 0 and 1: {}",
            js_number_to_string(l)
        ));
    }
    let RgbColor { r, g, b } = okhsl_to_rgb(h, s, l);
    rgb_color(f64::from(r), f64::from(g), f64::from(b))
}

/// Upstream `colorToOkhsl`.
pub fn color_to_okhsl(color: Color) -> OkhslChannels {
    let [h, s, l] = rgb_to_okhsl(color_to_rgb(color));
    OkhslChannels { h, s, l }
}
// JS \s (u-flag): \p{White_Space} minus U+0085 plus U+FEFF.
const JS_S: &str = "\\t\\n\\u{b}\\f\\r \\u{a0}\\u{1680}\\u{2000}-\\u{200a}\\u{2028}\\u{2029}\\u{202f}\\u{205f}\\u{3000}\\u{feff}";
const NUMBER_PATTERN: &str = "[+-]?(?:[0-9]+(?:\\.[0-9]*)?|\\.[0-9]+)(?:e[+-]?[0-9]+)?";

fn oklch_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!(
            "(?i)^oklch\\([{s}]*({n})(%)?[{s}]+({n})[{s}]+({n})(?:deg)?[{s}]*\\)$",
            s = JS_S,
            n = NUMBER_PATTERN
        ))
        .unwrap()
    })
}
fn okhsl_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!(
            "(?i)^okhsl\\([{s}]*({n})(?:deg)?[{s}]+({n})(%)?[{s}]+({n})(%)?[{s}]*\\)$",
            s = JS_S,
            n = NUMBER_PATTERN
        ))
        .unwrap()
    })
}
fn hex_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new("(?i)^#([0-9a-f]{3}|[0-9a-f]{6})$").unwrap())
}

fn js_parse_float(value: &str) -> f64 {
    // The patterns above already pin the full numeric grammar, which Rust
    // f64::FromStr accepts identically (leading +/-, integral, fractional,
    // scientific forms; no inf/nan reach here).
    value.parse().unwrap_or(f64::NAN)
}

/// Upstream `parseColor` (string form). The numeric form routes through
/// [`indexed_color`].
pub fn parse_color(value: &str) -> Result<Color, String> {
    if let Some(captures) = hex_pattern().captures(value) {
        let digits = captures[1].to_string();
        let digits = if digits.len() == 3 {
            digits
                .chars()
                .flat_map(|digit| [digit, digit])
                .collect::<String>()
        } else {
            digits
        };
        return rgb_color(
            f64::from(u8::from_str_radix(&digits[0..2], 16).unwrap_or(0)),
            f64::from(u8::from_str_radix(&digits[2..4], 16).unwrap_or(0)),
            f64::from(u8::from_str_radix(&digits[4..6], 16).unwrap_or(0)),
        );
    }

    if let Some(captures) = oklch_pattern().captures(value) {
        let divisor = if captures.get(2).is_some() {
            100.0
        } else {
            1.0
        };
        let lightness = js_parse_float(&captures[1]) / divisor;
        return oklch_color(
            lightness,
            js_parse_float(&captures[3]),
            js_parse_float(&captures[4]),
        );
    }

    if let Some(captures) = okhsl_pattern().captures(value) {
        let saturation = js_parse_float(&captures[2])
            / if captures.get(3).is_some() {
                100.0
            } else {
                1.0
            };
        let lightness = js_parse_float(&captures[4])
            / if captures.get(5).is_some() {
                100.0
            } else {
                1.0
            };
        return okhsl_color(js_parse_float(&captures[1]), saturation, lightness);
    }

    Err(format!("Invalid color value: {value}"))
}

/// Upstream `rgbColor` channels are JS numbers; sRGB math inside this module
/// carries `[f64; 3]` so fractional passthrough values stay exact.
type Rgb = [f64; 3];

const BASIC_COLORS: [Rgb; 16] = [
    [0.0, 0.0, 0.0],
    [128.0, 0.0, 0.0],
    [0.0, 128.0, 0.0],
    [128.0, 128.0, 0.0],
    [0.0, 0.0, 128.0],
    [128.0, 0.0, 128.0],
    [0.0, 128.0, 128.0],
    [192.0, 192.0, 192.0],
    [128.0, 128.0, 128.0],
    [255.0, 0.0, 0.0],
    [0.0, 255.0, 0.0],
    [255.0, 255.0, 0.0],
    [0.0, 0.0, 255.0],
    [255.0, 0.0, 255.0],
    [0.0, 255.0, 255.0],
    [255.0, 255.0, 255.0],
];
const CUBE_VALUES: [f64; 6] = [0.0, 95.0, 135.0, 175.0, 215.0, 255.0];
fn gray_values() -> &'static [f64; 24] {
    static VALUES: OnceLock<[f64; 24]> = OnceLock::new();
    VALUES.get_or_init(|| std::array::from_fn(|index| 8.0 + index as f64 * 10.0))
}

fn indexed_to_rgb(index: u8) -> Rgb {
    if index < 16 {
        return BASIC_COLORS[usize::from(index)];
    }
    if index < 232 {
        let cube_index = usize::from(index) - 16;
        return [
            CUBE_VALUES[cube_index / 36],
            CUBE_VALUES[(cube_index % 36) / 6],
            CUBE_VALUES[cube_index % 6],
        ];
    }
    let gray = (8 + (usize::from(index) - 232) * 10) as f64;
    [gray, gray, gray]
}

fn is_in_srgb_gamut(linear: &[f64; 3]) -> bool {
    let epsilon = 1e-7;
    linear
        .iter()
        .all(|channel| *channel >= -epsilon && *channel <= 1.0 + epsilon)
}

fn oklch_to_rgb(OklchChannels { l, c, h }: OklchChannels) -> Rgb {
    // Gamut mapping keeps the hue fixed, so its direction is computed once and
    // scaled by chroma.
    let radians = (h * std::f64::consts::PI) / 180.0;
    let cos = radians.cos();
    let sin = radians.sin();
    let at_chroma = |chroma: f64| oklab_to_linear_srgb([l, chroma * cos, chroma * sin]);

    let direct = at_chroma(c);
    if is_in_srgb_gamut(&direct) {
        return linear_srgb_to_rgb_f64(direct);
    }

    // Reduce chroma until the color fits. The achromatic color is always in
    // gamut, so it is the fallback when no bisection step fits, e.g.
    // `oklch(100% 0.3 150)` must map to white.
    let mut linear = at_chroma(0.0);
    let mut low = 0.0;
    let mut high = c;
    for _ in 0..20 {
        let chroma = (low + high) / 2.0;
        let candidate = at_chroma(chroma);
        if is_in_srgb_gamut(&candidate) {
            low = chroma;
            linear = candidate;
        } else {
            high = chroma;
        }
    }
    linear_srgb_to_rgb_f64(linear)
}

/// Upstream `linearSrgbToRgb` output as JS numbers (integral values).
fn linear_srgb_to_rgb_f64(linear: [f64; 3]) -> Rgb {
    let RgbColor { r, g, b } = linear_srgb_to_rgb(linear);
    [f64::from(r), f64::from(g), f64::from(b)]
}

/// Upstream `colorToRgb`.
pub fn color_to_rgb(color: Color) -> Rgb {
    match color {
        Color::Indexed(indexed) => indexed_to_rgb(indexed.index),
        Color::Rgb { r, g, b } => [r, g, b],
        Color::Oklch { l, c, h } => oklch_to_rgb(OklchChannels { l, c, h }),
    }
}

/// Upstream `colorToOklch`.
pub fn color_to_oklch(color: Color) -> OklchChannels {
    match color {
        Color::Oklch { l, c, h } => OklchChannels { l, c, h },
        other => {
            let [l, a, b] = rgb_to_oklab(color_to_rgb(other));
            OklchChannels {
                l,
                c: a.hypot(b),
                h: ((b.atan2(a) * 180.0) / std::f64::consts::PI + 360.0) % 360.0,
            }
        }
    }
}

/// Upstream `colorToHex`.
pub fn color_to_hex(color: Color) -> String {
    let [r, g, b] = color_to_rgb(color);
    // Math.round matches Rust round() for these non-negative channels.
    let channel = |value: f64| format!("{:02x}", value.round() as u8);
    format!("#{}{}{}", channel(r), channel(g), channel(b))
}

/// Upstream `mixColors`.
pub fn mix_colors(
    first: Color,
    second: Color,
    amount: f64,
    space: ColorMixSpace,
) -> Result<Color, String> {
    require_finite(amount, "amount")?;
    if !(0.0..=1.0).contains(&amount) {
        return Err(format!(
            "amount must be between 0 and 1: {}",
            js_number_to_string(amount)
        ));
    }

    if space == ColorMixSpace::Srgb {
        let a = color_to_rgb(first);
        let b = color_to_rgb(second);
        return rgb_color(
            a[0] + (b[0] - a[0]) * amount,
            a[1] + (b[1] - a[1]) * amount,
            a[2] + (b[2] - a[2]) * amount,
        );
    }

    let a = color_to_oklch(first);
    let b = color_to_oklch(second);
    let first_hue = if a.c < 1e-7 { b.h } else { a.h };
    let second_hue = if b.c < 1e-7 { first_hue } else { b.h };
    let hue_delta = ((second_hue - first_hue + 540.0) % 360.0) - 180.0;
    oklch_color(
        a.l + (b.l - a.l) * amount,
        a.c + (b.c - a.c) * amount,
        first_hue + hue_delta * amount,
    )
}

fn find_closest(values: &[f64], target: f64) -> usize {
    let mut closest_index = 0;
    let mut closest_distance = f64::INFINITY;
    for (index, &value) in values.iter().enumerate() {
        let distance = (target - value).abs();
        if distance < closest_distance {
            closest_index = index;
            closest_distance = distance;
        }
    }
    closest_index
}

fn color_distance(first: Rgb, second: Rgb) -> f64 {
    let dr = first[0] - second[0];
    let dg = first[1] - second[1];
    let db = first[2] - second[2];
    dr * dr * 0.299 + dg * dg * 0.587 + db * db * 0.114
}

fn rgb_to_ansi256(color: Rgb) -> u8 {
    let [r, g, b] = color;
    let r_index = find_closest(&CUBE_VALUES, r);
    let g_index = find_closest(&CUBE_VALUES, g);
    let b_index = find_closest(&CUBE_VALUES, b);
    let cube_color: Rgb = [
        CUBE_VALUES[r_index],
        CUBE_VALUES[g_index],
        CUBE_VALUES[b_index],
    ];
    let cube_index = 16 + 36 * r_index + 6 * g_index + b_index;

    let grays = gray_values();
    let gray = (0.299 * r + 0.587 * g + 0.114 * b).round();
    let gray_offset = find_closest(grays, gray);
    let gray_value = grays[gray_offset];
    let spread = r.max(g).max(b) - r.min(g).min(b);
    if spread < 10.0
        && color_distance(color, [gray_value, gray_value, gray_value])
            < color_distance(color, cube_color)
    {
        return (232 + gray_offset) as u8;
    }
    cube_index as u8
}

fn color_ansi(color: Color, mode: TerminalColorMode, background: bool) -> String {
    if let Color::Indexed(indexed) = color {
        return format!(
            "\x1b[{};5;{}m",
            if background { 48 } else { 38 },
            indexed.index
        );
    }

    let rgb = color_to_rgb(color);
    if mode == TerminalColorMode::Truecolor {
        return format!(
            "\x1b[{};2;{};{};{}m",
            if background { 48 } else { 38 },
            js_number_to_string(rgb[0].round()),
            js_number_to_string(rgb[1].round()),
            js_number_to_string(rgb[2].round())
        );
    }
    format!(
        "\x1b[{};5;{}m",
        if background { 48 } else { 38 },
        rgb_to_ansi256(rgb)
    )
}

/// Upstream `foregroundAnsi`.
pub fn foreground_ansi(color: Color, mode: TerminalColorMode) -> String {
    color_ansi(color, mode, false)
}

/// Upstream `backgroundAnsi`.
pub fn background_ansi(color: Color, mode: TerminalColorMode) -> String {
    color_ansi(color, mode, true)
}

/// Upstream `styleText`.
pub fn style_text(text: &str, options: &TextStyle, mode: TerminalColorMode) -> String {
    style_text_with_ansi(
        text,
        options.fg.map(|fg| foreground_ansi(fg, mode)),
        options.bg.map(|bg| background_ansi(bg, mode)),
        &options.attributes(),
    )
}

/// Like [`style_text`], but with precomputed color escape sequences, e.g.
/// cached theme colors. Colors in `options` are ignored.
pub fn style_text_with_ansi(
    text: &str,
    fg_ansi: Option<String>,
    bg_ansi: Option<String>,
    options: &TextAttributes,
) -> String {
    // Resets are prepended so they close in reverse order of the opening
    // sequences.
    let mut prefix = String::new();
    let mut suffix = String::new();
    if let Some(fg) = fg_ansi {
        prefix.push_str(&fg);
        suffix = "\x1b[39m".to_string();
    }
    if let Some(bg) = bg_ansi {
        prefix.push_str(&bg);
        suffix = format!("\x1b[49m{suffix}");
    }
    if options.bold == Some(true) {
        prefix.push_str("\x1b[1m");
    }
    if options.dim == Some(true) {
        prefix.push_str("\x1b[2m");
    }
    if options.bold == Some(true) || options.dim == Some(true) {
        suffix = format!("\x1b[22m{suffix}");
    }
    if options.italic == Some(true) {
        prefix.push_str("\x1b[3m");
        suffix = format!("\x1b[23m{suffix}");
    }
    if options.underline == Some(true) {
        prefix.push_str("\x1b[4m");
        suffix = format!("\x1b[24m{suffix}");
    }
    if options.inverse == Some(true) {
        prefix.push_str("\x1b[7m");
        suffix = format!("\x1b[27m{suffix}");
    }
    if options.strikethrough == Some(true) {
        prefix.push_str("\x1b[9m");
        suffix = format!("\x1b[29m{suffix}");
    }
    format!("{prefix}{text}{suffix}")
}
