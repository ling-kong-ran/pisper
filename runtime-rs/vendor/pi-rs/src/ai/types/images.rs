//! Image-generation types from upstream `packages/ai/src/types.ts`:
//! `ImagesContext` (types.ts:551-553), `AssistantImages` (types.ts:557-567),
//! `ImagesOptions` (types.ts:301-307), and `ImagesStopReason`
//! (types.ts:555).
//!
//! The image model descriptor is [`ImageModel`](super::model::ImageModel)
//! (upstream `ImageModel<ImageApi>`, the `type: "image"` catalog entry over
//! the shared `BaseModel` fields), defined alongside the chat
//! [`Model`](super::model::Model) since the unified catalog
//! infrastructure (#9948) made chat, image, and classifier entries one
//! family; the old `ImagesModel` shape (which carried `thinkingLevelMap`/
//! `samplingParams` and required `output`) is gone upstream.
//!
//! `ImagesInputContent`/`ImagesOutputContent` are the upstream
//! `TextContent | ImageContent` unions (types.ts:549-550) — the same tagged
//! union as the message layer's [`TextOrImageBlock`], aliased here so the
//! image surface names its own contract.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use super::message::TextOrImageBlock;
use super::model::ImageModel;
use super::options::{ProviderEnv, ProviderHeaders};
use super::primitives::Usage;
use super::request_callbacks::RequestCallbacks;

/// Upstream `ImagesInputContent`/`ImagesOutputContent` (types.ts:549-550):
/// `TextContent | ImageContent`. Same tagged union as the message layer's
/// [`TextOrImageBlock`] (`type: "text" | "image"`), shared by alias.
pub type ImagesContent = TextOrImageBlock;

/// Upstream `ImagesStopReason` (types.ts:555): `"stop" | "error" | "aborted"`.
/// Deliberately not the chat [`StopReason`](super::primitives::StopReason) —
/// image generation has no length/toolUse/deferred outcomes upstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImagesStopReason {
    Stop,
    Error,
    Aborted,
}

/// Upstream `ImagesContext` (types.ts:551-553): the generation input.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ImagesContext {
    pub input: Vec<ImagesContent>,
}

/// Upstream `AssistantImages` (types.ts:557-567): the generation result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantImages {
    pub api: String,
    pub provider: String,
    pub model: String,
    pub output: Vec<ImagesContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    pub stop_reason: ImagesStopReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    /// Unix timestamp in milliseconds.
    pub timestamp: i64,
}

impl AssistantImages {
    /// Upstream `imageErrorResult` (`utils/model-operations.ts`): the error
    /// result shape shared by every image-generation failure path, with
    /// `aborted` selecting the `"aborted"` stop reason.
    pub fn error(model: &ImageModel, message: impl Into<String>) -> Self {
        Self::with_stop_reason(model, message, ImagesStopReason::Error)
    }

    /// [`AssistantImages::error`] with an explicit stop reason (the upstream
    /// `aborted` flag).
    pub fn with_stop_reason(
        model: &ImageModel,
        message: impl Into<String>,
        stop_reason: ImagesStopReason,
    ) -> Self {
        AssistantImages {
            api: model.api.clone(),
            provider: model.provider.clone(),
            model: model.id.clone(),
            output: Vec::new(),
            response_id: None,
            usage: None,
            stop_reason,
            error_message: Some(message.into()),
            timestamp: crate::ai::now_ms(),
        }
    }
}

/// Upstream `ImagesOptions` (types.ts:301-307): the request options accepted
/// by every image-generation entry point — the
/// [`ProviderRequestOptions`](super::options::ProviderRequestOptions) fields
/// (flattened here like the chat `StreamOptions`, M2a ruling) plus `metadata`.
///
/// Port additions/deviations, mirroring the chat options module docs:
/// - `signal` (upstream `AbortSignal`, types.ts:125) has no serde shape; the
///   port carries it as a non-serialized [`CancellationToken`] so callers can
///   abort an in-flight generation (the chat M2b streams moved cancellation
///   onto the call signature instead — image generation has no stream, so the
///   options field is the only place it fits).
/// - `telemetryContext`, `fetch` are not ported (same M2a deferral as the
///   chat options); lifecycle callbacks ride on [`RequestCallbacks`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImagesOptions {
    /// Process-local lifecycle callbacks; never part of the wire
    /// representation.
    #[serde(skip)]
    pub callbacks: RequestCallbacks,
    /// Explicit credential override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// Provider-scoped environment overrides.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<ProviderEnv>,
    /// Custom HTTP headers; `None` values suppress provider defaults.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<ProviderHeaders>,
    /// HTTP request timeout in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Maximum client-side retry attempts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u32>,
    /// Cap on server-requested retry delays in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_retry_delay_ms: Option<u64>,
    /// Optional metadata included in API requests; providers extract the
    /// fields they understand and ignore the rest.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<BTreeMap<String, serde_json::Value>>,
    /// Caller cancellation (upstream `signal`, non-serialized port channel).
    #[serde(skip)]
    pub signal: Option<CancellationToken>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::types::model::ModelInput;
    use crate::ai::types::model::ModelType;
    use crate::ai::types::primitives::ModelCost;
    use crate::ai::types::primitives::UsageCost;

    /// Byte-pinned round-trip against the generated catalog entry shape:
    /// camelCase names, required fields (including the `type: "image"`
    /// discriminator), and the optional `BaseModel` fields omitted when
    /// absent.
    #[test]
    fn images_model_round_trips_generated_catalog_shape() {
        let fixture = r#"{"id":"google/gemini-2.5-flash-image","name":"Google: Nano Banana (Gemini 2.5 Flash Image)","api":"openrouter-images","provider":"openrouter","baseUrl":"https://openrouter.ai/api/v1","input":["image","text"],"cost":{"input":0.3,"output":2.5,"cacheRead":0.03,"cacheWrite":0.0833333333333333},"type":"image","output":["image","text"]}"#;
        let model: ImageModel = serde_json::from_str(fixture).unwrap();
        assert_eq!(model.id, "google/gemini-2.5-flash-image");
        assert_eq!(model.input, vec![ModelInput::Image, ModelInput::Text]);
        assert_eq!(model.output, vec![ModelInput::Image, ModelInput::Text]);
        assert_eq!(model.cost.input, 0.3);
        assert_eq!(model.headers, None);
        assert_eq!(serde_json::to_string(&model).unwrap(), fixture);
    }

    #[test]
    fn images_model_carries_optional_fields_when_present() {
        let model = ImageModel {
            id: "m".into(),
            name: "M".into(),
            api: "test-images".into(),
            provider: "p".into(),
            base_url: "https://example.test/v1".into(),
            input: vec![ModelInput::Text],
            input_limits: None,
            cost: ModelCost {
                input: 1.0,
                output: 2.0,
                cache_read: 0.0,
                cache_write: 0.0,
                tiers: None,
            },
            headers: Some(BTreeMap::from([(
                "HTTP-Referer".to_string(),
                Some("https://example.com".to_string()),
            )])),
            r#type: ModelType::Image,
            output: vec![ModelInput::Image],
        };
        let wire = serde_json::to_string(&model).unwrap();
        assert!(
            wire.contains(r#""headers":{"HTTP-Referer":"https://example.com"}"#),
            "{wire}"
        );
        assert!(wire.contains(r#""type":"image""#), "{wire}");
        let back: ImageModel = serde_json::from_str(&wire).unwrap();
        assert_eq!(back, model);
    }

    #[test]
    fn images_context_round_trips() {
        let context = ImagesContext {
            input: vec![
                TextOrImageBlock::Text(super::super::content::TextContent {
                    text: "a red circle".into(),
                    text_signature: None,
                }),
                TextOrImageBlock::Image(super::super::content::ImageContent {
                    data: "aGk=".into(),
                    mime_type: "image/png".into(),
                }),
            ],
        };
        let wire = serde_json::to_string(&context).unwrap();
        assert_eq!(
            wire,
            r#"{"input":[{"type":"text","text":"a red circle"},{"type":"image","data":"aGk=","mimeType":"image/png"}]}"#
        );
        let back: ImagesContext = serde_json::from_str(&wire).unwrap();
        assert_eq!(back, context);
    }

    #[test]
    fn assistant_images_round_trips_minimal_and_error() {
        let result = AssistantImages {
            api: "test-images".into(),
            provider: "p".into(),
            model: "m".into(),
            output: vec![TextOrImageBlock::Image(
                super::super::content::ImageContent {
                    data: "aGk=".into(),
                    mime_type: "image/png".into(),
                },
            )],
            response_id: None,
            usage: None,
            stop_reason: ImagesStopReason::Stop,
            error_message: None,
            timestamp: 1758240000000,
        };
        let wire = serde_json::to_string(&result).unwrap();
        assert_eq!(
            wire,
            r#"{"api":"test-images","provider":"p","model":"m","output":[{"type":"image","data":"aGk=","mimeType":"image/png"}],"stopReason":"stop","timestamp":1758240000000}"#
        );
        let back: AssistantImages = serde_json::from_str(&wire).unwrap();
        assert_eq!(back, result);
    }

    #[test]
    fn assistant_images_error_result_carries_message() {
        let model = test_image_model("ghost", "m");
        let result = AssistantImages::error(&model, "Unknown provider: ghost");
        assert_eq!(result.stop_reason, ImagesStopReason::Error);
        assert_eq!(
            result.error_message.as_deref(),
            Some("Unknown provider: ghost")
        );
        assert!(result.output.is_empty());
        assert!(result.timestamp > 0);
    }

    #[test]
    fn images_stop_reason_wire_values() {
        assert_eq!(
            serde_json::to_string(&ImagesStopReason::Stop).unwrap(),
            "\"stop\""
        );
        assert_eq!(
            serde_json::to_string(&ImagesStopReason::Error).unwrap(),
            "\"error\""
        );
        assert_eq!(
            serde_json::to_string(&ImagesStopReason::Aborted).unwrap(),
            "\"aborted\""
        );
    }

    pub(crate) fn test_image_model(provider: &str, id: &str) -> ImageModel {
        ImageModel {
            id: id.to_string(),
            name: id.to_string(),
            api: "test-images".to_string(),
            provider: provider.to_string(),
            base_url: "https://example.test/v1".to_string(),
            input: vec![ModelInput::Text],
            input_limits: None,
            cost: ModelCost::default(),
            headers: None,
            r#type: ModelType::Image,
            output: vec![ModelInput::Image],
        }
    }

    #[allow(dead_code)]
    pub(crate) fn usage(values: (u64, u64, u64, u64)) -> Usage {
        Usage {
            input: values.0,
            output: values.1,
            cache_read: values.2,
            cache_write: values.3,
            cache_write_1h: None,
            reasoning: None,
            total_tokens: values.0 + values.1 + values.2 + values.3,
            cost: UsageCost::default(),
        }
    }
}
