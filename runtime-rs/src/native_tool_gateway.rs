//! Release optional-tool discovery and execution. The host resolves the actual
//! native session and supplies only tools permitted by its current policy.
use futures::future::BoxFuture;
use jsonschema::error::{TypeKind, ValidationError, ValidationErrorKind};
use pi_rust::coding_agent::{
    core::resource_loader::InlineExtension,
    extensions::{
        loader::ExtensionFactory,
        types::{AbortSignal, ExtensionContext, ToolDefinition},
    },
    session_manager::SessionManager,
};
use regex::Regex;
use serde_json::{json, Map, Value};
use std::{
    cmp::Ordering,
    collections::HashSet,
    sync::{Arc, Mutex, OnceLock},
};

pub const TOOL_GATEWAY_NAME: &str = "call_tool";
pub const TOOL_DISCOVERY_NAME: &str = "discover_tools";

#[derive(Clone, Debug)]
pub struct GatewayBlock {
    pub reason: Option<String>,
}
pub type GatewayAuthorize = Arc<
    dyn Fn(
            String,
            String,
            Value,
            Option<Arc<AbortSignal>>,
        ) -> BoxFuture<'static, Result<Option<GatewayBlock>, String>>
        + Send
        + Sync,
>;
pub struct GatewaySession {
    /// Real definitions, including inactive tools, filtered by the host's
    /// current execution mode, enabled-tools configuration and child policy.
    pub callable: Vec<Arc<ToolDefinition>>,
    pub active_names: HashSet<String>,
    /// Same authority as a direct model call, using the derived target call id.
    pub authorize: GatewayAuthorize,
}
pub type GatewayPort =
    Arc<dyn Fn(String) -> BoxFuture<'static, Result<GatewaySession, String>> + Send + Sync>;

pub fn create_extension(port: GatewayPort) -> InlineExtension {
    let factory: ExtensionFactory = Arc::new(move |api| {
        api.register_tool(gateway_definition(port.clone()))?;
        api.register_tool(discovery_definition(port.clone()))
    });
    InlineExtension::Named {
        factory,
        name: "builtin:pisper-tool-gateway".into(),
        hidden: false,
    }
}

fn native_session_id(ctx: &ExtensionContext) -> Result<String, String> {
    let manager = ctx
        .session_manager()?
        .downcast::<Mutex<SessionManager>>()
        .map_err(|_| "Tool gateway session manager unavailable".to_owned())?;
    let id = manager
        .lock()
        .map_err(|_| "Session manager lock failed".to_owned())?
        .get_session_id()
        .to_owned();
    Ok(id)
}

fn gateway_definition(port: GatewayPort) -> ToolDefinition {
    let mut tool = ToolDefinition::new(
        TOOL_GATEWAY_NAME,
        "Call Tool",
        "Call an optional tool by exact name with schema-valid arguments.",
        json!({"type":"object","properties":{
            "name":{"type":"string","minLength":1,"maxLength":240,"description":"Exact tool name"},
            "arguments":{"type":"object","additionalProperties":{},"description":"Arguments validated against the selected tool schema"}
        },"required":["name"]}),
    );
    tool.prompt_snippet = Some("Call an optional tool by exact name".into());
    tool.prompt_guidelines = Some(vec![
        "Use the exact discovered name and only schema-supported arguments.".into(),
    ]);
    tool.execute_async = Some(Arc::new(move |call_id, params, signal, on_update, ctx| {
        let port = port.clone();
        Box::pin(async move {
            let name = params["name"]
                .as_str()
                .unwrap_or("")
                .trim_matches(js_whitespace);
            if matches!(name, TOOL_GATEWAY_NAME | TOOL_DISCOVERY_NAME) {
                return Err(format!("Optional tool is unavailable: {name}"));
            }
            let session = port(native_session_id(&ctx)?).await?;
            let target = session
                .callable
                .iter()
                .find(|tool| tool.name == name)
                .cloned()
                .ok_or_else(|| format!("Optional tool is unavailable: {name}"))?;
            let args = params
                .get("arguments")
                .filter(|value| truthy(value))
                .cloned()
                .unwrap_or_else(|| json!({}));
            if let Some(message) = validation_message(&target.parameters, &args)? {
                return Err(format!("Invalid arguments for {name}: {message}"));
            }
            let target_call_id = format!("{call_id}:{name}");
            if let Some(block) = (session.authorize)(
                name.to_owned(),
                target_call_id.clone(),
                args.clone(),
                signal.clone(),
            )
            .await?
            {
                return Err(block
                    .reason
                    .filter(|reason| !reason.is_empty())
                    .unwrap_or_else(|| format!("Tool blocked: {name}")));
            }
            // Do not race/drop the target future on cancellation: the original
            // signal reaches its executor, which owns any required finalizers.
            let result = if let Some(execute) = &target.execute_async {
                execute(target_call_id, args, signal, on_update, ctx).await?
            } else if let Some(execute) = &target.execute {
                execute(
                    &target_call_id,
                    &args,
                    signal.as_ref(),
                    on_update.as_ref(),
                    &ctx,
                )?
            } else {
                return Err(format!("Optional tool is unavailable: {name}"));
            };
            let mut output = spread_object(&result);
            let mut details = spread_object(result.get("details").unwrap_or(&Value::Null));
            details.insert("gatewayToolName".into(), json!(name));
            output.insert("details".into(), Value::Object(details));
            Ok(Value::Object(output))
        })
    }));
    tool
}

fn spread_object(value: &Value) -> Map<String, Value> {
    match value {
        Value::Object(value) => value.clone(),
        Value::Array(value) => value
            .iter()
            .enumerate()
            .map(|(index, value)| (index.to_string(), value.clone()))
            .collect(),
        Value::String(value) => value
            .encode_utf16()
            .enumerate()
            .map(|(index, unit)| (index.to_string(), json!(String::from_utf16_lossy(&[unit]))))
            .collect(),
        _ => Map::new(),
    }
}

fn literal_values(schema: &Value) -> Vec<Value> {
    if let Some(value) = schema.get("const") {
        vec![value.clone()]
    } else if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        values.clone()
    } else if let Some(values) = schema.get("anyOf").and_then(Value::as_array) {
        values.iter().flat_map(literal_values).collect()
    } else {
        Vec::new()
    }
}

fn validation_message(schema: &Value, args: &Value) -> Result<Option<String>, String> {
    // JSON Schema validates the original JSON value; no prepareArguments or
    // SDK argument coercion is applied to a selected target's arguments.
    let validator = jsonschema::options()
        .should_validate_formats(true)
        .build(schema)
        .map_err(|error| format!("Invalid tool schema: {error}"))?;
    let errors = validator.iter_errors(args).collect::<Vec<_>>();
    if errors.is_empty() {
        return Ok(None);
    }
    let action_values = literal_values(&schema["properties"]["action"]);
    if !action_values.is_empty()
        && errors
            .iter()
            .all(|error| error.instance_path().to_string() == "/action")
    {
        return Ok(Some(format!(
            "/action: must be one of {}",
            action_values
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    let mut messages = Vec::new();
    // TypeBox reports missing required properties in one error per object.
    for error in &errors {
        if matches!(error.kind(), ValidationErrorKind::Required { .. }) {
            let path = error.instance_path().to_string();
            if messages
                .iter()
                .any(|(seen, _, required)| *required && seen == &path)
            {
                continue;
            }
            let missing = errors
                .iter()
                .filter(|item| item.instance_path().to_string() == path)
                .filter_map(|item| match item.kind() {
                    ValidationErrorKind::Required { property } => {
                        Some(property.as_str().unwrap_or("").to_owned())
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(", ");
            messages.push((
                path,
                format!("must have required properties {missing}"),
                true,
            ));
        } else {
            append_validation_error(error, &mut messages);
        }
    }
    Ok(Some(
        messages
            .into_iter()
            .take(5)
            .map(|(path, message, _)| {
                format!("{}: {message}", if path.is_empty() { "/" } else { &path })
            })
            .collect::<Vec<_>>()
            .join("; "),
    ))
}

fn append_validation_error(
    error: &ValidationError<'_>,
    messages: &mut Vec<(String, String, bool)>,
) {
    use ValidationErrorKind as E;
    if let E::AnyOf { context } | E::OneOfNotValid { context } = error.kind() {
        for branch in context {
            for nested in branch {
                append_validation_error(nested, messages);
            }
        }
    }
    let message = match error.kind() {
        E::Type {
            kind: TypeKind::Single(kind),
        } => format!("must be {kind}"),
        E::Type {
            kind: TypeKind::Multiple(kinds),
        } => format!(
            "must be {}",
            kinds
                .iter()
                .map(|kind| kind.to_string())
                .collect::<Vec<_>>()
                .join(",")
        ),
        E::Constant { .. } => "must be equal to constant".into(),
        E::Enum { .. } => "must be equal to one of the allowed values".into(),
        E::AnyOf { .. } => "must match a schema in anyOf".into(),
        E::OneOfMultipleValid { .. } | E::OneOfNotValid { .. } => {
            "must match exactly one schema in oneOf".into()
        }
        E::Minimum { limit } => format!("must be >= {limit}"),
        E::Maximum { limit } => format!("must be <= {limit}"),
        E::ExclusiveMinimum { limit } => format!("must be > {limit}"),
        E::ExclusiveMaximum { limit } => format!("must be < {limit}"),
        E::MinLength { limit } => format!("must not have fewer than {limit} characters"),
        E::MaxLength { limit } => format!("must not have more than {limit} characters"),
        E::MinItems { limit } => format!("must not have fewer than {limit} items"),
        E::MaxItems { limit } => format!("must not have more than {limit} items"),
        E::MinProperties { limit } => format!("must not have fewer than {limit} properties"),
        E::MaxProperties { limit } => format!("must not have more than {limit} properties"),
        E::AdditionalProperties { .. } | E::UnevaluatedProperties { .. } => {
            "must not have additional properties".into()
        }
        E::AdditionalItems { limit } => format!("must not have more than {limit} items"),
        E::UniqueItems => "must NOT have duplicate items".into(),
        E::Pattern { pattern } => format!("must match pattern {pattern:?}"),
        E::Format { format } => format!("must match format {format:?}"),
        E::MultipleOf { multiple_of } => format!("must be multiple of {multiple_of}"),
        E::Contains => "must contain at least 1 valid item(s)".into(),
        E::Not { .. } => "must NOT be valid".into(),
        E::FalseSchema => "boolean schema is false".into(),
        E::Required { property } => format!(
            "must have required properties {}",
            property.as_str().unwrap_or("")
        ),
        _ => error.to_string(),
    };
    messages.push((error.instance_path().to_string(), message, false));
}

fn discovery_definition(port: GatewayPort) -> ToolDefinition {
    let mut tool = ToolDefinition::new(
        TOOL_DISCOVERY_NAME,
        "Discover Tools",
        "Find optional tools by capability: image/video generation & editing (generate_visual), web search, browser automation, mobile device control, memory, MCP, plugins, and more. Call results through call_tool.",
        json!({"type":"object","properties":{
            "query":{"type":"string","minLength":1,"maxLength":240,"description":"Capability or task to find"},
            "limit":{"type":"integer","minimum":1,"maximum":5,"description":"Maximum matches; default 3"}
        },"required":["query"]}),
    );
    tool.prompt_snippet = Some(
        "Find optional tools (image/video generation, web, device, memory, MCP) by capability"
            .into(),
    );
    tool.prompt_guidelines = Some(vec![
        "Search by capability, then call the exact result through call_tool.".into(),
        "When the user asks for generated images/videos, device actions, web info, or other app capabilities, discover and call the matching tool instead of claiming it is unavailable.".into(),
    ]);
    tool.execute_async = Some(Arc::new(move |_, params, _, _, ctx| {
        let port = port.clone();
        Box::pin(async move {
            let session = port(native_session_id(&ctx)?).await?;
            let query = params["query"].as_str().unwrap_or("");
            let tools = session
                .callable
                .iter()
                .filter(|tool| !is_undiscoverable(&tool.name))
                .map(|tool| discovery_metadata(tool, session.active_names.contains(&tool.name)))
                .collect::<Vec<_>>();
            let limit = params["limit"]
                .as_f64()
                .filter(|number| *number != 0.0 && !number.is_nan())
                .unwrap_or(3.0)
                .clamp(1.0, 5.0) as usize;
            let matches = search_optional_tools(&tools, query, limit);
            let text = if matches.is_empty() {
                format!("No optional tools matched: {query}")
            } else {
                std::iter::once("Matching tools. Call active tools directly; call inactive tools through call_tool with their exact names:".to_owned())
                    .chain(matches.iter().map(format_match))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            Ok(
                json!({"content":[{"type":"text","text":text}],"details":{"query":query,"matches":matches}}),
            )
        })
    }));
    tool
}

fn is_undiscoverable(name: &str) -> bool {
    matches!(
        name,
        "read"
            | "grep"
            | "find"
            | "ls"
            | "edit"
            | "write"
            | "bash"
            | "get_plan"
            | "update_plan"
            | "get_task_list"
            | "update_task_list"
            | "discover_tools"
            | "call_tool"
            | "spawn_agent"
            | "list_agents"
            | "send_message"
            | "followup_task"
            | "wait_agent"
            | "interrupt_agent"
    )
}

fn discovery_metadata(tool: &ToolDefinition, active: bool) -> Value {
    let description = tool
        .description
        .split('\n')
        .map(|line| line.trim_matches(js_whitespace))
        .find(|line| !line.is_empty())
        .unwrap_or("");
    json!({
        "name":tool.name,
        "label":if tool.label.is_empty() { &tool.name } else { &tool.label },
        "description":String::from_utf16_lossy(&description.encode_utf16().take(320).collect::<Vec<_>>()),
        "active":active,
        "required":tool.parameters.get("required").and_then(Value::as_array).cloned().unwrap_or_default(),
        "parameters":tool.parameters,
    })
}

fn js_whitespace(value: char) -> bool {
    matches!(value, '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')
}
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        _ => true,
    }
}
fn normalized(value: &str) -> String {
    let mut normalized = String::new();
    let mut whitespace = false;
    for character in value.to_lowercase().chars() {
        if character == '_' || character == '-' || js_whitespace(character) {
            whitespace = true;
        } else {
            if whitespace && !normalized.is_empty() {
                normalized.push(' ');
            }
            whitespace = false;
            normalized.push(character);
        }
    }
    normalized
}
fn alphanumeric(character: char) -> bool {
    static LETTER_OR_NUMBER: OnceLock<Regex> = OnceLock::new();
    LETTER_OR_NUMBER
        .get_or_init(|| Regex::new(r"^[\p{L}\p{N}]$").expect("Unicode category"))
        .is_match(character.encode_utf8(&mut [0; 4]))
}
fn query_terms(query: &str) -> Vec<&str> {
    query
        .split(|character| !alphanumeric(character))
        .filter(|term| term.encode_utf16().count() >= 2)
        .collect()
}
fn skip_bigrams(value: &str) -> HashSet<String> {
    let characters = value
        .chars()
        .filter(|c| alphanumeric(*c))
        .take(24)
        .collect::<Vec<_>>();
    let mut pairs = HashSet::new();
    for (index, first) in characters.iter().enumerate() {
        for second in characters.iter().skip(index + 1) {
            pairs.insert(format!("{first}{second}"));
        }
    }
    pairs
}
fn is_cjk(character: char) -> bool {
    matches!(character, '\u{3040}'..='\u{30ff}' | '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{f900}'..='\u{faff}')
}
fn score_tool(tool: &Value, query: &str) -> u32 {
    let query = normalized(query);
    if query.is_empty() {
        return 1;
    }
    let name = tool["name"].as_str().unwrap_or("");
    let normalized_name = normalized(name);
    let label = normalized(tool["label"].as_str().unwrap_or(""));
    let profile = discovery_profile(name);
    let aliases = discovery_aliases(name);
    let haystack = normalized(
        &[
            name,
            tool["label"].as_str().unwrap_or(""),
            tool["description"].as_str().unwrap_or(""),
            profile.map(|value| value.0).unwrap_or(""),
            profile.map(|value| value.1).unwrap_or(""),
        ]
        .into_iter()
        .chain(aliases.iter().copied())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join(" "),
    );
    let mut score = 0;
    if query == normalized_name || query == label {
        score += 240;
    }
    if normalized_name.contains(&query) || label.contains(&query) {
        score += 120;
    }
    if haystack.contains(&query) {
        score += 80;
    }
    for alias in aliases {
        let alias = normalized(alias);
        if !alias.is_empty() && (query.contains(&alias) || alias.contains(&query)) {
            score += 72;
        }
    }
    for term in query_terms(&query) {
        score += if normalized_name.contains(term) {
            36
        } else if label.contains(term) {
            30
        } else if haystack.contains(term) {
            18
        } else {
            0
        };
    }
    let query_characters = query
        .chars()
        .filter(|c| alphanumeric(*c))
        .collect::<HashSet<_>>();
    if query_characters.len() >= 2 {
        let haystack_characters = haystack.chars().collect::<HashSet<_>>();
        let matches = query_characters.intersection(&haystack_characters).count();
        let overlap = matches as f64 / query_characters.len() as f64;
        score += if overlap >= 0.8 {
            24
        } else if overlap >= 0.6 {
            12
        } else {
            0
        };
    }
    if query.chars().any(is_cjk) {
        let compact = haystack
            .chars()
            .filter(|c| alphanumeric(*c))
            .collect::<String>();
        let pairs = skip_bigrams(&query);
        if !pairs.is_empty() {
            let hits = pairs
                .iter()
                .filter(|pair| compact.contains(pair.as_str()))
                .count();
            score += ((hits as f64 / pairs.len() as f64) * 56.0).round() as u32;
        }
    }
    score
}

/// Tool names supplied by the native app/plugin/MCP registries use ASCII
/// provider-safe names. Retain localeCompare's punctuation and case ordering
/// on that alphabet rather than Rust's byte ordering.
fn locale_name_cmp(left: &str, right: &str) -> Ordering {
    fn weight(character: char) -> u32 {
        match character {
            '_' => 1,
            '-' => 2,
            ':' => 3,
            '.' => 4,
            _ => character.to_ascii_lowercase() as u32 + 16,
        }
    }
    left.chars()
        .map(weight)
        .cmp(right.chars().map(weight))
        .then_with(|| {
            left.chars()
                .map(|character| character.is_uppercase())
                .cmp(right.chars().map(|character| character.is_uppercase()))
        })
        .then_with(|| left.cmp(right))
}

pub fn search_optional_tools(tools: &[Value], query: &str, limit: usize) -> Vec<Value> {
    let mut ranked = tools
        .iter()
        .map(|tool| (tool, score_tool(tool, query)))
        .filter(|(_, score)| *score > 0)
        .collect::<Vec<_>>();
    ranked.sort_by(|(left, a), (right, b)| {
        b.cmp(a).then_with(|| {
            locale_name_cmp(
                left["name"].as_str().unwrap_or(""),
                right["name"].as_str().unwrap_or(""),
            )
        })
    });
    ranked
        .into_iter()
        .take(limit.clamp(1, 5))
        .map(|(tool, _)| tool.clone())
        .collect()
}

fn format_schema(schema: &Value) -> String {
    if let Some(value) = schema.get("const") {
        value.to_string()
    } else if let Some(values) = schema["enum"].as_array() {
        values
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join(" | ")
    } else if let Some(values) = schema["anyOf"].as_array() {
        values
            .iter()
            .map(format_schema)
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join(" | ")
    } else if let Some(types) = schema["type"].as_array() {
        types
            .iter()
            .map(|value| value.as_str().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("|")
    } else {
        schema["type"]
            .as_str()
            .filter(|value| !value.is_empty())
            .unwrap_or("any")
            .into()
    }
}
fn format_signature(tool: &Value) -> String {
    let Some(properties) = tool["parameters"]["properties"].as_object() else {
        return String::new();
    };
    let parts = properties
        .iter()
        .take(8)
        .map(|(name, schema)| {
            let required = tool["required"].as_array().is_some_and(|items| {
                items
                    .iter()
                    .any(|item| item.as_str() == Some(name.as_str()))
            });
            format!(
                "{name}{}: {}",
                if required { "" } else { "?" },
                format_schema(schema)
            )
        })
        .collect::<Vec<_>>();
    if parts.is_empty() {
        String::new()
    } else {
        format!("; params: {}", parts.join(", "))
    }
}
fn format_match(tool: &Value) -> String {
    let name = tool["name"].as_str().unwrap_or("");
    let profile = discovery_profile(name);
    let label = profile
        .map(|item| item.0)
        .or_else(|| tool["label"].as_str().filter(|value| !value.is_empty()))
        .unwrap_or(name);
    let description = profile
        .map(|item| item.1)
        .or_else(|| {
            tool["description"]
                .as_str()
                .filter(|value| !value.is_empty())
        })
        .or_else(|| tool["label"].as_str().filter(|value| !value.is_empty()))
        .unwrap_or("Optional tool");
    let exact_name = if label == name {
        name.to_owned()
    } else {
        format!("{label} [{name}]")
    };
    format!(
        "- {exact_name}: {description}{}; {}",
        format_signature(tool),
        if tool["active"] == true {
            "active: call this tool directly"
        } else {
            "inactive: call through call_tool"
        }
    )
}

fn discovery_aliases(name: &str) -> &'static [&'static str] {
    match name {
        "web_search" => &[
            "web search",
            "internet search",
            "online search",
            "联网",
            "上网",
            "网络搜索",
            "网页搜索",
            "官网",
            "最新资料",
        ],
        "browser_automation" => &[
            "browser",
            "browser automation",
            "screenshot",
            "浏览器",
            "打开网页",
            "点击网页",
            "网页截图",
            "页面操作",
            "自动化",
        ],
        "generate_visual" => &[
            "visual generation",
            "image generation",
            "video generation",
            "image editing",
            "生图",
            "画图",
            "图片生成",
            "图像生成",
            "视频生成",
            "图片编辑",
            "视觉生成",
        ],
        "find_roots" => &[
            "computer use",
            "computer-use",
            "desktop automation",
            "ui automation",
            "find window",
            "find app window",
            "电脑操作",
            "控制电脑",
            "桌面自动化",
            "视觉自动化",
            "查找窗口",
            "查找应用窗口",
        ],
        "observe_ui" => &[
            "computer use",
            "computer-use",
            "desktop automation",
            "ui automation",
            "screen",
            "screenshot",
            "observe screen",
            "capture screen",
            "电脑截图",
            "屏幕截图",
            "截屏",
            "观察界面",
            "查看窗口",
        ],
        "search_ui" => &[
            "computer use",
            "ui automation",
            "find ui element",
            "find button",
            "find control",
            "查找界面元素",
            "查找按钮",
            "查找控件",
        ],
        "expand_ui" => &[
            "computer use",
            "ui automation",
            "expand ui",
            "show more ui",
            "展开界面",
            "展开控件",
            "显示更多界面",
        ],
        "inspect_ui" => &[
            "computer use",
            "ui automation",
            "inspect ui",
            "inspect control",
            "检查界面",
            "检查控件",
            "查看控件详情",
        ],
        "act_ui" => &[
            "computer use",
            "computer-use",
            "desktop automation",
            "ui automation",
            "click window",
            "control computer",
            "mouse and keyboard",
            "click",
            "type text",
            "press key",
            "scroll",
            "drag",
            "点击窗口",
            "控制电脑",
            "鼠标键盘",
            "点击",
            "输入文字",
            "按键",
            "滚动",
            "拖拽",
        ],
        "read_text" => &[
            "computer use",
            "ui automation",
            "read screen text",
            "read ui text",
            "读取屏幕文字",
            "读取界面文字",
            "读屏",
        ],
        "wait_for" => &[
            "computer use",
            "ui automation",
            "wait for window",
            "wait for ui",
            "等待窗口",
            "等待界面变化",
        ],
        "launch_browser" => &[
            "computer use",
            "browser automation",
            "launch browser",
            "open browser",
            "启动浏览器",
            "打开浏览器",
        ],
        "navigate_browser" => &[
            "computer use",
            "browser automation",
            "navigate browser",
            "open url in browser",
            "浏览器导航",
            "浏览器打开网址",
        ],
        "evaluate_browser" => &[
            "computer use",
            "browser automation",
            "evaluate browser javascript",
            "run javascript in browser",
            "浏览器脚本",
            "在浏览器执行 javascript",
        ],
        "ocr_ui" => &[
            "computer use",
            "ui automation",
            "ocr",
            "screen text recognition",
            "read chinese text from screen",
            "文字识别",
            "屏幕文字识别",
            "图片文字识别",
            "中文识别",
            "英文识别",
            "中英文识别",
            "读取截图文字",
        ],
        "mobile_device" => &[
            "mobile device",
            "phone",
            "device status",
            "system information",
            "memory",
            "RAM",
            "移动设备",
            "手机",
            "手机设备",
            "设备状态",
            "设备信息",
            "系统信息",
            "系统内存",
            "内存",
        ],
        "memory_search" => &[
            "memory search",
            "recall",
            "记忆搜索",
            "搜索记忆",
            "星忆",
            "回忆",
        ],
        "memory_remember" => &[
            "remember",
            "save memory",
            "记住",
            "保存记忆",
            "写入记忆",
            "星忆",
        ],
        "mcp_list" => &["mcp", "mcp services", "mcp tools", "mcp 服务", "mcp 工具"],
        "mcp_manage" => &[
            "mcp configuration",
            "configure mcp",
            "mcp 配置",
            "管理 mcp",
            "添加 mcp",
        ],
        "skill_create" => &[
            "skill",
            "create skill",
            "agent skill",
            "skill.md",
            "技能",
            "创建技能",
            "编写技能",
            "可复用能力",
        ],
        "plugin_create" => &[
            "plugin",
            "create plugin",
            "agent tool",
            "插件",
            "创建插件",
            "创建工具",
            "生成工具",
            "可复用工具",
        ],
        "spawn_agent" => &[
            "subagent",
            "delegate",
            "parallel agent",
            "子 agent",
            "子agent",
            "委派",
            "并行 agent",
        ],
        "list_agents" => &[
            "agent status",
            "subagent status",
            "agent 状态",
            "子agent状态",
        ],
        "send_message" => &[
            "message agent",
            "steer agent",
            "给 agent 发消息",
            "补充 agent 信息",
        ],
        "followup_task" => &[
            "agent followup",
            "continue agent",
            "agent 后续任务",
            "让 agent 继续",
        ],
        "wait_agent" => &["wait agent", "等待 agent", "等待子agent"],
        "interrupt_agent" => &["stop agent", "interrupt agent", "停止 agent", "中断 agent"],
        _ => &[],
    }
}
fn discovery_profile(name: &str) -> Option<(&'static str, &'static str)> {
    Some(match name {
        "find_roots" => (
            "Computer Use: Find Window",
            "Find desktop windows, dialogs, menus, or browser pages before visual interaction.",
        ),
        "observe_ui" => (
            "Computer Use: Observe Screen",
            "Capture the current screen and return a bounded accessibility and visual UI outline.",
        ),
        "search_ui" => (
            "Computer Use: Find UI Element",
            "Search the observed UI for a button, field, control, text, or other target element.",
        ),
        "expand_ui" => (
            "Computer Use: Expand UI",
            "Show more bounded context below one observed UI element.",
        ),
        "inspect_ui" => (
            "Computer Use: Inspect UI Element",
            "Inspect one UI element for its text, role, geometry, state, and available actions.",
        ),
        "act_ui" => (
            "Computer Use: Act",
            "Click, type, press keys, scroll, drag, or move the pointer in a checked desktop UI.",
        ),
        "read_text" => (
            "Computer Use: Read Screen Text",
            "Read bounded text from an observed UI element or a continuation result.",
        ),
        "wait_for" => (
            "Computer Use: Wait For UI",
            "Wait for a scoped UI condition such as text appearing, disappearing, or changing.",
        ),
        "launch_browser" => (
            "Computer Use: Launch Browser",
            "Launch the configured managed browser and return its initial browser-page state.",
        ),
        "navigate_browser" => (
            "Computer Use: Navigate Browser",
            "Navigate the managed browser to an HTTP(S) URL from an observed browser-page state.",
        ),
        "evaluate_browser" => (
            "Computer Use: Evaluate Browser",
            "Evaluate bounded JavaScript in the managed browser and return selected page data.",
        ),
        "ocr_ui" => (
            "Computer Use: OCR Screen",
            "Extract English, Simplified Chinese, or mixed text from the current screen.",
        ),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pi_rust::coding_agent::extensions::{
        loader::ExtensionRuntime,
        runner::ExtensionRunner,
        types::{AgentToolUpdateCallbackValue, NoopProviderRegistry},
    };
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    fn context() -> (ExtensionContext, String) {
        let manager = SessionManager::in_memory(".", None, None).unwrap();
        let id = manager.get_session_id().to_owned();
        let runner = ExtensionRunner::new(
            Vec::new(),
            ExtensionRuntime::new(),
            ".",
            Arc::new(Mutex::new(manager)),
            Arc::new(NoopProviderRegistry),
        );
        (runner.create_context(), id)
    }
    fn port(
        tools: Vec<Arc<ToolDefinition>>,
        active: HashSet<String>,
        authorize: GatewayAuthorize,
    ) -> GatewayPort {
        Arc::new(move |_| {
            let (tools, active, authorize) = (tools.clone(), active.clone(), authorize.clone());
            Box::pin(async move {
                Ok(GatewaySession {
                    callable: tools,
                    active_names: active,
                    authorize,
                })
            })
        })
    }
    fn allowed() -> GatewayAuthorize {
        Arc::new(|_, _, _, _| Box::pin(async { Ok(None) }))
    }
    fn metadata(name: &str, label: &str, description: &str) -> Value {
        json!({"name":name,"label":label,"description":description,"active":false})
    }

    #[test]
    fn discovery_matches_release_aliases_cjk_fuzzy_and_exact_names() {
        let tools = vec![
            metadata(
                "web_search",
                "Web Search",
                "Search the public web for current information.",
            ),
            metadata(
                "generate_visual",
                "Visual Generate",
                "Generate or edit images and generate video.",
            ),
            metadata(
                "mcp_fixture_echo_12345678",
                "MCP fixture echo",
                "Echo fixture text through a remote MCP service.",
            ),
            metadata(
                "skill_create",
                "Skill Create",
                "Create a reusable Agent Skill in the project or global skills directory.",
            ),
            metadata(
                "plugin_create",
                "Plugin Create",
                "Create a reusable Pisper plugin and new Agent tools.",
            ),
            metadata(
                "browser_automation",
                "Browser Automation",
                "Navigate, inspect, click, type, wait, and capture screenshots.",
            ),
            metadata(
                "mobile_device",
                "Mobile Device",
                "Use approved phone capabilities.",
            ),
        ];
        for (query, expected) in [
            ("帮我生图", "generate_visual"),
            ("search the latest information online", "web_search"),
            ("MCP fixture echo", "mcp_fixture_echo_12345678"),
            ("创建技能", "skill_create"),
            ("生成工具", "plugin_create"),
            ("查看当前设备系统内存总量和可用内存", "mobile_device"),
            ("帮我截个图", "browser_automation"),
            ("想打开网页点一下", "browser_automation"),
            ("web_search", "web_search"),
        ] {
            assert_eq!(
                search_optional_tools(&tools, query, 1)[0]["name"],
                expected,
                "{query}"
            );
        }
        let computer = vec![
            metadata("observe_ui", "Observe UI", "Observe the current UI."),
            metadata("act_ui", "Act", "Perform a checked UI action."),
            metadata(
                "ocr_ui",
                "OCR UI",
                "Extract text from the current UI screenshot.",
            ),
        ];
        for (query, expected) in [
            ("控制电脑并点击窗口", "act_ui"),
            ("屏幕截图", "observe_ui"),
            ("OCR 中文文字", "ocr_ui"),
            ("computer use", "act_ui"),
        ] {
            assert_eq!(
                search_optional_tools(&computer, query, 1)[0]["name"],
                expected,
                "{query}"
            );
        }
    }

    #[test]
    fn discovery_signature_profile_and_literal_values_keep_exact_output() {
        let tool = json!({"name":"mobile_device","label":"Mobile Device","description":"Use approved phone capabilities.","active":false,"required":["action"],"parameters":{"type":"object","properties":{"action":{"anyOf":[{"const":"get_device_info"},{"const":"get_capabilities"}]},"limit":{"type":"integer"}}}});
        assert_eq!(format_match(&tool),"- Mobile Device [mobile_device]: Use approved phone capabilities.; params: action: \"get_device_info\" | \"get_capabilities\", limit?: integer; inactive: call through call_tool");
        let active = json!({"name":"act_ui","label":"Act","description":"Perform a checked UI action.","active":true});
        assert_eq!(format_match(&active),"- Computer Use: Act [act_ui]: Click, type, press keys, scroll, drag, or move the pointer in a checked desktop UI.; active: call this tool directly");
        assert_eq!(format_schema(&json!({"enum":["a","b"]})), "\"a\" | \"b\"");
        assert_eq!(
            format_schema(&json!({"type":["string","null"]})),
            "string|null"
        );
        let mut many = tool.clone();
        many["parameters"]["properties"] =
            json!({"a":{},"b":{},"c":{},"d":{},"e":{},"f":{},"g":{},"h":{},"ninth":{}});
        assert!(!format_signature(&many).contains("ninth"));
    }

    #[test]
    fn strict_schema_error_messages_match_release_and_do_not_coerce() {
        let schema =
            json!({"type":"object","properties":{"text":{"type":"string"}},"required":["text"]});
        assert_eq!(
            validation_message(&schema, &json!({"text":42})).unwrap(),
            Some("/text: must be string".into())
        );
        assert_eq!(
            validation_message(&schema, &json!({})).unwrap(),
            Some("/: must have required properties text".into())
        );
        assert_eq!(
            validation_message(&schema, &json!({"text":"hello"})).unwrap(),
            None
        );
        let action = json!({"type":"object","properties":{"action":{"anyOf":[{"const":"get_device_info"},{"const":"get_capabilities"}]}},"required":["action"]});
        assert_eq!(
            validation_message(&action, &json!({"action":"status"})).unwrap(),
            Some("/action: must be one of \"get_device_info\", \"get_capabilities\"".into())
        );
        let schema = json!({"type":"object","additionalProperties":false,"properties":{"limit":{"type":"integer","minimum":1,"maximum":5}},"required":["limit"]});
        for invalid in [
            json!({"limit":"2"}),
            json!({"limit":1.5}),
            json!({"limit":0}),
            json!({"limit":2,"unknown":true}),
        ] {
            assert!(
                validation_message(&schema, &invalid).unwrap().is_some(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn discovery_locale_order_metadata_limit_and_hidden_names_match_release() {
        let tools = ["a_A", "a_a", "a-1", "a_1", "a0", "aA", "aa", "aB", "ab"]
            .map(|name| metadata(name, "", ""));
        assert_eq!(
            search_optional_tools(&tools, "", 5)
                .iter()
                .map(|tool| tool["name"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["a_1", "a_a", "a_A", "a-1", "a0"]
        );
        assert!(search_optional_tools(&tools, "龘龖", 5).is_empty());
        for name in [
            "read",
            "get_plan",
            "get_task_list",
            "call_tool",
            "discover_tools",
            "spawn_agent",
        ] {
            assert!(is_undiscoverable(name));
        }
        assert!(!is_undiscoverable("web_search"));
        let mut tool = ToolDefinition::new(
            "web_search",
            "",
            " \n First line \nprivate later guideline",
            json!({"required":["query"]}),
        );
        let item = discovery_metadata(&tool, true);
        assert_eq!(item["label"], "web_search");
        assert_eq!(item["description"], "First line");
        assert_eq!(item["active"], true);
        assert_eq!(item["required"], json!(["query"]));
        tool.description = "x".repeat(400);
        assert_eq!(
            discovery_metadata(&tool, false)["description"]
                .as_str()
                .unwrap()
                .len(),
            320
        );
    }

    #[tokio::test]
    async fn gateway_executes_real_target_with_actual_native_context_authorization_update_and_details(
    ) {
        let (ctx, id) = context();
        let authorized = Arc::new(Mutex::new(Vec::new()));
        let observed = authorized.clone();
        let authorize: GatewayAuthorize = Arc::new(move |name, call, args, signal| {
            observed
                .lock()
                .unwrap()
                .push(json!({"name":name,"call":call,"args":args,"hasSignal":signal.is_some()}));
            Box::pin(async { Ok(None) })
        });
        let received = Arc::new(Mutex::new(Vec::new()));
        let seen = received.clone();
        let mut target = ToolDefinition::new(
            "fixture.echo",
            "Echo",
            "Echo text",
            json!({"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}),
        );
        target.execute_async = Some(Arc::new(move |call, args, signal, update, ctx| {
            let seen = seen.clone();
            Box::pin(async move {
                seen.lock().unwrap().push(json!({"id":native_session_id(&ctx)?,"cwd":ctx.cwd()?,"call":call,"args":args,"hasSignal":signal.is_some()}));
                update.unwrap()(&json!({"content":[{"type":"text","text":"working"}]}));
                Ok(
                    json!({"content":[{"type":"text","text":args["text"]}],"details":{"original":true,"gatewayToolName":"forged"},"structuredContent":{"echo":args["text"]}}),
                )
            })
        }));
        let base = port(vec![Arc::new(target)], HashSet::new(), authorize);
        let actual_id = id.clone();
        let port: GatewayPort = Arc::new(move |id| {
            assert_eq!(id, actual_id);
            base(id)
        });
        let gateway = gateway_definition(port);
        let updates = Arc::new(Mutex::new(Vec::new()));
        let observed = updates.clone();
        let update: AgentToolUpdateCallbackValue =
            Arc::new(move |value| observed.lock().unwrap().push(value.clone()));
        let result = gateway.execute_async.as_ref().unwrap()(
            "gateway-1".into(),
            json!({"name":"fixture.echo","arguments":{"text":"hello"}}),
            Some(Arc::new(AbortSignal::new())),
            Some(update),
            ctx,
        )
        .await
        .unwrap();
        assert_eq!(result["content"][0]["text"], "hello");
        assert_eq!(
            result["details"],
            json!({"original":true,"gatewayToolName":"fixture.echo"})
        );
        assert_eq!(result["structuredContent"]["echo"], "hello");
        assert_eq!(
            authorized.lock().unwrap()[0],
            json!({"name":"fixture.echo","call":"gateway-1:fixture.echo","args":{"text":"hello"},"hasSignal":true})
        );
        assert_eq!(received.lock().unwrap()[0]["id"], id);
        assert_eq!(
            received.lock().unwrap()[0]["call"],
            "gateway-1:fixture.echo"
        );
        assert_eq!(updates.lock().unwrap()[0]["content"][0]["text"], "working");
    }

    #[tokio::test]
    async fn gateway_rejects_invalid_unavailable_and_recursive_calls_before_authorizing() {
        let count = Arc::new(AtomicUsize::new(0));
        let seen = count.clone();
        let authorize: GatewayAuthorize = Arc::new(move |_, _, _, _| {
            seen.fetch_add(1, AtomicOrdering::SeqCst);
            Box::pin(async { Ok(None) })
        });
        let target = Arc::new(ToolDefinition::new(
            "fixture.echo",
            "Echo",
            "",
            json!({"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}),
        ));
        let gateway = gateway_definition(port(vec![target], HashSet::new(), authorize));
        for (args, expected) in [
            (
                json!({"name":"fixture.echo","arguments":{"text":42}}),
                "Invalid arguments for fixture.echo: /text: must be string",
            ),
            (
                json!({"name":"missing"}),
                "Optional tool is unavailable: missing",
            ),
            (
                json!({"name":"call_tool"}),
                "Optional tool is unavailable: call_tool",
            ),
            (
                json!({"name":"discover_tools"}),
                "Optional tool is unavailable: discover_tools",
            ),
        ] {
            assert_eq!(
                gateway.execute_async.as_ref().unwrap()("g".into(), args, None, None, context().0)
                    .await
                    .unwrap_err(),
                expected
            );
        }
        assert_eq!(count.load(AtomicOrdering::SeqCst), 0);
    }

    #[tokio::test]
    async fn gateway_block_preserves_host_reason_and_never_executes() {
        let mut target = ToolDefinition::new("fixture.echo", "Echo", "", json!({"type":"object"}));
        target.execute = Some(Arc::new(|_, _, _, _, _| panic!("blocked target executed")));
        for (reason, expected) in [
            (Some("actual host denial".into()), "actual host denial"),
            (None, "Tool blocked: fixture.echo"),
            (Some(String::new()), "Tool blocked: fixture.echo"),
        ] {
            let authorize: GatewayAuthorize = Arc::new(move |_, _, _, _| {
                let reason = reason.clone();
                Box::pin(async move { Ok(Some(GatewayBlock { reason })) })
            });
            let gateway = gateway_definition(port(
                vec![Arc::new(target.clone())],
                HashSet::new(),
                authorize,
            ));
            assert_eq!(
                gateway.execute_async.as_ref().unwrap()(
                    "g".into(),
                    json!({"name":"fixture.echo"}),
                    None,
                    None,
                    context().0
                )
                .await
                .unwrap_err(),
                expected
            );
        }
    }

    #[tokio::test]
    async fn gateway_retains_live_abort_signal_and_awaits_target_finalizer() {
        let signal = Arc::new(AbortSignal::new());
        let original = signal.clone();
        let arrived = Arc::new(tokio::sync::Notify::new());
        let reached = arrived.clone();
        let finalized = Arc::new(AtomicUsize::new(0));
        let finished = finalized.clone();
        let mut target = ToolDefinition::new("fixture.wait", "Wait", "", json!({"type":"object"}));
        target.execute_async = Some(Arc::new(move |_, _, signal, _, _| {
            let (reached, finished, original) =
                (reached.clone(), finished.clone(), original.clone());
            Box::pin(async move {
                let signal = signal.unwrap();
                assert!(Arc::ptr_eq(&signal, &original));
                reached.notify_one();
                signal.cancelled().await;
                finished.fetch_add(1, AtomicOrdering::SeqCst);
                Err("actual target cancellation".into())
            })
        }));
        let gateway = gateway_definition(port(vec![Arc::new(target)], HashSet::new(), allowed()));
        let task = tokio::spawn(gateway.execute_async.as_ref().unwrap()(
            "g".into(),
            json!({"name":"fixture.wait"}),
            Some(signal.clone()),
            None,
            context().0,
        ));
        tokio::time::timeout(std::time::Duration::from_secs(2), arrived.notified())
            .await
            .unwrap();
        signal.abort();
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(2), task)
                .await
                .unwrap()
                .unwrap()
                .unwrap_err(),
            "actual target cancellation"
        );
        assert_eq!(finalized.load(AtomicOrdering::SeqCst), 1);
    }

    #[tokio::test]
    async fn discovery_uses_actual_session_optional_tools_and_never_activates_them() {
        let mut web = ToolDefinition::new(
            "web_search",
            "Web Search",
            "Search the public web.\nsecondary",
            json!({"type":"object","properties":{"query":{"type":"string"},"limit":{"type":"integer"}},"required":["query"]}),
        );
        web.execute = Some(Arc::new(|_, _, _, _, _| {
            panic!("discovery executed a target")
        }));
        let hidden = ToolDefinition::new("read", "Read", "web search", json!({"type":"object"}));
        let tool = discovery_definition(port(
            vec![Arc::new(hidden), Arc::new(web)],
            HashSet::new(),
            allowed(),
        ));
        let result = tool.execute_async.as_ref().unwrap()(
            "d".into(),
            json!({"query":"web_search","limit":1}),
            None,
            None,
            context().0,
        )
        .await
        .unwrap();
        assert_eq!(result["details"]["matches"].as_array().unwrap().len(), 1);
        assert_eq!(result["details"]["matches"][0]["name"], "web_search");
        assert_eq!(result["details"]["matches"][0]["active"], false);
        assert!(result["details"].get("activated").is_none());
        assert_eq!(result["content"][0]["text"],"Matching tools. Call active tools directly; call inactive tools through call_tool with their exact names:\n- Web Search [web_search]: Search the public web.; params: query: string, limit?: integer; inactive: call through call_tool");
    }
}
