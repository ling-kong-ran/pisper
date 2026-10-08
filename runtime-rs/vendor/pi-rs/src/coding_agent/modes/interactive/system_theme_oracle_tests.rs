//! Byte-replay of the upstream `system-theme.ts` oracle
//! (`tests/fixtures/coding_agent_theme_delta_oracle/oracle/`, captured by
//! `capture.mjs` under node from the verbatim upstream sources, SHA-256
//! pinned below).

use serde_json::Value;

use crate::tui::terminal_colors::RgbColor;

use super::system_theme::{
    generate_system_theme_colors, relative_luminance, terminal_appearance, wcag_contrast, Rgb,
    SystemThemeInput,
};

const ORACLE: &str = include_str!(
    "../../../../tests/fixtures/coding_agent_theme_delta_oracle/oracle/system_theme_oracle.json"
);

/// The upstream sources the capture executed (source-of-truth pin). The
/// v1.0.0 `system-theme.ts` adds the OKLab-lightness recipe and the chroma
/// cap of `anchored`.
const SOURCE_SHAS: &[(&str, &str)] = &[
    (
        "system-theme.ts",
        "877a3dc24fd194f2dc5ee9efe8f1699acdcdac6defa197009bc7b39920bbb729",
    ),
    (
        "colors.ts",
        "d4fe729c424d2c07bc64cf0c3edfdbf5642865cba395dfb37234c6c88d65f468",
    ),
    (
        "oklab.ts",
        "45b067e6e3605b385f595adecd7c0216f1c6b6686680d5c73f661286de736be6",
    ),
];

#[test]
fn oracle_sources_are_pinned() {
    use sha2::{Digest, Sha256};
    for (name, sha) in SOURCE_SHAS {
        let bytes = std::fs::read(format!(
            "{}/tests/fixtures/coding_agent_theme_delta_oracle/oracle/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap_or_else(|error| panic!("missing oracle source {name}: {error}"));
        let digest = Sha256::digest(&bytes);
        let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        assert_eq!(hex, *sha, "{name}");
    }
}

#[test]
fn replay_generation_scenarios() {
    let oracle: Value = serde_json::from_str(ORACLE).expect("oracle parses");
    for scenario in oracle["scenarios"].as_array().expect("scenarios") {
        let name = scenario["name"].as_str().expect("scenario name");
        let input = &scenario["input"];
        let theme = generate_system_theme_colors(&SystemThemeInput {
            foreground: rgb_channels(input["foreground"].as_array()),
            background: rgb_channels(input["background"].as_array()),
            palette: input["palette"].as_array().map(|palette| {
                palette
                    .iter()
                    .map(|color| rgb_channels(color.as_array()).expect("palette rgb"))
                    .collect()
            }),
            saturation: input["saturation"].as_f64(),
            appearance_hint: match input["appearanceHint"].as_str() {
                Some("dark") => Some("dark"),
                Some("light") => Some("light"),
                _ => None,
            },
        });

        // Token order must match the capture (upstream object literal order).
        let colors = scenario["colors"].as_object().expect("colors object");
        let keys: Vec<&str> = colors.keys().map(String::as_str).collect();
        let ours: Vec<&str> = theme.colors.iter().map(|(name, _)| *name).collect();
        assert_eq!(ours, keys, "{name}: token order");

        for (token, expected) in colors {
            let actual = theme
                .colors
                .iter()
                .find(|(name, _)| name == token)
                .map(|(_, value)| value)
                .unwrap_or_else(|| panic!("{name}: missing token {token}"));
            let expected = match expected.as_str() {
                Some("") => "terminal-default".to_string(),
                Some(hex) => format!("hex:{hex}"),
                None => format!("index:{}", expected.as_i64().expect("index")),
            };
            let actual = match actual {
                super::system_theme::TokenColor::TerminalDefault => "terminal-default".to_string(),
                super::system_theme::TokenColor::Hex(hex) => format!("hex:{hex}"),
                super::system_theme::TokenColor::Index(index) => format!("index:{index}"),
            };
            assert_eq!(actual, expected, "{name}: token {token}");
        }

        let dim: Vec<&str> = scenario["dim"]
            .as_array()
            .map(|dim| dim.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        assert_eq!(theme.dim, dim, "{name}: dim tokens");
        assert_eq!(
            theme.appearance,
            scenario["appearance"].as_str(),
            "{name}: appearance"
        );
    }
}

/// Compare a Rust luminance against the JS-captured oracle. `powf` may round
/// to a neighboring double differently between the V8 runtime that captured
/// the oracle and the MSVC CRT this test runs under (1 ulp on the lum grid,
/// e.g. lum(128)); the arithmetic is otherwise identical, so a 1-ulp window
/// keeps the oracle byte-comparable without weakening the color outputs
/// (which still compare exactly).
fn assert_lum_eq(left: f64, right: f64, context: &str) {
    let ulp = f64::EPSILON * left.abs().max(1.0);
    assert!((left - right).abs() <= ulp, "{context}: {left} != {right}");
}

#[test]
fn replay_appearance_luminance_and_contrast_grids() {
    let oracle: Value = serde_json::from_str(ORACLE).expect("oracle parses");
    for scenario in oracle["appearanceScenarios"]
        .as_array()
        .expect("appearance")
    {
        let background = rgb_channels(scenario["background"].as_array()).expect("background");
        let foreground = rgb_channels(scenario["foreground"].as_array());
        assert_eq!(
            terminal_appearance(background, foreground),
            scenario["appearance"].as_str().expect("appearance"),
            "{scenario:?}"
        );
    }
    for row in oracle["lumGrid"].as_array().expect("lum grid") {
        let v = row["v"].as_f64().expect("v");
        if let Some(lum) = row["lum"].as_f64() {
            assert_lum_eq(relative_luminance(u8([v, v, v])), lum, &format!("lum({v})"));
        }
        if let Some(lum) = row["lumChannel"].as_f64() {
            assert_lum_eq(
                relative_luminance(u8([255.0, v, 0.0])),
                lum,
                &format!("lum(255,{v},0)"),
            );
        }
    }
    for row in oracle["contrastGrid"].as_array().expect("contrast grid") {
        let parse = |hex: &str| -> Rgb {
            let n = hex.trim_start_matches('#');
            RgbColor {
                r: u8::from_str_radix(&n[0..2], 16).unwrap(),
                g: u8::from_str_radix(&n[2..4], 16).unwrap(),
                b: u8::from_str_radix(&n[4..6], 16).unwrap(),
            }
        };
        let a = parse(row["a"].as_str().expect("a"));
        let b = parse(row["b"].as_str().expect("b"));
        assert_eq!(
            wcag_contrast(a, b),
            row["contrast"].as_f64().expect("contrast"),
            "{row:?}"
        );
    }
}

fn u8(channels: [f64; 3]) -> Rgb {
    RgbColor {
        r: channels[0] as u8,
        g: channels[1] as u8,
        b: channels[2] as u8,
    }
}

fn rgb_channels(channels: Option<&Vec<Value>>) -> Option<Rgb> {
    let channels = channels?;
    Some(RgbColor {
        r: channels[0].as_f64().expect("r").round() as u8,
        g: channels[1].as_f64().expect("g").round() as u8,
        b: channels[2].as_f64().expect("b").round() as u8,
    })
}
