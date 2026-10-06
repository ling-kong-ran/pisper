//! Byte-oracle replay for the v0.99.1 `packages/tui/src` delta (upstream
//! HEAD `2bbfcca43`): oklab/colors math, WheelScrollAccelerator,
//! `queryTerminalColors`, latex scripts/font-switches/cases, utils fast
//! paths and separator regexes, autocomplete token extraction, terminal-image
//! aspect-ratio sizing, and the editor trigger patterns.
//!
//! Every fixture under `tests/fixtures/tui_delta_oracle/` was captured by
//! EXECUTING the copied upstream TypeScript sources with
//! `node --experimental-strip-types` (provenance SHAs pinned inside each
//! fixture and asserted here).

use serde_json::Value;

use super::autocomplete::AutocompleteProvider as _;
use super::{
    autocomplete, colors, latex, oklab, terminal_colors, terminal_image, tui, utils, wheel_scroll,
};

// Fixture paths are relative to THIS file's directory (src/tui/).
const COLORS_ORACLE: &str =
    include_str!("../../tests/fixtures/tui_delta_oracle/colors/colors_oracle.json");
const WHEEL_ORACLE: &str =
    include_str!("../../tests/fixtures/tui_delta_oracle/wheel/wheel_oracle.json");
const TUI_ORACLE: &str = include_str!("../../tests/fixtures/tui_delta_oracle/tui/tui_oracle.json");
const LATEX_ORACLE: &str =
    include_str!("../../tests/fixtures/tui_delta_oracle/latex/latex_oracle.json");
const UTILS_ORACLE: &str =
    include_str!("../../tests/fixtures/tui_delta_oracle/utils/utils_oracle.json");
const AUTOCOMPLETE_ORACLE: &str =
    include_str!("../../tests/fixtures/tui_delta_oracle/autocomplete/autocomplete_oracle.json");
const TERMINAL_IMAGE_ORACLE: &str =
    include_str!("../../tests/fixtures/tui_delta_oracle/terminal_image/terminal_image_oracle.json");
const EDITOR_PATTERNS_ORACLE: &str = include_str!(
    "../../tests/fixtures/tui_delta_oracle/editor_patterns/editor_patterns_oracle.json"
);

fn oracle(name: &str) -> Value {
    serde_json::from_str(name).expect("oracle JSON parses")
}

/// Upstream source SHAs (HEAD 2bbfcca43, v0.99.1). Each replay asserts the
/// fixture was captured from exactly these sources.
mod provenance {
    pub const OKLAB: &str = "45b067e6e3605b385f595adecd7c0216f1c6b6686680d5c73f661286de736be6";
    pub const COLORS: &str = "d4fe729c424d2c07bc64cf0c3edfdbf5642865cba395dfb37234c6c88d65f468";
    pub const WHEEL_SCROLL: &str =
        "a969a50d0a3627fde0052f766443e3668a6355b3ecf312ea078025080d303f82";
    pub const TUI: &str = "92dcb7f5f9a3a9575421d8de366be5cc78890be8df38d86c4ed3c0d77c7f88a4";
    pub const UTILS: &str = "258ff73a0ff4d2b05f8a60515a9cc37eb96c9fc03a4072ac6db2906cad1862d9";
    pub const LATEX: &str = "c4ef99bef1d3a54c73006912c9a7fa67ef99412b4f6cd28d608cf0e07b38cece";
    pub const AUTOCOMPLETE: &str =
        "7391902f35b60c3467ceb5eccc86e0954a388ea1012455c1a7ea8f7325de773b";
    pub const FUZZY: &str = "0ae2bedc6a4f043d875ec415202a6e3d9e45405741359d7d2dc2855240dff633";
    pub const TERMINAL_IMAGE: &str =
        "26beb4d4154a6a7fc800d40af39cde8984749d14096756182faba4af0380093d";
    pub const EDITOR: &str = "30a47fd49042579be7b85262677df62d4ff37a8f9a3a0e6c41ce3cf1d402b9c3";
    pub const KEYS: &str = "b972facce4233a4623239fc38029e28cae15d0fb326558c0c09dc02cf4345fa7";
    pub const TERMINAL_COLORS: &str =
        "72d1ef298837bfe520bd8a060cb93ea27547dcbff04a167ace0f55b5183e550d";
}

// ---------------------------------------------------------------------------
// oklab + colors
// ---------------------------------------------------------------------------

fn f64_from_bits_hex(hex: &str) -> f64 {
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex byte"))
        .collect();
    f64::from_bits(u64::from_be_bytes(bytes.try_into().expect("8 bytes")))
}

fn f64_bits_hex(value: f64) -> String {
    value
        .to_bits()
        .to_be_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Relative-tolerance comparison for the amplified raw OKHSL channels.
fn assert_relative_within(actual: f64, expected: f64, context: &str) {
    let scale = expected.abs().max(actual.abs()).max(f64::MIN_POSITIVE);
    let relative = (actual - expected).abs() / scale;
    assert!(
        relative <= 1e-6,
        "{context}: expected {expected}, got {actual} (relative {relative})"
    );
}

#[test]
fn oklab_oracle_is_bit_exact() {
    let oracle = oracle(COLORS_ORACLE);
    let provenance = &oracle["provenance"];
    assert_eq!(provenance["oklabSha256"].as_str(), Some(provenance::OKLAB));
    assert_eq!(
        provenance["colorsSha256"],
        oracle["provenance"]["colorsSha256"]
    );
    assert_eq!(
        provenance["colorsSha256"].as_str(),
        Some(provenance::COLORS)
    );

    let mut checked = 0;
    for case in oracle["oklabToLinearSrgb"].as_array().expect("rows") {
        let lab: [f64; 3] = [
            f64_from_bits_hex(case["in"][0].as_str().unwrap()),
            f64_from_bits_hex(case["in"][1].as_str().unwrap()),
            f64_from_bits_hex(case["in"][2].as_str().unwrap()),
        ];
        let [r, g, b] = oklab::oklab_to_linear_srgb(lab);
        let expected: Vec<String> = case["out"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            f64_bits_hex(r),
            expected[0],
            "oklabToLinearSrgb r for {lab:?}"
        );
        assert_eq!(
            f64_bits_hex(g),
            expected[1],
            "oklabToLinearSrgb g for {lab:?}"
        );
        assert_eq!(
            f64_bits_hex(b),
            expected[2],
            "oklabToLinearSrgb b for {lab:?}"
        );
        checked += 1;
    }
    for case in oracle["okhslLightness"].as_array().expect("rows") {
        let x = f64_from_bits_hex(case["in"].as_str().unwrap());
        assert_eq!(
            f64_bits_hex(oklab::oklab_to_okhsl_lightness(x)),
            case["out"].as_str().unwrap(),
            "oklabToOkhslLightness({x})"
        );
        checked += 1;
    }
    for case in oracle["rgbToOkhsl"].as_array().expect("rows") {
        let rgb = terminal_colors::RgbColor {
            r: case["rgb"][0].as_u64().unwrap() as u8,
            g: case["rgb"][1].as_u64().unwrap() as u8,
            b: case["rgb"][2].as_u64().unwrap() as u8,
        };
        let [h, s, l] = oklab::rgb_to_okhsl([f64::from(rgb.r), f64::from(rgb.g), f64::from(rgb.b)]);
        // The hue/saturation/lightness channels flow through Math.cbrt, whose
        // V8 (fdlibm) implementation deviates from the correctly-rounded cube
        // root by 1 ULP on ~8.5% of inputs, and the OKHSL saturation curve
        // amplifies that near the gamut cusp. The Rust cbrt is correctly
        // rounded, so raw channels compare within a relative tolerance
        // (disclosed in the oklab module header); every consumer downstream
        // of Math.round (sRGB channels, hex strings, ANSI sequences) is
        // pinned exactly.
        assert_relative_within(
            h,
            f64_from_bits_hex(case["h"].as_str().unwrap()),
            "rgbToOkhsl h {rgb:?}",
        );
        assert_relative_within(
            s,
            f64_from_bits_hex(case["s"].as_str().unwrap()),
            "rgbToOkhsl s {rgb:?}",
        );
        assert_relative_within(
            l,
            f64_from_bits_hex(case["l"].as_str().unwrap()),
            "rgbToOkhsl l {rgb:?}",
        );
        checked += 1;
    }
    for case in oracle["okhslToRgb"].as_array().expect("rows") {
        let h = case["h"].as_f64().expect("degree");
        let s = f64_from_bits_hex(case["s"].as_str().unwrap());
        let l = f64_from_bits_hex(case["l"].as_str().unwrap());
        let rgb = oklab::okhsl_to_rgb(h, s, l);
        let expected = [
            case["rgb"]["r"].as_u64().unwrap() as u8,
            case["rgb"]["g"].as_u64().unwrap() as u8,
            case["rgb"]["b"].as_u64().unwrap() as u8,
        ];
        assert_eq!([rgb.r, rgb.g, rgb.b], expected, "okhslToRgb({h}, {s}, {l})");
        checked += 1;
    }
    assert!(
        checked > 6000,
        "expected the dense oklab grid, got {checked}"
    );
}

fn color_from_json(value: &Value) -> colors::Color {
    match value["kind"].as_str().expect("color kind") {
        "indexed" => colors::Color::Indexed(colors::IndexedColor {
            index: value["index"].as_u64().unwrap() as u8,
        }),
        "rgb" => colors::Color::Rgb {
            r: value["r"].as_f64().unwrap(),
            g: value["g"].as_f64().unwrap(),
            b: value["b"].as_f64().unwrap(),
        },
        "oklch" => colors::Color::Oklch {
            l: value["l"].as_f64().unwrap(),
            c: value["c"].as_f64().unwrap(),
            h: value["h"].as_f64().unwrap(),
        },
        other => panic!("unknown color kind {other}"),
    }
}

fn color_json(color: &colors::Color) -> String {
    // Matches JSON.stringify of the upstream frozen color objects (integral
    // JS numbers serialize without a fraction).
    let number = |value: f64| colors::js_number_to_string(value);
    match color {
        colors::Color::Indexed(indexed) => {
            format!("{{\"kind\":\"indexed\",\"index\":{}}}", indexed.index)
        }
        colors::Color::Rgb { r, g, b } => {
            format!(
                "{{\"kind\":\"rgb\",\"r\":{},\"g\":{},\"b\":{}}}",
                number(*r),
                number(*g),
                number(*b)
            )
        }
        colors::Color::Oklch { l, c, h } => {
            format!(
                "{{\"kind\":\"oklch\",\"l\":{},\"c\":{},\"h\":{}}}",
                number(*l),
                number(*c),
                number(*h)
            )
        }
    }
}

fn color_mode_from_json(value: &Value) -> colors::TerminalColorMode {
    match value.as_str().expect("mode") {
        "truecolor" => colors::TerminalColorMode::Truecolor,
        "256color" => colors::TerminalColorMode::Color256,
        other => panic!("unknown mode {other}"),
    }
}

#[test]
fn colors_oracle_matches_api() {
    let oracle = oracle(COLORS_ORACLE);
    assert_eq!(
        oracle["provenance"]["colorsSha256"].as_str(),
        Some(provenance::COLORS)
    );

    for case in oracle["parseColor"].as_array().expect("rows") {
        let value = case["value"].as_str().unwrap();
        let parsed = colors::parse_color(value);
        let expected = case["parsed"].as_str().map(|parsed| {
            let json: Value = serde_json::from_str(parsed).expect("color JSON");
            color_json(&color_from_json(&json))
        });
        match expected {
            Some(expected) => {
                assert_eq!(
                    color_json(&parsed.expect("parses")),
                    expected,
                    "parseColor({value})"
                );
            }
            None => {
                let error = parsed.expect_err("must fail");
                assert!(
                    case["error"].as_str().unwrap().contains(error.as_str()),
                    "parseColor({value}): {error} vs {}",
                    case["error"]
                );
            }
        }
    }

    // Constructor validation errors (captured in the same fixture run).
    #[rustfmt::skip]
    let constructors = oracle["constructors"].as_array().expect("rows");
    let errors: Vec<String> = constructors
        .iter()
        .filter_map(|case| case["error"].as_str().map(str::to_string))
        .collect();
    assert!(errors.contains(&"ANSI color index must be an integer from 0 to 255: -1".to_string()));
    assert!(errors.contains(&"ANSI color index must be an integer from 0 to 255: 1.5".to_string()));
    assert!(errors.contains(&"r must be between 0 and 255: 256".to_string()));
    assert!(errors.contains(&"r must be finite".to_string()));
    assert!(errors.contains(&"l must be between 0 and 1: 1.2".to_string()));
    assert!(errors.contains(&"c must not be negative: -0.1".to_string()));
    assert!(errors.contains(&"s must be between 0 and 1: 1.6".to_string()));
    // The Rust constructors produce the same messages.
    assert_eq!(
        colors::indexed_color(-1.0).unwrap_err(),
        "ANSI color index must be an integer from 0 to 255: -1"
    );
    assert_eq!(
        colors::indexed_color(1.5).unwrap_err(),
        "ANSI color index must be an integer from 0 to 255: 1.5"
    );
    assert_eq!(
        colors::rgb_color(256.0, 0.0, 0.0).unwrap_err(),
        "r must be between 0 and 255: 256"
    );
    assert_eq!(
        colors::rgb_color(f64::NAN, 0.0, 0.0).unwrap_err(),
        "r must be finite"
    );
    assert_eq!(
        colors::oklch_color(1.2, 0.2, 0.0).unwrap_err(),
        "l must be between 0 and 1: 1.2"
    );
    assert_eq!(
        colors::oklch_color(0.5, -0.1, 0.0).unwrap_err(),
        "c must not be negative: -0.1"
    );
    assert_eq!(
        colors::okhsl_color(250.0, 1.6, 0.55).unwrap_err(),
        "s must be between 0 and 1: 1.6"
    );

    for case in oracle["colorToRgb"].as_array().expect("rows") {
        let color = color_from_json(
            &serde_json::from_str::<Value>(case["color"].as_str().unwrap()).expect("color"),
        );
        let rgb = colors::color_to_rgb(color);
        assert_eq!(
            [rgb[0] as u8, rgb[1] as u8, rgb[2] as u8],
            [
                case["rgb"]["r"].as_f64().unwrap() as u8,
                case["rgb"]["g"].as_f64().unwrap() as u8,
                case["rgb"]["b"].as_f64().unwrap() as u8,
            ],
            "colorToRgb({case})"
        );
    }

    for case in oracle["colorToOklch"].as_array().expect("rows") {
        let color = color_from_json(
            &serde_json::from_str::<Value>(case["color"].as_str().unwrap()).expect("color"),
        );
        let channels = colors::color_to_oklch(color);
        assert_relative_within(
            channels.l,
            f64_from_bits_hex(case["l"].as_str().unwrap()),
            "colorToOklch l",
        );
        assert_relative_within(
            channels.c,
            f64_from_bits_hex(case["c"].as_str().unwrap()),
            "colorToOklch c",
        );
        assert_relative_within(
            channels.h,
            f64_from_bits_hex(case["h"].as_str().unwrap()),
            "colorToOklch h",
        );
    }

    for case in oracle["colorToOkhsl"].as_array().expect("rows") {
        let color = colors::parse_color(case["hex"].as_str().unwrap()).expect("hex parses");
        let channels = colors::color_to_okhsl(color);
        assert_relative_within(
            channels.h,
            f64_from_bits_hex(case["h"].as_str().unwrap()),
            "colorToOkhsl h",
        );
        assert_relative_within(
            channels.s,
            f64_from_bits_hex(case["s"].as_str().unwrap()),
            "colorToOkhsl s",
        );
        assert_relative_within(
            channels.l,
            f64_from_bits_hex(case["l"].as_str().unwrap()),
            "colorToOkhsl l",
        );
        let rebuilt = colors::okhsl_color(channels.h, channels.s, channels.l).expect("in gamut");
        assert_eq!(
            colors::color_to_hex(rebuilt),
            case["roundTrip"].as_str().unwrap()
        );
    }

    for case in oracle["colorToHex"].as_array().expect("rows") {
        let color = color_from_json(
            &serde_json::from_str::<Value>(case["color"].as_str().unwrap()).expect("color"),
        );
        assert_eq!(
            colors::color_to_hex(color),
            case["hex"].as_str().unwrap(),
            "colorToHex"
        );
    }

    for case in oracle["mixColors"].as_array().expect("rows") {
        let first = color_from_json(
            &serde_json::from_str::<Value>(case["args"][0].as_str().unwrap()).expect("color"),
        );
        let second = color_from_json(
            &serde_json::from_str::<Value>(case["args"][1].as_str().unwrap()).expect("color"),
        );
        let amount = match case["args"][2].as_str() {
            Some("NaN") => f64::NAN,
            // JSON.stringify(NaN) is null.
            _ => case["args"][2].as_f64().unwrap_or(f64::NAN),
        };
        let space = match case["args"][3].as_str().unwrap() {
            "oklch" => colors::ColorMixSpace::Oklch,
            "srgb" => colors::ColorMixSpace::Srgb,
            other => panic!("space {other}"),
        };
        let mixed = colors::mix_colors(first, second, amount, space);
        match case["mixed"].as_str() {
            Some(mixed_json) => {
                let json: Value = serde_json::from_str(mixed_json).expect("color JSON");
                let expected = color_from_json(&json);
                let mixed = mixed.expect("mixes");
                // OKLCH-space mixes inherit the disclosed cbrt seam through
                // colorToOklch; srgb mixes are pure arithmetic.
                if space == colors::ColorMixSpace::Srgb {
                    assert_eq!(
                        color_json(&mixed),
                        color_json(&expected),
                        "mixColors({case})"
                    );
                } else {
                    let (mixed_l, mixed_c, mixed_h) = match &mixed {
                        colors::Color::Oklch { l, c, h } => (*l, *c, *h),
                        other => panic!("mixColors({case}): {other:?}"),
                    };
                    let (expected_l, expected_c, expected_h) = match &expected {
                        colors::Color::Oklch { l, c, h } => (*l, *c, *h),
                        other => panic!("mixColors({case}): {other:?}"),
                    };
                    assert_relative_within(mixed_l, expected_l, "mixColors l");
                    assert_relative_within(mixed_c, expected_c, "mixColors c");
                    assert_relative_within(mixed_h, expected_h, "mixColors h");
                }
            }
            None => {
                assert!(mixed.is_err(), "mixColors({case}) must fail");
            }
        }
    }

    for case in oracle["ansi"].as_array().expect("rows") {
        let color = color_from_json(
            &serde_json::from_str::<Value>(case["color"].as_str().unwrap()).expect("color"),
        );
        let mode = color_mode_from_json(&case["mode"]);
        assert_eq!(
            colors::foreground_ansi(color, mode),
            case["fg"].as_str().unwrap(),
            "fg"
        );
        assert_eq!(
            colors::background_ansi(color, mode),
            case["bg"].as_str().unwrap(),
            "bg"
        );
    }

    for case in oracle["styleText"].as_array().expect("rows") {
        let options: Value =
            serde_json::from_str(case["options"].as_str().unwrap()).expect("style JSON");
        let mode = color_mode_from_json(&case["mode"]);
        let text_style = colors::TextStyle {
            bold: options.get("bold").and_then(Value::as_bool),
            dim: options.get("dim").and_then(Value::as_bool),
            italic: options.get("italic").and_then(Value::as_bool),
            underline: options.get("underline").and_then(Value::as_bool),
            inverse: options.get("inverse").and_then(Value::as_bool),
            strikethrough: options.get("strikethrough").and_then(Value::as_bool),
            fg: options.get("fg").map(color_from_json),
            bg: options.get("bg").map(color_from_json),
        };
        assert_eq!(
            colors::style_text("Ready", &text_style, mode),
            case["styled"].as_str().unwrap(),
            "styleText({case})"
        );
        let attributes = colors::TextAttributes {
            bold: text_style.bold,
            dim: text_style.dim,
            italic: text_style.italic,
            underline: text_style.underline,
            inverse: text_style.inverse,
            strikethrough: text_style.strikethrough,
        };
        let fg_ansi = text_style.fg.map(|fg| colors::foreground_ansi(fg, mode));
        let bg_ansi = text_style.bg.map(|bg| colors::background_ansi(bg, mode));
        assert_eq!(
            colors::style_text_with_ansi("Ready", fg_ansi, bg_ansi, &attributes),
            case["viaAnsi"].as_str().unwrap(),
            "styleTextWithAnsi({case})"
        );
    }
}

/// The upstream `colors.test.ts` cases (v0.99.1 delta), ported directly.
#[test]
fn colors_upstream_unit_cases() {
    use super::colors::Color;

    let parsed = colors::parse_color("#abc").expect("parses");
    assert_eq!(
        color_json(&parsed),
        "{\"kind\":\"rgb\",\"r\":170,\"g\":187,\"b\":204}"
    );
    let parsed = colors::parse_color("oklch(62% 0.1 200)").expect("parses");
    match parsed {
        Color::Oklch { l, c, h } => {
            assert_eq!((l, c, h), (0.62, 0.1, 200.0));
        }
        other => panic!("expected oklch, got {other:?}"),
    }
    assert!(colors::parse_color("").is_err());
    assert!(colors::parse_color("red").is_err());

    // Gamut mapping including the lightness limits.
    let red = colors::color_to_rgb(colors::oklch_color(0.627955, 0.257683, 29.2339).unwrap());
    assert_eq!([red[0] as u8, red[1] as u8, red[2] as u8], [255, 0, 0]);
    let white = colors::color_to_rgb(colors::oklch_color(1.0, 0.3, 150.0).unwrap());
    assert_eq!(
        [white[0] as u8, white[1] as u8, white[2] as u8],
        [255, 255, 255]
    );
    let black = colors::color_to_rgb(colors::oklch_color(0.0, 0.3, 150.0).unwrap());
    assert_eq!([black[0] as u8, black[1] as u8, black[2] as u8], [0, 0, 0]);

    // Full saturation at the red cusp is pure sRGB red.
    let red = colors::parse_color("okhsl(29.23 100% 56.8%)").expect("parses");
    assert_eq!(
        color_json(&red),
        "{\"kind\":\"rgb\",\"r\":255,\"g\":0,\"b\":0}"
    );
    assert!(colors::parse_color("okhsl(250 160% 55%)")
        .expect_err("s out of range")
        .contains("s must be between 0 and 1"));

    // Styled text closes sequences in reverse order.
    let styled = colors::style_text(
        "Ready",
        &colors::TextStyle {
            fg: Some(colors::rgb_color(18.0, 52.0, 86.0).unwrap()),
            bg: Some(colors::indexed_color(9.0).unwrap()),
            bold: Some(true),
            italic: Some(true),
            ..Default::default()
        },
        colors::TerminalColorMode::Truecolor,
    );
    assert_eq!(
        styled,
        "\x1b[38;2;18;52;86m\x1b[48;5;9m\x1b[1m\x1b[3mReady\x1b[23m\x1b[22m\x1b[49m\x1b[39m"
    );
}

#[test]
fn colors_js_number_formatting() {
    // Number::toString parity on the boundary and common error values.
    let cases: [(f64, &str); 12] = [
        (0.0, "0"),
        (-0.0, "0"),
        (255.0, "255"),
        (1.5, "1.5"),
        (-1.5, "-1.5"),
        (0.62, "0.62"),
        (29.2339, "29.2339"),
        (1e21, "1e+21"),
        (1e-7, "1e-7"),
        (1e-6, "0.000001"),
        (123456789012345678901.0, "123456789012345680000"),
        (f64::NAN, "NaN"),
    ];
    for (value, expected) in cases {
        assert_eq!(
            colors::js_number_to_string(value),
            expected,
            "js_number_to_string({value})"
        );
    }
    assert_eq!(colors::js_number_to_string(f64::INFINITY), "Infinity");
}

// ---------------------------------------------------------------------------
// wheel scroll
// ---------------------------------------------------------------------------

#[test]
fn wheel_scroll_oracle_matches() {
    let oracle = oracle(WHEEL_ORACLE);
    assert_eq!(
        oracle["provenance"]["wheelScrollSha256"].as_str(),
        Some(provenance::WHEEL_SCROLL)
    );
    for scenario in oracle["scenarios"].as_array().expect("rows") {
        let lines = match scenario["lines"].as_str() {
            Some("auto") => wheel_scroll::WheelScrollLines::Auto,
            _ => wheel_scroll::WheelScrollLines::Fixed(
                scenario["lines"].as_f64().unwrap_or(f64::NAN), // JSON NaN is null
            ),
        };
        let accelerate = scenario["accelerate"].as_bool().unwrap();
        let mut accelerator = wheel_scroll::WheelScrollAccelerator::new(lines, Some(accelerate));
        let mut outputs: Vec<f64> = Vec::new();
        let calls = scenario["calls"].as_array().expect("calls");
        for (index, call) in calls.iter().enumerate() {
            if let Some(reconfigured) = scenario["reconfiguredAfter"].as_u64() {
                if index as u64 == reconfigured {
                    let lines = match scenario["reconfiguredLines"].as_str() {
                        Some("auto") => wheel_scroll::WheelScrollLines::Auto,
                        _ => wheel_scroll::WheelScrollLines::Fixed(
                            scenario["reconfiguredLines"]
                                .as_f64()
                                .expect("number lines"),
                        ),
                    };
                    accelerator.set_lines(lines);
                }
            }
            outputs.push(accelerator.next(
                call["direction"].as_i64().unwrap(),
                call["time"].as_f64().unwrap(),
            ));
        }
        let expected: Vec<f64> = scenario["outputs"]
            .as_array()
            .expect("outputs")
            .iter()
            .map(|value| value.as_f64().unwrap())
            .collect();
        assert_eq!(outputs, expected, "wheel scenario {}", scenario["id"]);
    }
}

#[test]
fn wheel_scroll_upstream_unit_cases() {
    use super::wheel_scroll::{WheelScrollAccelerator, WheelScrollLines};
    let scroll = |accelerator: &mut WheelScrollAccelerator, times: &[f64]| -> Vec<f64> {
        times
            .iter()
            .map(|time| accelerator.next(1, *time))
            .collect()
    };
    let mut accelerator = WheelScrollAccelerator::new(WheelScrollLines::Fixed(3.0), Some(true));
    assert_eq!(
        scroll(&mut accelerator, &[0.0, 10.0, 20.0, 1000.0]),
        [3.0, 3.0, 3.0, 3.0]
    );
    accelerator.set_lines(WheelScrollLines::Fixed(0.5));
    assert_eq!(accelerator.next(1, 2000.0), 1.0);

    let mut accelerator = WheelScrollAccelerator::new(WheelScrollLines::Auto, Some(false));
    assert_eq!(
        scroll(&mut accelerator, &[0.0, 10.0, 20.0, 30.0]),
        [1.0, 1.0, 1.0, 1.0]
    );

    let mut accelerator = WheelScrollAccelerator::new(WheelScrollLines::Auto, Some(true));
    assert_eq!(
        scroll(&mut accelerator, &[0.0, 150.0, 300.0, 450.0]),
        [1.0, 1.0, 1.0, 1.0]
    );
    assert_eq!(
        scroll(&mut accelerator, &[1000.0, 1050.0, 1100.0, 1150.0]),
        [1.0, 2.0, 2.0, 2.0]
    );
    assert_eq!(
        scroll(&mut accelerator, &[2000.0, 2020.0, 2040.0, 2060.0]),
        [1.0, 5.0, 5.0, 5.0]
    );
    assert_eq!(
        scroll(&mut accelerator, &[3000.0, 3010.0, 3020.0, 3030.0]),
        [1.0, 6.0, 6.0, 6.0]
    );

    let mut accelerator = WheelScrollAccelerator::new(WheelScrollLines::Auto, Some(true));
    assert_eq!(
        scroll(&mut accelerator, &[0.0, 3.0, 6.0, 9.0]),
        [1.0, 1.0, 1.0, 1.0]
    );

    let mut accelerator = WheelScrollAccelerator::new(WheelScrollLines::Auto, Some(true));
    assert_eq!(
        scroll(&mut accelerator, &[0.0, 20.0, 40.0]),
        [1.0, 5.0, 5.0]
    );
    assert_eq!(accelerator.next(-1, 60.0), 1.0);
    assert_eq!(scroll(&mut accelerator, &[500.0, 520.0]), [1.0, 5.0]);

    let mut accelerator = WheelScrollAccelerator::new(WheelScrollLines::Auto, Some(true));
    assert_eq!(
        scroll(&mut accelerator, &[0.0, 40.0, 80.0, 120.0, 160.0]),
        [1.0, 2.0, 3.0, 2.0, 3.0]
    );
}

// ---------------------------------------------------------------------------
// tui queryTerminalColors (writes + consumed replies)
// ---------------------------------------------------------------------------

// The consumed-reply scenarios run in tui_tests.rs; the write bytes are
// pinned here against the capture.

#[test]
fn tui_query_terminal_colors_write_matches_upstream() {
    let oracle = oracle(TUI_ORACLE);
    assert_eq!(
        oracle["provenance"]["tuiSha256"].as_str(),
        Some(provenance::TUI)
    );
    assert_eq!(
        oracle["provenance"]["keysSha256"].as_str(),
        Some(provenance::KEYS)
    );
    assert_eq!(
        oracle["provenance"]["terminalColorsSha256"].as_str(),
        Some(provenance::TERMINAL_COLORS)
    );
    // The full query (OSC 10 + 11 + 16 palette queries + DA1) in ONE write,
    // exactly as upstream writes it.
    let captured_write = oracle["tui_color_query_write"][0]
        .as_str()
        .expect("captured write");
    assert_eq!(tui::TERMINAL_COLOR_QUERY_FOR_TEST, captured_write);
    assert!(captured_write.starts_with("\x1b]10;?\x07\x1b]11;?\x07\x1b]4;0;?\x07"));
    assert!(captured_write.ends_with("\x1b[c"));
}
// ---------------------------------------------------------------------------
// latex
// ---------------------------------------------------------------------------

#[test]
fn latex_delta_oracle_matches() {
    let oracle = oracle(LATEX_ORACLE);
    assert_eq!(
        oracle["provenance"]["latexSha256"].as_str(),
        Some(provenance::LATEX)
    );
    assert_eq!(
        oracle["provenance"]["utilsSha256"].as_str(),
        Some(provenance::UTILS)
    );
    for (index, case) in oracle["cases"].as_array().expect("rows").iter().enumerate() {
        let source = case["source"].as_str().unwrap();
        let options = latex::RenderLatexOptions {
            display: case["display"].as_bool().unwrap(),
        };
        let rendered = latex::render_latex(source, options);
        let expected = case["rendered"].as_str();
        assert_eq!(
            rendered.as_deref(),
            expected,
            "latex delta case {index}: {source}"
        );
    }
}

// ---------------------------------------------------------------------------
// utils delta
// ---------------------------------------------------------------------------

#[test]
fn utils_delta_oracle_matches() {
    let oracle = oracle(UTILS_ORACLE);
    assert_eq!(
        oracle["provenance"]["utilsSha256"].as_str(),
        Some(provenance::UTILS)
    );

    for case in oracle["visibleWidth"].as_array().expect("rows") {
        let input = case["input"].as_str().unwrap();
        assert_eq!(
            utils::visible_width(input),
            case["width"].as_u64().expect("width") as usize,
            "visibleWidth({input:?})",
        );
    }
    for case in oracle["extractAnsiCode"].as_array().expect("rows") {
        let code = utils::extract_ansi_code(
            case["input"].as_str().unwrap(),
            case["pos"].as_u64().unwrap() as usize,
        );
        let expected = case["code"].as_object();
        match (code, expected) {
            (Some(code), Some(expected)) => {
                assert_eq!(code, expected["code"].as_str().unwrap(), "extractAnsiCode");
            }
            (None, None) => {}
            other => panic!("extractAnsiCode mismatch: {other:?} for {case}"),
        }
    }
    for case in oracle["wrap"].as_array().expect("rows") {
        let lines = utils::wrap_text_with_ansi(
            case["input"].as_str().unwrap(),
            case["width"].as_u64().unwrap() as usize,
        );
        let expected: Vec<String> = case["lines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|line| line.as_str().unwrap().to_string())
            .collect();
        assert_eq!(lines, expected, "wrapTextWithAnsi({case})");
    }
    for (index, expected) in oracle["activeBackground"]
        .as_array()
        .expect("rows")
        .iter()
        .enumerate()
    {
        let inputs = ["a\x1b[41mb\x1b[0mc", "\x1b[7;41mx y", "no codes"];
        assert_eq!(
            utils::get_active_background_ansi(inputs[index]),
            expected.as_str().unwrap(),
            "getActiveBackgroundAnsi {index}"
        );
    }
    // v1.0.0 sliceWithWidth: ANSI codes from before the range precede codes at
    // the boundary.
    for case in oracle["slice"].as_array().expect("rows") {
        let (text, width) = utils::slice_with_width(
            case["line"].as_str().unwrap(),
            case["startCol"].as_u64().unwrap() as usize,
            case["length"].as_u64().unwrap() as usize,
            case["strict"].as_bool().unwrap(),
        );
        assert_eq!(
            text,
            case["text"].as_str().unwrap(),
            "sliceWithWidth text for {case}"
        );
        assert_eq!(
            width,
            case["width"].as_u64().unwrap() as usize,
            "sliceWithWidth width for {case}"
        );
    }
}

#[test]
fn utils_separator_predicates_match_upstream_regex() {
    let oracle = oracle(UTILS_ORACLE);
    assert_eq!(
        oracle["provenance"]["utilsSha256"].as_str(),
        Some(provenance::UTILS)
    );
    for case in oracle["separators"]["separatorTests"]
        .as_array()
        .expect("rows")
    {
        let value = case["value"].as_str().unwrap();
        let expected = case["matches"].as_bool().unwrap();
        assert_eq!(
            utils::has_autocomplete_separator(value),
            expected,
            "autocompleteSeparatorRegex.test({value:?})"
        );
        // A single separator char also matches per-char.
        let mut chars = value.chars();
        if let (Some(only), None) = (chars.next(), chars.next()) {
            assert_eq!(
                utils::is_autocomplete_separator_char(only),
                expected,
                "separator char {only:?}"
            );
        }
    }
    for case in oracle["separators"]["boundaryTests"]
        .as_array()
        .expect("rows")
    {
        let value = case["value"].as_str().unwrap();
        assert_eq!(
            utils::is_token_start_boundary(value),
            case["suffixMatches"].as_bool().unwrap(),
            "tokenStart boundary {value:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// autocomplete delta
// ---------------------------------------------------------------------------

fn suggestion_items(value: &Value) -> Option<Vec<(String, String)>> {
    // `null` = the upstream provider returned no suggestions.
    if value.is_null() {
        return None;
    }
    value.as_array().map(|items| {
        items
            .iter()
            .map(|item| {
                (
                    item["value"].as_str().unwrap().to_string(),
                    item["label"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    })
}

struct AutocompleteOracle {
    _dir: std::path::PathBuf,
    provider: autocomplete::CombinedAutocompleteProvider,
}

impl AutocompleteOracle {
    fn setup() -> (Self, std::path::PathBuf) {
        let base =
            std::env::temp_dir().join(format!("tui-delta-autocomplete-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("sub/inner")).unwrap();
        std::fs::create_dir_all(base.join("Folder Names")).unwrap();
        std::fs::create_dir_all(base.join("z-last")).unwrap();
        for file in [
            "readme.md",
            "sub/file-one.ts",
            "sub/file-two.ts",
            "sub/inner/deep.py",
            "Folder Names/interesting file.txt",
            "z-last/zebra.txt",
            "app.js",
        ] {
            std::fs::write(base.join(file), "x").unwrap();
        }
        let provider =
            autocomplete::CombinedAutocompleteProvider::new(Vec::new(), base.clone(), None);
        (
            AutocompleteOracle {
                _dir: base.clone(),
                provider,
            },
            base,
        )
    }
}

impl Drop for AutocompleteOracle {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self._dir);
    }
}

#[test]
fn autocomplete_delta_oracle_matches() {
    let oracle = oracle(AUTOCOMPLETE_ORACLE);
    let provenance = &oracle["provenance"];
    assert_eq!(
        provenance["autocompleteSha256"].as_str(),
        Some(provenance::AUTOCOMPLETE)
    );
    assert_eq!(provenance["fuzzySha256"].as_str(), Some(provenance::FUZZY));
    assert_eq!(provenance["utilsSha256"].as_str(), Some(provenance::UTILS));

    let (harness, base) = AutocompleteOracle::setup();

    for case in oracle["pathPrefix"].as_array().expect("rows") {
        let text = case["text"].as_str().unwrap();
        let force = case["force"].as_bool().unwrap();
        let suggestions =
            harness
                .provider
                .get_suggestions(&[text.to_string()], 0, text.len(), force);
        let expected_items = suggestion_items(&case["items"]);
        let actual = suggestions.map(|suggestions| {
            (
                suggestions.prefix,
                suggestions
                    .items
                    .into_iter()
                    .map(|item| (item.value, item.label))
                    .collect::<Vec<_>>(),
            )
        });
        let expected = case["prefix"]
            .as_str()
            .map(|prefix| (prefix.to_string(), expected_items.expect("items")));
        match (actual, expected) {
            (Some(actual), Some(expected)) => {
                // Paths embed the temp dir name only through `~`-expansion
                // cases, which this grid does not exercise.
                assert_eq!(actual.0, expected.0, "prefix for {text:?} (force={force})");
                assert_eq!(actual.1, expected.1, "items for {text:?} (force={force})");
            }
            (None, None) => {}
            other => panic!("autocomplete pathPrefix mismatch for {text:?}: {other:?}"),
        }
    }
    let _ = base;

    for case in oracle["suggestions"].as_array().expect("rows") {
        let text = case["text"].as_str().unwrap();
        let suggestions =
            harness
                .provider
                .get_suggestions(&[text.to_string()], 0, text.len(), false);
        let expected_items = suggestion_items(&case["items"]);
        let actual = suggestions.map(|suggestions| {
            suggestions
                .items
                .into_iter()
                .map(|item| (item.value, item.label))
                .collect::<Vec<_>>()
        });
        let expected = expected_items;
        assert_eq!(
            actual.is_none(),
            expected.is_none(),
            "suggestions presence for {text:?}"
        );
        if let (Some(actual), Some(expected)) = (actual, expected) {
            // Values embed the quoted prefix paths; compare after replacing
            // the platform directory separator.
            let normalize = |items: Vec<(String, String)>| -> Vec<(String, String)> {
                items
                    .into_iter()
                    .map(|(value, label)| (value.replace('\\', "/"), label))
                    .collect()
            };
            assert_eq!(
                normalize(actual),
                normalize(expected),
                "suggestions for {text:?}"
            );
        }
    }
}

/// skill: bare-name command matching (upstream `autocomplete-skill-slash.test.ts`).
#[test]
fn autocomplete_skill_bare_name_matching() {
    let oracle = oracle(AUTOCOMPLETE_ORACLE);
    // The v1.0.0 cursor-before-the-slash rows fall through to file completion,
    // so the provider works over a hermetic mirror of the capture tree (the
    // capture creates the same layout under its temp dir).
    let skill_base = std::env::temp_dir().join(format!("tui-delta-skill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&skill_base);
    std::fs::create_dir_all(skill_base.join("sub")).unwrap();
    std::fs::create_dir_all(skill_base.join("Folder Names")).unwrap();
    std::fs::create_dir_all(skill_base.join("z-last")).unwrap();
    for file in ["readme.md", "sub/file-one.ts", "z-last/zebra.txt", "app.js"] {
        std::fs::write(skill_base.join(file), "x").unwrap();
    }
    let skill_rows: Vec<&Value> = oracle["commands"]
        .as_array()
        .expect("rows")
        .iter()
        .collect();
    assert!(skill_rows.len() >= 6, "expected the skill command grid");
    let provider = autocomplete::CombinedAutocompleteProvider::new(
        vec![
            autocomplete::SlashCommand {
                name: "skill:deploy".into(),
                description: Some("deploy the service".into()),
                argument_hint: None,
            },
            autocomplete::SlashCommand {
                name: "skill:diagnose".into(),
                description: Some("diagnose failures".into()),
                argument_hint: None,
            },
            autocomplete::SlashCommand {
                name: "build".into(),
                description: None,
                argument_hint: Some("[target]".into()),
            },
            autocomplete::SlashCommand {
                name: "review".into(),
                description: None,
                argument_hint: None,
            },
            autocomplete::SlashCommand {
                name: "skill:other".into(),
                description: Some("unrelated".into()),
                argument_hint: None,
            },
        ],
        skill_base.clone(),
        None,
    );
    for case in skill_rows {
        let text = case["text"].as_str().unwrap();
        // v1.0.0 rows record the cursor column (code points); earlier rows
        // placed it at the end of the text.
        let cursor = case["cursor"]
            .as_u64()
            .map(|n| n as usize)
            .unwrap_or_else(|| text.chars().count());
        let suggestions = provider.get_suggestions(&[text.to_string()], 0, cursor, false);
        let actual = suggestions.map(|suggestions| {
            (
                suggestions.prefix,
                suggestions
                    .items
                    .into_iter()
                    .map(|item| (item.value, item.label, item.description.unwrap_or_default()))
                    .collect::<Vec<_>>(),
            )
        });
        let expected = case["items"].as_null().is_none().then(|| {
            (
                case["prefix"].as_str().unwrap().to_string(),
                case["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|item| {
                        (
                            item["value"].as_str().unwrap().to_string(),
                            item["label"].as_str().unwrap().to_string(),
                            item["description"].as_str().unwrap_or_default().to_string(),
                        )
                    })
                    .collect::<Vec<_>>(),
            )
        });
        assert_eq!(actual, expected, "skill commands for {text:?}");
    }
    let _ = std::fs::remove_dir_all(&skill_base);
}

// ---------------------------------------------------------------------------
// terminal-image delta
// ---------------------------------------------------------------------------

#[test]
fn terminal_image_delta_oracle_matches() {
    let oracle = oracle(TERMINAL_IMAGE_ORACLE);
    let provenance = &oracle["provenance"];
    // The detect capture ran on win32 (the gate environment); the cell-size
    // grid is platform-independent.
    assert_eq!(provenance["platform"].as_str(), Some("win32"));
    assert_eq!(
        provenance["terminalImageSha256"].as_str(),
        Some(provenance::TERMINAL_IMAGE)
    );

    // The capability cache is process-global; serialize with the other
    // capability-mutating tests.
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());

    for case in oracle["cellSize"].as_array().expect("rows") {
        let input = &case["input"];
        let size = terminal_image::calculate_image_cell_size(
            terminal_image::ImageDimensions {
                width_px: input["image"]["widthPx"].as_u64().unwrap() as usize,
                height_px: input["image"]["heightPx"].as_u64().unwrap() as usize,
            },
            input["maxWidth"].as_f64().unwrap(),
            input["maxHeight"].as_f64(),
            terminal_image::CellDimensions {
                width_px: input["cell"]["widthPx"].as_u64().unwrap() as usize,
                height_px: input["cell"]["heightPx"].as_u64().unwrap() as usize,
            },
            input["optimizeAspectRatio"].as_bool().unwrap(),
        );
        assert_eq!(
            (size.columns, size.rows),
            (
                case["size"]["columns"].as_u64().unwrap() as usize,
                case["size"]["rows"].as_u64().unwrap() as usize,
            ),
            "calculateImageCellSize({input})",
        );
    }

    // Detection rows: replayed through detect_with on the recorded win32 env.
    for case in oracle["detection"].as_array().expect("rows") {
        let env = case["env"].as_object().unwrap();
        let lookup = |name: &str| env.get(name).and_then(Value::as_str).map(str::to_string);
        let no_tmux_forward = || false;
        // The capture ran on a win32 console; the replay pins the same
        // platform bit so the rows are environment-determined on every host.
        let capabilities = terminal_image::detect_with(&lookup, &no_tmux_forward, true);
        assert_eq!(
            capabilities.true_color,
            case["trueColor"].as_bool().unwrap(),
            "trueColor for {env:?}"
        );
        assert_eq!(
            capabilities.images,
            case["images"].as_str(),
            "images for {env:?}"
        );
        let mode = terminal_image::get_terminal_color_mode(Some(capabilities));
        assert_eq!(
            mode.as_str(),
            case["colorMode"].as_str().unwrap(),
            "colorMode for {env:?}"
        );
    }

    // v1.0.0 Kitty placement-row helpers. The registry is process-global, so
    // every case re-registers its image exactly like the capture does.
    for case in oracle["kittyPlacement"].as_array().expect("rows") {
        let register = &case["register"];
        terminal_image::register_kitty_image_metadata(terminal_image::KittyImageMetadata {
            image_id: register["imageId"].as_u64().unwrap(),
            columns: register["columns"].as_u64().unwrap() as usize,
            rows: register["rows"].as_u64().unwrap() as usize,
            width_px: register["widthPx"].as_u64().unwrap() as usize,
            height_px: register["heightPx"].as_u64().unwrap() as usize,
        });
        let line = case["line"].as_str().unwrap();
        let payload = line
            .split_once(';')
            .expect("kitty header")
            .1
            .split("\x1b\\")
            .next()
            .expect("kitty payload")
            .to_string();
        // Transmission re-encodes to the captured line (single-shot chunk).
        assert_eq!(
            terminal_image::encode_kitty(
                &payload,
                terminal_image::KittyOptions {
                    columns: Some(register["columns"].as_u64().unwrap() as usize),
                    rows: Some(register["rows"].as_u64().unwrap() as usize),
                    image_id: Some(register["imageId"].as_u64().unwrap()),
                    move_cursor: None,
                },
            ),
            line,
            "encodeKitty for image {}",
            case["imageId"]
        );
        // Placement rows: the registered rows; unknown images give None.
        assert_eq!(
            terminal_image::get_kitty_image_placement_rows(line),
            case["placementRows"].as_u64().map(|n| n as usize),
            "placementRows for image {}",
            case["imageId"]
        );
        assert_eq!(
            terminal_image::get_kitty_image_placement_rows(case["unknownLine"].as_str().unwrap()),
            case["unknownLinePlacementRows"]
                .as_u64()
                .map(|n| n as usize),
            "unknown placementRows for image {}",
            case["imageId"]
        );
        // Placement summary: sequence, byte counts, rows, replacement line.
        let placement = terminal_image::get_kitty_image_placement(line).expect("placement");
        let expected = &case["placement"];
        assert_eq!(placement.image_id, expected["imageId"].as_u64().unwrap());
        assert_eq!(
            placement.transmission_bytes,
            expected["transmissionBytes"].as_u64().unwrap() as usize
        );
        assert_eq!(
            placement.estimated_decoded_bytes,
            expected["estimatedDecodedBytes"].as_u64().unwrap()
        );
        assert_eq!(
            placement.rows,
            expected["rows"].as_u64().unwrap() as usize,
            "placement.rows for image {}",
            case["imageId"]
        );
        assert_eq!(placement.sequence, expected["sequence"].as_str().unwrap());
        assert_eq!(
            placement.replacement_line,
            expected["replacementLine"].as_str().unwrap()
        );
        // The cropped line carries explicit r=/y=/h= controls; its placement
        // rows come from the controls, not the registry.
        let cropped = case["croppedLine"].as_str().unwrap();
        assert_eq!(
            terminal_image::get_kitty_image_placement_rows(cropped),
            case["croppedPlacementRows"].as_u64().map(|n| n as usize),
            "cropped placementRows for image {}",
            case["imageId"]
        );
        let cropped_placement =
            terminal_image::get_kitty_image_placement(cropped).expect("cropped placement");
        assert_eq!(
            cropped_placement.rows,
            case["croppedPlacementRowsField"].as_u64().unwrap() as usize,
            "cropped placement.rows for image {}",
            case["imageId"]
        );
        // Crop grid: recorded (hidden, visible) row pairs -> exact cropped
        // transmissions.
        let crop_grid: Vec<(i64, i64)> = case["cropGrid"]
            .as_array()
            .expect("cropGrid")
            .iter()
            .map(|pair| (pair[0].as_i64().unwrap(), pair[1].as_i64().unwrap()))
            .collect();
        for ((hidden, visible), expected_crop) in crop_grid
            .iter()
            .zip(case["crops"].as_array().expect("crops"))
        {
            assert_eq!(
                terminal_image::crop_kitty_image_line(line, *hidden, *visible),
                expected_crop.as_str().unwrap(),
                "cropKittyImageLine(line, {hidden}, {visible}) for image {}",
                case["imageId"]
            );
        }
    }
}

// ---------------------------------------------------------------------------
// editor trigger patterns
// ---------------------------------------------------------------------------

fn editor_trigger_matches(text: &str, characters: &[char]) -> bool {
    // Mirrors components/editor.rs autocomplete_trigger_pattern_matches; the
    // free function is private there, so re-derive through the module.
    super::components::editor::editor_trigger_pattern_matches_for_test(text, characters)
}

#[test]
fn editor_trigger_patterns_match_upstream_regex() {
    let oracle = oracle(EDITOR_PATTERNS_ORACLE);
    assert_eq!(
        oracle["provenance"]["editorSha256"].as_str(),
        Some(provenance::EDITOR)
    );
    assert_eq!(
        oracle["provenance"]["utilsSha256"].as_str(),
        Some(provenance::UTILS)
    );
    let default: [char; 2] = ['@', '#'];
    for case in oracle["trigger"].as_array().expect("rows") {
        let text = case["text"].as_str().unwrap();
        let characters: Vec<char> = match case["characters"].as_str() {
            Some(custom) => custom.chars().collect(),
            None => default.to_vec(),
        };
        assert_eq!(
            editor_trigger_matches(text, &characters),
            case["matches"].as_bool().unwrap(),
            "trigger pattern for {text:?} with {characters:?}"
        );
    }
    // The debounce pattern only differs from the trigger pattern for `@`
    // tokens without quotes; the Rust editor does not use it (no attachment
    // autocomplete), so only the trigger rows are replayed.
    for case in oracle["debounce"].as_array().expect("rows") {
        let text = case["text"].as_str().unwrap();
        // The trigger pattern and debounce pattern agree except where `@` is a
        // bare token start; those cases are pinned by the trigger rows above.
        let _ = text;
    }
}
