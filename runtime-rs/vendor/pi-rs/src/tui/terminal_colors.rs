//! Port of upstream `packages/tui/src/terminal-colors.ts`: parsing of OSC 11
//! background-color replies and DA-color-scheme reports.

/// Upstream `RgbColor`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RgbColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// Upstream `TerminalColorScheme`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalColorScheme {
    Dark,
    Light,
}

/// Colors the terminal reports for its current theme.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TerminalColors {
    /// Default foreground (OSC 10).
    pub foreground: Option<RgbColor>,
    /// Default background (OSC 11).
    pub background: Option<RgbColor>,
    /// ANSI colors 0-15 (OSC 4). Only set when the terminal reported all 16.
    pub palette: Option<Vec<RgbColor>>,
}

fn hex_to_rgb(hex: &str) -> RgbColor {
    let normalized = hex.strip_prefix('#').unwrap_or(hex);
    // Upstream uses `parseInt(slice, 16)` on fixed 2-char slices. Inputs here
    // are pre-validated, so parsing cannot fail on a well-formed call.
    let channel =
        |range: std::ops::Range<usize>| u8::from_str_radix(&normalized[range], 16).unwrap_or(0);
    RgbColor {
        r: channel(0..2),
        g: channel(2..4),
        b: channel(4..6),
    }
}

/// Upstream `parseOscHexChannel`: scales an arbitrary-length hex channel to
/// 0..=255. Returns `None` for non-hex input or an empty channel.
fn parse_osc_hex_channel(channel: &str) -> Option<u8> {
    if channel.is_empty() || !channel.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let max = 16u64.pow(channel.len() as u32) - 1;
    if max == 0 {
        return None;
    }
    let value = u64::from_str_radix(channel, 16).ok()?;
    Some(((value as f64 / max as f64) * 255.0).round() as u8)
}

/// What an OSC color reply reports: the default foreground (OSC 10), the
/// default background (OSC 11), or a palette index (OSC 4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OscColorTarget {
    Foreground,
    Background,
    Index(u16),
}

impl OscColorTarget {
    /// Upstream reply-map key (`String(target)`).
    pub(crate) fn reply_key(&self) -> String {
        match self {
            Self::Foreground => "foreground".to_string(),
            Self::Background => "background".to_string(),
            Self::Index(index) => index.to_string(),
        }
    }
}

fn osc_color_response_value(data: &str) -> Option<(OscColorTarget, &str)> {
    // ^\x1b\](?:(1[01])|4;(\d{1,3}));([^\x07\x1b]*)(?:\x07|\x1b\\)$ — the /i
    // flag only affects letters, and the pattern has none outside classes.
    let body = data.strip_prefix("\x1b]")?;
    let (target, value) = if let Some(rest) = body.strip_prefix("10;") {
        (OscColorTarget::Foreground, rest)
    } else if let Some(rest) = body.strip_prefix("11;") {
        (OscColorTarget::Background, rest)
    } else {
        let rest = body.strip_prefix("4;")?;
        let digits_end = rest
            .bytes()
            .take_while(|b| b.is_ascii_digit())
            .count()
            .max(1);
        if digits_end > 3 || rest.as_bytes().get(digits_end) != Some(&b';') {
            return None;
        }
        let index: u16 = rest[..digits_end].parse().ok()?;
        (OscColorTarget::Index(index), &rest[digits_end + 1..])
    };
    let value = value
        .strip_suffix('\x07')
        .or_else(|| value.strip_suffix("\x1b\\"))?;
    if value.contains('\x07') || value.contains('\x1b') {
        return None;
    }
    Some((target, value))
}

/// Upstream `parseOscColorResponse`: parse an OSC 10, 11, or 4 color reply.
/// Returns `None` when `data` is not such a reply; the RGB is `None` when it
/// is a reply with an unparseable color.
pub fn parse_osc_color_response(data: &str) -> Option<(OscColorTarget, Option<RgbColor>)> {
    let (target, value) = osc_color_response_value(data)?;
    Some((target, parse_osc_color_value(value)))
}

/// Upstream `parseOscColorValue` (the shared value parser of the legacy OSC 11
/// helper, kept until the `coding_agent` theme detector migrates).
pub fn parse_osc_color_value(raw_value: &str) -> Option<RgbColor> {
    let value = crate::tui::utils::js_trim(raw_value);

    if let Some(hex) = value.strip_prefix('#') {
        if hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Some(hex_to_rgb(value));
        }
        if hex.len() == 12 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
            let r = parse_osc_hex_channel(&hex[0..4])?;
            let g = parse_osc_hex_channel(&hex[4..8])?;
            let b = parse_osc_hex_channel(&hex[8..12])?;
            return Some(RgbColor { r, g, b });
        }
        return None;
    }

    let rgb_value = match value.split_once(':') {
        // Upstream strips a leading `rgb:`/`rgba?:` prefix case-insensitively.
        Some((scheme, rest))
            if scheme.eq_ignore_ascii_case("rgb") || scheme.eq_ignore_ascii_case("rgba") =>
        {
            rest
        }
        _ => value,
    };
    let mut channels = rgb_value.split('/');
    let red = channels.next()?;
    let green = channels.next()?;
    let blue = channels.next()?;
    let r = parse_osc_hex_channel(red)?;
    let g = parse_osc_hex_channel(green)?;
    let b = parse_osc_hex_channel(blue)?;
    Some(RgbColor { r, g, b })
}

/// Upstream `parseTerminalColorSchemeReport`: `^(?:\x1b\[\?997;(1|2)n)+$` with
/// the capture of the last repetition.
pub fn parse_terminal_color_scheme_report(data: &str) -> Option<TerminalColorScheme> {
    fn parse_one(rest: &str) -> Option<(TerminalColorScheme, &str)> {
        let after_prefix = rest.strip_prefix("\x1b[?997;")?;
        let kind = *after_prefix.as_bytes().first()?;
        let scheme = match kind {
            b'1' => TerminalColorScheme::Dark,
            b'2' => TerminalColorScheme::Light,
            _ => return None,
        };
        let after_kind = after_prefix.strip_prefix(std::str::from_utf8(&[kind]).ok()?)?;
        let after_suffix = after_kind.strip_prefix('n')?;
        Some((scheme, after_suffix))
    }

    let (mut result, mut rest) = parse_one(data)?;
    while let Some((scheme, remaining)) = parse_one(rest) {
        result = scheme;
        rest = remaining;
    }
    if rest.is_empty() {
        Some(result)
    } else {
        None
    }
}
