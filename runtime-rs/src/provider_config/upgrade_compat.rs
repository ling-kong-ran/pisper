//! Node 版配置的兼容投影；保留原文件，禁止把凭据放入公开模型或响应。
use super::*;
use sha2::{Digest, Sha256};

pub(super) fn canonical_base(value: &str, api: &str) -> String {
    let mut value = value.trim().trim_end_matches('/').to_owned();
    if api == "anthropic-messages" {
        if let Ok(mut url) = reqwest::Url::parse(&value) {
            let path = url.path().trim_end_matches('/');
            let path = path.strip_suffix("/v1").unwrap_or(path).to_owned();
            url.set_path(&path);
            value = url.as_str().trim_end_matches('/').to_owned();
        }
    }
    value
}

pub(super) fn catalog<'a>(docs: &'a Documents, provider: &str) -> Option<&'a Value> {
    let entry = docs.catalog.get("providers")?.get(provider)?;
    // 保存连接会规范化 Anthropic 的 /v1；等价地址不能使旧目录失效。
    let api = string(overlay(docs, provider), "api");
    let normalize = |url: &str| canonical_base(url, api).to_lowercase();
    let configured = normalize(string(overlay(docs, provider), "baseUrl"));
    (!configured.is_empty()
        && configured == normalize(string(entry, "baseUrl"))
        && entry.get("models").is_some_and(Value::is_array))
    .then_some(entry)
}

pub(super) fn project_models(
    docs: &Documents,
    provider: &str,
    original: &[Model],
) -> Result<Vec<Model>, ApiError> {
    let Some(entry) = catalog(docs, provider) else {
        return docs.metadata.project(overlay(docs, provider), original);
    };
    let connection = overlay(docs, provider);
    let mut result = Vec::new();
    for candidate in array(entry, "models") {
        let id = string(&candidate, "id");
        if id.is_empty() || result.iter().any(|model: &Model| model.id == id) {
            continue;
        }
        // 路由前缀不改变模型容量；只复用同一 Provider 下无歧义的目录参数，不改写请求型号。
        let base_id = id.rsplit('/').next().unwrap_or(id);
        let mut matches = original.iter().filter(|model| model.id == base_id);
        let base_model = matches.next().filter(|_| matches.next().is_none());
        let official = pi_catalog::canonical_model(base_id);
        let template = official
            .as_ref()
            .or_else(|| original.iter().find(|model| model.id == id))
            .or(base_model);
        let mut value = template
            .map(|model| serde_json::to_value(model).expect("serializable model"))
            .unwrap_or_else(|| {
                json!({
                    "id": id, "name": id, "provider": provider,
                    "api": if string(entry,"api").is_empty() { original.first().map(|model| model.api.as_str()).unwrap_or("openai-responses") } else { string(entry,"api") }, "baseUrl": canonical_base(string(connection, "baseUrl"), string(connection, "api")),
                    "reasoning": false, "input": ["text"],
                    "contextWindow": 200000, "maxTokens": original.first().map(|model| model.max_tokens).unwrap_or(128000),
                    "cost": original.first().map(|model| serde_json::to_value(&model.cost).expect("serializable model cost")).unwrap_or_else(|| json!({"input":0,"output":0,"cacheRead":0,"cacheWrite":0})),
                    "thinkingLevelMap": {"xhigh":null,"max":null}
                })
            });
        if value["id"] != id {
            value["name"] = json!(id);
        }
        value["id"] = json!(id);
        value["provider"] = json!(provider);
        let configured = pi_catalog::configured_fields(connection, id);
        let effective_api = configured
            .as_ref()
            .and_then(|model| model.get("api"))
            .and_then(Value::as_str)
            .filter(|api| !api.is_empty())
            .or_else(|| (!string(connection, "api").is_empty()).then(|| string(connection, "api")))
            .or_else(|| (!string(&candidate, "api").is_empty()).then(|| string(&candidate, "api")))
            .or_else(|| (!string(entry, "api").is_empty()).then(|| string(entry, "api")))
            .unwrap_or_else(|| string(&value, "api"))
            .to_owned();
        value["api"] = json!(effective_api);
        if official
            .as_ref()
            .is_some_and(|model| model.api != effective_api)
        {
            value["compat"] = original
                .iter()
                .find(|model| model.id == id)
                .and_then(|model| model.compat.clone())
                .unwrap_or(Value::Null);
        }
        if !string(connection, "baseUrl").is_empty() {
            value["baseUrl"] = json!(canonical_base(
                string(connection, "baseUrl"),
                &effective_api
            ));
        }
        if template.is_none() {
            if let Some((context, output)) = catalog_token_limits(base_id) {
                value["contextWindow"] = json!(context);
                value["maxTokens"] = json!(output);
            }
        }
        if !string(&candidate, "name").is_empty() {
            value["name"] = candidate["name"].clone();
        }
        if string(&value, "api").is_empty() {
            value["api"] = json!("openai-responses");
        }
        // 发现接口携带的真实容量优先于回退值；缺失字段不能伪装成统一的 200k。
        for field in ["contextWindow", "maxTokens"] {
            if candidate[field]
                .as_u64()
                .is_some_and(|limit| limit > 0 && limit <= 100_000_000)
            {
                value[field] = candidate[field].clone();
            }
        }
        for field in ["reasoning", "input", "thinkingLevelMap", "compat"] {
            if let Some(discovered) = candidate.get(field) {
                pi_catalog::override_field(&mut value, field, discovered);
            }
        }
        if let Some(compat) = connection.get("compat") {
            pi_catalog::override_field(&mut value, "compat", compat);
        }
        let model_headers = original
            .iter()
            .find(|model| model.id == id)
            .or(base_model)
            .and_then(|model| model.headers.as_ref());
        if let Some(headers) = model_headers {
            value["headers"] = json!(headers);
        }
        if let Some(headers) = connection.get("headers") {
            pi_catalog::override_field(&mut value, "headers", headers);
        }
        // 手动配置的模型字段优先；目录只决定发现范围，不覆盖用户能力设置。
        if let Some(configured) = configured.as_ref() {
            for field in [
                "name",
                "api",
                "reasoning",
                "input",
                "cost",
                "samplingParams",
                "samplingParamsByThinkingLevel",
                "contextWindow",
                "maxTokens",
                "thinkingLevelMap",
                "compat",
                "baseUrl",
                "headers",
            ] {
                if let Some(item) = configured.get(field) {
                    pi_catalog::override_field(&mut value, field, item);
                }
            }
        }
        if !candidate["maxTokens"]
            .as_u64()
            .is_some_and(|limit| limit > 0 && limit <= 100_000_000)
            && configured
                .as_ref()
                .and_then(|model| model.get("maxTokens"))
                .is_none()
            && value["maxTokens"].as_u64() > value["contextWindow"].as_u64()
        {
            value["maxTokens"] = value["contextWindow"].clone();
        }
        result.push(
            serde_json::from_value(value)
                .map_err(|_| ApiError::bad_request("旧版模型目录包含无效的模型定义。"))?,
        );
    }
    for model in original {
        if !result.iter().any(|item| item.id == model.id)
            && array(connection, "models")
                .iter()
                .any(|item| string(item, "id") == model.id && item["userConfigured"] == true)
        {
            result.push(model.clone());
        }
    }
    docs.metadata.project(connection, &result)
}

// 聚合网关可能通过任意协议转发模型；只借用无歧义的原厂容量，不复制协议、地址或凭据。
fn catalog_token_limits(id: &str) -> Option<(u64, u64)> {
    type Limits = BTreeMap<String, Option<(u64, u64)>>;
    static LIMITS: std::sync::OnceLock<Limits> = std::sync::OnceLock::new();
    LIMITS
        .get_or_init(|| {
            let mut result: Limits = BTreeMap::new();
            for provider in ["anthropic", "openai", "google", "deepseek", "xai"] {
                for model in pi_rust::ai::models::catalog::embedded_provider_catalog(provider) {
                    let limits = (model.context_window as u64, model.max_tokens as u64);
                    result
                        .entry(model.id)
                        .and_modify(|previous| {
                            if *previous != Some(limits) {
                                *previous = None;
                            }
                        })
                        .or_insert(Some(limits));
                }
            }
            result
        })
        .get(id)
        .copied()
        .flatten()
}

pub(super) fn adapt_thinking(model: &mut Model) {
    let id = model.id.rsplit('/').next().unwrap_or(&model.id);
    if model.api != "anthropic-messages"
        || !(id == "claude-opus-5-5" || id.starts_with("claude-opus-5-5-"))
    {
        return;
    }
    // Opus 5.5 不接受 enabled/budget 或 disabled；关闭时省略 thinking，由服务端选择默认行为。
    let compat = model.compat.get_or_insert_with(|| json!({}));
    if !compat.is_object() {
        *compat = json!({});
    }
    compat["forceAdaptiveThinking"] = json!(true);
    compat["supportsTemperature"] = json!(false);
    let map = model
        .thinking_level_map
        .get_or_insert_with(Default::default);
    map.insert("off".into(), None);
    map.insert("minimal".into(), None);
    for level in ["low", "medium", "high", "xhigh", "max"] {
        map.entry(level.into())
            .or_insert_with(|| Some(level.into()));
    }
}

fn key_id(key: &str) -> String {
    Sha256::digest(key.as_bytes())[..12]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(super) fn records(docs: &Documents, id: &str) -> Vec<Value> {
    let auth = &docs.auth[id];
    // 保留历史 Keyring 以便备份，但不得覆盖后来选择的 OAuth 登录。
    if string(auth, "type") == "oauth" {
        return Vec::new();
    }
    let primary = auth.as_str().or_else(|| {
        (string(auth, "type") == "api_key")
            .then(|| auth.get("key").and_then(Value::as_str))
            .flatten()
    });
    let mut keys: Vec<&str> = primary.into_iter().collect();
    if let Some(items) = docs.keyrings["providers"][id].as_array() {
        keys.extend(items.iter().filter_map(|item| item["key"].as_str()));
    }
    let mut output = Vec::new();
    for key in keys {
        let key = key.trim();
        if key.is_empty() || output.iter().any(|item: &Value| item["key"] == key) {
            continue;
        }
        output.push(json!({"id": key_id(key), "key":key}));
    }
    output
}

pub(super) fn summaries(docs: &Documents, id: &str) -> Value {
    json!(records(docs, id)
        .iter()
        .map(|item| {
            let chars: Vec<_> = string(item, "key").chars().collect();
            let hint = if chars.len() <= 8 {
                "********".into()
            } else {
                format!(
                    "{}...{}",
                    chars[..3].iter().collect::<String>(),
                    chars[chars.len() - 4..].iter().collect::<String>()
                )
            };
            json!({"id":item["id"],"hint":hint})
        })
        .collect::<Vec<_>>())
}

pub(super) fn replace_keys(
    docs: &mut Documents,
    id: &str,
    values: &[Value],
) -> Result<(), ApiError> {
    let mut keys = Vec::new();
    for value in values {
        let key = value
            .as_str()
            .ok_or_else(|| ApiError::bad_request("API Key 必须是文本。"))?
            .trim();
        let key = key.strip_prefix("Bearer ").unwrap_or(key).trim();
        if key.len() > 16384 {
            return Err(ApiError::bad_request("API Key 过长。"));
        }
        if !key.is_empty() && !keys.contains(&key) {
            keys.push(key);
        }
    }
    if keys.is_empty() && !values.is_empty() {
        return Err(ApiError::bad_request("API Key 不能为空。"));
    }
    if keys.len() > 32 {
        return Err(ApiError::bad_request(
            "单个 Provider 最多保存 32 个 API Key。",
        ));
    }
    if !docs.keyrings.get("providers").is_some_and(Value::is_object) {
        docs.keyrings["providers"] = json!({});
    }
    docs.keyrings["version"] = json!(1);
    if keys.is_empty() {
        docs.keyrings["providers"]
            .as_object_mut()
            .unwrap()
            .remove(id);
        docs.auth.as_object_mut().unwrap().remove(id);
    } else {
        let mut credential = docs
            .auth
            .get(id)
            .filter(|value| string(value, "type") == "api_key")
            .cloned()
            .unwrap_or_else(|| json!({}));
        credential["type"] = json!("api_key");
        credential["key"] = json!(keys[0]);
        docs.auth[id] = credential;
        docs.keyrings["providers"][id] = json!(keys
            .iter()
            .map(|key| json!({"id":key_id(key),"key":key}))
            .collect::<Vec<_>>());
    }
    Ok(())
}

/// 导入钥匙串不改变凭据文件所选的登录方式（尤其是历史 API Key 与现用 OAuth 共存时）。
pub(super) fn import_keyrings(docs: &mut Documents, input: &Value) -> Result<(), ApiError> {
    let Some(keyrings) = input
        .get("providerKeyrings")
        .filter(|value| !value.as_object().is_some_and(|v| v.is_empty()))
    else {
        return Ok(());
    };
    if keyrings["version"] != 1 || !keyrings["providers"].is_object() {
        return Err(ApiError::bad_request("Provider Keyring 格式无效。"));
    }
    let providers = input["providers"]
        .as_object()
        .ok_or_else(|| ApiError::bad_request("Provider 格式无效。"))?;
    for id in providers.keys() {
        let values = if let Some(records) = keyrings["providers"].get(id) {
            records
                .as_array()
                .ok_or_else(|| ApiError::bad_request("Provider Keyring 格式无效。"))?
                .iter()
                .map(|record| record["key"].clone())
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let credential = docs.auth.get(id).cloned();
        replace_keys(docs, id, &values)?;
        if let Some(credential) = credential {
            docs.auth[id] = credential;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn docs() -> Documents {
        Documents {
            models: json!({"providers":{"fixture":{"api":"anthropic-messages","baseUrl":"https://fixture.invalid/v1/", "models":[{"id":"manual", "userConfigured":true}]}}}),
            app: json!({}),
            settings: json!({}),
            auth: json!({}),
            keyrings: json!({}),
            metadata: Default::default(),
            catalog: json!({"providers":{"fixture":{"baseUrl":"https://fixture.invalid/v1","api":"anthropic-messages","models":[{"id":"claude-opus-5-5","name":"Opus","kind":"chat"}]}}}),
        }
    }
    fn model(id: &str) -> Model {
        serde_json::from_value(json!({"id":id,"name":id,"provider":"fixture","api":"anthropic-messages","baseUrl":"https://fixture.invalid/v1","reasoning":true,"input":["text"],"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0},"contextWindow":100000,"maxTokens":1000})).unwrap()
    }
    #[test]
    fn discovery_is_authoritative_but_retains_manual_models_and_ignores_old_endpoint() {
        let mut docs = docs();
        let original = vec![model("stale"), model("manual")];
        let models = project_models(&docs, "fixture", &original).unwrap();
        assert_eq!(
            models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["claude-opus-5-5", "manual"]
        );
        assert_eq!(models[0].name, "Opus");
        let official = pi_catalog::canonical_model("claude-opus-5-5").unwrap();
        assert_eq!(models[0].max_tokens, official.max_tokens);
        assert_eq!(models[0].cost, official.cost);
        assert_eq!(models[0].base_url, "https://fixture.invalid");
        docs.models["providers"]["fixture"]["baseUrl"] = json!("https://fixture.invalid");
        assert_eq!(project_models(&docs, "fixture", &original).unwrap(), models);
        docs.models["providers"]["fixture"]["baseUrl"] = json!("https://different.invalid");
        assert_eq!(
            project_models(&docs, "fixture", &original).unwrap(),
            original
        );
    }
    #[test]
    fn context_limits_preserve_discovery_manual_overrides_and_prefixed_identity() {
        let mut docs = docs();
        docs.catalog["providers"]["fixture"]["models"] = json!([
            {"id":"route/long-context", "contextWindow":1_000_000, "maxTokens":64000},
            {"id":"route/known"}
        ]);
        let mut known = model("known");
        known.context_window = 1_000_000;
        let original = vec![known];
        let projected = project_models(&docs, "fixture", &original).unwrap();
        assert_eq!(projected[0].context_window, 1_000_000);
        assert_eq!(projected[0].max_tokens, 64000);
        assert_eq!(projected[1].context_window, 1_000_000);
        assert_eq!(projected[1].id, "route/known");
        docs.models["providers"]["fixture"]["models"] = json!([
            {"id":"route/long-context", "contextWindow":256000, "maxTokens":32000}
        ]);
        let projected = project_models(&docs, "fixture", &original).unwrap();
        assert_eq!(projected[0].context_window, 256000);
        assert_eq!(projected[0].max_tokens, 32000);
    }

    #[test]
    fn prefixed_discovery_uses_official_defaults_ahead_of_stale_sdk_models() {
        let mut docs = docs();
        let id = "gateway/team/claude-opus-5-5";
        docs.catalog["providers"]["fixture"]["models"] = json!([{ "id": id }]);
        let mut stale = model(id);
        stale.context_window = 200000;
        stale.reasoning = false;
        let original = [stale, model("claude-opus-5-5")];
        let projected = project_models(&docs, "fixture", &original).unwrap();
        assert_eq!(projected[0].id, id);
        assert_eq!(projected[0].context_window, 1_000_000);
        assert_eq!(projected[0].max_tokens, 128000);
        assert!(projected[0].reasoning);
        docs.catalog["providers"]["fixture"]["models"][0]["contextWindow"] = json!(800000);
        assert_eq!(
            project_models(&docs, "fixture", &original).unwrap()[0].context_window,
            800000
        );
        docs.models["providers"]["fixture"]["models"] =
            json!([{"id":id,"contextWindow":200000,"maxTokens":8192,"reasoning":false}]);
        let manual = project_models(&docs, "fixture", &original).unwrap();
        assert_eq!(manual[0].context_window, 200000);
        assert_eq!(manual[0].max_tokens, 8192);
        assert!(!manual[0].reasoning);
    }

    #[test]
    fn prefixed_discovery_preserves_per_model_protocol_and_rejects_fuzzy_matches() {
        let mut docs = docs();
        let id = "xxx/claude-opus-5-5";
        docs.catalog["providers"]["fixture"]["models"] = json!([{ "id": id }]);
        docs.models["providers"]["fixture"]["models"] =
            json!([{"id":id,"api":"openai-completions"}]);
        let projected = project_models(&docs, "fixture", &[]).unwrap();
        assert_eq!(projected[0].context_window, 1_000_000);
        assert_eq!(projected[0].api, "openai-completions");
        assert_eq!(projected[0].base_url, "https://fixture.invalid/v1");
        assert!(projected[0].compat.is_none());
        docs.catalog["providers"]["fixture"]["models"] =
            json!([{ "id":"xxx/claude-opus-5-5-custom" }]);
        let unknown = project_models(&docs, "fixture", &[]).unwrap();
        assert_eq!(unknown[0].context_window, 200000);
        assert!(!unknown[0].reasoning);
        assert_eq!(
            serde_json::to_value(&unknown[0]).unwrap()["input"],
            json!(["text"])
        );
    }

    #[test]
    fn discovery_refresh_retains_explicit_sdk_overrides_and_matching_model_headers() {
        let mut docs = docs();
        let id = "xxx/claude-opus-5-5";
        docs.catalog["providers"]["fixture"]["models"] = json!([{ "id": id }]);
        docs.models["providers"]["fixture"] = json!({
            "api":"anthropic-messages", "baseUrl":"https://fixture.invalid/v1",
            "compat":{"supportsStrictTools":false}, "headers":{"X-Provider":"provider"},
            "models":[{"id":id,"cost":{"input":1,"output":2,"cacheRead":3,"cacheWrite":4},"samplingParams":{"temperature":0.3},"samplingParamsByThinkingLevel":{"high":{"topP":0.8}},"headers":{"X-Model":"model"}}],
            "modelOverrides":{id:{"contextWindow":64000,"maxTokens":4096,"cost":{"input":8},"compat":{"supportsCacheControl":false}}}
        });
        let mut other = model("other");
        other.headers = Some(BTreeMap::from([("X-Other".into(), Some("other".into()))]));
        let mut matching = model(id);
        matching.headers = Some(BTreeMap::from([(
            "X-Matching".into(),
            Some("matching".into()),
        )]));
        let projected = project_models(&docs, "fixture", &[other, matching]).unwrap();
        let value = serde_json::to_value(&projected[0]).unwrap();
        assert_eq!(value["contextWindow"], 64000);
        assert_eq!(value["maxTokens"], 4096);
        assert_eq!(value["cost"]["input"], 8);
        assert_eq!(value["cost"]["output"], 2);
        assert_eq!(value["samplingParams"]["temperature"], 0.3);
        assert_eq!(value["samplingParamsByThinkingLevel"]["high"]["topP"], 0.8);
        assert_eq!(value["headers"]["X-Provider"], "provider");
        assert_eq!(value["headers"]["X-Model"], "model");
        assert_eq!(value["headers"]["X-Matching"], "matching");
        assert!(value["headers"].get("X-Other").is_none());
        assert_eq!(value["compat"]["supportsStrictTools"], false);
        assert_eq!(value["compat"]["supportsCacheControl"], false);
    }

    #[test]
    fn missing_output_limits_are_clamped_to_discovered_or_manual_context() {
        let mut docs = docs();
        let id = "xxx/claude-opus-5-5";
        docs.catalog["providers"]["fixture"]["models"] =
            json!([{ "id": id,"contextWindow":64000 }]);
        assert_eq!(
            project_models(&docs, "fixture", &[]).unwrap()[0].max_tokens,
            64000
        );
        docs.models["providers"]["fixture"]["models"] = json!([{"id":id,"contextWindow":32000}]);
        assert_eq!(
            project_models(&docs, "fixture", &[]).unwrap()[0].max_tokens,
            32000
        );
        docs.models["providers"]["fixture"]["models"][0]["maxTokens"] = json!(4096);
        assert_eq!(
            project_models(&docs, "fixture", &[]).unwrap()[0].max_tokens,
            4096
        );
    }

    #[test]
    fn routed_opus_48_keeps_the_embedded_catalog_million_token_window() {
        let mut docs = docs();
        docs.catalog["providers"]["fixture"]["models"] =
            json!([{ "id": "baidu_overseas/claude-opus-4-8" }]);
        let original = pi_rust::ai::models::catalog::embedded_provider_catalog("anthropic");
        let projected = project_models(&docs, "fixture", &original).unwrap();
        assert_eq!(projected[0].id, "baidu_overseas/claude-opus-4-8");
        assert_eq!(projected[0].context_window, 1_000_000);
        let gateway = project_models(&docs, "fixture", &[]).unwrap();
        assert_eq!(gateway[0].context_window, 1_000_000);
        assert_eq!(gateway[0].api, "anthropic-messages");
        assert_eq!(gateway[0].provider, "fixture");
    }

    #[test]
    fn adaptive_thinking_is_protocol_scoped_and_keeps_unrelated_compatibility() {
        let mut opus = model("claude-opus-5-5");
        opus.compat = Some(json!({"supportsCacheControl":false}));
        adapt_thinking(&mut opus);
        assert_eq!(opus.compat.as_ref().unwrap()["forceAdaptiveThinking"], true);
        assert_eq!(opus.compat.as_ref().unwrap()["supportsCacheControl"], false);
        let map = opus.thinking_level_map.as_ref().unwrap();
        assert_eq!(map["off"], None);
        assert_eq!(map["minimal"], None);
        assert_eq!(map["max"].as_deref(), Some("max"));
        for (id, api) in [
            ("claude-opus-4-1", "anthropic-messages"),
            ("claude-opus-5-5", "openai-completions"),
        ] {
            let mut other = model(id);
            other.api = api.into();
            let before = other.clone();
            adapt_thinking(&mut other);
            assert_eq!(before, other);
        }
    }
    #[test]
    fn opus_55_request_uses_adaptive_effort_and_never_disabled_or_budget() {
        use pi_rust::ai::{
            api::anthropic::{build_request, options_from_simple},
            transcript::{normalize_context, Context},
            ProviderConfig, SimpleStreamOptions, ThinkingLevel,
        };
        let mut model = model("claude-opus-5-5");
        adapt_thinking(&mut model);
        let context = normalize_context(&Context {
            system_prompt: None,
            messages: vec![],
            tools: None,
        });
        let config = ProviderConfig {
            base_url: model.base_url.clone(),
            api_key: "synthetic-key".into(),
            max_tokens: model.max_tokens,
        };
        for level in [
            None,
            Some(ThinkingLevel::Low),
            Some(ThinkingLevel::Medium),
            Some(ThinkingLevel::Xhigh),
            Some(ThinkingLevel::Max),
        ] {
            let mut simple = SimpleStreamOptions {
                reasoning: level,
                ..Default::default()
            };
            simple.stream.temperature = Some(0.7);
            let options = options_from_simple(&model, &context, &simple);
            let request = build_request(&model, &config, &context, &options)
                .unwrap()
                .body;
            assert!(request.get("temperature").is_none());
            if level.is_none() {
                assert!(request.get("thinking").is_none());
            } else {
                assert_eq!(request["thinking"]["type"], "adaptive");
                assert!(request["thinking"].get("budget_tokens").is_none());
                assert!(request["output_config"]["effort"].is_string());
            }
        }
    }
    #[test]
    fn keys_are_normalized_preserved_masked_and_explicitly_cleared() {
        let mut docs = docs();
        apply_api_key(
            &mut docs,
            "fixture",
            &json!({"apiKeys":["synthetic-primary", "synthetic-secondary", "synthetic-primary"]}),
        )
        .unwrap();
        let keys = records(&docs, "fixture");
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[0]["id"], "2056c13edb51d3382dfe339f");
        let public = summaries(&docs, "fixture").to_string();
        assert!(!public.contains("synthetic-primary") && !public.contains("synthetic-secondary"));
        let saved = docs.keyrings.clone();
        apply_api_key(&mut docs, "fixture", &json!({"apiKey":""})).unwrap();
        assert_eq!(docs.keyrings, saved);
        assert!(apply_api_key(&mut docs, "fixture", &json!({"apiKeys":[true]})).is_err());
        assert_eq!(docs.keyrings, saved);
        apply_api_key(&mut docs, "fixture", &json!({"clearApiKey":true})).unwrap();
        assert!(records(&docs, "fixture").is_empty());
    }
    #[test]
    fn historical_keyring_does_not_override_oauth_credentials() {
        let mut docs = docs();
        replace_keys(&mut docs, "fixture", &[json!("synthetic-historical-key")]).unwrap();
        let keys = docs.keyrings.clone();
        docs.auth["fixture"] = json!({"type":"oauth","access":"synthetic-oauth-token"});
        assert!(records(&docs, "fixture").is_empty());
        assert_eq!(summaries(&docs, "fixture"), json!([]));
        assert_eq!(docs.keyrings, keys);
        let snapshot = json!({"providers":{"fixture":{}}, "providerKeyrings":keys});
        import_keyrings(&mut docs, &snapshot).unwrap();
        assert_eq!(docs.auth["fixture"]["type"], "oauth");
        assert_eq!(docs.keyrings, keys);
        import_keyrings(
            &mut docs,
            &json!({"providers":{"fixture":{}},"providerKeyrings":{"version":1,"providers":{}}}),
        )
        .unwrap();
        assert_eq!(docs.auth["fixture"]["type"], "oauth");
        assert!(docs.keyrings["providers"].get("fixture").is_none());
    }
    #[test]
    fn read_and_write_preserve_legacy_files_and_unknown_fields() {
        let dir = std::env::temp_dir().join(format!("pisper-upgrade-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = ProviderConfigStore::new(dir.to_str().unwrap());
        let mut source = docs();
        source.keyrings = json!({"version":1,"future":true,"providers":{"other":[{"id":"old","key":"synthetic-other"}]}});
        let empty = store.read().unwrap();
        store.persist(&empty, &source).unwrap();
        let before = std::fs::read(dir.join("pisper-provider-models.json")).unwrap();
        let mut next = store.read().unwrap();
        apply_api_key(&mut next, "fixture", &json!({"apiKeys":["synthetic-new"]})).unwrap();
        store.persist(&source, &next).unwrap();
        let reopened = store.read().unwrap();
        assert_eq!(reopened.keyrings["future"], true);
        assert_eq!(records(&reopened, "other").len(), 1);
        assert_eq!(records(&reopened, "fixture").len(), 1);
        assert_eq!(
            std::fs::read(dir.join("pisper-provider-models.json")).unwrap(),
            before
        );
        std::fs::write(dir.join("pisper-provider-keyrings.json"), b"broken").unwrap();
        assert!(store.read().is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
