//! 原模型元数据文件的只读投影，不搬迁数据，不把推断值写入用户模型配置。
use super::*;

#[derive(Clone, Default)]
pub(super) struct ModelMetadata {
    metadata: Value,
    reference: Vec<(Vec<String>, u64)>,
}

fn tokens(id: &str) -> Vec<String> {
    id.to_lowercase()
        .split(|c: char| c.is_whitespace() || "/:@._-&%".contains(c))
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect()
}

fn aligned(a: &[String], b: &[String]) -> bool {
    !a.is_empty() && !b.is_empty() && a.iter().rev().zip(b.iter().rev()).all(|(a, b)| a == b)
}

impl ModelMetadata {
    pub(super) fn read(root: &FsPath) -> Self {
        // 与旧服务相同：不可用的派生元数据不阻塞启动，也不覆盖原文件。
        let read = |name| {
            std::fs::read(root.join(name))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .unwrap_or(json!({}))
        };
        let metadata = read("pisper-model-metadata.json");
        let reference = read("pi-dev-models-cache.json")["models"]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(id, value)| {
                value["contextWindow"]
                    .as_u64()
                    .filter(|limit| *limit > 0 && *limit <= 100_000_000)
                    .map(|limit| (tokens(id), limit))
            })
            .collect();
        Self {
            metadata,
            reference,
        }
    }

    fn get(&self, id: &str) -> Value {
        let key = id.trim().to_lowercase();
        let mut metadata = self.metadata["models"]
            .get(&key)
            .or_else(|| self.metadata["models"].get(key.rsplit('/').next().unwrap_or(&key)))
            .filter(|value| value.is_object())
            .cloned()
            .unwrap_or(json!({}));
        let query = tokens(id);
        if let Some((canonical, _)) = self
            .reference
            .iter()
            .filter(|(tokens, _)| aligned(tokens, &query))
            .min_by_key(|(tokens, _)| tokens.len())
        {
            if let Some(limit) = self
                .reference
                .iter()
                .filter(|(tokens, _)| tokens.len() >= canonical.len() && aligned(tokens, canonical))
                .map(|(_, limit)| *limit)
                .max()
            {
                metadata["contextWindow"] = json!(limit);
            }
        }
        metadata
    }

    pub(super) fn project(
        &self,
        connection: &Value,
        models: &[Model],
    ) -> Result<Vec<Model>, ApiError> {
        models
            .iter()
            .map(|model| {
                let metadata = self.get(&model.id);
                let configured = pi_catalog::configured_fields(connection, &model.id);
                let mut value = serde_json::to_value(model).expect("serializable model");
                for field in [
                    "contextWindow",
                    "maxTokens",
                    "reasoning",
                    "input",
                    "thinkingLevelMap",
                ] {
                    if configured
                        .as_ref()
                        .and_then(|config| config.get(field))
                        .is_some()
                    {
                        continue;
                    }
                    if let Some(item) = metadata.get(field) {
                        let valid = match field {
                            "contextWindow" | "maxTokens" => {
                                item.as_u64().is_some_and(|v| v > 0 && v <= 100_000_000)
                            }
                            "reasoning" => item.is_boolean(),
                            "input" => item.as_array().is_some_and(|items| {
                                !items.is_empty()
                                    && items.iter().all(|v| v == "text" || v == "image")
                            }),
                            "thinkingLevelMap" => {
                                item.is_object()
                                    && item
                                        .as_object()
                                        .unwrap()
                                        .values()
                                        .all(|v| v.is_null() || v.is_string())
                            }
                            _ => false,
                        };
                        if valid {
                            pi_catalog::override_field(&mut value, field, item);
                        }
                    }
                }
                serde_json::from_value(value)
                    .map_err(|_| ApiError::bad_request("模型元数据格式无效。"))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_original_metadata_and_preserves_explicit_limits_and_file_bytes() {
        let root =
            std::env::temp_dir().join(format!("pisper-model-reference-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let metadata = br#"{"version":1,"models":{"fixture-model":{"contextWindow":200000,"maxTokens":4000,"reasoning":true}}}"#;
        let reference = br#"{"models":{"fixture-model":{"contextWindow":300000},"gateway/fixture-model":{"contextWindow":400000},"fixture-model-mini":{"contextWindow":10000}}}"#;
        std::fs::write(root.join("pisper-model-metadata.json"), metadata).unwrap();
        std::fs::write(root.join("pi-dev-models-cache.json"), reference).unwrap();
        let data = ModelMetadata::read(&root);
        assert_eq!(data.get("other/fixture-model")["contextWindow"], 400000);
        assert_eq!(data.get("fixture-model-mini")["contextWindow"], 10000);
        let model: Model = serde_json::from_value(json!({"id":"fixture-model","name":"fixture","provider":"fixture","api":"openai-completions","baseUrl":"https://fixture.invalid","reasoning":false,"input":["text"],"contextWindow":100000,"maxTokens":1000,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0}})).unwrap();
        let inferred = data.project(&json!({}), &[model.clone()]).unwrap();
        assert_eq!(inferred[0].context_window, 400000);
        assert_eq!(inferred[0].max_tokens, 4000);
        let explicit = data
            .project(
                &json!({"models":[{"id":"fixture-model","contextWindow":100000,"maxTokens":1000}]}),
                &[model],
            )
            .unwrap();
        assert_eq!(explicit[0].context_window, 100000);
        assert_eq!(explicit[0].max_tokens, 1000);
        assert_eq!(
            std::fs::read(root.join("pisper-model-metadata.json")).unwrap(),
            metadata
        );
        assert_eq!(
            std::fs::read(root.join("pi-dev-models-cache.json")).unwrap(),
            reference
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
