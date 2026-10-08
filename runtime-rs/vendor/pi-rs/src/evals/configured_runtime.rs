//! Port of `pi/packages/evals/evals/configured-runtime.ts` — provider probes
//! through the ported [`ModelRuntime`] against the local [`AcmeServer`]
//! fixture.

use crate::ai::models::{ModelsRefreshOptions, ModelsSimpleStreamOptions};
use crate::ai::transcript::Context;
use crate::ai::types::message::{Message, StringOrBlocks, UserMessage};
use crate::ai::types::model::{Model, ModelInput};
use crate::ai::types::options::{ProviderEnv, SimpleStreamOptions, StreamOptions};
use crate::coding_agent::core::model_runtime::{CreateModelRuntimeOptions, ModelRuntime};
use serde::Serialize;
use std::path::Path;
use std::sync::Arc;

/// Upstream `ModelFields`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ModelFields {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub reasoning: bool,
    pub input: Vec<String>,
    pub cost: CostFields,
    #[serde(rename = "contextWindow")]
    pub context_window: u64,
    #[serde(rename = "maxTokens")]
    pub max_tokens: u64,
}

/// Upstream `ModelFields.cost` subset.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CostFields {
    pub input: f64,
    pub output: f64,
    #[serde(rename = "cacheRead")]
    pub cache_read: f64,
    #[serde(rename = "cacheWrite")]
    pub cache_write: f64,
}

/// Upstream response summary.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ResponseSummary {
    pub text: String,
    #[serde(rename = "stopReason")]
    pub stop_reason: String,
    #[serde(rename = "inputTokens")]
    pub input_tokens: u64,
    #[serde(rename = "outputTokens")]
    pub output_tokens: u64,
}

/// Upstream `ProviderRuntimeOutput`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProviderRuntimeOutput {
    pub result: ProviderRuntimeResult,
}

/// Upstream `ProviderRuntimeOutput.result` union.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ProviderRuntimeResult {
    Probed {
        #[serde(rename = "validRequestReceived")]
        valid_request_received: bool,
        model: ModelFields,
        response: ResponseSummary,
    },
    Failed {
        error: String,
    },
}

/// Upstream `AddedModelOutput`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AddedModelOutput {
    pub result: AddedModelResult,
}

/// Upstream `AddedModelOutput.result` union.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum AddedModelResult {
    Added {
        model: ModelFields,
        #[serde(rename = "existingModelsPreserved")]
        existing_models_preserved: bool,
    },
    Failed {
        error: String,
    },
}

/// Upstream `ProviderScenario` (the closure surface).
pub struct ProviderScenario {
    pub provider_id: String,
    pub model_id: String,
    pub prompt: String,
    pub env: Option<ProviderEnv>,
    pub max_tokens: Option<u64>,
    pub valid_request_received: Arc<dyn Fn() -> bool + Send + Sync>,
}

/// Upstream `modelFields`.
fn model_fields(model: &Model) -> ModelFields {
    ModelFields {
        id: model.id.clone(),
        name: model.name.clone(),
        provider: model.provider.clone(),
        reasoning: model.reasoning,
        input: model
            .input
            .iter()
            .map(|input| match input {
                ModelInput::Text => "text".to_string(),
                ModelInput::Image => "image".to_string(),
            })
            .collect(),
        cost: CostFields {
            input: model.cost.input,
            output: model.cost.output,
            cache_read: model.cost.cache_read,
            cache_write: model.cost.cache_write,
        },
        context_window: model.context_window,
        max_tokens: model.max_tokens,
    }
}

fn blocks_text(blocks: &[crate::ai::types::message::AssistantBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            crate::ai::types::message::AssistantBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<&str>>()
        .join("\n")
}

fn stop_reason_string(stop_reason: &crate::ai::types::primitives::StopReason) -> String {
    match stop_reason {
        crate::ai::types::primitives::StopReason::Pending => "pending".to_string(),
        crate::ai::types::primitives::StopReason::Stop => "stop".to_string(),
        crate::ai::types::primitives::StopReason::Length => "length".to_string(),
        crate::ai::types::primitives::StopReason::ToolUse => "toolUse".to_string(),
        crate::ai::types::primitives::StopReason::Error => "error".to_string(),
        crate::ai::types::primitives::StopReason::Aborted => "aborted".to_string(),
        crate::ai::types::primitives::StopReason::Deferred => "deferred".to_string(),
    }
}

/// Upstream `loadConfiguredModelRuntime`.
pub async fn load_configured_model_runtime(agent_dir: &Path) -> Result<ModelRuntime, String> {
    let options = CreateModelRuntimeOptions {
        models_path: Some(Some(path_string(&agent_dir.join("models.json")))),
        auth_path: Some(path_string(&agent_dir.join("auth.json"))),
        models_store_path: Some(path_string(&agent_dir.join("models-store.json"))),
        allow_model_network: false,
        ..Default::default()
    };
    ModelRuntime::create(options).await
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

/// Upstream `inspectProvider`.
pub async fn inspect_provider(
    runtime: &ModelRuntime,
    scenario: ProviderScenario,
) -> ProviderRuntimeOutput {
    if let Err(error) = runtime
        .refresh(ModelsRefreshOptions {
            allow_network: Some(false),
            providers: None,
            force: None,
            signal: None,
        })
        .await
    {
        // Upstream refresh rejections surface through `getError`; a hard
        // rejection here maps to the same structured error.
        return ProviderRuntimeOutput {
            result: ProviderRuntimeResult::Failed { error },
        };
    }
    if let Some(configuration_error) = runtime.get_error() {
        return ProviderRuntimeOutput {
            result: ProviderRuntimeResult::Failed {
                error: configuration_error,
            },
        };
    }
    let Some(model) = runtime
        .get_model(&scenario.provider_id, &scenario.model_id)
        .await
    else {
        return ProviderRuntimeOutput {
            result: ProviderRuntimeResult::Failed {
                error: format!(
                    "Model {}/{} is unavailable after reload.",
                    scenario.provider_id, scenario.model_id
                ),
            },
        };
    };
    let context = Context {
        system_prompt: None,
        messages: vec![Message::User(UserMessage {
            content: StringOrBlocks::Text(scenario.prompt.clone()),
            timestamp: 0,
        })],
        tools: None,
    };
    let options = ModelsSimpleStreamOptions {
        simple: SimpleStreamOptions {
            stream: StreamOptions {
                env: scenario.env.clone(),
                max_tokens: scenario.max_tokens,
                ..Default::default()
            },
            ..Default::default()
        },
        transform_headers: None,
    };
    let response = runtime
        .complete_simple(&model, &context, Some(options))
        .await;
    // Upstream only catches thrown transport/setup errors into the structured
    // error form; a completed-but-failed provider exchange resolves with
    // `stopReason: "error"` and is reported as a probe result, exactly as the
    // ported runtime settles it here.
    ProviderRuntimeOutput {
        result: ProviderRuntimeResult::Probed {
            valid_request_received: (scenario.valid_request_received)(),
            model: model_fields(&model),
            response: ResponseSummary {
                text: blocks_text(&response.content),
                stop_reason: stop_reason_string(&response.stop_reason),
                input_tokens: response.usage.input,
                output_tokens: response.usage.output,
            },
        },
    }
}

/// Upstream `inspectAddedModel`.
pub async fn inspect_added_model(
    runtime: &ModelRuntime,
    provider_id: &str,
    model_id: &str,
) -> AddedModelOutput {
    let failed = |error: String| AddedModelOutput {
        result: AddedModelResult::Failed { error },
    };
    let pristine_options = CreateModelRuntimeOptions {
        models_path: Some(None),
        allow_model_network: false,
        ..Default::default()
    };
    let Ok(pristine_runtime) = ModelRuntime::create(pristine_options).await else {
        return failed("Built-in provider registry failed to load.".to_string());
    };
    let Some(pristine_provider) = pristine_runtime.get_provider(provider_id).await else {
        return failed(format!("Built-in provider {provider_id} is unavailable."));
    };
    let existing_model_ids: Vec<String> = pristine_provider
        .get_models()
        .map(|models| models.iter().map(|model| model.id.clone()).collect())
        .unwrap_or_default();
    if existing_model_ids.is_empty() {
        return failed(format!("Built-in provider {provider_id} has no models."));
    }
    if let Err(error) = runtime
        .refresh(ModelsRefreshOptions {
            allow_network: Some(false),
            providers: None,
            force: None,
            signal: None,
        })
        .await
    {
        return failed(error);
    }
    if let Some(configuration_error) = runtime.get_error() {
        return failed(configuration_error);
    }
    let Some(model) = runtime.get_model(provider_id, model_id).await else {
        return failed(format!(
            "Model {provider_id}/{model_id} is unavailable after reload."
        ));
    };
    let existing_models_preserved = {
        let available = runtime.get_models(Some(provider_id)).await;
        let ids: std::collections::BTreeSet<&str> =
            available.iter().map(|model| model.id.as_str()).collect();
        existing_model_ids
            .iter()
            .all(|id| ids.contains(id.as_str()))
    };
    AddedModelOutput {
        result: AddedModelResult::Added {
            model: model_fields(&model),
            existing_models_preserved,
        },
    }
}

#[cfg(test)]
#[path = "configured_runtime_tests.rs"]
mod tests;
