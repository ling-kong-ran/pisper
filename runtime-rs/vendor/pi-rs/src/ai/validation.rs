//! Port of upstream `packages/ai/src/utils/validation.ts`: find a tool by
//! name and validate (and coerce) a tool call's arguments against its JSON
//! Schema.
//!
//! Upstream validates with TypeBox `Compile`/`Check` plus a hand-rolled
//! plain-JSON-Schema coercion pass (`coerceWithJsonSchema`). Rust schemas
//! carry no TypeBox kind symbols, so every schema takes the plain-JSON-Schema
//! path: normalize optional nulls, coerce, then check with a JSON-Schema
//! interpreter emitting TypeBox's `en_US` error messages and paths.
//!
//! `$ref` resolution mirrors upstream's compile topology: sub-schema checks
//! (optional-null normalization, union-arm coercion) compile the sub-schema
//! standalone, so refs resolve only within that sub-schema; the final
//! top-level check resolves refs against the whole tool schema. An
//! unresolvable `$ref` acts like a `false` schema.

use crate::ai::types::content::ToolCall;
use crate::ai::types::tool::Tool;

/// TypeBox collects at most this many errors per validation context
/// (`Settings.Get().maxErrors`).
const TYPEBOX_MAX_ERRORS: usize = 8;

/// Cap on schema-recursion depth so cyclic `$ref` graphs cannot overflow the
/// stack (TypeBox compiles ahead of time and rejects such schemas).
const MAX_SCHEMA_DEPTH: usize = 128;

/// Upstream `validateToolCall` (validation.ts:302-308): finds the tool by name
/// and returns the call with validated (and potentially coerced) arguments.
/// Errors when the tool is not found or validation fails.
pub fn validate_tool_call(tools: &[Tool], call: &ToolCall) -> Result<ToolCall, String> {
    let tool = tools
        .iter()
        .find(|tool| tool.name == call.name)
        .ok_or_else(|| format!("Tool \"{}\" not found", call.name))?;
    let arguments = validate_tool_arguments(tool, call)?;
    Ok(ToolCall {
        arguments,
        ..call.clone()
    })
}

/// Upstream `validateToolArguments` (validation.ts:317-350): validates the
/// call's arguments against the tool's schema, returning the validated
/// (coerced) arguments.
pub fn validate_tool_arguments(tool: &Tool, call: &ToolCall) -> Result<serde_json::Value, String> {
    let schema = &tool.parameters;
    let mut args = call.arguments.clone();
    normalize_optional_nulls(&mut args, schema);

    let coerced = coerce_with_json_schema(args.clone(), schema, 0);
    // Upstream merges the coerced contents back into the root object and then
    // re-checks; for non-object roots it prefers the coerced value only when
    // it validates. Both reduce to: check the coerced value first, falling
    // back to the original arguments unless both roots are objects.
    let candidate = if args.is_object() && coerced.is_object() || coerced == args {
        coerced
    } else if check_standalone(schema, &coerced) {
        return Ok(coerced);
    } else {
        args
    };

    if check_standalone(schema, &candidate) {
        return Ok(candidate);
    }

    let mut errors = Vec::new();
    check_schema(schema, &candidate, "", &mut errors, schema, 0);
    errors.truncate(TYPEBOX_MAX_ERRORS);
    let rendered = if errors.is_empty() {
        "Unknown validation error".to_string()
    } else {
        errors
            .iter()
            .map(|error| format!("  - {}: {}", format_validation_path(error), error.message()))
            .collect::<Vec<_>>()
            .join("\n")
    };
    Err(format!(
        "Validation failed for tool \"{}\":\n{}\n\nReceived arguments:\n{}",
        call.name,
        rendered,
        serde_json::to_string_pretty(&call.arguments).unwrap_or_default(),
    ))
}

// ---------------------------------------------------------------------
// Optional-null normalization (validation.ts:240-269)
// ---------------------------------------------------------------------

/// Upstream `normalizeOptionalNulls`: deletes optional properties whose value
/// is `null` when the property schema disallows null, so a model emitting
/// `null` for an absent optional argument still validates.
fn normalize_optional_nulls(value: &mut serde_json::Value, schema: &serde_json::Value) {
    if value.is_array() {
        let items = schema.get("items").cloned();
        if let (Some(items), Some(value_items)) = (items, value.as_array_mut()) {
            if let Some(sized) = items.as_array() {
                for (item, item_schema) in value_items.iter_mut().zip(sized) {
                    normalize_optional_nulls(item, item_schema);
                }
            } else if items.is_object() {
                for item in value_items.iter_mut() {
                    normalize_optional_nulls(item, &items);
                }
            }
        }
        return;
    }
    let Some(properties) = schema
        .get("properties")
        .and_then(serde_json::Value::as_object)
    else {
        return;
    };
    let required: Vec<&str> = schema
        .get("required")
        .and_then(serde_json::Value::as_array)
        .map(|required| {
            required
                .iter()
                .filter_map(serde_json::Value::as_str)
                .collect()
        })
        .unwrap_or_default();
    let Some(object) = value.as_object_mut() else {
        return;
    };
    for (key, property_schema) in properties {
        let is_required = required.contains(&key.as_str());
        let has_string_ref = property_schema
            .get("$ref")
            .is_some_and(serde_json::Value::is_string);
        if !is_required
            && !has_string_ref
            && object.get(key).is_some_and(serde_json::Value::is_null)
            && !check_standalone(property_schema, &serde_json::Value::Null)
        {
            object.shift_remove(key);
        } else if let Some(property_value) = object.get_mut(key) {
            normalize_optional_nulls(property_value, property_schema);
        }
    }
}

// ---------------------------------------------------------------------
// Coercion (validation.ts:59-238)
// ---------------------------------------------------------------------

fn schema_types(schema: &serde_json::Value) -> Vec<String> {
    match schema.get("type") {
        Some(serde_json::Value::String(type_name)) => vec![type_name.clone()],
        Some(serde_json::Value::Array(types)) => types
            .iter()
            .filter_map(serde_json::Value::as_str)
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

fn matches_json_type(value: &serde_json::Value, type_name: &str) -> bool {
    match type_name {
        "number" => value.is_number(),
        "integer" => {
            value.as_i64().is_some()
                || value.as_u64().is_some()
                || value
                    .as_f64()
                    .is_some_and(|n| n.is_finite() && n.fract() == 0.0)
        }
        "boolean" => value.is_boolean(),
        "string" => value.is_string(),
        "null" => value.is_null(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        _ => false,
    }
}

/// JSON has no int/float distinction (`42 === 42.0` upstream), so integral
/// values keep an integer-backed `Number` for stable equality and display.
fn f64_to_number(parsed: f64) -> serde_json::Number {
    if parsed.is_finite()
        && parsed.fract() == 0.0
        && (i64::MIN as f64..=i64::MAX as f64).contains(&parsed)
    {
        serde_json::Number::from(parsed as i64)
    } else {
        serde_json::Number::from_f64(parsed).unwrap_or_else(|| serde_json::Number::from(0))
    }
}

/// Upstream `coercePrimitiveByType`: returns `Some(converted)` when a
/// conversion applies, `None` to keep the value unchanged.
fn coerce_primitive_by_type(
    value: &serde_json::Value,
    type_name: &str,
) -> Option<serde_json::Value> {
    match type_name {
        "number" => match value {
            serde_json::Value::Null => Some(serde_json::Value::from(0)),
            serde_json::Value::String(text) if !text.trim().is_empty() => text
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|parsed| parsed.is_finite())
                .map(|parsed| serde_json::Value::Number(f64_to_number(parsed))),
            serde_json::Value::Bool(flag) => Some(serde_json::Value::from(i64::from(*flag))),
            _ => None,
        },
        "integer" => match value {
            serde_json::Value::Null => Some(serde_json::Value::from(0)),
            serde_json::Value::String(text) if !text.trim().is_empty() => text
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|parsed| parsed.is_finite() && parsed.fract() == 0.0)
                .map(|parsed| serde_json::Value::Number(f64_to_number(parsed))),
            serde_json::Value::Bool(flag) => Some(serde_json::Value::from(i64::from(*flag))),
            _ => None,
        },
        "boolean" => match value {
            serde_json::Value::Null => Some(serde_json::Value::Bool(false)),
            serde_json::Value::String(text) => match text.as_str() {
                "true" => Some(serde_json::Value::Bool(true)),
                "false" => Some(serde_json::Value::Bool(false)),
                _ => None,
            },
            serde_json::Value::Number(number) => match number.as_f64() {
                Some(1.0) => Some(serde_json::Value::Bool(true)),
                Some(0.0) => Some(serde_json::Value::Bool(false)),
                _ => None,
            },
            _ => None,
        },
        "string" => match value {
            serde_json::Value::Null => Some(serde_json::Value::String(String::new())),
            serde_json::Value::Number(number) => {
                Some(serde_json::Value::String(number.to_string()))
            }
            serde_json::Value::Bool(flag) => Some(serde_json::Value::String(flag.to_string())),
            _ => None,
        },
        "null" => match value {
            serde_json::Value::String(text) if text.is_empty() => Some(serde_json::Value::Null),
            serde_json::Value::Number(number) if number.as_f64() == Some(0.0) => {
                Some(serde_json::Value::Null)
            }
            serde_json::Value::Bool(false) => Some(serde_json::Value::Null),
            _ => None,
        },
        _ => None,
    }
}

/// Upstream `coerceWithUnionSchema`: keep the value when it already matches an
/// arm; otherwise coerce a clone per arm and keep the first that validates.
/// Arms are checked standalone (their own `$ref` scope), matching upstream's
/// per-arm `Compile`.
fn coerce_with_union_schema(
    value: serde_json::Value,
    schemas: &[serde_json::Value],
    depth: usize,
) -> serde_json::Value {
    for schema in schemas {
        if check_standalone(schema, &value) {
            return value;
        }
    }
    for schema in schemas {
        let candidate = coerce_with_json_schema(value.clone(), schema, depth);
        if check_standalone(schema, &candidate) {
            return candidate;
        }
    }
    value
}

/// Upstream `coerceWithJsonSchema` (validation.ts:194-238).
fn coerce_with_json_schema(
    value: serde_json::Value,
    schema: &serde_json::Value,
    depth: usize,
) -> serde_json::Value {
    if depth > MAX_SCHEMA_DEPTH {
        return value;
    }
    let mut value = value;

    if let Some(nested_schemas) = schema.get("allOf").and_then(serde_json::Value::as_array) {
        for nested in nested_schemas {
            value = coerce_with_json_schema(value, nested, depth + 1);
        }
    }
    if let Some(union) = schema.get("anyOf").and_then(serde_json::Value::as_array) {
        value = coerce_with_union_schema(value, union, depth + 1);
    }
    if let Some(union) = schema.get("oneOf").and_then(serde_json::Value::as_array) {
        value = coerce_with_union_schema(value, union, depth + 1);
    }

    let types = schema_types(schema);
    let matches_union_member = types.len() > 1
        && types
            .iter()
            .any(|type_name| matches_json_type(&value, type_name));
    if !types.is_empty() && !matches_union_member {
        for type_name in &types {
            if let Some(candidate) = coerce_primitive_by_type(&value, type_name) {
                value = candidate;
                break;
            }
        }
    }

    if types.iter().any(|type_name| type_name == "object") && value.is_object() {
        let properties = schema
            .get("properties")
            .and_then(serde_json::Value::as_object);
        let defined_keys: Vec<String> = properties
            .map(|properties| properties.keys().cloned().collect())
            .unwrap_or_default();
        if let (Some(object), Some(properties)) = (value.as_object_mut(), properties) {
            for (key, property_schema) in properties {
                if let Some(property_value) = object.get_mut(key) {
                    *property_value =
                        coerce_with_json_schema(property_value.take(), property_schema, depth + 1);
                }
            }
        }
        if let Some(additional) = schema
            .get("additionalProperties")
            .filter(|value| value.is_object())
        {
            if let Some(object) = value.as_object_mut() {
                for (key, property_value) in object.iter_mut() {
                    if defined_keys.iter().any(|defined| defined == key) {
                        continue;
                    }
                    *property_value =
                        coerce_with_json_schema(property_value.take(), additional, depth + 1);
                }
            }
        }
    }

    if types.iter().any(|type_name| type_name == "array") && value.is_array() {
        let items = schema.get("items").cloned();
        if let Some(array) = value.as_array_mut() {
            match items {
                Some(serde_json::Value::Array(item_schemas)) => {
                    for (index, item) in array.iter_mut().enumerate() {
                        if let Some(item_schema) = item_schemas.get(index) {
                            *item = coerce_with_json_schema(item.take(), item_schema, depth + 1);
                        }
                    }
                }
                Some(items) if items.is_object() => {
                    for item in array.iter_mut() {
                        *item = coerce_with_json_schema(item.take(), &items, depth + 1);
                    }
                }
                _ => {}
            }
        }
    }

    value
}

// ---------------------------------------------------------------------
// JSON-Schema check emitting TypeBox error shapes
// ---------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum TypeSpec {
    Single(String),
    Multi(Vec<String>),
}

#[derive(Debug, Clone, PartialEq)]
enum ErrorParams {
    None,
    Type(TypeSpec),
    Required(Vec<String>),
    Comparison {
        comparison: &'static str,
        limit: serde_json::Number,
    },
    Limit(serde_json::Number),
    MultipleOf(serde_json::Number),
}

/// A validation error mirroring the TypeBox `TLocalizedValidationError` fields
/// upstream uses: `keyword`, `instancePath`, and the `params` needed for the
/// `en_US` message and for `formatValidationPath`.
#[derive(Debug, Clone, PartialEq)]
struct ValidationError {
    keyword: &'static str,
    instance_path: String,
    params: ErrorParams,
}

impl ValidationError {
    fn message(&self) -> String {
        match &self.params {
            ErrorParams::None => match self.keyword {
                "additionalProperties" => "must not have additional properties".to_string(),
                "anyOf" => "must match a schema in anyOf".to_string(),
                "boolean" => "schema is false".to_string(),
                "const" => "must be equal to constant".to_string(),
                "enum" => "must be equal to one of the allowed values".to_string(),
                "oneOf" => "must match exactly one schema in oneOf".to_string(),
                _ => "an unknown validation error occurred".to_string(),
            },
            ErrorParams::Type(TypeSpec::Single(type_name)) => format!("must be {type_name}"),
            ErrorParams::Type(TypeSpec::Multi(types)) => {
                format!("must be either {}", types.join(" or "))
            }
            ErrorParams::Required(required) => {
                format!("must have required properties {}", required.join(", "))
            }
            ErrorParams::Comparison { comparison, limit } => {
                format!("must be {comparison} {limit}")
            }
            ErrorParams::Limit(limit) => {
                let bound = if matches!(self.keyword, "maxLength" | "maxItems") {
                    "more"
                } else {
                    "fewer"
                };
                let unit = if matches!(self.keyword, "maxLength" | "minLength") {
                    "characters"
                } else {
                    "items"
                };
                format!("must not have {bound} than {limit} {unit}")
            }
            ErrorParams::MultipleOf(multiple_of) => format!("must be multiple of {multiple_of}"),
        }
    }
}

fn push_error(
    errors: &mut Vec<ValidationError>,
    keyword: &'static str,
    instance_path: &str,
    params: ErrorParams,
) {
    errors.push(ValidationError {
        keyword,
        instance_path: instance_path.to_string(),
        params,
    });
}

/// Upstream `formatValidationPath` (validation.ts:282-293): dotted instance
/// path (`/a/b` to `a.b`, empty to `root`); `required` errors are reported at
/// the first missing property.
fn format_validation_path(error: &ValidationError) -> String {
    let dotted = |instance_path: &str| -> String {
        instance_path
            .strip_prefix('/')
            .unwrap_or(instance_path)
            .replace('/', ".")
    };
    if error.keyword == "required" {
        if let ErrorParams::Required(required) = &error.params {
            if let Some(first) = required.first() {
                let base_path = dotted(&error.instance_path);
                return if base_path.is_empty() {
                    first.clone()
                } else {
                    format!("{base_path}.{first}")
                };
            }
        }
    }
    let path = dotted(&error.instance_path);
    if path.is_empty() {
        "root".to_string()
    } else {
        path
    }
}

/// Standalone check: the schema is its own `$ref` resolution root, matching
/// upstream's `Compile(schema)` for sub-schemas.
fn check_standalone(schema: &serde_json::Value, value: &serde_json::Value) -> bool {
    check_schema(schema, value, "", &mut Vec::new(), schema, 0)
}

/// JSON-Schema check with TypeBox-compatible error emission. `root` is the
/// schema document used to resolve `$ref`; `path` is the JSON pointer of
/// `value`. Returns whether the value validates; failing sub-schemas push
/// into `errors`.
fn check_schema(
    schema: &serde_json::Value,
    value: &serde_json::Value,
    path: &str,
    errors: &mut Vec<ValidationError>,
    root: &serde_json::Value,
    depth: usize,
) -> bool {
    if depth > MAX_SCHEMA_DEPTH {
        return false;
    }
    if let Some(flag) = schema.as_bool() {
        if !flag {
            push_error(errors, "boolean", path, ErrorParams::None);
            return false;
        }
        return true;
    }
    let Some(schema) = schema.as_object() else {
        return true;
    };

    let mut valid = true;

    // type (always first, evaluated even when other keywords fail)
    if let Some(type_spec) = schema.get("type") {
        let (matches, spec) = match type_spec {
            serde_json::Value::String(type_name) => (
                matches_json_type(value, type_name),
                TypeSpec::Single(type_name.clone()),
            ),
            serde_json::Value::Array(types) => {
                let names: Vec<String> = types
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::to_string)
                    .collect();
                let matches = names
                    .iter()
                    .any(|type_name| matches_json_type(value, type_name));
                (matches, TypeSpec::Multi(names))
            }
            _ => (true, TypeSpec::Single(String::new())),
        };
        if !matches {
            valid = false;
            push_error(errors, "type", path, ErrorParams::Type(spec));
        }
    }

    if value.is_object() {
        let object = value.as_object().expect("checked above");
        // required
        if let Some(required) = schema.get("required").and_then(serde_json::Value::as_array) {
            let missing: Vec<String> = required
                .iter()
                .filter_map(serde_json::Value::as_str)
                .filter(|key| !object.contains_key(*key))
                .map(str::to_string)
                .collect();
            if !missing.is_empty() {
                valid = false;
                push_error(errors, "required", path, ErrorParams::Required(missing));
            }
        }
        // additionalProperties (before properties, matching TypeBox order)
        let defined_keys: Vec<&String> = schema
            .get("properties")
            .and_then(serde_json::Value::as_object)
            .map(|properties| properties.keys().collect())
            .unwrap_or_default();
        let is_additional = |key: &str| !defined_keys.iter().any(|defined| defined.as_str() == key);
        match schema.get("additionalProperties") {
            Some(serde_json::Value::Bool(false)) => {
                let offenders: Vec<String> = object
                    .keys()
                    .filter(|key| is_additional(key))
                    .cloned()
                    .collect();
                if !offenders.is_empty() {
                    valid = false;
                    for key in &offenders {
                        push_error(
                            errors,
                            "boolean",
                            &format!("{path}/{key}"),
                            ErrorParams::None,
                        );
                    }
                    push_error(errors, "additionalProperties", path, ErrorParams::None);
                }
            }
            Some(additional_schema) if additional_schema.is_object() => {
                let mut offenders = 0;
                for (key, property_value) in object {
                    if is_additional(key)
                        && !check_schema(
                            additional_schema,
                            property_value,
                            &format!("{path}/{key}"),
                            errors,
                            root,
                            depth + 1,
                        )
                    {
                        offenders += 1;
                    }
                }
                if offenders > 0 {
                    valid = false;
                    push_error(errors, "additionalProperties", path, ErrorParams::None);
                }
            }
            _ => {}
        }
        // properties
        if let Some(properties) = schema
            .get("properties")
            .and_then(serde_json::Value::as_object)
        {
            for (key, property_schema) in properties {
                if let Some(property_value) = object.get(key) {
                    if !check_schema(
                        property_schema,
                        property_value,
                        &format!("{path}/{key}"),
                        errors,
                        root,
                        depth + 1,
                    ) {
                        valid = false;
                    }
                }
            }
        }
    }

    if let Some(array) = value.as_array() {
        match schema.get("items") {
            Some(serde_json::Value::Array(item_schemas)) => {
                for (index, item) in array.iter().enumerate() {
                    if let Some(item_schema) = item_schemas.get(index) {
                        if !check_schema(
                            item_schema,
                            item,
                            &format!("{path}/{index}"),
                            errors,
                            root,
                            depth + 1,
                        ) {
                            valid = false;
                        }
                    }
                }
            }
            Some(item_schema) if item_schema.is_object() => {
                for (index, item) in array.iter().enumerate() {
                    if !check_schema(
                        item_schema,
                        item,
                        &format!("{path}/{index}"),
                        errors,
                        root,
                        depth + 1,
                    ) {
                        valid = false;
                    }
                }
            }
            _ => {}
        }
        if let Some(serde_json::Value::Number(limit)) = schema.get("maxItems") {
            if (array.len() as f64) > limit.as_f64().unwrap_or(f64::INFINITY) {
                valid = false;
                push_error(errors, "maxItems", path, ErrorParams::Limit(limit.clone()));
            }
        }
        if let Some(serde_json::Value::Number(limit)) = schema.get("minItems") {
            if (array.len() as f64) < limit.as_f64().unwrap_or(f64::NEG_INFINITY) {
                valid = false;
                push_error(errors, "minItems", path, ErrorParams::Limit(limit.clone()));
            }
        }
    }

    if let Some(text) = value.as_str() {
        let length = text.encode_utf16().count() as f64;
        if let Some(serde_json::Value::Number(limit)) = schema.get("maxLength") {
            if length > limit.as_f64().unwrap_or(f64::INFINITY) {
                valid = false;
                push_error(errors, "maxLength", path, ErrorParams::Limit(limit.clone()));
            }
        }
        if let Some(serde_json::Value::Number(limit)) = schema.get("minLength") {
            if length < limit.as_f64().unwrap_or(f64::NEG_INFINITY) {
                valid = false;
                push_error(errors, "minLength", path, ErrorParams::Limit(limit.clone()));
            }
        }
    }

    if let Some(number) = value.as_number() {
        let n = number.as_f64().unwrap_or_default();
        let mut compare = |errors: &mut Vec<ValidationError>,
                           keyword: &'static str,
                           bound: &serde_json::Value,
                           ok: bool,
                           comparison: &'static str| {
            if let Some(limit) = bound.as_number() {
                if !ok {
                    valid = false;
                    push_error(
                        errors,
                        keyword,
                        path,
                        ErrorParams::Comparison {
                            comparison,
                            limit: limit.clone(),
                        },
                    );
                }
            }
        };
        if let Some(bound) = schema.get("exclusiveMaximum") {
            compare(
                errors,
                "exclusiveMaximum",
                bound,
                n < bound.as_f64().unwrap_or(f64::INFINITY),
                "<",
            );
        }
        if let Some(bound) = schema.get("exclusiveMinimum") {
            compare(
                errors,
                "exclusiveMinimum",
                bound,
                n > bound.as_f64().unwrap_or(f64::NEG_INFINITY),
                ">",
            );
        }
        if let Some(bound) = schema.get("maximum") {
            compare(
                errors,
                "maximum",
                bound,
                n <= bound.as_f64().unwrap_or(f64::INFINITY),
                "<=",
            );
        }
        if let Some(bound) = schema.get("minimum") {
            compare(
                errors,
                "minimum",
                bound,
                n >= bound.as_f64().unwrap_or(f64::NEG_INFINITY),
                ">=",
            );
        }
        if let Some(serde_json::Value::Number(multiple_of)) = schema.get("multipleOf") {
            let divisor = multiple_of.as_f64().unwrap_or_default();
            if divisor != 0.0 && n % divisor != 0.0 {
                valid = false;
                push_error(
                    errors,
                    "multipleOf",
                    path,
                    ErrorParams::MultipleOf(multiple_of.clone()),
                );
            }
        }
    }

    // $ref: resolves against `root`; unresolvable refs act like a `false`
    // schema (upstream `stack.Ref(schema) ?? false`).
    if let Some(reference) = schema.get("$ref").and_then(serde_json::Value::as_str) {
        match resolve_pointer(root, reference) {
            Some(target) => {
                if !check_schema(target, value, path, errors, root, depth + 1) {
                    valid = false;
                }
            }
            None => {
                valid = false;
                push_error(errors, "boolean", path, ErrorParams::None);
            }
        }
    }

    if let Some(const_value) = schema.get("const") {
        if value != const_value {
            valid = false;
            push_error(errors, "const", path, ErrorParams::None);
        }
    }
    if let Some(allowed) = schema.get("enum").and_then(serde_json::Value::as_array) {
        if !allowed.contains(value) {
            valid = false;
            push_error(errors, "enum", path, ErrorParams::None);
        }
    }
    if let Some(arms) = schema.get("allOf").and_then(serde_json::Value::as_array) {
        for arm in arms {
            let mut arm_errors = Vec::new();
            if !check_schema(arm, value, path, &mut arm_errors, root, depth + 1) {
                valid = false;
                errors.append(&mut arm_errors);
            }
        }
    }
    for keyword in ["anyOf", "oneOf"] {
        if let Some(arms) = schema.get(keyword).and_then(serde_json::Value::as_array) {
            let mut passing = 0;
            let mut failed: Vec<ValidationError> = Vec::new();
            for arm in arms {
                let mut arm_errors = Vec::new();
                if check_schema(arm, value, path, &mut arm_errors, root, depth + 1) {
                    passing += 1;
                } else {
                    failed.append(&mut arm_errors);
                }
            }
            let is_valid = if keyword == "anyOf" {
                passing > 0
            } else {
                passing == 1
            };
            if !is_valid {
                valid = false;
                // Upstream merges failed-arm errors only when nothing matched;
                // an ambiguous oneOf reports just the union error.
                if keyword == "anyOf" || passing == 0 {
                    errors.append(&mut failed);
                }
                push_error(
                    errors,
                    if keyword == "anyOf" { "anyOf" } else { "oneOf" },
                    path,
                    ErrorParams::None,
                );
            }
        }
    }

    valid
}

/// Resolves a local JSON pointer reference like `#/$defs/value` against the
/// root schema document.
fn resolve_pointer<'a>(
    root: &'a serde_json::Value,
    reference: &str,
) -> Option<&'a serde_json::Value> {
    let pointer = reference.strip_prefix('#')?;
    let mut target = root;
    if pointer.is_empty() {
        return Some(target);
    }
    for segment in pointer.strip_prefix('/')?.split('/') {
        let segment = segment.replace("~1", "/").replace("~0", "~");
        target = target.as_object()?.get(&segment)?;
    }
    Some(target)
}

#[cfg(test)]
mod tests {
    #[test]
    fn optional_null_deletion_preserves_surviving_key_order() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "drop": {"type": "string"},
                "nested": {"type": "object", "properties": {
                    "drop": {"type": "string"}
                }}
            }
        });
        let mut value: serde_json::Value =
            serde_json::from_str(r#"{"drop":null,"z":1,"a":2,"nested":{"drop":null,"y":3,"b":4}}"#)
                .unwrap();
        super::normalize_optional_nulls(&mut value, &schema);
        assert_eq!(
            serde_json::to_string(&value).unwrap(),
            r#"{"z":1,"a":2,"nested":{"y":3,"b":4}}"#
        );
    }

    use super::*;
    use crate::ai::types::content::ToolCall;
    use crate::ai::types::tool::Tool;
    use serde_json::json;

    fn tool(parameters: serde_json::Value) -> Tool {
        Tool {
            name: "echo".to_string(),
            description: "Echo tool".to_string(),
            parameters,
            constrained_sampling: None,
        }
    }

    fn tool_call(arguments: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "tool-1".to_string(),
            name: "echo".to_string(),
            arguments,
            thought_signature: None,
            namespace: None,
        }
    }

    fn object_tool(property_schema: serde_json::Value) -> Tool {
        tool(json!({
            "type": "object",
            "properties": { "value": property_schema },
            "required": ["value"],
        }))
    }

    #[test]
    fn unknown_tool_name_errors() {
        let tools = vec![tool(json!({"type": "object", "properties": {}}))];
        let call = ToolCall {
            name: "nope".to_string(),
            ..tool_call(json!({}))
        };
        let error = validate_tool_call(&tools, &call).unwrap_err();
        assert_eq!(error, "Tool \"nope\" not found");
    }

    #[test]
    fn missing_required_property_errors_with_upstream_shape() {
        let tool = tool(json!({
            "type": "object",
            "properties": { "value": { "type": "number" } },
            "required": ["value"],
        }));
        let error = validate_tool_call(&[tool], &tool_call(json!({}))).unwrap_err();
        assert_eq!(
            error,
            "Validation failed for tool \"echo\":\n  - value: must have required properties value\n\nReceived arguments:\n{}"
        );
    }

    #[test]
    fn missing_required_property_reports_first_missing_and_lists_all() {
        let tool = tool(json!({ "type": "object", "properties": {}, "required": ["a", "b"] }));
        let error = validate_tool_call(&[tool], &tool_call(json!({}))).unwrap_err();
        assert_eq!(
            error,
            "Validation failed for tool \"echo\":\n  - a: must have required properties a, b\n\nReceived arguments:\n{}"
        );
    }

    #[test]
    fn nested_required_path_is_dotted() {
        let tool = tool(json!({
            "type": "object",
            "properties": {
                "metadata": {
                    "type": "object",
                    "properties": { "enabled": { "type": "boolean" } },
                    "required": ["enabled"],
                }
            },
            "required": ["metadata"],
        }));
        let error = validate_tool_call(&[tool], &tool_call(json!({ "metadata": {} }))).unwrap_err();
        assert_eq!(
            error,
            "Validation failed for tool \"echo\":\n  - metadata.enabled: must have required properties enabled\n\nReceived arguments:\n{\n  \"metadata\": {}\n}"
        );
    }

    #[test]
    fn type_mismatch_errors_with_upstream_shape() {
        let tool = tool(json!({
            "type": "object",
            "properties": { "value": { "type": "number" } },
            "required": ["value"],
        }));
        let error = validate_tool_call(&[tool], &tool_call(json!({ "value": "abc" }))).unwrap_err();
        assert_eq!(
            error,
            "Validation failed for tool \"echo\":\n  - value: must be number\n\nReceived arguments:\n{\n  \"value\": \"abc\"\n}"
        );
    }

    #[test]
    fn root_type_mismatch_reports_root_path() {
        let tool = tool(json!({ "type": "string" }));
        let error = validate_tool_arguments(&tool, &tool_call(json!({}))).unwrap_err();
        assert_eq!(
            error,
            "Validation failed for tool \"echo\":\n  - root: must be string\n\nReceived arguments:\n{}"
        );
    }

    #[test]
    fn additional_properties_false_reports_per_key_then_aggregate() {
        let tool = tool(json!({
            "type": "object",
            "properties": { "command": { "type": "string" } },
            "required": ["command"],
            "additionalProperties": false,
        }));
        let error = validate_tool_call(
            &[tool],
            &tool_call(json!({ "command": "x", "extra": 1, "other": "y" })),
        )
        .unwrap_err();
        assert_eq!(
            error,
            "Validation failed for tool \"echo\":\n  - extra: schema is false\n  - other: schema is false\n  - root: must not have additional properties\n\nReceived arguments:\n{\n  \"command\": \"x\",\n  \"extra\": 1,\n  \"other\": \"y\"\n}"
        );
    }

    #[test]
    fn any_of_failure_reports_arm_errors_then_union_error() {
        let tool = tool(json!({
            "type": "object",
            "properties": { "value": { "anyOf": [{ "type": "number" }, { "type": "null" }] } },
            "required": ["value"],
        }));
        let error = validate_tool_call(&[tool], &tool_call(json!({ "value": "abc" }))).unwrap_err();
        assert_eq!(
            error,
            "Validation failed for tool \"echo\":\n  - value: must be number\n  - value: must be null\n  - value: must match a schema in anyOf\n\nReceived arguments:\n{\n  \"value\": \"abc\"\n}"
        );
    }

    #[test]
    fn multiple_errors_keep_typebox_order() {
        let tool = tool(json!({
            "type": "object",
            "properties": {
                "a": { "type": "number" },
                "b": { "type": "string" },
                "c": { "type": "boolean" },
            },
            "required": ["a", "b", "c", "d"],
        }));
        let error =
            validate_tool_call(&[tool], &tool_call(json!({ "a": "x", "b": 1 }))).unwrap_err();
        assert_eq!(
            error,
            "Validation failed for tool \"echo\":\n  - c: must have required properties c, d\n  - a: must be number\n\nReceived arguments:\n{\n  \"a\": \"x\",\n  \"b\": 1\n}"
        );
    }

    #[test]
    fn enum_const_and_bound_errors_use_typebox_messages() {
        let tool = tool(json!({
            "type": "object",
            "properties": {
                "v": { "type": "string", "enum": ["a", "b"] },
                "n": { "type": "number", "minimum": 5 },
                "s": { "type": "string", "minLength": 3 },
                "c": { "const": "fixed" },
            },
            "required": ["v", "n", "s", "c"],
        }));
        let error = validate_tool_call(
            &[tool],
            &tool_call(json!({ "v": "z", "n": 3, "s": "ab", "c": "other" })),
        )
        .unwrap_err();
        // Property errors follow the schema's declaration order, as in TypeBox.
        let expected = [
            "  - v: must be equal to one of the allowed values",
            "  - n: must be >= 5",
            "  - s: must not have fewer than 3 characters",
            "  - c: must be equal to constant",
        ]
        .join("\n");
        assert!(
            error.starts_with(&format!(
                "Validation failed for tool \"echo\":\n{expected}\n\nReceived arguments:"
            )),
            "{error}"
        );
    }

    #[test]
    fn one_of_ambiguous_reports_only_the_union_error() {
        let tool = tool(json!({
            "type": "object",
            "properties": { "v": { "oneOf": [{ "type": "number" }, { "type": "integer" }] } },
            "required": ["v"],
        }));
        let error = validate_tool_call(&[tool], &tool_call(json!({ "v": 3 }))).unwrap_err();
        assert_eq!(
            error,
            "Validation failed for tool \"echo\":\n  - v: must match exactly one schema in oneOf\n\nReceived arguments:\n{\n  \"v\": 3\n}"
        );
    }

    #[test]
    fn one_of_coerces_through_a_matching_arm_when_nothing_matches_as_is() {
        let tool = tool(json!({
            "type": "object",
            "properties": { "v": { "oneOf": [{ "type": "string" }, { "type": "null" }] } },
            "required": ["v"],
        }));
        let validated = validate_tool_call(&[tool], &tool_call(json!({ "v": 3 }))).unwrap();
        assert_eq!(validated.arguments, json!({ "v": "3" }));
    }

    #[test]
    fn items_error_path_uses_dotted_index() {
        let tool = tool(json!({
            "type": "object",
            "properties": { "list": { "type": "array", "items": { "type": "number" } } },
            "required": ["list"],
        }));
        let error =
            validate_tool_call(&[tool], &tool_call(json!({ "list": [1, "x", 3] }))).unwrap_err();
        assert_eq!(
            error,
            "Validation failed for tool \"echo\":\n  - list.1: must be number\n\nReceived arguments:\n{\n  \"list\": [\n    1,\n    \"x\",\n    3\n  ]\n}"
        );
    }

    #[test]
    fn unresolvable_ref_behaves_like_a_false_schema() {
        let tool = tool(json!({
            "type": "object",
            "properties": { "v": { "$ref": "https://example.com/x.json" } },
            "required": ["v"],
        }));
        let error = validate_tool_call(&[tool], &tool_call(json!({ "v": null }))).unwrap_err();
        assert!(
            error.starts_with("Validation failed for tool \"echo\":\n  - v: schema is false\n"),
            "{error}"
        );
    }

    #[test]
    fn coerces_serialized_plain_json_schemas_with_ajv_compatible_primitive_rules() {
        let cases: Vec<(serde_json::Value, serde_json::Value, serde_json::Value)> = vec![
            (json!({ "type": "number" }), json!("42"), json!(42)),
            (json!({ "type": "number" }), json!(true), json!(1)),
            (json!({ "type": "number" }), json!(null), json!(0)),
            (json!({ "type": "integer" }), json!("42"), json!(42)),
            (json!({ "type": "boolean" }), json!("true"), json!(true)),
            (json!({ "type": "boolean" }), json!("false"), json!(false)),
            (json!({ "type": "boolean" }), json!(1), json!(true)),
            (json!({ "type": "boolean" }), json!(0), json!(false)),
            (json!({ "type": "string" }), json!(null), json!("")),
            (json!({ "type": "string" }), json!(true), json!("true")),
            (json!({ "type": "null" }), json!(""), json!(null)),
            (json!({ "type": "null" }), json!(0), json!(null)),
            (json!({ "type": "null" }), json!(false), json!(null)),
            (
                json!({ "type": ["number", "string"] }),
                json!("1"),
                json!("1"),
            ),
            (
                json!({ "type": ["boolean", "number"] }),
                json!("1"),
                json!(1),
            ),
        ];
        for (property_schema, input, expected) in cases {
            let tool = object_tool(property_schema.clone());
            let validated =
                validate_tool_arguments(&tool, &tool_call(json!({ "value": input.clone() })))
                    .unwrap();
            assert_eq!(
                validated,
                json!({ "value": expected }),
                "schema {property_schema} input {input}"
            );
        }
    }

    #[test]
    fn rejects_invalid_coercions_for_serialized_plain_json_schemas() {
        let cases = vec![
            (json!({ "type": "boolean" }), json!("1")),
            (json!({ "type": "boolean" }), json!("0")),
            (json!({ "type": "null" }), json!("null")),
            (json!({ "type": "integer" }), json!("42.1")),
        ];
        for (property_schema, input) in cases {
            let tool = object_tool(property_schema.clone());
            let error =
                validate_tool_arguments(&tool, &tool_call(json!({ "value": input.clone() })))
                    .unwrap_err();
            assert!(
                error.starts_with("Validation failed"),
                "schema {property_schema} input {input}"
            );
        }
    }

    #[test]
    fn treats_null_as_omission_for_optional_non_nullable_properties() {
        let tool = tool(json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "offset": { "type": "number" },
                "nullable": { "anyOf": [{ "type": "string" }, { "type": "null" }] },
                "metadata": { "type": "object", "properties": { "enabled": { "type": "boolean" } } },
            },
            "required": ["path"],
        }));
        let validated = validate_tool_arguments(
            &tool,
            &tool_call(json!({ "path": "file.txt", "offset": null, "nullable": null, "metadata": { "enabled": null } })),
        )
        .unwrap();
        assert_eq!(
            validated,
            json!({ "path": "file.txt", "nullable": null, "metadata": {} })
        );
    }

    #[test]
    fn preserves_optional_nulls_whose_referenced_schema_is_nullable() {
        let tool = tool(json!({
            "type": "object",
            "properties": { "value": { "$ref": "#/$defs/value" } },
            "$defs": { "value": { "anyOf": [{ "type": "number" }, { "type": "null" }] } },
        }));
        let validated =
            validate_tool_arguments(&tool, &tool_call(json!({ "value": null }))).unwrap();
        assert_eq!(validated, json!({ "value": null }));
    }

    #[test]
    fn preserves_a_value_that_already_matches_a_nullable_union_arm() {
        let tool = tool(json!({
            "type": "object",
            "properties": { "value": { "anyOf": [{ "type": "number" }, { "type": "null" }] } },
            "required": ["value"],
        }));
        let validated =
            validate_tool_arguments(&tool, &tool_call(json!({ "value": null }))).unwrap();
        assert_eq!(validated, json!({ "value": null }));
    }

    #[test]
    fn preserves_a_value_that_already_matches_a_one_of_nullable_union_arm() {
        let tool = object_tool(json!({ "oneOf": [{ "type": "number" }, { "type": "null" }] }));
        let validated =
            validate_tool_arguments(&tool, &tool_call(json!({ "value": null }))).unwrap();
        assert_eq!(validated, json!({ "value": null }));
    }

    #[test]
    fn still_coerces_nullable_unions_when_the_original_value_does_not_match_any_arm() {
        let tool = object_tool(json!({ "anyOf": [{ "type": "number" }, { "type": "null" }] }));
        let validated =
            validate_tool_arguments(&tool, &tool_call(json!({ "value": "42" }))).unwrap();
        assert_eq!(validated, json!({ "value": 42 }));
    }

    #[test]
    fn accepts_null_for_nullable_array_schemas_with_items() {
        let tool = object_tool(json!({ "type": ["array", "null"], "items": { "type": "string" } }));
        let validated =
            validate_tool_arguments(&tool, &tool_call(json!({ "value": null }))).unwrap();
        assert_eq!(validated, json!({ "value": null }));
    }

    #[test]
    fn coerces_array_items() {
        let tool = tool(json!({
            "type": "object",
            "properties": { "list": { "type": "array", "items": { "type": "number" } } },
            "required": ["list"],
        }));
        let validated =
            validate_tool_call(&[tool], &tool_call(json!({ "list": ["1", 2] }))).unwrap();
        assert_eq!(validated.arguments, json!({ "list": [1, 2] }));
    }

    #[test]
    fn valid_arguments_pass_through_with_id_and_name_preserved() {
        let tools = vec![tool(json!({
            "type": "object",
            "properties": { "command": { "type": "string" } },
            "required": ["command"],
        }))];
        let call = ToolCall {
            id: "call_9".to_string(),
            name: "echo".to_string(),
            arguments: json!({ "command": "ls" }),
            thought_signature: Some("sig".to_string()),
            namespace: Some("ns".to_string()),
        };
        let validated = validate_tool_call(&tools, &call).unwrap();
        assert_eq!(validated.id, "call_9");
        assert_eq!(validated.name, "echo");
        assert_eq!(validated.thought_signature.as_deref(), Some("sig"));
        assert_eq!(validated.namespace.as_deref(), Some("ns"));
        assert_eq!(validated.arguments, json!({ "command": "ls" }));
    }
}
