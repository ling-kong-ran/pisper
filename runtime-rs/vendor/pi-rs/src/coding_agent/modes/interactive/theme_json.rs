//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Port of upstream `coding-agent/src/modes/interactive/theme/theme-json.ts`
//! (148 lines at v0.99.1 HEAD `2bbfcca43`, sha256
//! `48d2ba48735eafb24e4812cf7c0d9b5c7e26253e6731616432850154079b3545`)
//! — theme JSON validation and the parsed [`ThemeJson`] document shape
//! (including the delta's optional `appearance` key).
//!
//! SEAM (D3 in `interactive/mod.rs`): upstream delegates the structural check
//! to typebox (`Compile(ThemeJsonSchema)`); typebox is not installable in this
//! offline workspace, so the schema semantics (required color keys, the
//! `string | 0..=255` ColorValue union, optional keys) are re-stated here and
//! the upstream error-assembly body is reproduced verbatim. The
//! required-colors / name-rule outputs are byte-exact against
//! `tests/fixtures/interactive_r17_oracle/validate_theme_json_oracle.mjs`; typebox's
//! "Other errors" message wording is not exercised by the oracle.

use std::collections::BTreeMap;

use serde_json::Value;

use super::theme::ColorValue;

/// Upstream `ThemeJsonSchema` required `colors` keys, in declaration order.
pub const REQUIRED_COLORS: [&str; 50] = [
    "accent",
    "border",
    "borderAccent",
    "borderMuted",
    "success",
    "error",
    "warning",
    "muted",
    "dim",
    "text",
    "thinkingText",
    "selectedBg",
    "userMessageBg",
    "customMessageBg",
    "customMessageText",
    "customMessageLabel",
    "toolPendingBg",
    "toolSuccessBg",
    "toolErrorBg",
    "toolTitle",
    "toolOutput",
    "mdHeading",
    "mdLink",
    "mdLinkUrl",
    "mdCode",
    "mdCodeBlock",
    "mdCodeBlockBorder",
    "mdQuote",
    "mdQuoteBorder",
    "mdHr",
    "mdListBullet",
    "toolDiffAdded",
    "toolDiffRemoved",
    "toolDiffContext",
    "syntaxComment",
    "syntaxKeyword",
    "syntaxFunction",
    "syntaxVariable",
    "syntaxString",
    "syntaxNumber",
    "syntaxType",
    "syntaxOperator",
    "syntaxPunctuation",
    "thinkingOff",
    "thinkingMinimal",
    "thinkingLow",
    "thinkingMedium",
    "thinkingHigh",
    "thinkingXhigh",
    "bashMode",
];

/// Optional color keys per the schema.
const OPTIONAL_COLORS: [&str; 5] = [
    "scrollbarTrack",
    "scrollbarThumb",
    "searchMatchBg",
    "searchMatchText",
    "thinkingMax",
];

/// Upstream `ValidatedThemeJson`.
#[derive(Debug, Clone)]
pub struct ThemeJson {
    pub name: String,
    /// Upstream `appearance`: the background the theme is designed for
    /// ("dark" | "light"). Detected from the theme colors when omitted.
    pub appearance: Option<String>,
    pub vars: Option<BTreeMap<String, ColorValue>>,
    pub colors: BTreeMap<String, ColorValue>,
    pub export: Option<ThemeExportSection>,
}

impl ThemeJson {
    /// The declared appearance as the upstream `"dark" | "light"` union;
    /// absent or undeclared values detect from the theme colors instead.
    pub fn appearance(&self) -> Option<&'static str> {
        match self.appearance.as_deref() {
            Some("dark") => Some("dark"),
            Some("light") => Some("light"),
            _ => None,
        }
    }
}

/// Upstream `export` section (`pageBg`/`cardBg`/`infoBg`, all optional).
#[derive(Debug, Clone)]
pub struct ThemeExportSection {
    pub page_bg: Option<ColorValue>,
    pub card_bg: Option<ColorValue>,
    pub info_bg: Option<ColorValue>,
}

impl ThemeJson {
    /// Parse without schema validation (the upstream no-validator default for
    /// built-in documents).
    pub fn parse(content: &str) -> Result<Self, String> {
        parse_theme_json(content)
    }
}

fn parse_color_value(value: &Value) -> Option<ColorValue> {
    match value {
        Value::String(s) => Some(ColorValue::Str(s.clone())),
        // Type.Union([Type.String(), Type.Integer({ minimum: 0, maximum: 255 })]):
        // typebox's Integer rejects non-integral numbers.
        Value::Number(n) if n.is_u64() => {
            let raw = n.as_u64().expect("u64");
            if raw <= 255 {
                Some(ColorValue::Index(raw as u8))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn parse_color_map(value: &Value) -> Option<BTreeMap<String, ColorValue>> {
    let map = value.as_object()?;
    let mut result = BTreeMap::new();
    for (key, value) in map {
        result.insert(key.clone(), parse_color_value(value)?);
    }
    Some(result)
}

/// Parse the document shape (no required-key enforcement).
fn parse_theme_json(json: &str) -> Result<ThemeJson, String> {
    let value: Value =
        serde_json::from_str(json).map_err(|error| format!("Failed to parse theme: {error}"))?;
    let object = value.as_object().ok_or("expected an object")?;
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .ok_or("expected name")?
        .to_string();
    let appearance = match object.get("appearance") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => return Err("invalid appearance".to_string()),
    };
    let vars = match object.get("vars") {
        None | Some(Value::Null) => None,
        Some(map) => Some(parse_color_map(map).ok_or("invalid vars")?),
    };
    let colors =
        parse_color_map(object.get("colors").ok_or("expected colors")?).ok_or("invalid colors")?;
    let export = match object.get("export") {
        None | Some(Value::Null) => None,
        Some(section) => {
            let section = section.as_object().ok_or("invalid export")?;
            let get = |key: &str| -> Result<Option<ColorValue>, String> {
                match section.get(key) {
                    None | Some(Value::Null) => Ok(None),
                    Some(value) => parse_color_value(value)
                        .map(Some)
                        .ok_or_else(|| "invalid export color".to_string()),
                }
            };
            Some(ThemeExportSection {
                page_bg: get("pageBg")?,
                card_bg: get("cardBg")?,
                info_bg: get("infoBg")?,
            })
        }
    };
    Ok(ThemeJson {
        name,
        appearance,
        vars,
        colors,
        export,
    })
}

/// A schema error in the shape upstream reads off typebox's `Errors(json)`.
struct SchemaError {
    keyword: &'static str,
    instance_path: &'static str,
    required_properties: Vec<&'static str>,
}

/// Mirrors `compiledThemeSchema.Check(json)` + `Errors(json)` for the
/// required-colors path (the only error shape the byte oracle exercises).
fn schema_errors(value: &Value) -> Vec<SchemaError> {
    let mut errors = Vec::new();
    let Some(object) = value.as_object() else {
        return errors;
    };
    let missing = |colors: Option<&Value>| -> Vec<&'static str> {
        match colors.and_then(Value::as_object) {
            None => REQUIRED_COLORS.to_vec(),
            Some(map) => REQUIRED_COLORS
                .iter()
                .copied()
                .filter(|key| !map.contains_key(*key))
                .collect(),
        }
    };
    let missing_colors = missing(object.get("colors"));
    if !missing_colors.is_empty() {
        errors.push(SchemaError {
            keyword: "required",
            instance_path: "/colors",
            required_properties: missing_colors,
        });
    }
    // `appearance: Type.Optional(Type.Union([Type.Literal("dark"), Type.Literal("light")]))`.
    if !appearance_ok(object.get("appearance")) {
        errors.push(SchemaError {
            keyword: "union",
            instance_path: "/appearance",
            required_properties: Vec::new(),
        });
    }
    errors
}

/// Mirrors the optional `appearance` union check used by the schema.
fn appearance_ok(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => true,
        Some(Value::String(s)) => s == "dark" || s == "light",
        Some(_) => false,
    }
}

/// Mirrors the ColorValue union check used by the schema.
fn color_map_values_ok(value: Option<&Value>) -> bool {
    let Some(map) = value.and_then(Value::as_object) else {
        return false;
    };
    map.values().all(|value| {
        value.is_string()
            || value
                .as_u64()
                .is_some_and(|raw| raw <= 255 && value.is_u64())
    })
}

/// Validate one theme document, throwing a message that names the offending
/// tokens (verbatim upstream `validateThemeJson`).
pub fn validate_theme_json(label: &str, json: &Value) -> Result<ThemeJson, String> {
    let check_failed = match json.as_object() {
        None => true,
        Some(object) => {
            let name_ok = object.get("name").and_then(Value::as_str).is_some();
            let appearance_ok = appearance_ok(object.get("appearance"));
            let vars_ok = match object.get("vars") {
                None | Some(Value::Null) => true,
                Some(map) => color_map_values_ok(Some(map)),
            };
            let colors_ok = color_map_values_ok(object.get("colors"));
            let export_ok = match object.get("export") {
                None | Some(Value::Null) => true,
                Some(section) => color_map_values_ok(Some(section)),
            };
            !(name_ok && appearance_ok && vars_ok && colors_ok && export_ok)
        }
    };

    if check_failed || !schema_errors(json).is_empty() {
        let errors = schema_errors(json);
        let mut missing_colors = std::collections::BTreeSet::new();
        let mut other_errors: Vec<String> = Vec::new();

        for error in &errors {
            if error.keyword == "required" && error.instance_path == "/colors" {
                for required_property in &error.required_properties {
                    missing_colors.insert((*required_property).to_string());
                }
                continue;
            }

            let path = if error.instance_path.is_empty() {
                "/"
            } else {
                error.instance_path
            };
            other_errors.push(format!("  - {path}: {path} is invalid"));
        }

        let mut error_message = format!("Invalid theme \"{label}\":\n");
        if !missing_colors.is_empty() {
            error_message.push_str("\nMissing required color tokens:\n");
            // Array.from(missingColors).sort() — UTF-16 code-unit sort; the
            // BTreeSet iteration matches it for the ASCII token names.
            let sorted: Vec<String> = missing_colors.into_iter().collect();
            error_message.push_str(
                &sorted
                    .iter()
                    .map(|color| format!("  - {color}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
            error_message
                .push_str("\n\nPlease add these colors to your theme's \"colors\" object.");
            error_message.push_str(
                "\nSee the built-in themes (dark.json, light.json) for reference values.",
            );
        }
        if !other_errors.is_empty() {
            error_message.push_str(&format!("\n\nOther errors:\n{}", other_errors.join("\n")));
        }

        return Err(error_message);
    }

    let theme_json = parse_theme_json(&serde_json::to_string(json).expect("round-trip"))
        .expect("shape re-parses");
    if theme_json.name.contains('/') {
        return Err(format!(
            "Invalid theme name \"{}\": theme names cannot contain \"/\" because it is reserved for automatic light/dark theme settings.",
            theme_json.name
        ));
    }
    Ok(theme_json)
}

/// The optional color keys, exposed for registry tooling.
pub fn optional_colors() -> &'static [&'static str; 5] {
    &OPTIONAL_COLORS
}
