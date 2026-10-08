//! Port of upstream `packages/tui/src/oklab.ts`: Oklab and OKHSL <-> sRGB
//! conversion. [`crate::tui::colors`] builds its OKLCH, OKHSL, and color
//! mixing on it.
//!
//! Oklab and OKHSL are Björn Ottosson's color spaces; OKHSL's saturation is
//! relative to the sRGB gamut at each hue and lightness. This is a port of his
//! reference implementation (https://bottosson.github.io/posts/colorpicker/),
//! Copyright (c) 2021 Björn Ottosson, used under the MIT license.
//!
//! Float operations mirror the JavaScript evaluation order exactly (f64
//! addition/multiplication are IEEE, `**` becomes `powf`, `Math.cbrt` becomes
//! `cbrt`); upstream parity is pinned bit-for-bit by the colors oracle.

use crate::tui::terminal_colors::RgbColor;

type Vector = [f64; 3];

fn multiply(m: &[[f64; 3]; 3], [x, y, z]: Vector) -> Vector {
    m.map(|row| row[0] * x + row[1] * y + row[2] * z)
}

// ============================================================================
// OKHSL <-> sRGB
// ============================================================================

#[allow(clippy::excessive_precision)]
const LINEAR_SRGB_TO_LMS: [[f64; 3]; 3] = [
    [0.4122214694707629, 0.5363325372617349, 0.0514459932675022],
    [0.2119034958178251, 0.6806995506452344, 0.1073969535369405],
    [0.0883024591900564, 0.2817188391361215, 0.6299787016738222],
];
#[allow(clippy::excessive_precision)]
const LMS_TO_LAB: [[f64; 3]; 3] = [
    [0.210454268309314, 0.793617774702305, -0.0040720430116193],
    [1.9779985324311684, -2.42859224204858, 0.450593709617411],
    [0.0259040424655478, 0.7827717124575296, -0.8086757549230774],
];
#[allow(clippy::excessive_precision)]
const LAB_TO_LMS: [[f64; 3]; 3] = [
    [1.0, 0.3963377773761749, 0.2158037573099136],
    [1.0, -0.1055613458156586, -0.0638541728258133],
    [1.0, -0.0894841775298119, -1.2914855480194092],
];
#[allow(clippy::excessive_precision)]
const LMS_TO_LINEAR_SRGB: [[f64; 3]; 3] = [
    [4.0767416360759583, -3.3077115392580629, 0.2309699031821043],
    [-1.2684379732850315, 2.6097573492876882, -0.341319376002657],
    [-0.0041960761386756, -0.7034186179359362, 1.7076146940746117],
];
/// Per sRGB channel (red, green, blue): the (a, b) half-plane where that
/// channel clips first, and the polynomial approximating the maximum
/// saturation there.
#[allow(clippy::excessive_precision)]
const SATURATION_FIT: [([f64; 2], [f64; 5]); 3] = [
    (
        [-1.8817031, -0.80936501],
        [1.19086277, 1.76576728, 0.59662641, 0.75515197, 0.56771245],
    ),
    (
        [1.8144408, -1.19445267],
        [0.73956515, -0.45954404, 0.08285427, 0.12541073, -0.14503204],
    ),
    (
        [0.13110758, 1.81333971],
        [1.35733652, -0.00915799, -1.1513021, -0.50559606, 0.00692167],
    ),
];
const K1: f64 = 0.206;
const K2: f64 = 0.03;
fn k3() -> f64 {
    (1.0 + K1) / (1.0 + K2)
}

/// Oklab lightness to OKHSL lightness.
pub fn oklab_to_okhsl_lightness(x: f64) -> f64 {
    let k3 = k3();
    0.5 * (k3 * x - K1 + ((k3 * x - K1) * (k3 * x - K1) + 4.0 * K2 * k3 * x).sqrt())
}
/// OKHSL lightness to Oklab lightness.
fn okhsl_to_oklab_lightness(x: f64) -> f64 {
    (x * x + K1 * x) / (k3() * (x + K2))
}

/// sRGB transfer function: linear to encoded channel, both 0-1.
fn linear_to_srgb(value: f64) -> f64 {
    if value > 0.0031308 {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    } else {
        12.92 * value
    }
}
/// Inverse sRGB transfer function: encoded to linear channel, both 0-1.
fn srgb_to_linear(value: f64) -> f64 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

/// Oklab [L, a, b] to linear sRGB [r, g, b] (0-1, may leave the gamut).
pub fn oklab_to_linear_srgb(lab: Vector) -> Vector {
    let lms: Vector = multiply(&LAB_TO_LMS, lab).map(|value| value.powf(3.0));
    multiply(&LMS_TO_LINEAR_SRGB, lms)
}

/// V8's `Math.cbrt` is the fdlibm implementation; `libm`'s `cbrt` is a
/// faithful port of the same algorithm, so the results are bit-identical to
/// node. This pins the OKHSL saturation curve exactly, including the
/// raw-channel floats the system-theme solver chains through further
/// conversions (the colors oracle's raw-channel tolerance seam closes).
fn js_cbrt(x: f64) -> f64 {
    libm::cbrt(x)
}

/// Linear sRGB [r, g, b] (0-1) to Oklab [L, a, b].
fn linear_srgb_to_oklab(rgb: Vector) -> Vector {
    let lms: Vector = multiply(&LINEAR_SRGB_TO_LMS, rgb).map(js_cbrt);
    multiply(&LMS_TO_LAB, lms)
}

/// sRGB channels (0-255) to Oklab [L, a, b]. Channels are JS numbers upstream;
/// fractional input is preserved.
pub fn rgb_to_oklab(rgb: [f64; 3]) -> Vector {
    let linear: Vector = [rgb[0] / 255.0, rgb[1] / 255.0, rgb[2] / 255.0].map(srgb_to_linear);
    linear_srgb_to_oklab(linear)
}

/// Linear sRGB [r, g, b] to sRGB channels (0-255, rounded), clipping
/// out-of-gamut channels.
pub fn linear_srgb_to_rgb(linear: Vector) -> RgbColor {
    let [r, g, b] =
        linear.map(|value| (linear_to_srgb(value).clamp(0.0, 1.0) * 255.0).round() as u8);
    RgbColor { r, g, b }
}

/// Rate of change of each cube-root LMS component along a chroma direction
/// (a, b).
fn lms_slopes(a: f64, b: f64) -> Vector {
    [
        LAB_TO_LMS[0][1] * a + LAB_TO_LMS[0][2] * b,
        LAB_TO_LMS[1][1] * a + LAB_TO_LMS[1][2] * b,
        LAB_TO_LMS[2][1] * a + LAB_TO_LMS[2][2] * b,
    ]
}

/// Largest saturation (C/L) inside sRGB for hue (a, b): polynomial fit plus
/// one Halley step.
fn max_saturation(a: f64, b: f64) -> f64 {
    // JS `findIndex`: the third row is the fallback when no half-plane matches.
    let channel = SATURATION_FIT
        .iter()
        .enumerate()
        .find(|(index, (half_plane, _))| *index == 2 || half_plane[0] * a + half_plane[1] * b > 1.0)
        .map(|(index, _)| index)
        .unwrap_or(2);
    let [k0, k1, k2, k3, k4] = SATURATION_FIT[channel].1;
    let weights = &LMS_TO_LINEAR_SRGB[channel];
    let saturation = k0 + k1 * a + k2 * b + k3 * a * a + k4 * a * b;

    let slopes = lms_slopes(a, b);
    let base: Vector = slopes.map(|k| 1.0 + saturation * k);
    let dot =
        |values: Vector| weights[0] * values[0] + weights[1] * values[1] + weights[2] * values[2];
    let f = dot(base.map(|value| value.powf(3.0)));
    // JS `3 * slopes[i] * value ** 2` / `6 * slopes[i] ** 2 * value`: `**`
    // binds tighter, so the squared term is multiplied as one operand.
    let f1 = dot([
        3.0 * slopes[0] * (base[0] * base[0]),
        3.0 * slopes[1] * (base[1] * base[1]),
        3.0 * slopes[2] * (base[2] * base[2]),
    ]);
    let f2 = dot([
        6.0 * (slopes[0] * slopes[0]) * base[0],
        6.0 * (slopes[1] * slopes[1]) * base[1],
        6.0 * (slopes[2] * slopes[2]) * base[2],
    ]);
    saturation - (f * f1) / (f1 * f1 - 0.5 * f * f2)
}

/// Oklab lightness and chroma of the most saturated sRGB color of hue (a, b).
fn cusp(a: f64, b: f64) -> (f64, f64) {
    let saturation = max_saturation(a, b);
    let linear = oklab_to_linear_srgb([1.0, saturation * a, saturation * b]);
    // JS Math.max(...vector) propagates NaN; Rust's f64::max ignores it. The
    // input is finite for every reachable call, so the JS fold order (left to
    // right) is reproduced directly.
    let max = linear[0].max(linear[1]).max(linear[2]);
    let lightness = js_cbrt(1.0 / max);
    (lightness, lightness * saturation)
}

/// Chroma where the constant-lightness line at `lightness` leaves the sRGB
/// gamut.
fn max_chroma(a: f64, b: f64, lightness: f64, cusp: (f64, f64)) -> f64 {
    let (cusp_l, cusp_c) = cusp;
    if lightness <= cusp_l {
        return (cusp_c * lightness) / cusp_l;
    }
    // Upper half: triangle edge, then one Halley step against each channel
    // reaching 1.
    let t = (cusp_c * (lightness - 1.0)) / (cusp_l - 1.0);
    let slopes = lms_slopes(a, b);
    let lms: Vector = slopes.map(|k| lightness + t * k);
    let cubes: Vector = lms.map(|value| value.powf(3.0));
    let first: Vector = [
        3.0 * slopes[0] * (lms[0] * lms[0]),
        3.0 * slopes[1] * (lms[1] * lms[1]),
        3.0 * slopes[2] * (lms[2] * lms[2]),
    ];
    let second: Vector = [
        6.0 * (slopes[0] * slopes[0]) * lms[0],
        6.0 * (slopes[1] * slopes[1]) * lms[1],
        6.0 * (slopes[2] * slopes[2]) * lms[2],
    ];
    let dot = |row: &[f64; 3], values: Vector| {
        row[0] * values[0] + row[1] * values[1] + row[2] * values[2]
    };
    let steps: Vector = LMS_TO_LINEAR_SRGB.map(|row| {
        let f = dot(&row, cubes) - 1.0;
        let f1 = dot(&row, first);
        let f2 = dot(&row, second);
        let u = f1 / (f1 * f1 - 0.5 * f * f2);
        if u >= 0.0 {
            -f * u
        } else {
            f64::MAX
        }
    });
    // JS Math.min(...steps) left-to-right fold; NaN only from 0 * Infinity,
    // which requires f = 0 with u = +Infinity (unreachable for finite input).
    t + steps[0].min(steps[1]).min(steps[2])
}

/// OKHSL's chroma reference points at lightness L and hue (a, b):
/// [c0, cMid, cMax].
fn chroma_stops(l: f64, a: f64, b: f64) -> (f64, f64, f64) {
    let peak = cusp(a, b);
    let c_max = max_chroma(a, b, l, peak);
    let k = c_max / (l * (peak.1 / peak.0)).min((1.0 - l) * (peak.1 / (1.0 - peak.0)));
    let mid_s = 0.11516993
        + 1.0
            / (7.4477897
                + 4.1590124 * b
                + a * (-2.19557347
                    + 1.75198401 * b
                    + a * (-2.13704948 - 10.02301043 * b
                        + a * (-4.24894561 + 5.38770819 * b + 4.69891013 * a))));
    let mid_t = 0.11239642
        + 1.0
            / (1.6132032 - 0.68124379 * b
                + a * (0.40370612
                    + 0.90148123 * b
                    + a * (-0.27087943
                        + 0.6122399 * b
                        + a * (0.00299215 - 0.45399568 * b - 0.14661872 * a))));
    // JS `(x) ** 4` / `(x) ** 2`: pow with an integer exponent; x*x is exact
    // for exponent 2, powf(4.0) mirrors the general path for exponent 4.
    let c_mid = 0.9
        * k
        * (1.0 / (1.0 / (l * mid_s).powf(4.0) + 1.0 / ((1.0 - l) * mid_t).powf(4.0)))
            .sqrt()
            .sqrt();
    let c0 = (1.0
        / (1.0 / ((l * 0.4) * (l * 0.4)) + 1.0 / (((1.0 - l) * 0.8) * ((1.0 - l) * 0.8))))
        .sqrt();
    (c0, c_mid, c_max)
}

/// Convert OKHSL to sRGB channels (0-255, rounded), clipping out-of-gamut
/// channels.
///
/// * `hue` - Hue in degrees.
/// * `saturation` - Saturation, 0-1.
/// * `lightness` - Lightness, 0-1.
pub fn okhsl_to_rgb(hue: f64, saturation: f64, lightness: f64) -> RgbColor {
    let [r, g, b] = okhsl_to_rgb_f64(hue, saturation, lightness);
    RgbColor {
        r: r.round() as u8,
        g: g.round() as u8,
        b: b.round() as u8,
    }
}

/// The unrounded float pipeline behind [`okhsl_to_rgb`]: upstream
/// `linearSrgbToRgb` returns 0-255 floats and consumers round at display
/// time. The system-theme solver's lightness binary searches run on these
/// floats, so the rounding cannot happen inside `okhslColor`.
pub fn okhsl_to_rgb_f64(hue: f64, saturation: f64, lightness: f64) -> [f64; 3] {
    let l = okhsl_to_oklab_lightness(lightness);
    let mut lab: Vector = [l, 0.0, 0.0];
    if l > 0.0 && l < 1.0 && saturation > 0.0 {
        let angle = (2.0 * std::f64::consts::PI * ((hue % 360.0 + 360.0) % 360.0)) / 360.0;
        let a = angle.cos();
        let b = angle.sin();
        let (c0, c_mid, c_max) = chroma_stops(l, a, b);
        // Chroma rises from 0 through cMid at s = 0.8 to cMax at s = 1.
        let chroma = if saturation < 0.8 {
            let t = 1.25 * saturation;
            let k1 = 0.8 * c0;
            (t * k1) / (1.0 - (1.0 - k1 / c_mid) * t)
        } else {
            let t = 5.0 * (saturation - 0.8);
            let k1 = (0.2 * (c_mid * c_mid) * (1.25 * 1.25)) / c0;
            c_mid + (t * k1) / (1.0 - (1.0 - k1 / (c_max - c_mid)) * t)
        };
        lab = [l, chroma * a, chroma * b];
    }
    oklab_to_linear_srgb(lab).map(|value| linear_to_srgb(value).clamp(0.0, 1.0) * 255.0)
}

/// Convert sRGB channels (0-255) to OKHSL. Channels are JS numbers upstream.
///
/// Returns hue `h` in degrees (0 for grays), saturation `s` and lightness `l`
/// 0-1.
pub fn rgb_to_okhsl(rgb: [f64; 3]) -> [f64; 3] {
    let [l, lab_a, lab_b] = rgb_to_oklab(rgb);
    let chroma = lab_a.hypot(lab_b);
    let lightness = oklab_to_okhsl_lightness(l);
    if chroma < 1e-9 || lightness <= 0.0 || lightness >= 1.0 {
        return [0.0, 0.0, lightness];
    }

    let hue = ((lab_b.atan2(lab_a) * 180.0) / std::f64::consts::PI + 360.0) % 360.0;
    let (c0, c_mid, c_max) = chroma_stops(l, lab_a / chroma, lab_b / chroma);
    let saturation = if chroma < c_mid {
        let k1 = 0.8 * c0;
        0.8 * (chroma / (k1 + (1.0 - k1 / c_mid) * chroma))
    } else {
        let k1 = (0.2 * (c_mid * c_mid) * (1.25 * 1.25)) / c0;
        let offset = chroma - c_mid;
        0.8 + 0.2 * (offset / (k1 + (1.0 - k1 / (c_max - c_mid)) * offset))
    };
    [hue, saturation.clamp(0.0, 1.0), lightness]
}
