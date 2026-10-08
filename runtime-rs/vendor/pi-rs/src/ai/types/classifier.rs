//! Classifier types from upstream `packages/ai/src/types.ts` (the
//! `classify()` surface introduced with the unified model-catalog
//! infrastructure): [`ClassifierContext`], [`ClassifierQuestion`],
//! [`ClassifierAnswer`], [`ClassifierResult`], [`ClassifierOptions`], and the
//! [`ProviderClassifier`] contract.
//!
//! Wire format matches the TypeScript interfaces field-for-field (camelCase,
//! optional fields omitted when `None`). Map-valued fields
//! (`questions`, `state`, answer `probabilities`) are `BTreeMap`s for
//! deterministic key order — the port-wide precedent for upstream
//! `Record<string, T>` (JS insertion order is not representable; the model
//! set and values are identical).

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use super::options::{ProviderEnv, ProviderHeaders};
use super::ordered_map::OrderedMap;
use super::primitives::Usage;

/// Upstream `ClassifierChoiceQuestion` (types.ts): pick one of the criteria
/// keys. The `type: "choice"` discriminator is carried by the
/// [`ClassifierQuestion`] tag.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassifierChoiceQuestion {
    pub instructions: String,
    pub criteria: OrderedMap<String>,
}

/// Upstream `ClassifierScoreQuestion` (types.ts): rate on the ordered levels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassifierScoreQuestion {
    pub instructions: String,
    pub criteria: Vec<String>,
}

/// Upstream `ClassifierBoolQuestion` (types.ts): yes/no with the two meanings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassifierBoolQuestion {
    pub instructions: String,
    pub criteria: ClassifierBoolCriteria,
}

/// Upstream `ClassifierBoolQuestion.criteria`: `{ true: string; false:
/// string }`. Both sides are optional upstream (`string | undefined` via the
/// render sites); the interface declares them required, so both are required
/// here and serialized even when empty.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub struct ClassifierBoolCriteria {
    #[serde(default)]
    pub r#true: String,
    #[serde(default)]
    pub r#false: String,
}

/// Upstream `ClassifierQuestion` (types.ts): the `type`-tagged union.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ClassifierQuestion {
    Choice(ClassifierChoiceQuestion),
    Score(ClassifierScoreQuestion),
    Bool(ClassifierBoolQuestion),
}

impl ClassifierQuestion {
    /// The `type` discriminator value.
    pub fn tag(&self) -> &'static str {
        match self {
            ClassifierQuestion::Choice(_) => "choice",
            ClassifierQuestion::Score(_) => "score",
            ClassifierQuestion::Bool(_) => "bool",
        }
    }
}

/// Upstream `ClassifierContext` (types.ts): the state to judge plus the
/// questions to answer about it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassifierContext {
    /// Free-form JSON object. Upstream `JsonObject`; `serde_json::Value` is
    /// the port's open-ended object carrier.
    pub state: serde_json::Value,
    pub questions: OrderedMap<ClassifierQuestion>,
}

impl Default for ClassifierContext {
    fn default() -> Self {
        ClassifierContext {
            state: serde_json::Value::Null,
            questions: OrderedMap::new(),
        }
    }
}

/// Upstream `ClassifierChoiceAnswer` (types.ts).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassifierChoiceAnswer {
    pub choice: String,
    #[serde(with = "js_number_map")]
    pub probabilities: OrderedMap<f64>,
    #[serde(with = "crate::ai::types::primitives::js_number")]
    pub confidence: f64,
}

/// Upstream `ClassifierScoreAnswer` (types.ts).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassifierScoreAnswer {
    #[serde(with = "crate::ai::types::primitives::js_number")]
    pub score: f64,
    #[serde(with = "crate::ai::types::primitives::js_number")]
    pub confidence: f64,
}

/// Upstream `ClassifierBoolAnswer` (types.ts): the probability of `true`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassifierBoolAnswer {
    #[serde(with = "crate::ai::types::primitives::js_number")]
    pub probability: f64,
}

/// Upstream `ClassifierAnswer` (types.ts): the `type`-tagged union.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ClassifierAnswer {
    Choice(ClassifierChoiceAnswer),
    Score(ClassifierScoreAnswer),
    Bool(ClassifierBoolAnswer),
}

/// Upstream `ClassifierStopReason` (types.ts): `"stop" | "error" | "aborted"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClassifierStopReason {
    Stop,
    Error,
    Aborted,
}

/// Upstream `ClassifierResult` (types.ts): one classification run's answers
/// and accounting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassifierResult {
    pub api: String,
    pub provider: String,
    pub model: String,
    pub answers: OrderedMap<ClassifierAnswer>,
    /// Token usage and its cost at the model's catalog price, when the
    /// service reports token counts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    pub stop_reason: ClassifierStopReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    /// Unix timestamp in milliseconds.
    pub timestamp: i64,
}

/// Upstream `ClassifierOptions` (types.ts): the request options accepted by
/// every classifier entry point — the
/// [`ProviderRequestOptions`](super::options::ProviderRequestOptions) fields
/// (flattened here like the chat [`StreamOptions`](super::options::StreamOptions)
/// and image [`ImagesOptions`](super::images::ImagesOptions)) plus
/// `temperature`.
///
/// Port deviations, mirroring the chat/image options modules:
/// - `signal` carries a non-serialized [`CancellationToken`].
/// - `telemetryContext`/`fetch` are not ported (same deferral as chat);
///   lifecycle callbacks ride on [`RequestCallbacks`](super::request_callbacks::RequestCallbacks).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassifierOptions {
    /// Process-local lifecycle callbacks; never part of the wire
    /// representation.
    #[serde(skip)]
    pub callbacks: super::request_callbacks::RequestCallbacks,
    /// Caller cancellation (upstream `signal`, non-serialized port channel).
    #[serde(skip)]
    pub signal: Option<CancellationToken>,
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
    /// Divides the answer logits by this value before they are normalized
    /// into probabilities. Values above 1 soften the distribution; values
    /// below 1 sharpen it. Must be positive. APIs that cannot apply it
    /// ignore it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
}

/// JSON-map serializer whose f64 values take the JS-number wire format
/// (whole numbers emit as integers), matching upstream number encoding.
mod js_number_map {
    use super::super::ordered_map::OrderedMap;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::collections::BTreeMap;

    pub(crate) fn serialize<S: Serializer>(
        value: &OrderedMap<f64>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let with_js_numbers: BTreeMap<&String, JsNumber> = value
            .iter()
            .map(|(key, number)| (key, JsNumber(*number)))
            .collect();
        with_js_numbers.serialize(serializer)
    }

    pub(crate) fn deserialize<'de: 'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<OrderedMap<f64>, D::Error> {
        let raw = BTreeMap::<String, f64>::deserialize(deserializer)?;
        Ok(raw.into_iter().collect())
    }

    #[derive(Serialize)]
    struct JsNumber(#[serde(with = "crate::ai::types::primitives::js_number")] f64);
}
