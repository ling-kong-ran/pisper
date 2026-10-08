//! Upstream `Model` (`packages/ai/src/types.ts:951-983`): the unified model
//! descriptor shared by every provider API and the model catalog.
//!
//! Wire format must match the TypeScript interface field-for-field: struct
//! fields serialize with the upstream camelCase names in upstream declaration
//! order, `input` serializes the literal union values `"text"`/`"image"`, and
//! the four optional fields are omitted from JSON when `None` (like upstream
//! `undefined`) while all other fields are required on deserialize (missing
//! required fields are a hard error, like the TypeScript type).
//!
//! `compat` is deliberately a raw `serde_json::Value`, not a typed enum:
//! upstream catalog compat objects carry no discriminator, so an untagged
//! enum is ambiguous — `{"supportsStrictMode": true}` is valid for both
//! `OpenAIResponsesCompat` and `BedrockCompat`. Typed access is on demand via
//! the `*_compat` methods, which deserialize the stored object into the
//! struct for one API (all compat fields are optional, so every key defaults).
//!
//! Port-safer difference, kept deliberately: a type-invalid compat field
//! (e.g. a string where upstream declares boolean) makes the typed read fail
//! and callers fall back to the default struct (`unwrap_or_default`), while
//! upstream JS — with no runtime validation — reads the raw object with
//! truthiness and keeps the rest of the overrides.

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::BTreeMap;

use super::compat::{
    AnthropicMessagesCompat, BedrockCompat, MistralConversationsCompat, OpenAiCompletionsCompat,
    OpenAiResponsesCompat,
};
use super::options::ProviderHeaders;
use super::primitives::{
    KnownApi, ModelCost, SamplingParams, SamplingParamsByThinkingLevel, ThinkingLevelMap,
};

/// Upstream `ModelType` (types.ts, `keyof ModelTypeMap`): what a catalog entry
/// is for. Wire values are the upstream literal strings; chat is the default —
/// models without `type` are chat models (upstream `Model.type?: "chat"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelType {
    Chat,
    Image,
    Classifier,
}

impl ModelType {
    /// Every model type, in upstream `ModelTypeMap` key order.
    pub const ALL: [ModelType; 3] = [ModelType::Chat, ModelType::Image, ModelType::Classifier];
}

/// Upstream `ModelPromptCache` (types.ts): best-effort prompt cache lifetime in
/// seconds per retention tier. A missing tier means the lifetime is unknown; pi
/// does not warm such caches. Upstream keys are `Exclude<CacheRetention,
/// "none">` — `"short"` and `"long"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelPromptCache(pub BTreeMap<PromptCacheTier, f64>);

/// Upstream `ModelPromptCache` key: the non-`"none"` [`CacheRetention`] tiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptCacheTier {
    Short,
    Long,
}

/// Upstream `ModelImageResizeOptions` (types.ts): one cache-safe resize profile.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelImageResizeOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_width: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_height: Option<f64>,
    /// Maximum base64-encoded payload size in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jpeg_quality: Option<f64>,
}

/// Upstream `ModelImageInputLimits` (types.ts): per-request image input limits.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelImageInputLimits {
    /// Cache-safe resize profile applied before a new image enters
    /// conversation history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resize: Option<ModelImageResizeOptions>,
    /// Maximum images accepted in one provider message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_per_message: Option<f64>,
    /// Maximum images accepted across one provider request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_per_request: Option<f64>,
}

/// Upstream `ModelInputLimits` (types.ts): provider input limits and
/// cache-safe preprocessing metadata.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInputLimits {
    /// Maximum serialized provider request size in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_request_bytes: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub images: Option<ModelImageInputLimits>,
}

/// Upstream `Model["input"]` element (types.ts:963): input modalities the
/// model accepts. Wire values are the upstream literal union strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelInput {
    Text,
    Image,
}

/// Upstream `Model` (types.ts:951-983). Upstream narrows `compat` (and the
/// whole interface) by the model's `api` type parameter; Rust erases that
/// generic here — `api` stays an open-ended string (upstream
/// `Api = KnownApi | (string & {})`, types.ts:29) and compat is accessed per
/// API through the typed methods below.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    /// Model identifier used in provider requests (types.ts:952).
    pub id: String,
    /// Display name (types.ts:953).
    pub name: String,
    /// Provider API id (types.ts:954). Open-ended upstream, so a plain string
    /// checked against `KNOWN_API` via [`Model::is_known_api`] — same pattern
    /// as `AssistantMessage.api` in `message.rs`.
    pub api: String,
    /// Provider id (types.ts:955). Upstream `ProviderId`
    /// (`KnownProvider | string`); open-ended like the ids checked against
    /// `KNOWN_PROVIDERS`.
    pub provider: String,
    /// Base URL of the provider endpoint (types.ts:956).
    pub base_url: String,
    /// Optional chat discriminator (types.ts, `Model.type?: "chat"`). Chat is
    /// the default, so catalog entries without `type` deserialize to `None`
    /// and omit the key on reserialization; an explicit `"chat"` round-trips.
    /// Non-chat catalog entries deserialize into [`ImageModel`]/
    /// [`ClassifierModel`] via [`AnyModel`], not into this struct.
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub r#type: Option<ModelType>,
    /// Whether the model supports reasoning/thinking (types.ts:957).
    pub reasoning: bool,
    /// Maps pi thinking levels to provider/model-specific values
    /// (types.ts:959-962). Missing keys use provider defaults; `null` marks a
    /// level as unsupported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_level_map: Option<ThinkingLevelMap>,
    /// Prompt cache lifetimes per retention tier (types.ts
    /// `Model.promptCache`). Unset when the provider's cache behavior is
    /// unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_cache: Option<ModelPromptCache>,
    /// Input modalities the model accepts (types.ts:963).
    pub input: Vec<ModelInput>,
    /// Provider input limits and cache-safe preprocessing metadata
    /// (types.ts `BaseModel.inputLimits`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_limits: Option<ModelInputLimits>,
    /// Pricing in dollars per million tokens, with optional request-wide
    /// tiers (types.ts:964; `ModelCost` in `primitives.rs`).
    pub cost: ModelCost,
    /// Context window size in tokens (types.ts:965).
    pub context_window: u64,
    /// Maximum output tokens (types.ts:966).
    pub max_tokens: u64,
    /// Default sampling parameters merged into request bodies; per-request
    /// keys override these (types.ts:1129; see `StreamOptions.samplingParams`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sampling_params: Option<SamplingParams>,
    /// Sampling parameter overrides selected by the effective pi thinking
    /// level (types.ts:1130, `Model.samplingParamsByThinkingLevel`). Looked up
    /// with the clamped thinking level of each request and merged over
    /// [`Model::sampling_params`] before the per-request keys.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sampling_params_by_thinking_level: Option<SamplingParamsByThinkingLevel>,
    /// Custom HTTP headers merged over provider defaults (types.ts:968).
    /// Upstream declares `Record<string, string>` but every merge path treats
    /// it as [`ProviderHeaders`]: a `null` value (here `None`) suppresses the
    /// default header with the same name (delete-then-set-null in
    /// `mergeHeaders`, models.ts:250-262), and `providerHeadersToRecord`
    /// drops nulls from the final record.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<ProviderHeaders>,
    /// Compatibility overrides for the model's API (types.ts:969-982), stored
    /// untagged — see the module docs for why this is a raw value and how to
    /// get typed access. Upstream: "If not set, auto-detected from baseUrl."
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compat: Option<serde_json::Value>,
}

impl Model {
    /// Whether [`Model::api`] names one of the ten APIs pi ships adapters for
    /// (`KNOWN_API`). Upstream accepts any `Api` string on a model; unknown
    /// APIs have no adapter and no typed compat struct.
    pub fn is_known_api(&self) -> bool {
        serde_json::from_value::<KnownApi>(serde_json::Value::String(self.api.clone())).is_ok()
    }

    /// Upstream `Model["compat"]` when `api` is `"openai-completions"`
    /// (`OpenAICompletionsCompat`, types.ts:969-971). Deserializes the stored
    /// compat object on demand; missing `compat` yields the default struct.
    pub fn openai_completions_compat(&self) -> Result<OpenAiCompletionsCompat, serde_json::Error> {
        self.compat_as()
    }

    /// Upstream `Model["compat"]` when `api` is `"openai-responses"`,
    /// `"azure-openai-responses"`, or `"openai-codex-responses"`
    /// (`OpenAIResponsesCompat`, types.ts:971-973).
    pub fn openai_responses_compat(&self) -> Result<OpenAiResponsesCompat, serde_json::Error> {
        self.compat_as()
    }

    /// Upstream `Model["compat"]` when `api` is `"anthropic-messages"`
    /// (`AnthropicMessagesCompat`, types.ts:974-975).
    pub fn anthropic_compat(&self) -> Result<AnthropicMessagesCompat, serde_json::Error> {
        self.compat_as()
    }

    /// Upstream `Model["compat"]` when `api` is `"bedrock-converse-stream"`
    /// (`BedrockCompat`, types.ts:976-977).
    pub fn bedrock_compat(&self) -> Result<BedrockCompat, serde_json::Error> {
        self.compat_as()
    }

    /// Upstream `Model["compat"]` when `api` is `"mistral-conversations"`
    /// (`MistralConversationsCompat`, types.ts:978-979).
    pub fn mistral_compat(&self) -> Result<MistralConversationsCompat, serde_json::Error> {
        self.compat_as()
    }

    /// Shared accessor body: deserialize the raw compat object into a typed
    /// per-API struct, or the default struct when `compat` is absent.
    fn compat_as<T>(&self) -> Result<T, serde_json::Error>
    where
        T: DeserializeOwned + Default,
    {
        match &self.compat {
            Some(value) => T::deserialize(value),
            None => Ok(T::default()),
        }
    }
}

/// Upstream `BaseModel<TApi>` (types.ts): the fields shared by every catalog
/// entry, regardless of what you can do with it. Rust cannot express the
/// shared-interface inheritance, so [`ImageModel`]/[`ClassifierModel`]
/// repeat these fields in the same declaration order.
#[derive(Debug, Clone, PartialEq)]
pub struct BaseModelFields {
    pub id: String,
    pub name: String,
    pub api: String,
    pub provider: String,
    pub base_url: String,
    pub input: Vec<ModelInput>,
    /// Provider input limits and cache-safe preprocessing metadata.
    pub input_limits: Option<ModelInputLimits>,
    pub cost: ModelCost,
    pub headers: Option<ProviderHeaders>,
}

/// Upstream `ImageModel<TApi>` (types.ts): image-generation model, usable with
/// `generateImages()` only. `type: "image"` is required on the wire; `output`
/// always includes `"image"`, `"text"` means the model can also return text
/// blocks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageModel {
    pub id: String,
    pub name: String,
    /// Image-generation API id (upstream `ImageApi`, open-ended string).
    pub api: String,
    pub provider: String,
    pub base_url: String,
    pub input: Vec<ModelInput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_limits: Option<ModelInputLimits>,
    pub cost: ModelCost,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<ProviderHeaders>,
    /// Required `"image"` discriminator (upstream `type: "image"`).
    pub r#type: ModelType,
    /// Output modalities; always includes `"image"`.
    pub output: Vec<ModelInput>,
}

/// Upstream `ClassifierModel<TApi>` (types.ts): structured classifier model,
/// usable with `classify()` only. `type: "classifier"` and `contextWindow`
/// are required on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassifierModel {
    pub id: String,
    pub name: String,
    /// Classifier API id (upstream `ClassifierApi`, open-ended string).
    pub api: String,
    pub provider: String,
    pub base_url: String,
    pub input: Vec<ModelInput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_limits: Option<ModelInputLimits>,
    pub cost: ModelCost,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<ProviderHeaders>,
    /// Required `"classifier"` discriminator (upstream `type: "classifier"`).
    pub r#type: ModelType,
    pub context_window: u64,
}

impl From<&ImageModel> for BaseModelFields {
    fn from(model: &ImageModel) -> Self {
        Self {
            id: model.id.clone(),
            name: model.name.clone(),
            api: model.api.clone(),
            provider: model.provider.clone(),
            base_url: model.base_url.clone(),
            input: model.input.clone(),
            input_limits: model.input_limits.clone(),
            cost: model.cost.clone(),
            headers: model.headers.clone(),
        }
    }
}

impl From<&ClassifierModel> for BaseModelFields {
    fn from(model: &ClassifierModel) -> Self {
        Self {
            id: model.id.clone(),
            name: model.name.clone(),
            api: model.api.clone(),
            provider: model.provider.clone(),
            base_url: model.base_url.clone(),
            input: model.input.clone(),
            input_limits: model.input_limits.clone(),
            cost: model.cost.clone(),
            headers: model.headers.clone(),
        }
    }
}

impl From<&Model> for BaseModelFields {
    fn from(model: &Model) -> Self {
        Self {
            id: model.id.clone(),
            name: model.name.clone(),
            api: model.api.clone(),
            provider: model.provider.clone(),
            base_url: model.base_url.clone(),
            input: model.input.clone(),
            input_limits: model.input_limits.clone(),
            cost: model.cost.clone(),
            headers: model.headers.clone(),
        }
    }
}

/// Upstream `AnyModel` (types.ts, `ModelTypeMap[ModelType]`): anything a
/// provider can list — a chat [`Model`], an [`ImageModel`], or a
/// [`ClassifierModel`]. Serialized as exactly the inner variant (the
/// upstream objects differ only by their `type` field and shape); the
/// deserializer dispatches on the `type` field, treating an absent or
/// `"chat"` value as a chat model like upstream `getModelType`.
#[derive(Debug, Clone, PartialEq)]
pub enum AnyModel {
    Chat(Model),
    Image(ImageModel),
    Classifier(ClassifierModel),
}

impl AnyModel {
    /// Upstream `getModelType` (`utils/model-operations.ts`): the type of a
    /// model. Models without `type` are chat models.
    pub fn model_type(&self) -> ModelType {
        match self {
            AnyModel::Chat(model) => model.r#type.unwrap_or(ModelType::Chat),
            AnyModel::Image(_) => ModelType::Image,
            AnyModel::Classifier(_) => ModelType::Classifier,
        }
    }

    /// Shared catalog identity fields (`BaseModel`).
    pub fn base(&self) -> BaseModelFields {
        match self {
            AnyModel::Chat(model) => model.into(),
            AnyModel::Image(model) => model.into(),
            AnyModel::Classifier(model) => model.into(),
        }
    }

    pub fn id(&self) -> &str {
        match self {
            AnyModel::Chat(model) => &model.id,
            AnyModel::Image(model) => &model.id,
            AnyModel::Classifier(model) => &model.id,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            AnyModel::Chat(model) => &model.name,
            AnyModel::Image(model) => &model.name,
            AnyModel::Classifier(model) => &model.name,
        }
    }

    pub fn api(&self) -> &str {
        match self {
            AnyModel::Chat(model) => &model.api,
            AnyModel::Image(model) => &model.api,
            AnyModel::Classifier(model) => &model.api,
        }
    }

    pub fn provider(&self) -> &str {
        match self {
            AnyModel::Chat(model) => &model.provider,
            AnyModel::Image(model) => &model.provider,
            AnyModel::Classifier(model) => &model.provider,
        }
    }

    pub fn base_url(&self) -> &str {
        match self {
            AnyModel::Chat(model) => &model.base_url,
            AnyModel::Image(model) => &model.base_url,
            AnyModel::Classifier(model) => &model.base_url,
        }
    }

    pub fn cost(&self) -> &ModelCost {
        match self {
            AnyModel::Chat(model) => &model.cost,
            AnyModel::Image(model) => &model.cost,
            AnyModel::Classifier(model) => &model.cost,
        }
    }

    pub fn headers(&self) -> Option<&ProviderHeaders> {
        match self {
            AnyModel::Chat(model) => model.headers.as_ref(),
            AnyModel::Image(model) => model.headers.as_ref(),
            AnyModel::Classifier(model) => model.headers.as_ref(),
        }
    }

    /// The chat model, when this is one (upstream `isModelType(model,
    /// "chat")` narrowing).
    pub fn as_chat(&self) -> Option<&Model> {
        match self {
            AnyModel::Chat(model) => Some(model),
            _ => None,
        }
    }

    pub fn as_image(&self) -> Option<&ImageModel> {
        match self {
            AnyModel::Image(model) => Some(model),
            _ => None,
        }
    }

    pub fn as_classifier(&self) -> Option<&ClassifierModel> {
        match self {
            AnyModel::Classifier(model) => Some(model),
            _ => None,
        }
    }
}

impl From<Model> for AnyModel {
    fn from(model: Model) -> Self {
        AnyModel::Chat(model)
    }
}

impl From<ImageModel> for AnyModel {
    fn from(model: ImageModel) -> Self {
        AnyModel::Image(model)
    }
}

impl From<ClassifierModel> for AnyModel {
    fn from(model: ClassifierModel) -> Self {
        AnyModel::Classifier(model)
    }
}

impl Serialize for AnyModel {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            AnyModel::Chat(model) => model.serialize(serializer),
            AnyModel::Image(model) => model.serialize(serializer),
            AnyModel::Classifier(model) => model.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for AnyModel {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        let kind = value
            .as_object()
            .and_then(|object| object.get("type"))
            .and_then(|kind| kind.as_str());
        match kind {
            Some("image") => Ok(AnyModel::Image(
                ImageModel::deserialize(value).map_err(serde::de::Error::custom)?,
            )),
            Some("classifier") => Ok(AnyModel::Classifier(
                ClassifierModel::deserialize(value).map_err(serde::de::Error::custom)?,
            )),
            // Chat is the default: absent, null, and explicit "chat" (plus any
            // other value, which the chat struct stores verbatim in
            // `Model.r#type` — upstream has no runtime validation either).
            _ => Ok(AnyModel::Chat(
                Model::deserialize(value).map_err(serde::de::Error::custom)?,
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::compat::{MaxTokensField, ThinkingFormat};
    use super::super::primitives::KNOWN_API;
    use super::*;
    use serde_json::json;

    /// Anthropic-style catalog entry exercising every field, including cost
    /// tiers. `thinkingLevelMap` has a single key so the byte-pinned round-trip
    /// is deterministic (BTreeMap key order), and compat/sampling/headers keys
    /// are pre-sorted because `serde_json::Value` objects reserialize in
    /// BTreeMap (sorted) order without the `preserve_order` feature. The
    /// `headers` entry includes a `null` value: upstream treats model headers
    /// as `ProviderHeaders`, where null suppresses.
    const ANTHROPIC_MODEL_FIXTURE: &str = r#"{"id":"claude-sonnet-4-5","name":"Claude Sonnet 4.5","api":"anthropic-messages","provider":"anthropic","baseUrl":"https://api.anthropic.com","reasoning":true,"thinkingLevelMap":{"off":null},"input":["text","image"],"cost":{"input":3,"output":15,"cacheRead":0.3,"cacheWrite":3.75,"tiers":[{"input":1.5,"output":7.5,"cacheRead":0.15,"cacheWrite":1.875,"inputTokensAbove":200000}]},"contextWindow":200000,"maxTokens":64000,"samplingParams":{"custom_flag":true,"top_p":0.95},"headers":{"x-custom":"value","x-drop":null},"compat":{"forceAdaptiveThinking":true,"supportsCacheControlOnTools":false}}"#;

    /// OpenAI-completions-style catalog entry with a completions compat object.
    const OPENAI_COMPLETIONS_MODEL_FIXTURE: &str = r#"{"id":"gpt-test","name":"GPT Test","api":"openai-completions","provider":"openai","baseUrl":"https://api.openai.com/v1","reasoning":false,"input":["text"],"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0},"contextWindow":128000,"maxTokens":16384,"compat":{"maxTokensField":"max_tokens","supportsStore":false,"thinkingFormat":"zai"}}"#;

    fn base_model() -> Model {
        Model {
            id: "claude-test".to_string(),
            name: "Claude Test".to_string(),
            api: "anthropic-messages".to_string(),
            provider: "anthropic".to_string(),
            base_url: "https://api.anthropic.com".to_string(),
            r#type: None,
            reasoning: false,
            thinking_level_map: None,
            prompt_cache: None,
            input: vec![ModelInput::Text],
            input_limits: None,
            cost: ModelCost::default(),
            context_window: 100000,
            max_tokens: 4096,
            sampling_params: None,
            sampling_params_by_thinking_level: None,
            headers: None,
            compat: None,
        }
    }

    #[test]
    fn model_input_wire_values_match_upstream() {
        assert_eq!(
            serde_json::to_string(&ModelInput::Text).unwrap(),
            "\"text\""
        );
        assert_eq!(
            serde_json::to_string(&ModelInput::Image).unwrap(),
            "\"image\""
        );
        assert_eq!(
            serde_json::from_str::<ModelInput>("\"text\"").unwrap(),
            ModelInput::Text
        );
        assert_eq!(
            serde_json::from_str::<ModelInput>("\"image\"").unwrap(),
            ModelInput::Image
        );
        assert!(serde_json::from_str::<ModelInput>("\"audio\"").is_err());
    }

    #[test]
    fn model_round_trips_all_fields_with_compat_present() {
        let model: Model = serde_json::from_str(ANTHROPIC_MODEL_FIXTURE).unwrap();

        assert_eq!(model.id, "claude-sonnet-4-5");
        assert_eq!(model.name, "Claude Sonnet 4.5");
        assert_eq!(model.api, "anthropic-messages");
        assert_eq!(model.provider, "anthropic");
        assert_eq!(model.base_url, "https://api.anthropic.com");
        assert!(model.reasoning);
        assert_eq!(
            model.thinking_level_map.as_ref().unwrap().get("off"),
            Some(&None)
        );
        assert_eq!(model.input, vec![ModelInput::Text, ModelInput::Image]);
        assert_eq!(model.cost.input, 3.0);
        assert_eq!(model.cost.output, 15.0);
        assert_eq!(model.cost.cache_read, 0.3);
        assert_eq!(model.cost.cache_write, 3.75);
        let tiers = model.cost.tiers.as_ref().unwrap();
        assert_eq!(tiers.len(), 1);
        assert_eq!(tiers[0].input, 1.5);
        assert_eq!(tiers[0].input_tokens_above, 200000);
        assert_eq!(model.context_window, 200000);
        assert_eq!(model.max_tokens, 64000);
        let sampling = model.sampling_params.as_ref().unwrap();
        assert_eq!(sampling.get("top_p"), Some(&json!(0.95)));
        assert_eq!(sampling.get("custom_flag"), Some(&json!(true)));
        assert_eq!(
            model.headers.as_ref().unwrap().get("x-custom"),
            Some(&Some("value".to_string()))
        );
        // A null header value parses as the suppression form (None) and
        // round-trips back to null on the wire.
        assert_eq!(model.headers.as_ref().unwrap().get("x-drop"), Some(&None));
        assert_eq!(
            model.compat,
            Some(json!({
                "forceAdaptiveThinking": true,
                "supportsCacheControlOnTools": false
            }))
        );

        // Byte-pinned round-trip: serializing back reproduces the exact wire
        // format (upstream field order, camelCase names, required fields).
        assert_eq!(
            serde_json::to_string(&model).unwrap(),
            ANTHROPIC_MODEL_FIXTURE
        );
        let reparsed: Model = serde_json::from_str(ANTHROPIC_MODEL_FIXTURE).unwrap();
        assert_eq!(reparsed, model);
    }

    #[test]
    fn model_optional_fields_omitted_when_absent() {
        let model = base_model();
        let json = serde_json::to_string(&model).unwrap();
        assert!(!json.contains("thinkingLevelMap"), "{json}");
        assert!(!json.contains("samplingParams"), "{json}");
        assert!(!json.contains("headers"), "{json}");
        assert!(!json.contains("compat"), "{json}");
        let back: Model = serde_json::from_str(&json).unwrap();
        assert_eq!(back, model);
    }

    #[test]
    fn model_missing_required_field_is_rejected() {
        let model: serde_json::Value =
            serde_json::from_str(OPENAI_COMPLETIONS_MODEL_FIXTURE).unwrap();
        for required in ["reasoning", "contextWindow", "maxTokens", "cost", "input"] {
            let mut broken = model.clone();
            broken.as_object_mut().unwrap().remove(required);
            assert!(
                serde_json::from_str::<Model>(&broken.to_string()).is_err(),
                "missing {required} must be rejected"
            );
        }
    }

    #[test]
    fn compat_accessors_deserialize_typed_values() {
        let openai: Model = serde_json::from_str(OPENAI_COMPLETIONS_MODEL_FIXTURE).unwrap();
        let completions = openai.openai_completions_compat().unwrap();
        assert_eq!(
            completions.max_tokens_field,
            Some(MaxTokensField::MaxTokens)
        );
        assert_eq!(completions.supports_store, Some(false));
        assert_eq!(completions.thinking_format, Some(ThinkingFormat::Zai));
        assert_eq!(completions.supports_developer_role, None);

        // The raw compat object type-checks against any API's struct; fields
        // it does not declare are simply absent (upstream structural typing).
        let responses = openai.openai_responses_compat().unwrap();
        assert_eq!(responses, OpenAiResponsesCompat::default());

        let anthropic: Model = serde_json::from_str(ANTHROPIC_MODEL_FIXTURE).unwrap();
        let compat = anthropic.anthropic_compat().unwrap();
        assert_eq!(compat.force_adaptive_thinking, Some(true));
        assert_eq!(compat.supports_cache_control_on_tools, Some(false));
        assert_eq!(compat.supports_temperature, None);
    }

    #[test]
    fn compat_accessors_cover_every_api() {
        let mut bedrock = base_model();
        bedrock.api = "bedrock-converse-stream".to_string();
        bedrock.compat = Some(json!({"supportsStrictMode": true}));
        assert_eq!(
            bedrock.bedrock_compat().unwrap().supports_strict_mode,
            Some(true)
        );

        let mut mistral = base_model();
        mistral.api = "mistral-conversations".to_string();
        mistral.compat = Some(json!({"supportsMidConvoSystemMessages": true}));
        assert_eq!(
            mistral
                .mistral_compat()
                .unwrap()
                .supports_mid_convo_system_messages,
            Some(true)
        );

        let mut responses = base_model();
        responses.api = "openai-responses".to_string();
        responses.compat = Some(json!({"supportsDeveloperRole": false}));
        assert_eq!(
            responses
                .openai_responses_compat()
                .unwrap()
                .supports_developer_role,
            Some(false)
        );
    }

    #[test]
    fn compat_accessors_return_default_when_compat_missing() {
        let model = base_model();
        assert_eq!(
            model.openai_completions_compat().unwrap(),
            OpenAiCompletionsCompat::default()
        );
        assert_eq!(
            model.openai_responses_compat().unwrap(),
            OpenAiResponsesCompat::default()
        );
        assert_eq!(
            model.anthropic_compat().unwrap(),
            AnthropicMessagesCompat::default()
        );
        assert_eq!(model.bedrock_compat().unwrap(), BedrockCompat::default());
        assert_eq!(
            model.mistral_compat().unwrap(),
            MistralConversationsCompat::default()
        );
    }

    #[test]
    fn compat_accessors_error_on_invalid_compat_shape() {
        let mut model = base_model();

        model.compat = Some(json!({"maxTokensField": "bogus"}));
        assert!(model.openai_completions_compat().is_err());

        model.compat = Some(json!({"thinkingFormat": 42}));
        assert!(model.openai_completions_compat().is_err());

        // AnthropicAllowedFallbackModel has required fields upstream.
        model.compat = Some(json!({"allowedFallbackModels": [{"provider": "anthropic"}]}));
        assert!(model.anthropic_compat().is_err());
    }

    #[test]
    fn is_known_api_matches_known_list() {
        for known in KNOWN_API {
            let wire = serde_json::to_string(known).unwrap();
            let mut model = base_model();
            model.api = wire.trim_matches('"').to_string();
            assert!(model.is_known_api(), "{} must be known", model.api);
        }

        let mut custom = base_model();
        custom.api = "my-custom-api".to_string();
        assert!(!custom.is_known_api());
    }
}
