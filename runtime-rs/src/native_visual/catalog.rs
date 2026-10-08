//! 视觉目录只消费一次配置快照；凭据保留在私有候选，客户端投影不携带密钥。
use super::{VisualConfigSnapshot, VisualError, VisualKind, VisualModel};
use icu_collator::Collator;
use icu_locale_core::Locale;
use regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::HashMap;

pub(super) fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

pub(super) fn js_whitespace(value: char) -> bool {
    matches!(value, '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')
}

pub(super) fn slice_utf16(value: &str, limit: usize) -> String {
    // JS 的截断计量单位是 UTF-16；孤立代理项替换以维持 Web/TUI 的合法 UTF-8 契约。
    String::from_utf16_lossy(&value.encode_utf16().take(limit).collect::<Vec<_>>())
}

pub(super) fn js_string_checked(value: &Value) -> Result<String, VisualError> {
    match value {
        Value::Null => Ok("null".into()),
        Value::Bool(value) => Ok(value.to_string()),
        Value::Number(value) => {
            let number = value.as_f64().unwrap_or_default();
            if number == 0.0 {
                Ok("0".into())
            } else if number.abs() >= 1e21 || number.abs() < 1e-6 {
                let text = format!("{number:e}");
                let (mantissa, exponent) = text
                    .split_once('e')
                    .ok_or_else(|| VisualError::new("Cannot convert number to string"))?;
                let exponent = exponent
                    .parse::<i32>()
                    .map_err(|_| VisualError::new("Cannot convert number to string"))?;
                Ok(format!(
                    "{mantissa}e{}{exponent}",
                    if exponent >= 0 { "+" } else { "" }
                ))
            } else {
                Ok(number.to_string())
            }
        }
        Value::String(value) => Ok(value.clone()),
        Value::Array(values) => values
            .iter()
            .map(|value| {
                if value.is_null() {
                    Ok(String::new())
                } else {
                    js_string_checked(value)
                }
            })
            .collect::<Result<Vec<_>, _>>()
            .map(|values| values.join(",")),
        Value::Object(values) => {
            if values.contains_key("toString") {
                Err(VisualError::new("Cannot convert object to primitive value"))
            } else {
                Ok("[object Object]".into())
            }
        }
    }
}

pub(super) fn string_or_empty(value: &Value) -> Result<String, VisualError> {
    if truthy(value) {
        js_string_checked(value)
    } else {
        Ok(String::new())
    }
}

fn first<'a>(values: &[&'a Value]) -> &'a Value {
    values
        .iter()
        .copied()
        .find(|value| truthy(value))
        .unwrap_or(&Value::Null)
}

fn credential(value: &Value) -> Value {
    if value.is_string() {
        return value.clone();
    }
    if value["type"] == "api_key" {
        return if truthy(&value["key"]) {
            value["key"].clone()
        } else {
            Value::String(String::new())
        };
    }
    first(&[&value["key"], &value["token"], &value["access_token"]]).clone()
}

fn default_base_url(provider: &str, api: &str) -> &'static str {
    let value = format!("{provider} {api}").to_lowercase();
    if value.contains("google") {
        "https://generativelanguage.googleapis.com/v1beta"
    } else if ["xai", "x-ai", "grok"]
        .iter()
        .any(|pattern| value.contains(pattern))
    {
        "https://api.x.ai/v1"
    } else if value.contains("openrouter") {
        "https://openrouter.ai/api/v1"
    } else if value.contains("openai") {
        "https://api.openai.com/v1"
    } else {
        ""
    }
}

fn infer_kind(value: &Value) -> &str {
    value
        .as_str()
        .filter(|value| ["chat", "image", "video"].contains(value))
        .unwrap_or("chat")
}

fn supports(definition: &Value, model_kind: &str, kind: VisualKind) -> bool {
    let explicit = definition["capabilities"]
        .as_array()
        .map(|values| {
            ["chat", "image", "video"]
                .into_iter()
                .filter(|kind| values.iter().any(|value| value.as_str() == Some(*kind)))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if explicit.is_empty() {
        model_kind == kind.as_str()
    } else {
        explicit.contains(&kind.as_str())
    }
}

fn visual_provider(provider_id: &str, provider: &Value, app: &Value) -> bool {
    let explicit = &app["providerTypes"][provider_id];
    if truthy(explicit) {
        return explicit == "visual";
    }
    let Some(models) = provider["models"].as_array() else {
        return false;
    };
    !models.is_empty()
        && models
            .iter()
            .all(|model| infer_kind(&model["kind"]) != "chat")
}

fn spread(source: &Value, destination: &mut Map<String, Value>) {
    match source {
        Value::Object(values) => {
            for (key, value) in values {
                destination.insert(key.clone(), value.clone());
            }
        }
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                destination.insert(index.to_string(), value.clone());
            }
        }
        Value::String(value) => {
            for (index, unit) in value.encode_utf16().enumerate() {
                destination.insert(
                    index.to_string(),
                    Value::String(String::from_utf16_lossy(&[unit])),
                );
            }
        }
        _ => {}
    }
}

fn configured_provider_ids(providers: &Value) -> Vec<String> {
    let mut values = match providers {
        Value::Object(values) => values.keys().cloned().collect::<Vec<_>>(),
        Value::Array(values) => (0..values.len()).map(|index| index.to_string()).collect(),
        _ => Vec::new(),
    };
    // Object.keys 把数组索引属性放在前面，其他键仍保留 JSON 的插入顺序。
    values.sort_by_key(|value| {
        value
            .parse::<u32>()
            .ok()
            .filter(|number| *number != u32::MAX && number.to_string() == *value)
            .map(|number| (0, number))
            .unwrap_or((1, 0))
    });
    values
}

fn runtime_provider(model: &Value) -> &Value {
    first(&[&model["provider"], &model["providerId"]])
}

fn driver(public: &Value) -> Result<String, VisualError> {
    if truthy(&public["visualApi"]) {
        return js_string_checked(&public["visualApi"]);
    }
    let mut parts = Vec::new();
    for field in ["providerId", "api", "baseUrl", "id"] {
        parts.push(js_string_checked(&public[field])?);
    }
    let value = parts.join(" ").to_lowercase();
    let kind = if public["kind"] == "video" {
        "video"
    } else {
        "image"
    };
    Ok(
        if value.contains("google") || value.contains("generativelanguage.googleapis.com") {
            format!("google-{kind}")
        } else if ["xai", "x.ai", "grok"]
            .iter()
            .any(|pattern| value.contains(pattern))
        {
            format!("xai-{kind}")
        } else if value.contains("openrouter") && kind == "image" {
            "openrouter-image".into()
        } else {
            format!("openai-{kind}")
        },
    )
}

fn score(provider: &str, id: &str) -> Result<i64, VisualError> {
    let id = id.to_lowercase();
    let gpt_image = Regex::new(r"gpt-image|gpt-[0-9][^\r\n\u{2028}\u{2029}]*image")
        .map_err(|error| VisualError::new(error.to_string()))?;
    let mut score = if id.contains("gpt-image-2") || id.contains("gpt-5.4-image") {
        120
    } else if gpt_image.is_match(&id) {
        105
    } else {
        0
    };
    if id.contains("gemini-3") || id.contains("imagen-4") {
        score += 100;
    }
    if id.contains("grok-imagine") {
        score += 95;
    }
    if id.contains("sora-2-pro") || id.contains("veo-3.1") {
        score += 120;
    } else if id.contains("sora-2") || id.contains("veo-3") {
        score += 105;
    }
    Ok(score
        + match provider {
            "openai" => 8,
            "google" => 7,
            "xai" => 6,
            _ => 0,
        })
}

pub(super) fn candidates(
    snapshot: &VisualConfigSnapshot,
    kind: VisualKind,
) -> Result<Vec<VisualModel>, VisualError> {
    if snapshot.app_json.is_null() {
        return Err(VisualError::new(
            "Cannot read properties of null (reading 'disabledProviders')",
        ));
    }
    let disabled = &snapshot.app_json["disabledProviders"];
    let disabled_names: Vec<Value> = match disabled {
        Value::Array(values) => values.clone(),
        Value::String(value) => value
            .chars()
            .map(|value| json!(value.to_string()))
            .collect(),
        value if !truthy(value) => Vec::new(),
        _ => {
            return Err(VisualError::new(
                "object is not iterable (cannot read property Symbol(Symbol.iterator))",
            ))
        }
    };
    if snapshot.models_json.is_null() {
        return Err(VisualError::new(
            "Cannot read properties of null (reading 'providers')",
        ));
    }
    let providers = &snapshot.models_json["providers"];
    let mut provider_ids = configured_provider_ids(providers);
    for provider_id in snapshot.runtime_provider_names.keys() {
        if !provider_ids.contains(provider_id) {
            provider_ids.push(provider_id.clone());
        }
    }
    let mut result = Vec::new();
    for provider_id in provider_ids {
        if disabled_names.iter().any(|value| value == &provider_id) {
            continue;
        }
        let provider = &providers[&provider_id];
        if snapshot.auth_json.is_null() {
            return Err(VisualError::new(format!(
                "Cannot read properties of null (reading '{provider_id}')"
            )));
        }
        let key = credential(&snapshot.auth_json[&provider_id]);
        if !truthy(&key) {
            continue;
        }
        let mut definitions: Vec<(String, Value)> = Vec::new();
        if truthy(&provider["models"]) {
            let values = provider["models"]
                .as_array()
                .ok_or_else(|| VisualError::new("(provider.models || []).map is not a function"))?;
            for definition in values {
                let id = definition["id"]
                    .as_str()
                    .ok_or_else(|| VisualError::new("model.id.toLowerCase is not a function"))?
                    .to_owned();
                if let Some((_, previous)) = definitions.iter_mut().find(|(key, _)| key == &id) {
                    *previous = definition.clone();
                } else {
                    definitions.push((id, definition.clone()));
                }
            }
        }
        let runtime_models = snapshot
            .runtime_models
            .iter()
            .filter(|model| runtime_provider(model) == &provider_id)
            .collect::<Vec<_>>();
        let mut model_ids = definitions
            .iter()
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for model in &runtime_models {
            let id = model["id"]
                .as_str()
                .ok_or_else(|| VisualError::new("model.id.toLowerCase is not a function"))?;
            if !model_ids.iter().any(|value| value == id) {
                model_ids.push(id.to_owned());
            }
        }
        for id in model_ids {
            let definition = definitions.iter().find(|(key, _)| key == &id);
            let configured = definition.is_some();
            let definition = definition.map(|(_, value)| value).unwrap_or(&Value::Null);
            let native = runtime_models
                .iter()
                .copied()
                .find(|model| model["id"] == id)
                .unwrap_or(&Value::Null);
            let model_kind = infer_kind(first(&[&definition["kind"], &native["pisperKind"]]));
            if !supports(definition, model_kind, kind) {
                continue;
            }
            let api = first(&[&definition["api"], &provider["api"], &native["api"]]);
            let api = if truthy(api) {
                api.clone()
            } else {
                Value::String(String::new())
            };
            let base = first(&[
                &definition["baseUrl"],
                &provider["baseUrl"],
                &native["baseUrl"],
            ]);
            let base = if truthy(base) {
                js_string_checked(base)?
            } else {
                default_base_url(&provider_id, &js_string_checked(&api)?).into()
            };
            let base = base.trim_end_matches('/').to_owned();
            if base.is_empty() {
                continue;
            }
            let name = first(&[&definition["name"], &native["name"]]);
            let name = if truthy(name) {
                name.clone()
            } else {
                Value::String(id.clone())
            };
            let runtime_name = snapshot
                .runtime_provider_names
                .get(&provider_id)
                .unwrap_or(&Value::Null);
            let runtime_name = if runtime_name.is_object() {
                &runtime_name["name"]
            } else {
                runtime_name
            };
            let provider_name = first(&[&provider["name"], runtime_name]);
            let provider_name = if truthy(provider_name) {
                provider_name.clone()
            } else {
                Value::String(provider_id.clone())
            };
            let visual_api = first(&[&definition["visualApi"], &provider["visualApi"]]);
            let visual_api = if truthy(visual_api) {
                visual_api.clone()
            } else {
                Value::String(String::new())
            };
            let mut headers = Map::new();
            for value in [
                &provider["headers"],
                &native["headers"],
                &definition["headers"],
            ] {
                spread(value, &mut headers);
            }
            let mut public = json!({
                "id":id,"name":name,"providerId":provider_id,"providerName":provider_name,
                "api":api,"kind":kind.as_str(),"baseUrl":base,"visualApi":visual_api,
            });
            let driver = driver(&public)?;
            let score = score(&provider_id, &id)?;
            public["driver"] = Value::String(driver);
            public["score"] = json!(score);
            result.push(VisualModel {
                public,
                key: key.clone(),
                headers,
                visual: visual_provider(&provider_id, provider, &snapshot.app_json),
                configured,
                score,
            });
        }
    }
    Ok(result)
}

pub(super) fn reference(model: &VisualModel) -> String {
    format!(
        "{}/{}",
        model.public["providerId"].as_str().unwrap_or_default(),
        model.public["id"].as_str().unwrap_or_default()
    )
}

pub(super) fn preferred_reference(
    snapshot: &VisualConfigSnapshot,
    kind: VisualKind,
) -> Result<String, VisualError> {
    if snapshot.app_json.is_null() {
        return Err(VisualError::new(
            "Cannot read properties of null (reading 'visualDefaultModels')",
        ));
    }
    Ok(
        string_or_empty(&snapshot.app_json["visualDefaultModels"][kind.as_str()])?
            .trim_matches(js_whitespace)
            .to_owned(),
    )
}

pub(super) fn ordered_models(
    snapshot: &VisualConfigSnapshot,
    kind: VisualKind,
    candidates: Vec<VisualModel>,
) -> Result<Vec<VisualModel>, VisualError> {
    let mut unique: Vec<VisualModel> = Vec::new();
    let mut positions: HashMap<String, usize> = HashMap::new();
    for model in candidates {
        let base = js_string_checked(&model.public["baseUrl"])?
            .trim_matches(js_whitespace)
            .trim_end_matches('/')
            .to_lowercase();
        let key = format!(
            "{base}\0{}\0{}\0{}",
            model.public["id"]
                .as_str()
                .unwrap_or_default()
                .to_lowercase(),
            kind.as_str(),
            js_string_checked(&model.public["driver"])?
        );
        let priority = |model: &VisualModel| {
            i64::from(model.visual) * 10_000 + i64::from(model.configured) * 1_000 + model.score
        };
        if let Some(position) = positions.get(&key).copied() {
            if priority(&model) > priority(&unique[position]) {
                unique[position] = model;
            }
        } else {
            positions.insert(key, unique.len());
            unique.push(model);
        }
    }
    let locale: Locale = snapshot
        .locale
        .parse()
        .map_err(|_| VisualError::new("视觉模型排序 locale 无效。"))?;
    let collator = Collator::try_new(locale.into(), Default::default())
        .map_err(|error| VisualError::new(error.to_string()))?;
    if unique.len() > 1 && unique.iter().any(|model| !model.public["name"].is_string()) {
        return Err(VisualError::new(
            "left.name.localeCompare is not a function",
        ));
    }
    unique.sort_by(|left, right| {
        let priority = |model: &VisualModel| {
            model.score + i64::from(model.visual) * 25 + i64::from(model.configured) * 2
        };
        priority(right).cmp(&priority(left)).then_with(|| {
            collator.compare(
                left.public["name"].as_str().unwrap_or_default(),
                right.public["name"].as_str().unwrap_or_default(),
            )
        })
    });
    let preferred = preferred_reference(snapshot, kind)?.to_lowercase();
    if let Some(position) = unique
        .iter()
        .position(|model| reference(model).to_lowercase() == preferred)
        .filter(|position| *position > 0)
    {
        let model = unique.remove(position);
        unique.insert(0, model);
    }
    Ok(unique)
}

pub(super) fn status(
    snapshot: &VisualConfigSnapshot,
    kind: VisualKind,
) -> Result<Value, VisualError> {
    let models = ordered_models(snapshot, kind, candidates(snapshot, kind)?)?;
    let preferred = preferred_reference(snapshot, kind)?;
    let selection = if models
        .iter()
        .any(|model| reference(model).to_lowercase() == preferred.to_lowercase())
    {
        preferred
    } else {
        String::new()
    };
    Ok(json!({
        "model":models.first().map(|model| &model.public),
        "models":models.iter().map(|model| &model.public).collect::<Vec<_>>(),
        "selection":selection,
    }))
}

fn no_models(kind: VisualKind) -> VisualError {
    VisualError::new(format!(
        "没有已配置并启用的{}生成模型。请先在配置页添加视觉模型。",
        if kind == VisualKind::Video {
            "视频"
        } else {
            "图像"
        }
    ))
}

pub(super) fn select(
    snapshot: &VisualConfigSnapshot,
    kind: VisualKind,
    requested: Option<&str>,
) -> Result<VisualModel, VisualError> {
    let candidates = candidates(snapshot, kind)?;
    if candidates.is_empty() {
        return Err(no_models(kind));
    }
    let requested_lower = requested
        .unwrap_or_default()
        .trim_matches(js_whitespace)
        .to_lowercase();
    if !requested_lower.is_empty() {
        if let Some(model) = candidates
            .iter()
            .find(|model| reference(model).to_lowercase() == requested_lower)
        {
            return Ok(model.clone());
        }
    }
    let models = ordered_models(snapshot, kind, candidates)?;
    if requested_lower.is_empty() {
        return models.into_iter().next().ok_or_else(|| no_models(kind));
    }
    models
        .into_iter()
        .find(|model| {
            model.public["id"]
                .as_str()
                .unwrap_or_default()
                .to_lowercase()
                == requested_lower
        })
        .ok_or_else(|| {
            VisualError::new(format!(
                "未找到已启用的视觉模型：{}",
                requested.unwrap_or_default()
            ))
        })
}

pub(super) fn all(
    snapshot: &VisualConfigSnapshot,
    kind: VisualKind,
) -> Result<Vec<VisualModel>, VisualError> {
    let candidates = candidates(snapshot, kind)?;
    if candidates.is_empty() {
        return Err(no_models(kind));
    }
    ordered_models(snapshot, kind, candidates)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(models: Value, auth: Value, app: Value) -> VisualConfigSnapshot {
        VisualConfigSnapshot {
            models_json: models,
            auth_json: auth,
            app_json: app,
            runtime_provider_names: Map::new(),
            runtime_models: Vec::new(),
            locale: "zh-CN".into(),
        }
    }

    fn duplicate_fixture() -> VisualConfigSnapshot {
        let mut snapshot = snapshot(
            json!({"providers":{
                "openai":{"name":"Chat Provider","api":"openai-responses","baseUrl":"https://RELAY.test/v1/"},
                "openai-image":{"name":"Visual Provider","api":"openai-responses","baseUrl":"https://relay.test/v1","models":[{"id":"gpt-image-2","name":"A primary image","kind":"image"}]},
                "backup":{"name":"Visual Backup","api":"openai-responses","baseUrl":"https://backup.test/v1","models":[{"id":"gpt-image-2","name":"Z backup image","kind":"image"}]}
            }}),
            json!({"openai":{"type":"api_key","key":"chat-fictional-secret"},"openai-image":{"type":"api_key","key":"visual-fictional-secret"},"backup":"backup-fictional-secret"}),
            json!({"providerTypes":{"openai":"chat","openai-image":"visual","backup":"visual"}}),
        );
        snapshot
            .runtime_provider_names
            .insert("openai".into(), json!("OpenAI runtime"));
        snapshot.runtime_models.push(json!({"provider":"openai","id":"gpt-image-2","name":"Discovered copy","api":"openai-responses","baseUrl":"https://relay.test/v1","pisperKind":"image"}));
        snapshot
    }

    #[test]
    fn dedicated_visual_dedup_and_qualified_selection_keep_correct_credential() {
        let snapshot = duplicate_fixture();
        let ordered = all(&snapshot, VisualKind::Image).unwrap();
        assert_eq!(
            ordered.iter().map(reference).collect::<Vec<_>>(),
            ["openai-image/gpt-image-2", "backup/gpt-image-2"]
        );
        for requested in [None, Some("gpt-image-2"), Some("OPENAI-IMAGE/GPT-IMAGE-2")] {
            let model = select(&snapshot, VisualKind::Image, requested).unwrap();
            assert_eq!(reference(&model), "openai-image/gpt-image-2");
            assert_eq!(model.key, "visual-fictional-secret");
        }
        let qualified = select(&snapshot, VisualKind::Image, Some(" openai/gpt-image-2 ")).unwrap();
        assert_eq!(qualified.key, "chat-fictional-secret");
        assert_eq!(reference(&qualified), "openai/gpt-image-2");
        let status = status(&snapshot, VisualKind::Image).unwrap();
        assert_eq!(status["models"].as_array().unwrap().len(), 2);
        for model in status["models"].as_array().unwrap() {
            assert_eq!(model.as_object().unwrap().len(), 10);
            for field in [
                "apiKey",
                "headers",
                "key",
                "visualProvider",
                "configuredDefinition",
            ] {
                assert!(model.get(field).is_none());
            }
        }
        assert!(!serde_json::to_string(&status)
            .unwrap()
            .contains("fictional-secret"));
    }

    #[test]
    fn stored_preference_is_case_insensitive_and_invalid_preference_is_not_mutated() {
        let mut snapshot = duplicate_fixture();
        snapshot.app_json["visualDefaultModels"] =
            json!({"image":" BACKUP/GPT-IMAGE-2 ","video":"missing/video"});
        let image = status(&snapshot, VisualKind::Image).unwrap();
        assert_eq!(image["model"]["providerId"], "backup");
        assert_eq!(image["selection"], "BACKUP/GPT-IMAGE-2");
        snapshot.app_json["visualDefaultModels"]["image"] = json!("openai/gpt-image-2");
        let image = status(&snapshot, VisualKind::Image).unwrap();
        assert_eq!(image["model"]["providerId"], "openai-image");
        assert_eq!(image["selection"], "");
        assert_eq!(
            snapshot.app_json["visualDefaultModels"]["image"],
            "openai/gpt-image-2"
        );
        let video = status(&snapshot, VisualKind::Video).unwrap();
        assert!(video["model"].is_null());
        assert_eq!(video["models"], json!([]));
        assert_eq!(video["selection"], "");
    }

    #[test]
    fn capabilities_override_kind_and_definition_runtime_precedence_matches_release() {
        let mut snapshot = snapshot(
            json!({"providers":{"p":{
                "name":"Configured Provider","api":"provider-api","baseUrl":"https://provider.test/v1/","visualApi":"provider-driver",
                "headers":{"X-Order":"provider","X-Provider":"p"},
                "models":[
                    {"id":"multi","kind":"chat","capabilities":["video","image","unknown"],"name":"Configured Model","api":"definition-api","baseUrl":"https://definition.test/v1///","visualApi":"definition-driver","headers":{"X-Order":"definition"}},
                    {"id":"fallback","kind":"image","capabilities":["unknown"]},
                    {"id":"gpt-image-2"},
                    {"id":"runtime-kind","kind":"auto"}
                ]
            }}}),
            json!({"p":{"type":"oauth","token":"fictional-token"}}),
            json!({}),
        );
        snapshot
            .runtime_provider_names
            .insert("p".into(), json!("Runtime Provider"));
        snapshot.runtime_models = vec![
            json!({"providerId":"p","id":"multi","name":"Runtime Model","api":"runtime-api","baseUrl":"https://runtime.test/v1","pisperKind":"video","headers":{"X-Order":"runtime","X-Runtime":"r"}}),
            json!({"provider":"p","id":"runtime-kind","pisperKind":"video"}),
            json!({"provider":"p","id":"runtime-only","name":"Runtime Only","pisperKind":"video"}),
        ];
        let images = candidates(&snapshot, VisualKind::Image).unwrap();
        assert_eq!(
            images.iter().map(reference).collect::<Vec<_>>(),
            ["p/multi", "p/fallback"]
        );
        let videos = candidates(&snapshot, VisualKind::Video).unwrap();
        assert_eq!(
            videos.iter().map(reference).collect::<Vec<_>>(),
            ["p/multi", "p/runtime-only"]
        );
        let multi = &images[0];
        assert_eq!(multi.public["kind"], "image");
        assert_eq!(multi.public["name"], "Configured Model");
        assert_eq!(multi.public["providerName"], "Configured Provider");
        assert_eq!(multi.public["api"], "definition-api");
        assert_eq!(multi.public["baseUrl"], "https://definition.test/v1");
        assert_eq!(multi.public["visualApi"], "definition-driver");
        assert_eq!(multi.public["driver"], "definition-driver");
        assert_eq!(multi.headers["X-Order"], "definition");
        assert_eq!(multi.headers["X-Provider"], "p");
        assert_eq!(multi.headers["X-Runtime"], "r");
        assert_eq!(multi.key, "fictional-token");
        assert!(!multi.visual);
        assert!(multi.configured);
        assert!(!videos[1].configured);
    }

    #[test]
    fn credentials_disabled_providers_and_no_extra_model_filters_match_release() {
        let snapshot = snapshot(
            json!({"providers":{
                "api-key-token":{"api":"openai-responses","models":[{"id":"i","kind":"image"}]},
                "oauth":{"api":"openai-responses","apiKey":"ignored-provider-key","models":[{"id":"i","kind":"image","apiKey":"ignored-model-key","enabled":false}]},
                "no-auth":{"api":"openai-responses","apiKey":"ignored","models":[{"id":"i","kind":"image"}]},
                "disabled":{"api":"openai-responses","models":[{"id":"i","kind":"image"}]}
            }}),
            json!({"api-key-token":{"type":"api_key","key":"","token":"must-be-ignored"},"oauth":{"key":0,"token":"accepted"},"disabled":"disabled-key"}),
            json!({"disabledProviders":["disabled"],"excludedModels":["oauth/i"],"providerTypes":{"oauth":"chat"}}),
        );
        let models = candidates(&snapshot, VisualKind::Image).unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(reference(&models[0]), "oauth/i");
        assert_eq!(models[0].key, "accepted");
        assert!(!models[0].visual);
        assert_eq!(credential(&json!({"type":"api_key","token":"ignored"})), "");
        assert_eq!(
            credential(&json!({"key":"","token":"t","access_token":"a"})),
            "t"
        );
        assert_eq!(credential(&json!({"access_token":"a"})), "a");
        assert_eq!(credential(&json!("as-is")), "as-is");
        assert!(visual_provider(
            "p",
            &json!({"models":[]}),
            &json!({"providerTypes":{"p":"visual"}})
        ));
        assert!(!visual_provider(
            "p",
            &json!({"models":[{"capabilities":["image"]}]}),
            &json!({})
        ));
        assert!(visual_provider(
            "p",
            &json!({"models":[{"kind":"image"}]}),
            &json!({"providerTypes":{"p":false}})
        ));
    }

    #[test]
    fn runtime_provider_union_and_definition_duplicates_preserve_insertion_ties() {
        let mut snapshot = snapshot(
            json!({"providers":{"p":{"api":"openai-responses","models":[
                {"id":"z","kind":"image","name":"old"},
                {"id":"a","kind":"image","name":"same"},
                {"id":"z","kind":"image","name":"same"}
            ]}}}),
            json!({"p":"p-key","runtime-only":"r-key","empty-provider":"e-key"}),
            json!({"providerTypes":{"p":"chat","runtime-only":"chat"}}),
        );
        snapshot
            .runtime_provider_names
            .insert("empty-provider".into(), json!("No Models"));
        snapshot
            .runtime_provider_names
            .insert("runtime-only".into(), json!("Runtime Name"));
        snapshot.runtime_models = vec![
            json!({"provider":"p","id":"b","name":"same","pisperKind":"image"}),
            json!({"provider":"runtime-only","id":"first","name":"same","api":"openai-responses","pisperKind":"image"}),
            json!({"provider":"runtime-only","id":"second","name":"same","api":"openai-responses","pisperKind":"image"}),
        ];
        let candidates = candidates(&snapshot, VisualKind::Image).unwrap();
        assert_eq!(
            candidates.iter().map(reference).collect::<Vec<_>>(),
            [
                "p/z",
                "p/a",
                "p/b",
                "runtime-only/first",
                "runtime-only/second"
            ]
        );
        assert_eq!(candidates[3].public["providerName"], "Runtime Name");
        assert_eq!(
            all(&snapshot, VisualKind::Image)
                .unwrap()
                .iter()
                .map(reference)
                .collect::<Vec<_>>(),
            [
                "p/z",
                "p/a",
                "p/b",
                "runtime-only/first",
                "runtime-only/second"
            ]
        );
        assert_eq!(
            configured_provider_ids(&json!({"10":{},"2":{},"01":{},"z":{},"a":{}})),
            ["2", "10", "01", "z", "a"]
        );
    }

    #[test]
    fn locale_sort_and_equal_name_ties_match_current_node_oracle() {
        let names = ["é", "e", "E", "a", "A", "a-", "a_", "a.", "图", "𐐀"];
        let models = names
            .iter()
            .enumerate()
            .map(|(index, name)| json!({"id":format!("m{index}"),"name":name,"kind":"image"}))
            .collect::<Vec<_>>();
        let snapshot = snapshot(
            json!({"providers":{"p":{"api":"openai-responses","models":models}}}),
            json!({"p":"key"}),
            json!({}),
        );
        let sorted = all(&snapshot, VisualKind::Image).unwrap();
        assert_eq!(
            sorted
                .iter()
                .map(|model| model.public["name"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["图", "a", "A", "a_", "a-", "a.", "é", "e", "E", "𐐀"]
        );
    }

    #[test]
    fn driver_defaults_and_scores_cover_both_kinds_without_kind_guessing() {
        for (provider, api, expected) in [
            (
                "google",
                "",
                "https://generativelanguage.googleapis.com/v1beta",
            ),
            ("custom", "x-ai", "https://api.x.ai/v1"),
            ("openrouter", "", "https://openrouter.ai/api/v1"),
            ("custom", "openai-responses", "https://api.openai.com/v1"),
            ("custom", "custom", ""),
        ] {
            assert_eq!(default_base_url(provider, api), expected);
        }
        for (provider, id, expected) in [
            ("openai", "gpt-image-2", 128),
            ("google", "veo-3.1", 127),
            ("xai", "grok-imagine-video", 101),
            ("OpenAI", "gpt-abc-image", 0),
            ("openai", "gpt-1thing\nimage", 8),
            ("openai", "gpt-1thing\rimage", 8),
            ("openai", "gpt-١-image", 8),
            ("openai", "gpt-1thing\u{2028}image", 8),
            ("openai", "sora-2-pro-veo-3", 128),
        ] {
            assert_eq!(score(provider, id).unwrap(), expected);
        }
        for (provider, kind, expected) in [
            ("google", "image", "google-image"),
            ("google", "video", "google-video"),
            ("xai", "image", "xai-image"),
            ("xai", "video", "xai-video"),
            ("openrouter", "image", "openrouter-image"),
            ("openrouter", "video", "openai-video"),
            ("other", "image", "openai-image"),
            ("other", "video", "openai-video"),
        ] {
            assert_eq!(driver(&json!({"providerId":provider,"api":"","baseUrl":"https://example.test","id":"i","kind":kind,"visualApi":""})).unwrap(), expected);
        }
    }

    #[test]
    fn empty_catalog_and_invalid_selection_report_exact_errors() {
        let empty = snapshot(json!({}), json!({}), json!({}));
        assert_eq!(
            select(&empty, VisualKind::Image, Some("missing"))
                .err()
                .unwrap()
                .message,
            "没有已配置并启用的图像生成模型。请先在配置页添加视觉模型。"
        );
        assert_eq!(
            all(&empty, VisualKind::Video).err().unwrap().message,
            "没有已配置并启用的视频生成模型。请先在配置页添加视觉模型。"
        );
        assert_eq!(
            select(&duplicate_fixture(), VisualKind::Image, Some(" Missing "))
                .err()
                .unwrap()
                .message,
            "未找到已启用的视觉模型： Missing "
        );
        let mut malformed = empty.clone();
        malformed.app_json = json!({"visualDefaultModels":{"image":{"toString":"bad"}}});
        assert_eq!(
            status(&malformed, VisualKind::Image).err().unwrap().message,
            "Cannot convert object to primitive value"
        );
        assert_eq!(
            js_string_checked(&json!([null, 0, false, ["a", null], {}])).unwrap(),
            ",0,false,a,,[object Object]"
        );
        assert_eq!(js_string_checked(&json!(1e21)).unwrap(), "1e+21");
        assert_eq!(js_string_checked(&json!(0.0000001)).unwrap(), "1e-7");
    }
}
