//! Upstream `utils/model-operations.ts` (`packages/ai/src`): the model-type
//! operations shared by the `Models` collection and the one-shot entry
//! points — [`get_model_type`]/[`is_model_type`], the `assert*Model` checks
//! (Rust: `Result`-returning extractors), and the shared error-result
//! builders for image generation and classification.

use crate::ai::models_error::{ModelsError, ModelsErrorCode};
use crate::ai::types::images::{AssistantImages, ImagesStopReason};
use crate::ai::types::ordered_map::OrderedMap;
use crate::ai::types::{
    AnyModel, ClassifierModel, ClassifierResult, ClassifierStopReason, ImageModel, Model, ModelType,
};

/// Upstream `getModelType` (utils/model-operations.ts): the type of a model.
/// Models without `type` are chat models.
pub fn get_model_type(model: &AnyModel) -> ModelType {
    model.model_type()
}

/// Upstream `isModelType` (utils/model-operations.ts): runtime-checked model
/// type narrowing, including legacy chat models without `type`.
pub fn is_model_type(model: &AnyModel, model_type: ModelType) -> bool {
    get_model_type(model) == model_type
}

/// Upstream `assertChatModel` (utils/model-operations.ts): the chat model or
/// the `ModelsError("provider", ...)` the assert throws.
pub fn assert_chat_model(model: &AnyModel) -> Result<&Model, ModelsError> {
    model.as_chat().ok_or_else(|| {
        ModelsError::new(
            ModelsErrorCode::Provider,
            format!(
                "Model {}/{} is not a chat model",
                model.provider(),
                model.id()
            ),
        )
    })
}

/// Upstream `assertImageModel` (utils/model-operations.ts).
pub fn assert_image_model(model: &AnyModel) -> Result<&ImageModel, ModelsError> {
    model.as_image().ok_or_else(|| {
        ModelsError::new(
            ModelsErrorCode::Provider,
            format!(
                "Model {}/{} is not an image model",
                model.provider(),
                model.id()
            ),
        )
    })
}

/// Upstream `assertClassifierModel` (utils/model-operations.ts).
pub fn assert_classifier_model(model: &AnyModel) -> Result<&ClassifierModel, ModelsError> {
    model.as_classifier().ok_or_else(|| {
        ModelsError::new(
            ModelsErrorCode::Provider,
            format!(
                "Model {}/{} is not a classifier model",
                model.provider(),
                model.id()
            ),
        )
    })
}

/// Upstream `imageErrorResult` (utils/model-operations.ts): the error
/// `AssistantImages` shared by every image-generation failure path; `aborted`
/// selects the `"aborted"` stop reason (upstream the third parameter,
/// `options?.signal?.aborted` at the call sites).
pub fn image_error_result(
    model: &ImageModel,
    error: impl std::fmt::Display,
    aborted: bool,
) -> AssistantImages {
    AssistantImages {
        api: model.api.clone(),
        provider: model.provider.clone(),
        model: model.id.clone(),
        output: Vec::new(),
        response_id: None,
        usage: None,
        stop_reason: if aborted {
            ImagesStopReason::Aborted
        } else {
            ImagesStopReason::Error
        },
        error_message: Some(error.to_string()),
        timestamp: crate::ai::now_ms(),
    }
}

/// Upstream `classifierErrorResult` (utils/model-operations.ts): the error
/// `ClassifierResult` shared by every classification failure path.
pub fn classifier_error_result(
    model: &ClassifierModel,
    error: impl std::fmt::Display,
    aborted: bool,
) -> ClassifierResult {
    ClassifierResult {
        api: model.api.clone(),
        provider: model.provider.clone(),
        model: model.id.clone(),
        answers: OrderedMap::new(),
        usage: None,
        stop_reason: if aborted {
            ClassifierStopReason::Aborted
        } else {
            ClassifierStopReason::Error
        },
        error_message: Some(error.to_string()),
        timestamp: crate::ai::now_ms(),
    }
}
