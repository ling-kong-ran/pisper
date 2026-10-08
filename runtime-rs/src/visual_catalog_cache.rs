//! release 的已发现 Provider 目录在视觉组合根中的只读投影。
//!
//! Pi 的 Model 是对话类型，不能携带 Pisper 的 pisperKind。这里只装饰视觉目录
//! 消费的字段，不把视觉模型重新注册为对话模型，也不写回发现缓存或配置。
use serde_json::{json, Map, Value};
use std::path::Path;

pub(crate) fn augment_runtime_models(
    data_dir: &Path,
    configured_providers: &Value,
    registered_provider_ids: &[String],
    runtime_models: Vec<Value>,
    provider_user_agent: &str,
) -> Result<Vec<Value>, String> {
    let path = data_dir.join("pisper-provider-models.json");
    let cache = match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|_| "pisper-provider-models.json contains invalid JSON".to_owned())?,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            json!({"providers":{}})
        }
        Err(error) => return Err(format!("cannot read provider model catalog: {error}")),
    };
    decorate_runtime_models(
        &cache,
        configured_providers,
        registered_provider_ids,
        runtime_models,
        provider_user_agent,
    )
}

/// 与 release decorateRuntime 的视觉字段投影一致；保留原生对象的未知字段。
pub(crate) fn decorate_runtime_models(
    cache: &Value,
    configured_providers: &Value,
    registered_provider_ids: &[String],
    runtime_models: Vec<Value>,
    provider_user_agent: &str,
) -> Result<Vec<Value>, String> {
    // release init 的 this.state.providers ||= {} 对 null 和原始值同样失败。
    if cache.is_null() {
        return Err("Cannot read properties of null (reading 'providers')".into());
    }
    if !cache.is_object() && !cache.is_array() {
        return Err("Cannot create property 'providers' on provider model catalog".into());
    }
    let mut result = Vec::new();
    let mut provider_ids = configured_providers
        .as_object()
        .map(|providers| providers.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    // 与视觉 catalog 的 Object.keys 顺序相同；未配置、未注册的缓存 Provider
    // 不能仅凭缓存文件进入目录。
    provider_ids.sort_by_key(|id| {
        id.parse::<u32>()
            .ok()
            .filter(|number| *number != u32::MAX && number.to_string() == *id)
            .map(|number| (0, number))
            .unwrap_or((1, 0))
    });
    for provider_id in registered_provider_ids {
        if !provider_ids.contains(provider_id) {
            provider_ids.push(provider_id.clone());
        }
    }
    for provider_id in &provider_ids {
        let overlay = &configured_providers[provider_id];
        let configured_api = overlay["api"]
            .as_str()
            .filter(|api| PROTOCOLS.contains(api));
        let mut raw = runtime_models
            .iter()
            .filter(|model| model["provider"] == *provider_id)
            .cloned()
            .collect::<Vec<_>>();
        for model in &mut raw {
            if let Some(api) = configured_api {
                model["api"] = json!(api);
            }
        }
        let base = first(&[
            &overlay["baseUrl"],
            &json!(default_base_url(provider_id)),
            raw.first()
                .map(|model| &model["baseUrl"])
                .unwrap_or(&Value::Null),
        ]);
        let base = normalize_base(&base)?;
        let cached = &cache["providers"][provider_id];
        let own = !base.is_empty() && normalize_base(&cached["baseUrl"])? == base;
        let options = model_options(overlay)?;
        let mut models = Vec::new();
        let mut seen = Vec::<Value>::new();
        if own {
            let candidates = if truthy(&cached["models"]) {
                cached["models"]
                    .as_array()
                    .ok_or_else(|| "(own.entry.models || []) is not iterable".to_owned())?
                    .as_slice()
            } else {
                &[]
            };
            for candidate in candidates {
                append_cached(
                    &mut models,
                    &mut seen,
                    provider_id,
                    cached,
                    candidate,
                    &raw,
                    configured_api,
                )?;
            }
            // 目录是权威来源；只显式手动配置的 raw 模型可以跨刷新保留。
            for model in &raw {
                if options
                    .iter()
                    .find(|(id, _)| id == &model["id"])
                    .is_some_and(|(_, option)| truthy(&option["userConfigured"]))
                {
                    let mut candidate = model.clone();
                    candidate["kind"] = options
                        .iter()
                        .find(|(id, _)| id == &model["id"])
                        .map(|(_, option)| option["kind"].clone())
                        .unwrap_or(Value::Null);
                    append_cached(
                        &mut models,
                        &mut seen,
                        provider_id,
                        cached,
                        &candidate,
                        &raw,
                        configured_api,
                    )?;
                }
            }
        } else {
            for mut model in raw {
                model["pisperAuthProvider"] = json!(provider_id);
                models.push(model);
            }
        }
        let excluded = overlay["excludedModels"].as_array();
        let mut provider_headers = Map::new();
        if truthy(&overlay["baseUrl"])
            && normalize_base(&overlay["baseUrl"])?
                != normalize_base(&json!(default_base_url(provider_id)))?
        {
            spread(&overlay["headers"], &mut provider_headers);
            if !provider_headers
                .keys()
                .any(|name| name.eq_ignore_ascii_case("user-agent"))
            {
                provider_headers.insert("User-Agent".into(), json!(provider_user_agent));
            }
        }
        for mut model in models {
            if excluded
                .is_some_and(|values| values.iter().any(|id| id.is_string() && id == &model["id"]))
            {
                continue;
            }
            if let Some((_, option)) = options.iter().find(|(id, _)| id == &model["id"]) {
                if truthy(&option["name"]) {
                    model["name"] = option["name"].clone();
                }
                let capabilities = capabilities(option, &model["pisperKind"]);
                model["pisperKind"] = capabilities[0].clone();
                model["capabilities"] = Value::Array(capabilities);
            }
            if !provider_headers.is_empty() {
                let mut headers = provider_headers.clone();
                spread(&model["headers"], &mut headers);
                model["headers"] = Value::Object(headers);
            }
            result.push(model);
        }
    }
    Ok(result)
}

const PROTOCOLS: &[&str] = &[
    "openai-responses",
    "openai-completions",
    "anthropic-messages",
    "google-generative-ai",
];
fn default_base_url(provider: &str) -> &str {
    match provider {
        "openai" => "https://api.openai.com/v1",
        "openai-codex" => "https://chatgpt.com/backend-api",
        "anthropic" => "https://api.anthropic.com",
        "google" => "https://generativelanguage.googleapis.com/v1beta",
        "deepseek" => "https://api.deepseek.com",
        "xai" => "https://api.x.ai/v1",
        "openrouter" => "https://openrouter.ai/api/v1",
        "kimi-coding" => "https://api.kimi.com/coding/",
        "zai-coding-cn" => "https://open.bigmodel.cn/api/paas/v4",
        _ => "",
    }
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
fn first(values: &[&Value]) -> Value {
    values
        .iter()
        .find(|value| truthy(value))
        .map(|value| (*value).clone())
        .unwrap_or(Value::Null)
}
fn normalize_base(value: &Value) -> Result<String, String> {
    let value = if truthy(value) {
        js_string(value)?
    } else {
        String::new()
    };
    Ok(value
        .trim_matches(js_whitespace)
        .trim_end_matches('/')
        .to_lowercase())
}
fn js_whitespace(value: char) -> bool {
    matches!(value, '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')
}
fn js_string(value: &Value) -> Result<String, String> {
    match value {
        Value::String(value) => Ok(value.clone()),
        Value::Null => Ok("null".into()),
        Value::Bool(value) => Ok(value.to_string()),
        Value::Number(value) => Ok(value.to_string()),
        Value::Array(values) => values
            .iter()
            .map(|value| {
                if value.is_null() {
                    Ok(String::new())
                } else {
                    js_string(value)
                }
            })
            .collect::<Result<Vec<_>, _>>()
            .map(|values| values.join(",")),
        Value::Object(values) if values.contains_key("toString") => {
            Err("Cannot convert object to primitive value".into())
        }
        _ => Ok("[object Object]".into()),
    }
}
fn spread(value: &Value, result: &mut Map<String, Value>) {
    match value {
        Value::Object(values) => result.extend(values.clone()),
        Value::Array(values) => result.extend(
            values
                .iter()
                .enumerate()
                .map(|(index, value)| (index.to_string(), value.clone())),
        ),
        Value::String(value) => result.extend(
            value
                .encode_utf16()
                .enumerate()
                .map(|(index, unit)| (index.to_string(), json!(String::from_utf16_lossy(&[unit])))),
        ),
        _ => {}
    }
}
fn model_options(overlay: &Value) -> Result<Vec<(Value, Value)>, String> {
    let Some(models) = overlay.get("models") else {
        return Ok(Vec::new());
    };
    if !truthy(models) {
        return Ok(Vec::new());
    }
    let models = models
        .as_array()
        .ok_or_else(|| "(overlay.models || []) is not iterable".to_owned())?;
    let mut result = Vec::<(Value, Value)>::new();
    for model in models {
        if let Some((_, previous)) = result.iter_mut().find(|(id, _)| id == &model["id"]) {
            *previous = model.clone();
        } else {
            result.push((model["id"].clone(), model.clone()));
        }
    }
    Ok(result)
}
fn capabilities(option: &Value, native_kind: &Value) -> Vec<Value> {
    let explicit = ["chat", "image", "video"]
        .into_iter()
        .filter(|kind| {
            option["capabilities"]
                .as_array()
                .is_some_and(|values| values.iter().any(|value| value == kind))
        })
        .map(|kind| json!(kind))
        .collect::<Vec<_>>();
    if !explicit.is_empty() {
        return explicit;
    }
    let kind = option.get("kind").unwrap_or(native_kind);
    vec![
        if ["chat", "image", "video"].iter().any(|value| kind == value) {
            kind.clone()
        } else {
            json!("chat")
        },
    ]
}
fn append_cached(
    models: &mut Vec<Value>,
    seen: &mut Vec<Value>,
    provider: &str,
    entry: &Value,
    candidate: &Value,
    raw: &[Value],
    configured_api: Option<&str>,
) -> Result<(), String> {
    let id = candidate["id"].clone();
    if seen.contains(&id) {
        return Ok(());
    }
    seen.push(id.clone());
    let mut model = if let Some(existing) = raw.iter().find(|model| model["id"] == id) {
        let mut model = existing.clone();
        if truthy(&candidate["name"]) {
            model["name"] = candidate["name"].clone();
        }
        model
    } else {
        let template = raw.first().unwrap_or(&Value::Null);
        let mut model = json!({"id":id,"name":first(&[&candidate["name"],&candidate["id"]]),"provider":provider,
            "api":first(&[&configured_api.map(|api|json!(api)).unwrap_or(Value::Null),&entry["api"],&template["api"],&json!("openai-responses")]),
            "baseUrl":first(&[&entry["baseUrl"],&template["baseUrl"],&json!("")])});
        if truthy(&template["headers"]) {
            model["headers"] = template["headers"].clone();
        }
        model
    };
    model["pisperKind"] = first(&[&candidate["kind"], &json!("chat")]);
    model["pisperAuthProvider"] = json!(provider);
    model["pisperAuthKeyId"] = json!("");
    models.push(model);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_visual_kinds_are_restored_without_importing_orphan_providers() {
        let cache = json!({"future":{"keep":true},"providers":{
            "relay":{"baseUrl":"https://relay.test/v1/","api":"openai-responses","models":[
                {"id":"picture","name":"Discovered Image","kind":"image"},
                {"id":"movie","kind":"video"}]},
            "orphan":{"baseUrl":"https://orphan.test","models":[{"id":"orphan","kind":"image"}]}
        }});
        let original = cache.clone();
        let models = decorate_runtime_models(
            &cache,
            &json!({"relay":{"baseUrl":"HTTPS://RELAY.TEST/v1","headers":{"X-Test":null}}}),
            &[],
            vec![],
            "Pisper/test",
        )
        .unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0]["pisperKind"], "image");
        assert_eq!(models[1]["pisperKind"], "video");
        assert_eq!(models[0]["headers"]["User-Agent"], "Pisper/test");
        assert!(models[0]["headers"]["X-Test"].is_null());
        assert_eq!(cache, original);
    }

    #[test]
    fn cache_authority_keeps_only_manual_raw_models_and_preserves_private_headers() {
        let cache = json!({"providers":{"relay":{"baseUrl":"https://relay.test","models":[
            {"id":"current","name":"Discovered label","kind":"image"},
            {"id":"excluded","kind":"video"}]}}});
        let providers = json!({"relay":{"baseUrl":"https://relay.test","headers":{"X-Same":"provider"},
            "models":[{"id":"current","name":"Saved label","capabilities":["video","image"]},
                {"id":"manual","kind":"video","userConfigured":true}],"excludedModels":["excluded"]}});
        let raw = vec![
            json!({"provider":"relay","id":"current","future":{"keep":true},"headers":{"X-Same":"runtime","X-Array":["a","b"]}}),
            json!({"provider":"relay","id":"manual"}),
            json!({"provider":"relay","id":"stale"}),
        ];
        let models = decorate_runtime_models(&cache, &providers, &[], raw, "Pisper/test").unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0]["name"], "Saved label");
        assert_eq!(models[0]["pisperKind"], "image");
        assert_eq!(models[0]["capabilities"], json!(["image", "video"]));
        assert_eq!(models[0]["future"]["keep"], true);
        assert_eq!(models[0]["headers"]["X-Same"], "runtime");
        assert_eq!(models[0]["headers"]["X-Array"], json!(["a", "b"]));
        assert_eq!(models[1]["id"], "manual");
    }

    #[test]
    fn known_default_matches_cache_without_custom_provider_headers() {
        let cache = json!({"providers":{"openai":{"baseUrl":"https://api.openai.com/v1/","models":[
            {"id":"known-image","kind":"image"}]}}});
        let models = decorate_runtime_models(
            &cache,
            &json!({}),
            &["openai".into()],
            vec![],
            "Pisper/test",
        )
        .unwrap();
        assert_eq!(models[0]["id"], "known-image");
        assert!(models[0].get("headers").is_none());
        assert!(decorate_runtime_models(&Value::Null, &json!({}), &[], vec![], "Pisper").is_err());
    }

    #[test]
    fn changed_endpoint_and_absent_cache_retain_raw_model_options_and_unknown_fields() {
        for cache in [
            json!({}),
            json!({"providers":{"relay":{"baseUrl":"https://old.test","models":[{"id":"old","kind":"image"}]}}}),
        ] {
            let providers = json!({"relay":{"baseUrl":"https://current.test","api":"google-generative-ai",
                "models":[{"id":"raw","name":"Saved name","kind":"video"}]}});
            let raw = json!({"provider":"relay","id":"raw","api":"openai-responses","future":["keep"],"headers":{"Authorization":null}});
            let models =
                decorate_runtime_models(&cache, &providers, &[], vec![raw], "Pisper/test").unwrap();
            assert_eq!(models.len(), 1);
            assert_eq!(models[0]["id"], "raw");
            assert_eq!(models[0]["name"], "Saved name");
            assert_eq!(models[0]["api"], "google-generative-ai");
            assert_eq!(models[0]["pisperKind"], "video");
            assert_eq!(models[0]["future"], json!(["keep"]));
            assert!(models[0]["headers"]["Authorization"].is_null());
        }
    }

    #[test]
    fn dynamic_models_inherit_template_headers_and_ignore_cache_extension_headers() {
        let cache = json!({"providers":{"relay":{"baseUrl":"https://relay.test","headers":{"X-Cache":"ignored"},
            "models":[{"id":"dynamic","kind":"image","future":"ignored","headers":{"X-Candidate":"ignored"}}]}}});
        let providers = json!({"relay":{"baseUrl":"https://relay.test","headers":{"user-agent":"Configured-UA","X-Same":"provider"}}});
        let raw = json!({"provider":"relay","id":"template","api":"openai-responses","baseUrl":"https://relay.test",
            "headers":{"X-Same":"runtime","X-Array":["a","b"],"X-Null":null}});
        let models =
            decorate_runtime_models(&cache, &providers, &[], vec![raw], "Pisper/test").unwrap();
        assert_eq!(models[0]["headers"]["user-agent"], "Configured-UA");
        assert!(models[0]["headers"].get("User-Agent").is_none());
        assert_eq!(models[0]["headers"]["X-Same"], "runtime");
        assert_eq!(models[0]["headers"]["X-Array"], json!(["a", "b"]));
        assert!(models[0]["headers"]["X-Null"].is_null());
        assert!(models[0]["headers"].get("X-Cache").is_none());
        assert!(models[0]["headers"].get("X-Candidate").is_none());
        assert!(models[0].get("future").is_none());
    }

    #[test]
    fn cache_read_is_optional_read_only_and_rejects_corruption() {
        let directory =
            std::env::temp_dir().join(format!("pisper-visual-cache-read-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let cache_path = directory.join("pisper-provider-models.json");
        assert!(
            augment_runtime_models(&directory, &json!({}), &[], vec![], "Pisper")
                .unwrap()
                .is_empty()
        );
        let bytes = br#"{"future":{"keep":true},"providers":{}}"#;
        std::fs::write(&cache_path, bytes).unwrap();
        augment_runtime_models(&directory, &json!({}), &[], vec![], "Pisper").unwrap();
        assert_eq!(std::fs::read(&cache_path).unwrap(), bytes);
        std::fs::write(&cache_path, b"invalid JSON").unwrap();
        assert!(augment_runtime_models(&directory, &json!({}), &[], vec![], "Pisper").is_err());
        assert_eq!(std::fs::read(&cache_path).unwrap(), b"invalid JSON");
        std::fs::remove_file(cache_path).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }
}
