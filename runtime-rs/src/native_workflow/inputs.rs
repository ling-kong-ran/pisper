use super::{Result, WorkflowError};
use serde_json::{json, Map, Value};
use std::collections::HashSet;

fn error(code: &str) -> WorkflowError {
    WorkflowError::coded(code, "工作流输入无效，请检查字段名称、类型和必填值。")
}
pub(crate) fn safe_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    matches!(bytes.next(), Some(b'a'..=b'z' | b'A'..=b'Z' | b'_'))
        && name.len() <= 80
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        && !["__proto__", "constructor", "prototype"].contains(&name)
}
pub(crate) fn media(value: &Value) -> Result<Value> {
    let object = value
        .as_object()
        .ok_or_else(|| error("workflow_input_type"))?;
    let id = value["id"].as_str().unwrap_or("");
    let name = value["name"].as_str().unwrap_or("");
    let mime = value["mimeType"].as_str().unwrap_or("");
    let size = value["size"].as_u64().unwrap_or(0);
    if object
        .keys()
        .any(|key| !["id", "name", "mimeType", "size"].contains(&key.as_str()))
        || id.is_empty()
        || id.len() > 80
        || !id
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        || name.trim().is_empty()
        || name.encode_utf16().count() > 160
        || name.contains(['/', '\\'])
        || ![
            "image/png",
            "image/jpeg",
            "image/webp",
            "video/mp4",
            "video/webm",
        ]
        .contains(&mime)
        || size == 0
        || size
            > if mime.starts_with("image/") {
                8 * 1024 * 1024
            } else {
                64 * 1024 * 1024
            }
    {
        return Err(error("workflow_input_type"));
    }
    Ok(json!({"id":id,"name":name,"mimeType":mime,"size":size}))
}
fn value(input: &Value, raw: &Value, required: bool) -> Result<Value> {
    let kind = input["type"].as_str().unwrap_or("string");
    let empty = raw.is_null() || raw.as_str().is_some_and(|text| text.trim().is_empty());
    if required && empty {
        return Err(error("workflow_input_required"));
    }
    if empty {
        return Ok(match kind {
            "boolean" => json!(false),
            "image" | "video" => Value::Null,
            _ => json!(""),
        });
    }
    if raw
        .as_str()
        .is_some_and(|text| text.encode_utf16().count() > 16000)
    {
        return Err(error("workflow_input_too_long"));
    }
    match kind {
        "number" => {
            let numeric = if let Some(number) = raw.as_f64() {
                Some(number)
            } else {
                raw.as_str().and_then(|text| {
                    static DECIMAL: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
                    let decimal = DECIMAL.get_or_init(|| {
                        regex::Regex::new(r"^[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?$")
                            .expect("fixed decimal expression")
                    });
                    let text = text.trim();
                    decimal
                        .is_match(text)
                        .then(|| text.parse::<f64>().ok())
                        .flatten()
                })
            };
            let numeric = numeric
                .filter(|number| number.is_finite())
                .ok_or_else(|| error("workflow_input_type"))?;
            Ok(json!(numeric))
        }
        "boolean" => match raw {
            Value::Bool(_) => Ok(raw.clone()),
            Value::String(text) if text == "true" || text == "false" => Ok(json!(text == "true")),
            _ => Err(error("workflow_input_type")),
        },
        "image" | "video" => {
            let media = media(raw)?;
            if !media["mimeType"]
                .as_str()
                .unwrap_or("")
                .starts_with(&format!("{kind}/"))
            {
                return Err(error("workflow_input_type"));
            }
            Ok(media)
        }
        _ if raw.is_string() => Ok(raw.clone()),
        _ => Err(error("workflow_input_type")),
    }
}
pub(crate) fn definitions(raw: Option<&Value>) -> Result<Vec<Value>> {
    let Some(raw) = raw else { return Ok(vec![]) };
    let array = raw
        .as_array()
        .ok_or_else(|| error("workflow_input_invalid_definition"))?;
    if array.len() > 30 {
        return Err(error("workflow_input_invalid_definition"));
    }
    let mut names = HashSet::new();
    let mut ids = HashSet::new();
    let mut result = Vec::new();
    for (index, raw) in array.iter().enumerate() {
        let input = raw
            .as_object()
            .ok_or_else(|| error("workflow_input_invalid_definition"))?;
        let name = input
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| error("workflow_input_unsafe_name"))?;
        if !safe_name(name) {
            return Err(error("workflow_input_unsafe_name"));
        }
        let default_id = format!("input-{}", index + 1);
        let id = input
            .get("id")
            .map(|v| v.as_str())
            .unwrap_or(Some(&default_id))
            .ok_or_else(|| error("workflow_input_invalid_definition"))?;
        let label = input
            .get("label")
            .map(|v| v.as_str())
            .unwrap_or(Some(name))
            .ok_or_else(|| error("workflow_input_invalid_definition"))?;
        let kind = input
            .get("type")
            .map(|v| v.as_str())
            .unwrap_or(Some("string"))
            .ok_or_else(|| error("workflow_input_invalid_definition"))?;
        if id.is_empty()
            || id.chars().count() > 80
            || label.trim().is_empty()
            || label.chars().count() > 120
            || !["string", "text", "number", "boolean", "image", "video"].contains(&kind)
            || input
                .get("description")
                .is_some_and(|v| v.as_str().is_none_or(|s| s.chars().count() > 300))
            || input.get("required").is_some_and(|v| !v.is_boolean())
        {
            return Err(error("workflow_input_invalid_definition"));
        }
        if !names.insert(name.to_owned()) || !ids.insert(id.to_owned()) {
            return Err(error("workflow_input_duplicate_name"));
        }
        let mut normalized = json!({"id":id,"name":name,"label":label.trim(),"type":kind,"required":raw["required"].as_bool().unwrap_or(false),"description":raw["description"].as_str().unwrap_or(""),"defaultValue":""});
        let default = raw.get("defaultValue").unwrap_or(&Value::Null);
        normalized["defaultValue"] = if default.is_null() || default == "" {
            if ["image", "video"].contains(&kind) {
                Value::Null
            } else {
                json!("")
            }
        } else {
            value(&normalized, default, false)?
        };
        result.push(normalized);
    }
    Ok(result)
}
pub(crate) fn validate(
    definitions_value: Option<&Value>,
    supplied: Option<&Value>,
) -> Result<Value> {
    let inputs = definitions(definitions_value)?;
    let empty = Map::new();
    let values = match supplied {
        Some(value) => value
            .as_object()
            .ok_or_else(|| error("workflow_input_type"))?,
        None => &empty,
    };
    let effective = if inputs.is_empty() {
        vec![json!({"name":"task","type":"text","required":false,"defaultValue":""})]
    } else {
        inputs.clone()
    };
    for key in values.keys() {
        if !effective.iter().any(|input| input["name"] == key.as_str()) {
            return Err(error("workflow_input_unknown"));
        }
    }
    let mut result = Map::new();
    for input in effective {
        let name = input["name"].as_str().unwrap_or("");
        if inputs.is_empty() && !values.contains_key(name) {
            continue;
        }
        let raw = values.get(name).unwrap_or(&input["defaultValue"]);
        result.insert(
            name.into(),
            value(&input, raw, input["required"].as_bool().unwrap_or(false))?,
        );
    }
    Ok(Value::Object(result))
}
fn variables(template: &str) -> Result<Vec<(&str, Vec<&str>)>> {
    let mut result = vec![];
    let mut cursor = 0;
    while let Some(offset) = template[cursor..].find("{{") {
        let start = cursor + offset;
        let Some(end_offset) = template[start + 2..].find("}}") else {
            break;
        };
        let end = start + 2 + end_offset;
        let inside = &template[start + 2..end];
        if inside.contains(['{', '}']) {
            cursor = end + 2;
            continue;
        }
        let path = inside.trim();
        let segments = path.split('.').collect::<Vec<_>>();
        if path.is_empty()
            || segments.iter().any(|part| {
                part.is_empty()
                    || !["__proto__", "constructor", "prototype"]
                        .iter()
                        .all(|reserved| part != reserved)
                    || !part
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            })
        {
            return Err(error("workflow_template_unknown"));
        }
        result.push((&template[start..end + 2], segments));
        cursor = end + 2;
    }
    Ok(result)
}
pub(crate) fn validate_template(template: &str, inputs: &[String], nodes: &[String]) -> Result<()> {
    for (_, parts) in variables(template)? {
        let root = parts[0];
        let field = parts.get(1).copied();
        if !["inputs", "previous", "nodes", "workflow", "run"].contains(&root)
            || root == "inputs"
                && field.is_some_and(|field| !inputs.iter().any(|name| name == field))
            || root == "nodes" && field.is_none_or(|field| !nodes.iter().any(|id| id == field))
            || root == "workflow"
                && field.is_some_and(|field| !["id", "name", "description"].contains(&field))
            || root == "run" && field.is_some_and(|field| !["id", "startedAt"].contains(&field))
        {
            return Err(error("workflow_template_unknown"));
        }
    }
    Ok(())
}
pub(crate) fn render(template: &str, context: &Value) -> Result<String> {
    let mut result = template.to_owned();
    for (token, parts) in variables(template)? {
        let mut current = context;
        for part in parts {
            current = current
                .get(part)
                .ok_or_else(|| error("workflow_template_unknown"))?;
        }
        let text = if current.is_null() {
            String::new()
        } else if let Some(text) = current.as_str() {
            text.to_owned()
        } else {
            serde_json::to_string_pretty(current).map_err(WorkflowError::io)?
        };
        result = result.replace(token, &text);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn typed_inputs_reject_unknown_and_unsafe_media() {
        let defs = json!([{"name":"count","type":"number","required":true},{"name":"enabled","type":"boolean"}]);
        assert_eq!(
            validate(Some(&defs), Some(&json!({"count":"1e2","enabled":"false"}))).unwrap(),
            json!({"count":100.0,"enabled":false})
        );
        assert_eq!(
            validate(Some(&defs), Some(&json!({"other":1})))
                .unwrap_err()
                .code,
            "workflow_input_unknown"
        );
        assert!(media(
            &json!({"id":"id","name":"photo.png","mimeType":"image/png","size":1,"path":"secret"})
        )
        .is_err());
        assert!(definitions(Some(&json!([{"name":"__proto__"}]))).is_err());
    }
    #[test]
    fn templates_validate_and_render_without_prototype_access() {
        assert_eq!(
            render(
                "{{inputs.count}}/{{nodes.a.output}}",
                &json!({"inputs":{"count":2},"nodes":{"a":{"output":{"ok":true}}}})
            )
            .unwrap(),
            "2/{\n  \"ok\": true\n}"
        );
        assert!(render(
            "{{inputs.constructor}}",
            &json!({"inputs":{"constructor":1}})
        )
        .is_err());
        assert!(validate_template("{{inputs.unknown}}", &["count".into()], &[]).is_err());
    }
}
