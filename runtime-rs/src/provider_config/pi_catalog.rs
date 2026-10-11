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
    // 局部兼容项和思考映射只覆盖已声明键，不丢掉官方目录中的其他约束。
    if matches!(field, "compat" | "thinkingLevelMap") {
        if let (Some(existing), Some(overrides)) = (model[field].as_object_mut(), value.as_object())
        {
            existing.extend(overrides.clone());
            return;
        }
    }
    model[field] = value.clone();
}

pub(super) fn overlay(
    provider: &str,
    original: &[Model],
    connection: &Value,
) -> Result<Vec<Model>, ApiError> {
    let mut models: BTreeMap<String, Model> = original
        .iter()
        .cloned()
        .map(|model| (model.id.clone(), model))
        .collect();
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
        if !string(connection, "baseUrl").is_empty() {
            value["baseUrl"] = json!(upgrade_compat::canonical_base(
                string(connection, "baseUrl"),
                string(&value, "api")
            ));
        }
        if let Some(headers) = connection.get("headers") {
            value["headers"] = headers.clone();
        }
        if let Some(custom) = array(connection, "models")
            .iter()
            .find(|model| string(model, "id") == fresh.id)
        {
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
