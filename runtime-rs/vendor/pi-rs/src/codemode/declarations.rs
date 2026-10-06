//! Port of upstream `codemode/src/declarations.ts` (verbatim logic) —
//! TypeScript declaration rendering for the script-visible API.
//!
//! Byte-parity notes (disclosed): `JSON.stringify` of schema `const`/`enum`
//! values uses `serde_json` (`preserve_order`), which matches V8 for
//! JSON-representable values except exotic float spellings (`1e+21` renders
//! `1e21`, `-0` renders `-0.0`); property-name sorting is UTF-8 byte order
//! (differs from JS UTF-16 order only for astral-vs-U+E000..U+FFFF keys), and
//! `decodeURIComponent` failures inside `$ref` pointers decode leniently
//! where upstream would throw.

use std::collections::HashSet;

use serde_json::Value;

use super::identifier::to_codemode_identifier;
use super::types::CodemodeJsonSchema;

fn is_identifier_char_start(char: char) -> bool {
    char.is_ascii_alphabetic() || char == '_' || char == '$'
}

fn is_identifier_char(char: char) -> bool {
    char.is_ascii_alphanumeric() || char == '_' || char == '$'
}

fn matches_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => is_identifier_char_start(first) && chars.all(is_identifier_char),
        None => false,
    }
}

const INDENT: &str = "  ";
/// Largest rendered input type, in characters, before it becomes `unknown`.
pub const DEFAULT_INPUT_SCHEMA_MAX_CHARS: usize = 16_000;
/// Local `$ref` expansions per rendered schema, so shared definitions cannot
/// blow up the output.
const MAX_REF_EXPANSIONS: usize = 32;

/// TypeScript types for MCP results, from the MCP `CallToolResult` schema, so
/// `CallToolResult<T>` declarations can refer to them (upstream
/// `MCP_TYPESCRIPT_PREAMBLE`, byte-exact).
pub const MCP_TYPESCRIPT_PREAMBLE: &str = "type Role = \"user\" | \"assistant\";
type MetaObject = Record<string, unknown>;
type Annotations = {
  audience?: Role[];
  priority?: number;
  lastModified?: string;
};
type Icon = {
  src: string;
  mimeType?: string;
  sizes?: string[];
  theme?: \"light\" | \"dark\";
};
type TextResourceContents = {
  uri: string;
  mimeType?: string;
  _meta?: MetaObject;
  text: string;
};
type BlobResourceContents = {
  uri: string;
  mimeType?: string;
  _meta?: MetaObject;
  blob: string;
};
type TextContent = {
  type: \"text\";
  text: string;
  annotations?: Annotations;
  _meta?: MetaObject;
};
type ImageContent = {
  type: \"image\";
  data: string;
  mimeType: string;
  annotations?: Annotations;
  _meta?: MetaObject;
};
type AudioContent = {
  type: \"audio\";
  data: string;
  mimeType: string;
  annotations?: Annotations;
  _meta?: MetaObject;
};
type ResourceLink = {
  icons?: Icon[];
  name: string;
  title?: string;
  uri: string;
  description?: string;
  mimeType?: string;
  annotations?: Annotations;
  size?: number;
  _meta?: MetaObject;
  type: \"resource_link\";
};
type EmbeddedResource = {
  type: \"resource\";
  resource: TextResourceContents | BlobResourceContents;
  annotations?: Annotations;
  _meta?: MetaObject;
};
type ContentBlock =
  | TextContent
  | ImageContent
  | AudioContent
  | ResourceLink
  | EmbeddedResource;
type CallToolResult<TStructured = { [key: string]: unknown }> = {
  _meta?: MetaObject;
  content: ContentBlock[];
  isError?: boolean;
  structuredContent?: TStructured;
  [key: string]: unknown;
};";

/// Tool plus globals for [`render_declarations`] (upstream
/// `RenderDeclarationsOptions`).
#[derive(Default)]
pub struct RenderDeclarationsOptions<'a> {
    pub tools: &'a [Declarable],
    pub globals: &'a [Declarable],
}

/// The declaration-relevant part of a tool (upstream `CodemodeTool` without
/// `execute`).
#[derive(Debug, Clone, Default)]
pub struct Declarable {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: Option<CodemodeJsonSchema>,
    pub output_schema: Option<CodemodeJsonSchema>,
    pub spread: bool,
    pub signature: Option<String>,
}

impl Declarable {
    pub fn new(name: &str) -> Self {
        Declarable {
            name: name.to_string(),
            ..Default::default()
        }
    }
}

/// Render TypeScript declarations for the script-visible API. Tools become
/// members of `declare const tools`, globals become `declare function`
/// statements, and `ns.member` globals members of `declare const ns`.
pub fn render_declarations(options: RenderDeclarationsOptions<'_>) -> String {
    let mut sections: Vec<String> = Vec::new();
    if !options.tools.is_empty() {
        let members = options
            .tools
            .iter()
            .map(|tool| {
                format!(
                    "{}{}{}",
                    doc_comment(tool.description.as_deref(), INDENT),
                    INDENT,
                    render_tool_signature(tool, None)
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        sections.push(format!("declare const tools: {{\n{members}\n}};"));
    }
    let mut namespaces: Vec<(String, Vec<String>)> = Vec::new();
    for global in options.globals {
        let dot = global.name.find('.');
        let Some(dot) = dot else {
            sections.push(render_global(
                &format!("declare function {}", global.name),
                global,
                "",
            ));
            continue;
        };
        let namespace = global.name[..dot].to_string();
        let member = render_global(&global.name[dot + 1..], global, INDENT);
        match namespaces.iter_mut().find(|(name, _)| *name == namespace) {
            Some((_, members)) => members.push(member),
            None => namespaces.push((namespace, vec![member])),
        }
    }
    // Upstream iterates the `namespaces` Map in first-insertion order.
    for (namespace, members) in namespaces {
        sections.push(format!(
            "declare const {namespace}: {{\n{}\n}};",
            members.join("\n")
        ));
    }
    sections.join("\n\n")
}

/// One tool as a member of the `tools` object:
/// `name(args: T): Promise<R>;`. Input types longer than `input_max_chars`
/// render as `unknown`.
pub fn render_tool_signature(tool: &Declarable, input_max_chars: Option<usize>) -> String {
    let input = match &tool.input_schema {
        None => "unknown".to_string(),
        Some(schema) => schema_to_type(
            schema,
            Some(input_max_chars.unwrap_or(DEFAULT_INPUT_SCHEMA_MAX_CHARS)),
        ),
    };
    format!(
        "{}(args: {input}): Promise<{}>;",
        to_codemode_identifier(&tool.name),
        render_tool_output_type(tool.output_schema.as_ref())
    )
}

/// A tool's sample: the description followed by the tool's declaration. Used
/// for tool listings and `ALL_TOOLS` entries.
pub fn render_tool_sample(tool: &Declarable, input_max_chars: Option<usize>) -> String {
    let declaration = format!(
        "declare const tools: {{ {} }};",
        render_tool_signature(tool, input_max_chars)
    );
    let description = tool
        .description
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();
    format!("{description}\n\ncodemode tool declaration:\n```ts\n{declaration}\n```")
}

/// The `structuredContent` schema of an MCP `CallToolResult` output schema
/// (detected by a `content` array of objects, boolean `isError`, and object
/// `_meta`), `Some(Value::Bool(true))` when it declares none, or `None` when
/// the schema is not a `CallToolResult`.
pub fn mcp_structured_content_schema(
    schema: Option<&CodemodeJsonSchema>,
) -> Option<CodemodeJsonSchema> {
    let schema = is_object(schema)?;
    let properties = schema.get("properties")?;
    let properties = is_object(Some(properties))?;
    let content = properties.get("content")?;
    let content = is_object(Some(content))?;
    if content.get("type").and_then(Value::as_str) != Some("array") {
        return None;
    }
    let items = is_object(content.get("items"))?;
    if items.get("type").and_then(Value::as_str) != Some("object") {
        return None;
    }
    let is_error = is_object(properties.get("isError"))?;
    if is_error.get("type").and_then(Value::as_str) != Some("boolean") {
        return None;
    }
    let meta = is_object(properties.get("_meta"))?;
    if meta.get("type").and_then(Value::as_str) != Some("object") {
        return None;
    }
    let structured_content = properties.get("structuredContent");
    match structured_content {
        Some(value) if is_object(Some(value)).is_some() => Some(value.clone()),
        Some(Value::Bool(_)) => Some(structured_content.unwrap().clone()),
        _ => Some(Value::Bool(true)),
    }
}

/// The type a tool call resolves to (upstream `renderToolOutputType`):
/// `CallToolResult<T>` for MCP output schemas (needs
/// [`MCP_TYPESCRIPT_PREAMBLE`]), the schema's type otherwise, and `unknown`
/// without a schema.
pub fn render_tool_output_type(schema: Option<&CodemodeJsonSchema>) -> String {
    if let Some(structured) = mcp_structured_content_schema(schema) {
        let ty = schema_to_type(&structured, None);
        return if ty == "unknown" {
            "CallToolResult".to_string()
        } else {
            format!("CallToolResult<{ty}>")
        };
    }
    match schema {
        None => "unknown".to_string(),
        Some(schema) => schema_to_type(schema, None),
    }
}

fn render_global(head: &str, global: &Declarable, indent: &str) -> String {
    if let Some(signature) = &global.signature {
        return format!(
            "{}{}{head}{signature};",
            doc_comment(global.description.as_deref(), indent),
            indent
        );
    }
    let input = match &global.input_schema {
        None => "unknown".to_string(),
        Some(schema) => schema_to_type(schema, None),
    };
    let output = match &global.output_schema {
        None => "unknown".to_string(),
        Some(schema) => schema_to_type(schema, None),
    };
    format!(
        "{}{}{head}(args: {input}): Promise<{output}>;",
        doc_comment(global.description.as_deref(), indent),
        indent
    )
}

fn doc_comment(description: Option<&str>, indent: &str) -> String {
    let Some(text) = description.map(str::trim).filter(|text| !text.is_empty()) else {
        return String::new();
    };
    let text = text.replace("*/", "*\\/");
    let lines: Vec<&str> = split_lines(&text);
    if lines.len() == 1 {
        return format!("{indent}/** {} */\n", lines[0]);
    }
    let body = lines
        .iter()
        .map(|line| {
            if line.is_empty() {
                format!("{indent} *")
            } else {
                format!("{indent} * {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("{indent}/**\n{body}\n{indent} */\n")
}

/// JS `.split(/\r?\n/)` (does not split on a lone `\r`).
fn split_lines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut rest = text;
    while let Some(index) = rest.find("\r\n").or_else(|| rest.find('\n')) {
        let (line, next) = rest.split_at(index);
        lines.push(line);
        let stripped = next
            .strip_prefix("\r\n")
            .or_else(|| next.strip_prefix('\n'))
            .unwrap_or(next);
        rest = stripped;
    }
    lines.push(rest);
    lines
}

fn property_key(name: &str) -> String {
    if matches_identifier(name) {
        name.to_string()
    } else {
        json_stringify(&Value::String(name.to_string()))
    }
}

fn is_object(value: Option<&Value>) -> Option<&serde_json::Map<String, Value>> {
    value.and_then(Value::as_object)
}

fn union(types: Vec<String>) -> String {
    let mut unique: Vec<String> = Vec::new();
    for ty in types {
        if !unique.contains(&ty) {
            unique.push(ty);
        }
    }
    if unique.iter().any(|ty| ty == "unknown") {
        return "unknown".to_string();
    }
    if unique.is_empty() {
        return "never".to_string();
    }
    unique.join(" | ")
}

/// Upstream `JSON.stringify(value) ?? "unknown"` for schema-level JSON values.
fn json_stringify(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "unknown".to_string())
}

/// Convert a JSON Schema to a TypeScript type expression: objects on one line
/// (`{ a: string; b?: number; }`) with properties sorted by name, or one
/// property per line with `//` comments when a property has a description;
/// `Array<T>` for arrays. Local references (`#/$defs/...`,
/// `#/definitions/...`) resolve against `schema`; recursive and remote
/// references render as `unknown`. A result longer than `max_chars` renders
/// as `unknown`.
pub fn schema_to_type(schema: &CodemodeJsonSchema, max_chars: Option<usize>) -> String {
    let mut context = SchemaContext {
        root: schema,
        resolving: HashSet::new(),
        expansions: 0,
    };
    let ty = to_type(schema, &mut context);
    match max_chars {
        // JS `.length` counts UTF-16 code units.
        Some(max_chars) if ty.encode_utf16().count() > max_chars => "unknown".to_string(),
        _ => ty,
    }
}

struct SchemaContext<'a> {
    root: &'a CodemodeJsonSchema,
    /// References being expanded on the current path, to stop at recursive
    /// types.
    resolving: HashSet<String>,
    expansions: usize,
}

/// JS `decodeURIComponent` + JSON pointer `~1`/`~0` unescaping. Lenient on
/// invalid escapes (upstream would throw a `URIError`).
fn decode_pointer_segment(segment: &str) -> String {
    let segment = segment.replace("~1", "/").replace("~0", "~");
    let bytes = segment.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = bytes
                .get(index + 1..index + 3)
                .and_then(|slice| std::str::from_utf8(slice).ok())
                .and_then(|text| u8::from_str_radix(text, 16).ok());
            if let Some(byte) = hex {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn resolve_ref<'a>(reference: &str, root: &'a Value) -> Option<&'a Value> {
    if reference != "#" && !reference.starts_with("#/") {
        return None;
    }
    let mut current: &Value = root;
    let body = reference.strip_prefix('#').unwrap_or(reference);
    for segment in body
        .strip_prefix('/')
        .unwrap_or(body)
        .split('/')
        .filter(|segment| !segment.is_empty())
    {
        let key = decode_pointer_segment(segment);
        let object = current.as_object()?;
        current = object.get(&key)?;
    }
    match current {
        Value::Bool(_) | Value::Object(_) => Some(current),
        _ => None,
    }
}

fn to_type(schema: &Value, context: &mut SchemaContext<'_>) -> String {
    match schema {
        Value::Bool(true) => "unknown".to_string(),
        Value::Bool(false) => "never".to_string(),
        Value::Object(schema) => to_object_type(schema, context),
        _ => "unknown".to_string(),
    }
}

fn to_object_type(
    schema: &serde_json::Map<String, Value>,
    context: &mut SchemaContext<'_>,
) -> String {
    if let Some(Value::String(reference)) = schema.get("$ref") {
        let reference = reference.clone();
        if context.resolving.contains(&reference) || context.expansions >= MAX_REF_EXPANSIONS {
            return "unknown".to_string();
        }
        let Some(target) = resolve_ref(&reference, context.root) else {
            return "unknown".to_string();
        };
        context.expansions += 1;
        context.resolving.insert(reference.clone());
        let result = to_type(target, context);
        context.resolving.remove(&reference);
        return result;
    }

    if let Some(const_value) = schema.get("const") {
        return json_stringify(const_value);
    }
    if let Some(Value::Array(enum_values)) = schema.get("enum") {
        return union(enum_values.iter().map(json_stringify).collect());
    }

    let variants = schema
        .get("anyOf")
        .filter(|value| value.is_array())
        .or_else(|| schema.get("oneOf").filter(|value| value.is_array()));
    if let Some(Value::Array(variants)) = variants {
        return union(
            variants
                .iter()
                .map(|variant| to_type(variant, context))
                .collect(),
        );
    }
    if let Some(Value::Array(all_of)) = schema.get("allOf") {
        let parts: Vec<String> = all_of
            .iter()
            .map(|part| to_type(part, context))
            .filter(|part| part != "unknown")
            .collect();
        if parts.is_empty() {
            return "unknown".to_string();
        }
        return parts
            .into_iter()
            .map(|part| {
                if part.contains(" | ") {
                    format!("({part})")
                } else {
                    part
                }
            })
            .collect::<Vec<_>>()
            .join(" & ");
    }

    let ty = schema.get("type");
    if let Some(Value::Array(types)) = ty {
        return union(
            types
                .iter()
                .map(|entry| {
                    let mut combined = schema.clone();
                    combined.insert("type".to_string(), entry.clone());
                    to_type(&Value::Object(combined), context)
                })
                .collect(),
        );
    }
    let ty = ty.and_then(Value::as_str);
    match ty {
        Some("string") => "string".to_string(),
        Some("number") | Some("integer") => "number".to_string(),
        Some("boolean") => "boolean".to_string(),
        Some("null") => "null".to_string(),
        Some("array") => array_type(schema, context),
        Some("object") => object_type(schema, context),
        Some(_) => "unknown".to_string(),
        None => {
            if schema.contains_key("properties")
                || schema.contains_key("additionalProperties")
                || schema.contains_key("required")
            {
                return object_type(schema, context);
            }
            if schema.contains_key("items") || schema.contains_key("prefixItems") {
                return array_type(schema, context);
            }
            "unknown".to_string()
        }
    }
}

fn array_type(schema: &serde_json::Map<String, Value>, context: &mut SchemaContext<'_>) -> String {
    if let Some(items) = schema.get("items") {
        if !items.is_array() {
            return format!("Array<{}>", to_type(items, context));
        }
    }
    let tuple = schema
        .get("prefixItems")
        .filter(|value| value.is_array())
        .or_else(|| schema.get("items").filter(|value| value.is_array()));
    if let Some(Value::Array(tuple)) = tuple {
        if !tuple.is_empty() {
            let items = tuple
                .iter()
                .map(|item| to_type(item, context))
                .collect::<Vec<_>>()
                .join(", ");
            return format!("[{items}]");
        }
    }
    "unknown[]".to_string()
}

fn description_of(property: Option<&Value>) -> String {
    property
        .and_then(Value::as_object)
        .and_then(|object| object.get("description"))
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .to_string()
}

fn object_type(schema: &serde_json::Map<String, Value>, context: &mut SchemaContext<'_>) -> String {
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let required: HashSet<String> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let mut names: Vec<&String> = properties.keys().collect();
    names.sort();
    let members: Vec<String> = names
        .iter()
        .map(|name| {
            let optional = if required.contains(name.as_str()) {
                ""
            } else {
                "?"
            };
            format!(
                "{}{optional}: {};",
                property_key(name),
                to_type(&properties[*name], context)
            )
        })
        .collect();
    let mut members = members;
    let additional = schema.get("additionalProperties");
    match additional {
        Some(Value::Bool(false)) => {}
        Some(additional) => {
            let ty = match additional {
                Value::Bool(true) => "unknown".to_string(),
                other => to_type(other, context),
            };
            members.push(format!("[key: string]: {ty};"));
        }
        None if names.is_empty() => members.push("[key: string]: unknown;".to_string()),
        None => {}
    }
    if members.is_empty() {
        return "{}".to_string();
    }
    let has_description = names
        .iter()
        .any(|name| !description_of(properties.get(*name)).is_empty());
    if !has_description {
        return format!("{{ {} }}", members.join(" "));
    }

    let mut lines = vec!["{".to_string()];
    for (index, name) in names.iter().enumerate() {
        for line in split_lines(&description_of(properties.get(*name))) {
            if !line.trim().is_empty() {
                lines.push(format!("{INDENT}// {}", line.trim()));
            }
        }
        lines.push(format!(
            "{INDENT}{}",
            members[index].replace('\n', &format!("\n{INDENT}"))
        ));
    }
    for member in members.iter().skip(names.len()) {
        lines.push(format!("{INDENT}{member}"));
    }
    lines.push("}".to_string());
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn renders_tool_declarations() {
        let tools = vec![Declarable {
            name: "read".to_string(),
            description: Some("Read a file.\nSecond line".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "The path" },
                    "count": { "type": "integer" }
                },
                "required": ["path"]
            })),
            output_schema: None,
            spread: false,
            signature: None,
        }];
        let rendered = render_declarations(RenderDeclarationsOptions {
            tools: &tools,
            globals: &[],
        });
        // Verified byte-exactly against upstream declarations.ts (multi-line
        // doc comment; object type rendered one property per line with `//`
        // comments because `path` has a description).
        assert_eq!(
            rendered,
            "declare const tools: {\n  /**\n   * Read a file.\n   * Second line\n   */\n  read(args: {\n  count?: number;\n  // The path\n  path: string;\n}): Promise<unknown>;\n};"
        );
    }

    #[test]
    fn renders_namespaced_globals() {
        let globals = vec![
            Declarable {
                name: "models.classify".to_string(),
                description: Some("Classify".to_string()),
                signature: Some("(model: ModelInfo): Promise<ClassifierResult>".to_string()),
                ..Declarable::new("models.classify")
            },
            Declarable {
                name: "plain".to_string(),
                ..Declarable::new("plain")
            },
        ];
        let rendered = render_declarations(RenderDeclarationsOptions {
            tools: &[],
            globals: &globals,
        });
        assert_eq!(
            rendered,
            "declare function plain(args: unknown): Promise<unknown>;\n\ndeclare const models: {\n  /** Classify */\n  classify(model: ModelInfo): Promise<ClassifierResult>;\n};"
        );
    }

    #[test]
    fn schema_basics() {
        assert_eq!(schema_to_type(&json!(true), None), "unknown");
        assert_eq!(schema_to_type(&json!(false), None), "never");
        assert_eq!(schema_to_type(&json!("string"), None), "unknown");
        assert_eq!(
            schema_to_type(
                &json!({ "type": "array", "items": { "type": "string" } }),
                None
            ),
            "Array<string>"
        );
        assert_eq!(
            schema_to_type(&json!({ "type": ["string", "null"] }), None),
            "string | null"
        );
        assert_eq!(schema_to_type(&json!({ "const": "a" }), None), "\"a\"");
        assert_eq!(
            schema_to_type(&json!({ "enum": [1, "b"] }), None),
            "1 | \"b\""
        );
        assert_eq!(
            schema_to_type(
                &json!({ "anyOf": [{ "type": "string" }, { "type": "string" }] }),
                None
            ),
            "string"
        );
        assert_eq!(schema_to_type(&json!({}), None), "unknown");
        assert_eq!(
            schema_to_type(&json!({ "properties": {} }), None),
            "{ [key: string]: unknown; }"
        );
        assert_eq!(
            schema_to_type(
                &json!({ "properties": {}, "additionalProperties": { "type": "string" } }),
                None
            ),
            "{ [key: string]: string; }"
        );
    }

    #[test]
    fn ref_resolution_and_recursion() {
        let schema = json!({
            "type": "object",
            "properties": { "next": { "$ref": "#/$defs/node" }, "value": { "type": "string" } },
            "$defs": { "node": { "type": "object", "properties": { "next": { "$ref": "#/$defs/node" } } } }
        });
        assert_eq!(
            schema_to_type(&schema, None),
            // Verified against upstream: both objects lack `required`, so the
            // properties render optional; the recursive `$ref` stops at
            // `unknown`.
            "{ next?: { next?: unknown; }; value?: string; }"
        );
    }

    #[test]
    fn mcp_call_tool_result_detection() {
        let schema = json!({
            "type": "object",
            "properties": {
                "content": { "type": "array", "items": { "type": "object" } },
                "isError": { "type": "boolean" },
                "_meta": { "type": "object" },
                "structuredContent": { "type": "object", "properties": { "name": { "type": "string" } } }
            }
        });
        let structured = mcp_structured_content_schema(Some(&schema)).unwrap();
        assert_eq!(schema_to_type(&structured, None), "{ name?: string; }");
        assert_eq!(
            render_tool_signature(
                &Declarable {
                    name: "mcp__x__y".to_string(),
                    output_schema: Some(schema),
                    ..Declarable::new("mcp__x__y")
                },
                None
            ),
            "mcp__x__y(args: unknown): Promise<CallToolResult<{ name?: string; }>>;"
        );
    }
}
