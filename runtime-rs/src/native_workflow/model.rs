use super::{id, inputs, now, Result, WorkflowError};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

pub(crate) const IMAGE_KINDS: &[&str] = &[
    "media-input",
    "media-background",
    "media-inpaint",
    "media-generate",
    "media-frames",
    "media-transform",
    "media-preview",
    "media-export",
];
pub(crate) fn text(value: &Value, max: usize) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        v => v.to_string(),
    }
    .trim()
    .chars()
    .take(max)
    .collect()
}
pub(crate) fn strings(value: &Value, max: usize) -> Vec<String> {
    let mut seen = HashSet::new();
    value
        .as_array()
        .into_iter()
        .flatten()
        .map(|v| text(v, 120))
        .filter(|s| !s.is_empty())
        .take(max)
        .filter(|s| seen.insert(s.clone()))
        .collect()
}
pub(crate) fn model(value: &Value) -> Value {
    if value["provider"].as_str().is_some_and(|s| !s.is_empty())
        && value["model"].as_str().is_some_and(|s| !s.is_empty())
    {
        json!({"provider":value["provider"],"model":value["model"]})
    } else {
        Value::Null
    }
}
fn number(value: &Value, default: f64, min: f64, max: f64) -> Value {
    let number = value
        .as_f64()
        .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
        .filter(|n| n.is_finite() && *n != 0.0)
        .unwrap_or(default);
    json!(number.clamp(min, max))
}
fn choice<'a>(value: &Value, choices: &[&str], default: &'a str) -> String {
    value
        .as_str()
        .filter(|s| choices.contains(s))
        .unwrap_or(default)
        .to_owned()
}
fn targets(value: &Value) -> Vec<String> {
    strings(value, 30)
        .into_iter()
        .filter(|s| ["browser", "feishu", "weixin", "qq", "telegram"].contains(&s.as_str()))
        .collect()
}

pub(crate) fn node(raw: &Value, index: usize) -> Result<Value> {
    let kinds = [
        "trigger",
        "prompt",
        "skill",
        "file",
        "mcp",
        "notification",
        "condition",
        "parallel",
        "approval",
    ];
    let kind = raw["kind"]
        .as_str()
        .filter(|s| kinds.contains(s) || IMAGE_KINDS.contains(s))
        .unwrap_or("prompt");
    let id = raw["id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .map(Ok)
        .unwrap_or_else(id)?;
    let mut node = json!({"id":id,"kind":kind,"label":text(&raw.get("label").cloned().unwrap_or(json!(format!("步骤 {}",index+1))),120),"prompt":text(&raw["prompt"],100000),
        "x":number(&raw["x"],0.0,0.0,4000.0),"y":number(&raw["y"],0.0,0.0,4000.0),"model":model(&raw["model"]),
        "executionMode":choice(&raw["executionMode"],&["workspace-write","full-access"],"full-access"),"retries":number(&raw["retries"],0.0,0.0,3.0),
        "timeoutMinutes":number(&raw["timeoutMinutes"],20.0,1.0,240.0),"failurePolicy":choice(&raw["failurePolicy"],&["stop","skip"],"stop"),"enabled":raw["enabled"]!=false,
        "outputFormat":choice(&raw["outputFormat"],&["text","json"],"text"),"skillName":text(&raw["skillName"],120),"requestedToolNames":strings(&raw["requestedToolNames"],20),
        "condition":{"source":text(&raw["condition"]["source"],240),"operator":choice(&raw["condition"]["operator"],&["exists","not_exists","equals","not_equals","contains","greater_than","less_than"],"exists"),"value":raw["condition"].get("value").cloned().unwrap_or(json!(""))},
        "approval":{"message":text(&raw["approval"].get("message").cloned().unwrap_or(raw["prompt"].clone()),1000),"timeoutMinutes":number(&raw["approval"].get("timeoutMinutes").cloned().unwrap_or(raw["timeoutMinutes"].clone()),60.0,1.0,10080.0)},
        "notification":{"title":text(&raw["notification"]["title"],160),"content":text(&raw["notification"]["content"],12000)},"notificationTargets":targets(&raw["notificationTargets"])});
    // 媒体设置由媒体执行器的共享边界校验；不能在 DAG 层丢弃帧或变换配置。
    if IMAGE_KINDS.contains(&kind) {
        node["image"] = super::image_protocol::settings(raw.get("image"))?;
    }
    Ok(node)
}
pub(crate) fn normalize(raw: &Value, cwd: &str, strict_inputs: bool) -> Result<Value> {
    let stamp = now();
    let nodes = raw["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .take(100)
        .enumerate()
        .map(|(i, n)| node(n, i))
        .collect::<Result<Vec<_>>>()?;
    let source_edges = if let Some(edges) = raw["edges"].as_array() {
        edges.iter().take(300).cloned().collect()
    } else {
        nodes
            .windows(2)
            .map(|pair| json!({"source":pair[0]["id"],"target":pair[1]["id"]}))
            .collect::<Vec<_>>()
    };
    let ids = nodes
        .iter()
        .filter_map(|n| n["id"].as_str())
        .collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    let mut edges = Vec::new();
    for edge in source_edges {
        let source = text(&edge["source"], usize::MAX);
        let target = text(&edge["target"], usize::MAX);
        let port = choice(&edge["sourcePort"], &["output", "true", "false"], "output");
        if !ids.contains(source.as_str())
            || !ids.contains(target.as_str())
            || source == target
            || !seen.insert((source.clone(), port.clone(), target.clone()))
        {
            continue;
        }
        edges.push(json!({"id":edge["id"].as_str().filter(|s|!s.is_empty()).map(str::to_owned).map(Ok).unwrap_or_else(id)?,"source":source,"sourcePort":port,"target":target,"targetPort":"input"}));
    }
    let definitions = if strict_inputs {
        inputs::definitions(raw.get("inputs"))?
    } else {
        raw["inputs"].as_array().into_iter().flatten().take(30).enumerate().map(|(index,input)|json!({
            "id":text(&input.get("id").cloned().unwrap_or(json!(format!("input-{}",index+1))),80),
            "name":text(&input.get("name").cloned().unwrap_or(json!(format!("input_{}",index+1))),80),
            "label":text(&input.get("label").cloned().unwrap_or(input.get("name").cloned().unwrap_or(json!(format!("Input {}",index+1)))),120),
            "type":choice(&input["type"],&["string","number","boolean","text","image","video"],"string"),"required":input["required"].as_bool().unwrap_or(false),
            "defaultValue":input.get("defaultValue").cloned().unwrap_or(if ["image","video"].contains(&input["type"].as_str().unwrap_or("")){Value::Null}else{json!("")}),"description":text(&input["description"],300)})).collect()
    };
    Ok(
        json!({"id":raw["id"].as_str().filter(|s|!s.is_empty()).map(str::to_owned).map(Ok).unwrap_or_else(id)?,"name":text(&raw.get("name").cloned().unwrap_or(json!("未命名工作流")),120),"description":text(&raw["description"],600),
        "status":choice(&raw["status"],&["published"],"draft"),"revision":number(&raw["revision"],1.0,1.0,f64::MAX),"cwd":raw["cwd"].as_str().filter(|s|!s.is_empty()).unwrap_or(cwd),"model":model(&raw["model"]),"inputs":definitions,"tags":strings(&raw["tags"],12),"visibility":choice(&raw["visibility"],&["shared"],"private"),"notifications":targets(&raw["notifications"]),"nodes":nodes,"edges":edges,
        "createdAt":raw.get("createdAt").cloned().unwrap_or(json!(stamp)),"updatedAt":raw.get("updatedAt").cloned().unwrap_or(json!(stamp)),"publishedAt":raw.get("publishedAt").cloned().unwrap_or(Value::Null),"lastRunAt":raw.get("lastRunAt").cloned().unwrap_or(Value::Null),"lastStatus":raw.get("lastStatus").cloned().unwrap_or(json!("idle")),"lastSummary":text(&raw["lastSummary"],1200),"lastError":text(&raw["lastError"],1200)}),
    )
}

#[derive(Clone, Debug)]
pub(crate) struct Graph {
    pub(crate) nodes: Vec<Value>,
    pub(crate) edges: Vec<Value>,
    pub(crate) order: Vec<String>,
    pub(crate) incoming: HashMap<String, Vec<Value>>,
    pub(crate) outgoing: HashMap<String, Vec<Value>>,
}
pub(crate) fn graph(workflow: &Value) -> Result<Graph> {
    let nodes = workflow["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|n| n["enabled"] != false)
        .cloned()
        .collect::<Vec<_>>();
    let ids = nodes
        .iter()
        .map(|n| n["id"].as_str().unwrap_or("").to_owned())
        .collect::<HashSet<_>>();
    if ids.len() != nodes.len() {
        return Err(WorkflowError::invalid("工作流节点 ID 必须唯一。"));
    }
    let edges = workflow["edges"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| {
            ids.contains(e["source"].as_str().unwrap_or(""))
                && ids.contains(e["target"].as_str().unwrap_or(""))
        })
        .cloned()
        .collect::<Vec<_>>();
    let mut incoming = ids
        .iter()
        .map(|id| (id.clone(), Vec::new()))
        .collect::<HashMap<_, _>>();
    let mut outgoing = incoming.clone();
    for edge in &edges {
        if let Some(v) = incoming.get_mut(edge["target"].as_str().unwrap_or("")) {
            v.push(edge.clone());
        }
        if let Some(v) = outgoing.get_mut(edge["source"].as_str().unwrap_or("")) {
            v.push(edge.clone());
        }
    }
    let mut indegrees = incoming
        .iter()
        .map(|(id, edges)| (id.clone(), edges.len()))
        .collect::<HashMap<_, _>>();
    let mut order = Vec::new();
    while order.len() < nodes.len() {
        let ready = nodes.iter().find(|node| {
            let id = node["id"].as_str().unwrap_or("");
            indegrees[id] == 0 && !order.iter().any(|seen| seen == id)
        });
        let Some(node) = ready else { break };
        let id = node["id"].as_str().unwrap_or("").to_owned();
        order.push(id.clone());
        for edge in &outgoing[&id] {
            if let Some(degree) = indegrees.get_mut(edge["target"].as_str().unwrap_or("")) {
                *degree = degree.saturating_sub(1);
            }
        }
    }
    if !nodes.iter().any(|n| {
        ["prompt", "skill", "file", "mcp", "condition"].contains(&n["kind"].as_str().unwrap_or(""))
            || IMAGE_KINDS.contains(&n["kind"].as_str().unwrap_or(""))
    }) {
        return Err(WorkflowError::invalid("工作流至少需要一个可执行节点。"));
    }
    for node in &nodes {
        let kind = node["kind"].as_str().unwrap_or("");
        if ["prompt", "skill", "file", "mcp"].contains(&kind)
            && text(&node["prompt"], 100000).is_empty()
            && !(kind == "skill" && !text(&node["skillName"], 120).is_empty())
        {
            return Err(WorkflowError::invalid(format!(
                "节点「{}」还没有填写 Prompt。",
                text(&node["label"], 120)
            )));
        }
    }
    if nodes.len() > 1 && edges.is_empty() {
        return Err(WorkflowError::invalid("工作流节点尚未建立连接。"));
    }
    for node in &nodes {
        if node["kind"] == "trigger" && !incoming[node["id"].as_str().unwrap_or("")].is_empty() {
            return Err(WorkflowError::invalid("触发器不能连接上游节点。"));
        }
    }
    let roots = nodes
        .iter()
        .filter(|n| incoming[n["id"].as_str().unwrap_or("")].is_empty())
        .collect::<Vec<_>>();
    if roots.len() > 1 {
        return Err(WorkflowError::invalid("节点尚未连接到工作流。"));
    }
    if order.len() != nodes.len() {
        return Err(WorkflowError::invalid("工作流不能包含循环连接。"));
    }
    for node in &nodes {
        if node["kind"] == "condition" {
            let edges = &outgoing[node["id"].as_str().unwrap_or("")];
            if !["true", "false"]
                .iter()
                .all(|port| edges.iter().any(|e| e["sourcePort"] == *port))
            {
                return Err(WorkflowError::invalid(
                    "判断节点需要同时连接 true 和 false 分支。",
                ));
            }
        }
    }
    Ok(Graph {
        nodes,
        edges,
        order,
        incoming,
        outgoing,
    })
}
pub(crate) fn configuration_hash(node: &Value) -> String {
    use sha2::{Digest, Sha256};
    let mut value = node.clone();
    if let Some(object) = value.as_object_mut() {
        object.remove("x");
        object.remove("y");
        object.remove("label");
    }
    normalize_js_numbers(&mut value);
    Sha256::digest(value.to_string().as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn normalize_js_numbers(value: &mut Value) {
    match value {
        Value::Number(number) if number.is_f64() => {
            if let Some(value_number) = number
                .as_f64()
                .filter(|number| number.fract() == 0.0 && number.abs() < 9_007_199_254_740_992.0)
            {
                *value = json!(value_number as i64);
            }
        }
        Value::Object(object) => object.values_mut().for_each(normalize_js_numbers),
        Value::Array(array) => array.iter_mut().for_each(normalize_js_numbers),
        _ => {}
    }
}
pub(crate) fn template_context(workflow: &Value, run: &Value, predecessors: &[Value]) -> Value {
    let nodes = run["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|n| n["status"] == "completed")
        .filter_map(|n| n["id"].as_str().map(|id| (id.to_owned(), n.clone())))
        .collect::<serde_json::Map<_, _>>();
    json!({"inputs":run["inputs"],"previous":predecessors.last().cloned().unwrap_or(Value::Null),"nodes":nodes,"workflow":{"id":workflow["id"],"name":workflow["name"],"description":workflow["description"]},"run":{"id":run["id"],"startedAt":run["startedAt"]}})
}
fn js_string(value: Option<&Value>) -> String {
    match value {
        None => "undefined".into(),
        Some(Value::Null) => "null".into(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(v)) => v
            .iter()
            .map(|v| {
                if v.is_null() {
                    String::new()
                } else {
                    js_string(Some(v))
                }
            })
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::Object(_)) => "[object Object]".into(),
        Some(v) => v.to_string(),
    }
}
pub(crate) fn condition(condition: &Value, context: &Value) -> bool {
    let path = condition["source"].as_str().unwrap_or("").trim();
    let segments = path
        .split('.')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let (mut actual, offset) = match segments.first().copied() {
        Some("inputs") => (context.get("inputs"), 1),
        Some("previous") => (context["previous"].get("output"), 1),
        Some("nodes") => (
            segments
                .get(1)
                .and_then(|id| context["nodes"].get(*id))
                .and_then(|n| n.get("output")),
            2,
        ),
        _ => (
            if path.is_empty() {
                context["previous"]
                    .get("output")
                    .or_else(|| context["previous"].get("summary"))
            } else {
                context.get("inputs")
            },
            0,
        ),
    };
    let mut owned_actual = actual.cloned();
    for part in segments.iter().skip(offset) {
        owned_actual = if ["__proto__", "constructor", "prototype"].contains(part) {
            None
        } else {
            owned_actual.and_then(|value| match value {
                Value::Object(object) => object.get(*part).cloned(),
                Value::Array(array) if *part == "length" => Some(json!(array.len())),
                Value::Array(array) => part
                    .parse::<usize>()
                    .ok()
                    .and_then(|index| array.get(index))
                    .cloned(),
                _ => None,
            })
        };
    }
    actual = owned_actual.as_ref();
    let expected = condition.get("value");
    let exists = actual.is_some_and(|v| !v.is_null() && v != "");
    let numeric = |value: Option<&Value>| js_number(value);
    match condition["operator"].as_str().unwrap_or("exists") {
        "exists" => exists,
        "not_exists" => !exists,
        "equals" => js_string(actual) == js_string(expected),
        "not_equals" => js_string(actual) != js_string(expected),
        "contains" => actual.is_some_and(|v| {
            if let Some(values) = v.as_array() {
                values
                    .iter()
                    .any(|v| js_string(Some(v)) == js_string(expected))
            } else {
                (if v.is_null() {
                    String::new()
                } else {
                    js_string(Some(v))
                })
                .contains(&js_string(expected))
            }
        }),
        "greater_than" => numeric(actual) > numeric(expected),
        "less_than" => numeric(actual) < numeric(expected),
        _ => exists,
    }
}
fn js_number(value: Option<&Value>) -> f64 {
    match value {
        None => f64::NAN,
        Some(Value::Null) => 0.0,
        Some(Value::Bool(value)) => {
            if *value {
                1.0
            } else {
                0.0
            }
        }
        Some(Value::Number(value)) => value.as_f64().unwrap_or(f64::NAN),
        Some(Value::Object(_)) => f64::NAN,
        Some(value) => {
            let text = js_string(Some(value));
            let text = text.trim();
            if text.is_empty() {
                return 0.0;
            }
            if ["Infinity", "+Infinity"].contains(&text) {
                return f64::INFINITY;
            }
            if text == "-Infinity" {
                return f64::NEG_INFINITY;
            }
            for (prefix, radix) in [
                ("0x", 16),
                ("0X", 16),
                ("0b", 2),
                ("0B", 2),
                ("0o", 8),
                ("0O", 8),
            ] {
                if let Some(digits) = text.strip_prefix(prefix) {
                    return u64::from_str_radix(digits, radix)
                        .map(|value| value as f64)
                        .unwrap_or(f64::NAN);
                }
            }
            if text.to_lowercase().contains("inf") {
                f64::NAN
            } else {
                text.parse().unwrap_or(f64::NAN)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn graph_uses_edges_and_rejects_cycles() {
        let wf=normalize(&json!({"name":"graph","nodes":[{"id":"b","kind":"prompt","prompt":"b"},{"id":"a","kind":"prompt","prompt":"a"}],"edges":[{"source":"a","target":"b"}]}),".",true).unwrap();
        assert_eq!(graph(&wf).unwrap().order, vec!["a", "b"]);
        let mut cyc = wf.clone();
        cyc["edges"]
            .as_array_mut()
            .unwrap()
            .push(json!({"source":"b","target":"a"}));
        assert!(graph(&cyc).is_err());
    }
    #[test]
    fn node_hash_ignores_only_presentation() {
        let a = node(&json!({"id":"a","prompt":"test"}), 0).unwrap();
        let mut b = a.clone();
        b["x"] = json!(55);
        b["label"] = json!("move");
        assert_eq!(configuration_hash(&a), configuration_hash(&b));
        b["prompt"] = json!("changed");
        assert_ne!(configuration_hash(&a), configuration_hash(&b));
    }
}
