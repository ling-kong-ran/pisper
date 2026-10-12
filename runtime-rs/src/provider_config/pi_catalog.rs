//! 发布时校验并冻结的官方目录；运行时离线读取，不改写用户的连接或认证文件。
use super::*;
use std::sync::OnceLock;

fn catalog() -> &'static BTreeMap<String, Vec<Model>> {
    static CATALOG: OnceLock<BTreeMap<String, Vec<Model>>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let snapshot: Value =
            serde_json::from_str(include_str!("../../assets/pi-model-catalog.json"))
                .expect("validated Pi model catalog JSON");
        assert_eq!(snapshot["schemaVersion"], 1);
        snapshot["models"]
            .as_object()
            .expect("Pi providers")
            .iter()
            .map(|(provider, entries)| {
                let models = entries
                    .as_array()
                    .expect("Pi provider models")
                    .iter()
                    .filter(|entry| entry["type"] == "chat")
                    .map(|entry| {
                        serde_json::from_value(entry.clone())
                            .expect("Pi model compatible with the pinned Rust SDK")
                    })
                    .collect();
                (provider.clone(), models)
            })
            .collect()
    })
}

pub(super) fn canonical_model(id: &str) -> Option<Model> {
    let id = id.rsplit('/').next().unwrap_or(id);
    let mut selected: Option<&Model> = None;
    for provider in [
        "anthropic",
        "openai",
        "google",
        "deepseek",
        "xai",
        "zai",
        "moonshotai",
        "minimax",
    ] {
        for candidate in catalog()
            .get(provider)
            .into_iter()
            .flatten()
            .filter(|model| model.id == id)
        {
            if let Some(previous) = selected {
                if previous.context_window != candidate.context_window
                    || previous.max_tokens != candidate.max_tokens
                    || previous.api != candidate.api
                {
                    return None;
                }
            }
            selected = Some(candidate);
        }
    }
    selected.cloned()
}

pub(super) fn override_field(model: &mut Value, field: &str, value: &Value) {
    // 显式映射逐键合并，避免更新目录或重放 SDK 覆盖时丢掉其余配置。
    if field == "samplingParamsByThinkingLevel" {
        if let (Some(existing), Some(overrides)) = (model[field].as_object_mut(), value.as_object())
        {
            for (level, parameters) in overrides {
                if let (Some(current), Some(additions)) = (
                    existing.get_mut(level).and_then(Value::as_object_mut),
                    parameters.as_object(),
                ) {
                    current.extend(additions.clone());
                } else {
                    existing.insert(level.clone(), parameters.clone());
                }
            }
            return;
        }
    }
    if matches!(
        field,
        "compat" | "thinkingLevelMap" | "cost" | "samplingParams" | "headers"
    ) {
        if let (Some(existing), Some(overrides)) = (model[field].as_object_mut(), value.as_object())
        {
            existing.extend(overrides.clone());
            return;
        }
    }
    model[field] = value.clone();
}

// 与 SDK 相同，单模型覆盖在定义之后生效；不能把已显式设置的能力误认成旧默认值。
pub(super) fn configured_fields(connection: &Value, id: &str) -> Option<Value> {
    let definition = connection
        .get("models")
        .and_then(Value::as_array)
        .and_then(|models| models.iter().find(|model| string(model, "id") == id))
        .cloned();
    let overrides = connection
        .get("modelOverrides")
        .and_then(|items| items.get(id));
    if definition.is_none() && overrides.is_none() {
        return None;
    }
    let mut result = definition.unwrap_or_else(|| json!({"id":id}));
    if let Some(overrides) = overrides.and_then(Value::as_object) {
        for (field, value) in overrides {
            override_field(&mut result, field, value);
        }
    }
    Some(result)
}

// 仅补全原始配置未声明的能力；完整请求 ID、代理协议和端点仍由当前连接拥有。
pub(super) fn configured_model(
    provider: &str,
    original: &Model,
    connection: &Value,
    configured: &Value,
) -> Result<Model, ApiError> {
    let Some(official) = canonical_model(&original.id) else {
        return Ok(original.clone());
    };
    let mut value = serde_json::to_value(&official).expect("serializable official model");
    value["id"] = json!(original.id);
    value["provider"] = json!(provider);
    value["name"] = json!(original.name);
    value["api"] = json!(if string(connection, "api").is_empty() {
        original.api.as_str()
    } else {
        string(connection, "api")
    });
    if let Some(api) = configured
        .get("api")
        .and_then(Value::as_str)
        .filter(|api| !api.is_empty())
    {
        value["api"] = json!(api);
    }
    value["baseUrl"] = json!(if string(connection, "baseUrl").is_empty() {
        original.base_url.clone()
    } else {
        upgrade_compat::canonical_base(string(connection, "baseUrl"), string(&value, "api"))
    });
    value["headers"] = connection
        .get("headers")
        .cloned()
        .unwrap_or_else(|| json!(original.headers));
    let effective_api = string(&value, "api");
    if effective_api != official.api {
        value["compat"] = json!(original.compat);
    }
    if let Some(compat) = connection.get("compat") {
        override_field(&mut value, "compat", compat);
    }
    for field in [
        "name",
        "api",
        "baseUrl",
        "reasoning",
        "input",
        "cost",
        "contextWindow",
        "maxTokens",
        "thinkingLevelMap",
        "compat",
        "headers",
        "samplingParams",
        "samplingParamsByThinkingLevel",
    ] {
        if let Some(overridden) = configured.get(field) {
            override_field(&mut value, field, overridden);
        }
    }
    if configured.get("maxTokens").is_none()
        && value["maxTokens"].as_u64() > value["contextWindow"].as_u64()
    {
        value["maxTokens"] = value["contextWindow"].clone();
    }
    serde_json::from_value(value).map_err(|_| ApiError::bad_request("模型目录与自定义配置不兼容。"))
}

pub(super) fn overlay(
    provider: &str,
    original: &[Model],
    connection: &Value,
) -> Result<Vec<Model>, ApiError> {
    let mut models = BTreeMap::new();
    for original in original {
        let configured = configured_fields(connection, &original.id);
        let model = match configured.as_ref().filter(|model| kind(model) == "chat") {
            Some(configured) => configured_model(provider, original, connection, configured)?,
            None => original.clone(),
        };
        models.insert(model.id.clone(), model);
    }
    for fresh in catalog().get(provider).into_iter().flatten() {
        // 新协议需要 SDK 明确实现；不能因为远程目录出现一个名字就宣称支持。
        if !PROTOCOLS.contains(&fresh.api.as_str())
            && !models
                .get(&fresh.id)
                .is_some_and(|old| old.api == fresh.api)
        {
            continue;
        }
        let mut value = serde_json::to_value(fresh).expect("serializable Pi model");
        if !string(connection, "api").is_empty() {
            value["api"] = connection["api"].clone();
        }
        let configured = configured_fields(connection, &fresh.id);
        if let Some(api) = configured
            .as_ref()
            .and_then(|model| model.get("api"))
            .and_then(Value::as_str)
            .filter(|api| !api.is_empty())
        {
            value["api"] = json!(api);
        }
        if string(&value, "api") != fresh.api {
            value["compat"] = models
                .get(&fresh.id)
                .and_then(|model| model.compat.clone())
                .unwrap_or(Value::Null);
        }
        if !string(connection, "baseUrl").is_empty() {
            value["baseUrl"] = json!(upgrade_compat::canonical_base(
                string(connection, "baseUrl"),
                string(&value, "api")
            ));
        }
        if let Some(headers) = connection.get("headers") {
            value["headers"] = headers.clone();
        }
        if let Some(compat) = connection.get("compat") {
            override_field(&mut value, "compat", compat);
        }
        if let Some(custom) = configured.as_ref() {
            for field in [
                "name",
                "api",
                "baseUrl",
                "reasoning",
                "input",
                "cost",
                "contextWindow",
                "maxTokens",
                "thinkingLevelMap",
                "compat",
                "headers",
                "samplingParams",
                "samplingParamsByThinkingLevel",
            ] {
                if let Some(overridden) = custom.get(field) {
                    override_field(&mut value, field, overridden);
                }
            }
        }
        if configured
            .as_ref()
            .and_then(|model| model.get("maxTokens"))
            .is_none()
            && value["maxTokens"].as_u64() > value["contextWindow"].as_u64()
        {
            value["maxTokens"] = value["contextWindow"].clone();
        }
        let model: Model = serde_json::from_value(value)
            .map_err(|_| ApiError::bad_request("官方模型目录与自定义模型配置不兼容。"))?;
        models.insert(model.id.clone(), model);
    }
    Ok(models.into_values().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn official_catalog_decodes_with_current_sdk_and_supplies_opus_55_api_contract() {
        assert!(catalog().len() > 1);
        let opus = canonical_model("claude-opus-5-5").expect("official Opus 5.5");
        assert_eq!(opus.context_window, 1_000_000);
        assert_eq!(opus.api, "anthropic-messages");
        assert_eq!(opus.compat.as_ref().unwrap()["forceAdaptiveThinking"], true);
        assert_eq!(opus.compat.as_ref().unwrap()["supportsTemperature"], false);
        assert_eq!(opus.thinking_level_map.as_ref().unwrap()["minimal"], None);
    }

    #[test]
    fn routing_prefixes_match_only_an_exact_official_model_suffix() {
        for id in ["xxx/claude-opus-5-5", "gateway/team/claude-opus-5-5"] {
            assert_eq!(canonical_model(id).unwrap().context_window, 1_000_000);
        }
        for id in ["xxx/claude-opus-5-50", "xxx/claude-opus-5-5-custom", "xxx/"] {
            assert!(canonical_model(id).is_none());
        }
    }

    #[test]
    fn configured_prefix_models_fill_official_defaults_without_changing_request_identity() {
        for id in ["xxx/claude-opus-5-5", "gateway/team/claude-opus-5-5"] {
            let mut original = canonical_model(id).unwrap();
            original.id = id.into();
            original.context_window = 200000;
            original.max_tokens = 8192;
            original.reasoning = false;
            original.compat = None;
            original.thinking_level_map = None;
            let connection = json!({"api":"anthropic-messages", "baseUrl":"https://fixture.invalid/proxy/v1", "models":[{"id":id}]});
            let output = overlay("fixture", &[original], &connection).unwrap();
            let model = &output[0];
            assert_eq!(model.id, id);
            assert_eq!(model.provider, "fixture");
            assert_eq!(model.context_window, 1_000_000);
            assert_eq!(model.max_tokens, 128000);
            assert!(model.reasoning);
            assert_eq!(model.base_url, "https://fixture.invalid/proxy");
            assert_eq!(
                model.compat.as_ref().unwrap()["forceAdaptiveThinking"],
                true
            );
        }
    }

    #[test]
    fn explicit_prefixed_limits_and_api_override_official_defaults() {
        let id = "xxx/claude-opus-5-5";
        let mut original = canonical_model(id).unwrap();
        original.id = id.into();
        original.compat = None;
        let connection = json!({"api":"anthropic-messages", "baseUrl":"https://fixture.invalid/v1", "models":[{"id":id,"api":"openai-completions","contextWindow":200000,"maxTokens":8192,"reasoning":false}]});
        let output = overlay("fixture", &[original], &connection).unwrap();
        let model = &output[0];
        assert_eq!(model.id, id);
        assert_eq!(model.context_window, 200000);
        assert_eq!(model.max_tokens, 8192);
        assert!(!model.reasoning);
        assert_eq!(model.api, "openai-completions");
        assert_eq!(model.base_url, "https://fixture.invalid/v1");
        assert!(model.compat.is_none());
    }

    #[test]
    fn provider_and_model_overrides_survive_official_refresh_for_bare_and_routed_models() {
        for (provider, id) in [
            ("anthropic", "claude-opus-5-5"),
            ("fixture", "xxx/claude-opus-5-5"),
        ] {
            let mut original = canonical_model(id).unwrap();
            original.id = id.into();
            let connection = json!({
                "api":"anthropic-messages", "baseUrl":"https://fixture.invalid/v1",
                "compat":{"supportsStrictTools":false}, "headers":{"X-Provider":"provider"},
                "models":[{"id":id,"contextWindow":200000,"cost":{"input":1,"output":2,"cacheRead":3,"cacheWrite":4},"samplingParams":{"temperature":0.3},"samplingParamsByThinkingLevel":{"high":{"temperature":0.4,"topP":0.8}},"headers":{"X-Model":"model"}}],
                "modelOverrides":{id:{"contextWindow":64000,"maxTokens":4096,"reasoning":false,"cost":{"input":8},"samplingParams":{"topP":0.9},"samplingParamsByThinkingLevel":{"high":{"topP":0.7}},"compat":{"supportsCacheControl":false},"thinkingLevelMap":{"max":null}}}
            });
            let models = overlay(provider, &[original], &connection).unwrap();
            let model = models.iter().find(|model| model.id == id).unwrap();
            assert_eq!(model.context_window, 64000);
            assert_eq!(model.max_tokens, 4096);
            assert!(!model.reasoning);
            let value = serde_json::to_value(model).unwrap();
            assert_eq!(value["cost"]["input"], 8);
            assert_eq!(value["cost"]["output"], 2);
            assert_eq!(value["samplingParams"]["temperature"], 0.3);
            assert_eq!(value["samplingParams"]["topP"], 0.9);
            assert_eq!(
                value["samplingParamsByThinkingLevel"]["high"]["temperature"],
                0.4
            );
            assert_eq!(value["samplingParamsByThinkingLevel"]["high"]["topP"], 0.7);
            assert_eq!(value["headers"]["X-Provider"], "provider");
            assert_eq!(value["headers"]["X-Model"], "model");
            assert_eq!(value["compat"]["supportsStrictTools"], false);
            assert_eq!(value["compat"]["supportsCacheControl"], false);
            assert_eq!(value["thinkingLevelMap"]["max"], Value::Null);
        }
    }

    #[test]
    fn manual_limits_and_proxy_connection_win_without_changing_credentials() {
        let connection = json!({"api":"anthropic-messages", "baseUrl":"https://fixture.invalid/v1", "models":[{"id":"claude-opus-5-5","contextWindow":64000,"compat":{"supportsStrictTools":false},"thinkingLevelMap":{"max":null}}]});
        let result = overlay("anthropic", &[], &connection).unwrap();
        let opus = result
            .iter()
            .find(|model| model.id == "claude-opus-5-5")
            .unwrap();
        assert_eq!(opus.context_window, 64000);
        assert_eq!(opus.base_url, "https://fixture.invalid");
        assert_eq!(opus.compat.as_ref().unwrap()["forceAdaptiveThinking"], true);
        assert_eq!(opus.compat.as_ref().unwrap()["supportsStrictTools"], false);
        assert_eq!(opus.thinking_level_map.as_ref().unwrap()["off"], None);
        assert_eq!(opus.thinking_level_map.as_ref().unwrap()["max"], None);
    }
}
