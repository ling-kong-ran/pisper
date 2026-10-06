//! Upstream `packages/ai/src/api/system-one-shared.ts`: the shared transport
//! for the System One classifier protocol (`classifySystemOne`), served by
//! TypeSafe natively and repackaged by OpenRouter, Cloudflare Workers AI, and
//! (via the providers' `classifiers` maps) Vercel AI Gateway and OpenCode
//! Zen.
//!
//! Wire contract (pinned against upstream captures in
//! `tests/fixtures/ai_delta_oracle/system-one/`): `POST` the transport URL
//! with `authorization: Bearer <key>` + `content-type: application/json`
//! (lowercase names, merged case-insensitively over the model's and the
//! request's headers), the body `{ model, state, questions }` with public
//! `bool` questions mapped to TypeSafe's wire-level `noul`, and the answer
//! parse of [`parse_answers`]. Usage (`{ input_tokens, output_tokens }`) is
//! priced from the model catalog like chat usage; a missing or malformed
//! usage object leaves the result without usage instead of failing it.
//!
//! Error-message composition: upstream throws `httpError` (message
//! `"{label} returned {status}"`, plus `status`/`body` fields that the
//! catch-all's `formatProviderError(normalizeProviderError(error), prefix)`
//! composes into `"{label} error ({status}): {body}"`). The port composes
//! that final string at the throw site (the [`ProviderError`] carries the
//! status and headers the retry policy reads); plain parse/shape failures and
//! the timeout error pass their bare messages through, exactly like
//! upstream's status-less path. The one disclosed micro-divergence: upstream
//! keeps the body out of the message when it is already contained in it —
//! unreachable here, where the thrown message is the fixed
//! `"{label} returned {status}"` literal.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::ai::cost::calculate_cost_any;
use crate::ai::retry::{retry_provider_request, ProviderError};
use crate::ai::types::classifier::{
    ClassifierAnswer, ClassifierBoolAnswer, ClassifierChoiceAnswer, ClassifierContext,
    ClassifierOptions, ClassifierQuestion, ClassifierResult, ClassifierScoreAnswer,
    ClassifierStopReason,
};
use crate::ai::types::model::ClassifierModel;
use crate::ai::types::model::ModelInput;
use crate::ai::types::options::ProviderHeaders;
use crate::ai::types::ordered_map::OrderedMap;
use crate::ai::types::primitives::{ModelCost, Usage};
use crate::ai::types::request_callbacks::ProviderResponse;
use crate::ai::types::Model;

/// Upstream `MAX_PROVIDER_ERROR_BODY_CHARS` (utils/error-body.ts:17).
const MAX_PROVIDER_ERROR_BODY_CHARS: usize = 4000;

/// Upstream `truncateErrorText` (utils/error-body.ts).
fn truncate_error_text(text: &str) -> String {
    let length = text.chars().count();
    if length <= MAX_PROVIDER_ERROR_BODY_CHARS {
        return text.to_string();
    }
    let truncated: String = text.chars().take(MAX_PROVIDER_ERROR_BODY_CHARS).collect();
    format!(
        "{truncated}... [truncated {} chars]",
        length - MAX_PROVIDER_ERROR_BODY_CHARS
    )
}

/// Upstream `SystemOneWireRequest` (system-one-shared.ts:14-17): the request
/// body without the transport-specific envelope.
#[derive(Debug, Clone)]
pub struct SystemOneWireRequest {
    pub state: Value,
    pub questions: BTreeMap<String, Value>,
}

/// Upstream `SystemOneTransport` (system-one-shared.ts:26-40): differences
/// between services that serve System One models.
pub trait SystemOneTransport: Send + Sync {
    /// Classifier API implemented by this transport.
    fn api(&self) -> &'static str;
    /// Service name used in error messages.
    fn label(&self) -> &'static str;
    /// Absolute request URL.
    fn url(&self, model: &ClassifierModel) -> String;
    /// Wraps the System One request in the service's request envelope.
    fn payload(&self, model: &ClassifierModel, request: &SystemOneWireRequest) -> Value;
    /// Extracts the System One output (`{ answers, usage }`) from the
    /// service's response envelope; `Err` carries the error message.
    fn output(&self, body: &Value) -> Result<Value, String>;
}

/// Upstream `isRecord` (system-one-shared.ts:56): a plain JSON object.
pub fn is_record(value: &Value) -> bool {
    value.is_object()
}

/// Upstream `requiredNumber`: a finite number or the typed parse error.
fn required_number(label: &str, value: &Value, field: &str) -> Result<f64, String> {
    match value.as_f64() {
        Some(value) if value.is_finite() => Ok(value),
        _ => Err(format!("{label} returned an invalid {field}")),
    }
}

/// Upstream `probabilities`: every entry a finite number.
fn probabilities(label: &str, value: &Value, id: &str) -> Result<BTreeMap<String, f64>, String> {
    let Some(map) = value.as_object() else {
        return Err(format!("{label} returned invalid probabilities for {id}"));
    };
    map.iter()
        .map(|(key, probability)| {
            Ok((
                key.clone(),
                required_number(label, probability, &format!("probability for {id}.{key}"))?,
            ))
        })
        .collect()
}

/// Upstream `parseAnswers` (system-one-shared.ts:81-123): one answer per
/// question id, typed by the question; the answers map is the port's
/// deterministic key order.
fn parse_answers(
    label: &str,
    value: &Value,
    context: &ClassifierContext,
) -> Result<BTreeMap<String, ClassifierAnswer>, String> {
    let Some(map) = value.as_object() else {
        return Err(format!("{label} returned an unexpected response"));
    };
    let mut answers: BTreeMap<String, ClassifierAnswer> = BTreeMap::new();
    for (id, question) in context.questions.iter() {
        let Some(answer) = map.get(id.as_str()) else {
            return Err(format!("{label} did not return an answer for {id}"));
        };
        match question {
            ClassifierQuestion::Choice(_) => {
                if answer.get("type").and_then(Value::as_str) != Some("choice")
                    || answer.get("choice").and_then(Value::as_str).is_none()
                {
                    return Err(format!("{label} did not return a choice answer for {id}"));
                }
                answers.insert(
                    id.clone(),
                    ClassifierAnswer::Choice(ClassifierChoiceAnswer {
                        choice: answer["choice"].as_str().unwrap_or_default().to_string(),
                        probabilities: probabilities(label, &answer["probabilities"], id)?
                            .into_iter()
                            .collect(),
                        confidence: required_number(
                            label,
                            &answer["confidence"],
                            &format!("confidence for {id}"),
                        )?,
                    }),
                );
            }
            ClassifierQuestion::Score(_) => {
                if answer.get("type").and_then(Value::as_str) != Some("score") {
                    return Err(format!("{label} did not return a score answer for {id}"));
                }
                answers.insert(
                    id.clone(),
                    ClassifierAnswer::Score(ClassifierScoreAnswer {
                        score: required_number(
                            label,
                            &answer["score"],
                            &format!("score for {id}"),
                        )?,
                        confidence: required_number(
                            label,
                            &answer["confidence"],
                            &format!("confidence for {id}"),
                        )?,
                    }),
                );
            }
            ClassifierQuestion::Bool(_) => {
                // Public `bool` questions map to TypeSafe's wire-level `noul`.
                if answer.get("type").and_then(Value::as_str) != Some("noul") {
                    return Err(format!("{label} did not return a bool answer for {id}"));
                }
                answers.insert(
                    id.clone(),
                    ClassifierAnswer::Bool(ClassifierBoolAnswer {
                        probability: required_number(
                            label,
                            &answer["noul"],
                            &format!("probability for {id}"),
                        )?,
                    }),
                );
            }
        }
    }
    Ok(answers)
}

/// Upstream `tokenCount`: a positive finite number, else 0.
fn token_count(value: &Value) -> u64 {
    match value.as_f64() {
        Some(value) if value.is_finite() && value > 0.0 => value as u64,
        _ => 0,
    }
}

/// Upstream `parseUsage` (system-one-shared.ts:133-152): usage from System
/// One's `{ input_tokens, output_tokens }`, priced from the model catalog
/// like chat usage.
fn parse_usage(value: &Value, model: &ClassifierModel) -> Option<Usage> {
    let is_usage_shape = value.is_object()
        && (value.get("input_tokens").is_some() || value.get("output_tokens").is_some());
    if !is_usage_shape {
        return None;
    }
    let input = token_count(&value["input_tokens"]);
    let output = token_count(&value["output_tokens"]);
    let mut usage = Usage {
        input,
        output,
        cache_read: 0,
        cache_write: 0,
        cache_write_1h: None,
        reasoning: None,
        total_tokens: input + output,
        cost: Default::default(),
    };
    calculate_cost_any(
        &crate::ai::types::AnyModel::Classifier(model.clone()),
        &mut usage,
    );
    Some(usage)
}

/// Upstream `wireRequest` (system-one-shared.ts:155-166): map public `bool`
/// questions to TypeSafe's wire-level `noul` type.
pub fn wire_request(context: &ClassifierContext) -> SystemOneWireRequest {
    SystemOneWireRequest {
        state: context.state.clone(),
        questions: context
            .questions
            .iter()
            .map(|(id, question)| {
                let mut wire = serde_json::to_value(question).unwrap_or(Value::Null);
                if matches!(question, ClassifierQuestion::Bool(_)) {
                    if let Some(object) = wire.as_object_mut() {
                        object.insert("type".to_string(), json!("noul"));
                    }
                }
                (id.clone(), wire)
            })
            .collect(),
    }
}

/// Upstream `providerHeadersToRecord` (utils/headers.ts), the variadic
/// case-insensitive merge: later sources replace earlier ones per lowercase
/// name (a `None` value, upstream `null`, deletes), and `None` when nothing
/// survives. Key order is the port's sorted-map carrier.
pub(crate) fn provider_headers_to_record(sources: &[&ProviderHeaders]) -> Option<ProviderHeaders> {
    let mut merged: BTreeMap<String, Option<String>> = BTreeMap::new();
    for source in sources {
        for (name, value) in source.iter() {
            let normalized = name.to_lowercase();
            merged.remove(&normalized);
            if value.is_some() {
                merged.insert(normalized, value.clone());
            }
        }
    }
    if merged.is_empty() {
        None
    } else {
        Some(merged)
    }
}

/// Upstream `requestHeaders` (system-one-shared.ts:169-179): the auth/content
/// defaults, then the model headers, then the request headers.
pub(crate) fn request_headers(
    model: &ClassifierModel,
    api_key: &str,
    options_headers: Option<&ProviderHeaders>,
) -> ProviderHeaders {
    let defaults: ProviderHeaders = [
        (
            "authorization".to_string(),
            Some(format!("Bearer {api_key}")),
        ),
        (
            "content-type".to_string(),
            Some("application/json".to_string()),
        ),
    ]
    .into_iter()
    .collect();
    let empty = ProviderHeaders::new();
    provider_headers_to_record(&[
        &defaults,
        model.headers.as_ref().unwrap_or(&empty),
        options_headers.unwrap_or(&empty),
    ])
    .unwrap_or_default()
}

/// The process-local [`RequestCallbacks`] are typed over the chat
/// [`Model`]; the one-shot operations hand the hooks a catalog-view of the
/// shared fields (id/name/api/provider/baseUrl), the same identity fields
/// the extension bridge reads.
pub(crate) fn model_view(
    id: &str,
    name: &str,
    api: &str,
    provider: &str,
    base_url: &str,
    headers: Option<ProviderHeaders>,
) -> Model {
    Model {
        id: id.to_string(),
        name: name.to_string(),
        api: api.to_string(),
        provider: provider.to_string(),
        base_url: base_url.to_string(),
        r#type: None,
        reasoning: false,
        thinking_level_map: None,
        prompt_cache: None,
        input: vec![ModelInput::Text],
        input_limits: None,
        cost: ModelCost::default(),
        context_window: 0,
        max_tokens: 0,
        sampling_params: None,
        sampling_params_by_thinking_level: None,
        headers,
        compat: None,
    }
}

/// Upstream `classifySystemOne` (system-one-shared.ts:182-237): run one
/// System One classification over the given transport. Never rejects — every
/// failure path returns the error [`ClassifierResult`].
pub async fn classify_system_one(
    transport: &dyn SystemOneTransport,
    model: &ClassifierModel,
    context: &ClassifierContext,
    options: &ClassifierOptions,
) -> ClassifierResult {
    let mut output = ClassifierResult {
        api: model.api.clone(),
        provider: model.provider.clone(),
        model: model.id.clone(),
        answers: OrderedMap::new(),
        usage: None,
        stop_reason: ClassifierStopReason::Stop,
        error_message: None,
        timestamp: crate::ai::now_ms(),
    };

    let run = async {
        if model.api != transport.api() {
            return Err(ProviderError::transport(format!(
                "Unsupported classifier API: {}",
                model.api
            )));
        }
        let Some(api_key) = options.api_key.clone().filter(|key| !key.is_empty()) else {
            return Err(ProviderError::transport(format!(
                "No API key for provider: {}",
                model.provider
            )));
        };
        let mut payload = transport.payload(model, &wire_request(context));
        // options.onPayload: the payload hook may replace the body.
        let hook_model = model_view(
            &model.id,
            &model.name,
            &model.api,
            &model.provider,
            &model.base_url,
            model.headers.clone(),
        );
        payload = options
            .callbacks
            .payload(payload, &hook_model)
            .await
            .map_err(|error| ProviderError::transport(error.to_string()))?;
        let url = transport.url(model);
        let headers = request_headers(model, &api_key, options.headers.as_ref());
        let max_retries = options.max_retries.unwrap_or(2);
        let label = transport.label();

        let send = || {
            let url = url.clone();
            let headers = headers.clone();
            let payload = payload.clone();
            async move {
                let request = crate::ai::api::http_client()
                    .post(&url)
                    .headers(
                        headers
                            .iter()
                            .filter_map(|(name, value)| {
                                let name =
                                    reqwest::header::HeaderName::try_from(name.as_str()).ok()?;
                                let value =
                                    reqwest::header::HeaderValue::from_str(value.as_deref()?)
                                        .ok()?;
                                Some((name, value))
                            })
                            .collect::<reqwest::header::HeaderMap>(),
                    )
                    .json(&payload)
                    .send();
                // Upstream composes `AbortSignal.any([options.signal,
                // timeoutSignal])`; the port layers `tokio::time::timeout`
                // (an elapse is the TimeoutError) over the shared client and
                // lets the caller's token guard the retries.
                let started = std::time::Instant::now();
                let response = match options.timeout_ms.map(std::time::Duration::from_millis) {
                    Some(timeout) => match tokio::time::timeout(timeout, request).await {
                        Ok(response) => response,
                        Err(_) => {
                            return Err(ProviderError::transport(format!(
                                "Request timed out after {}ms",
                                timeout.as_millis()
                            )))
                        }
                    },
                    None => request.await,
                }
                .map_err(|error| ProviderError::transport(error.to_string()))?;
                let _ = started;
                let status = response.status().as_u16();
                let response_headers = response.headers().clone();
                let body = response.text().await.unwrap_or_default();
                if !(200..300).contains(&status) {
                    // Upstream `httpError(label, response, await next.text())`
                    // with the catch-all composing
                    // `"{label} error ({status}): {body}"` (body trimmed and
                    // truncated to the 4000-char cap).
                    let body = truncate_error_text(body.trim());
                    return Err(ProviderError::http(
                        status,
                        response_headers,
                        if body.is_empty() {
                            format!("{label} error ({status}): {label} returned {status}")
                        } else {
                            format!("{label} error ({status}): {body}")
                        },
                    ));
                }
                let parsed: Value = serde_json::from_str(&body)
                    .map_err(|error| ProviderError::transport(error.to_string()))?;
                Ok((status, response_headers, parsed))
            }
        };

        let (status, response_headers, body) = retry_provider_request(
            max_retries,
            options.max_retry_delay_ms,
            options.signal.as_ref(),
            send,
        )
        .await?;
        // options.onResponse: the response-status observer.
        options
            .callbacks
            .response(
                ProviderResponse {
                    status,
                    headers: response_headers
                        .iter()
                        .map(|(name, value)| {
                            (
                                name.as_str().to_owned(),
                                String::from_utf8_lossy(value.as_bytes()).into_owned(),
                            )
                        })
                        .collect(),
                },
                &hook_model,
            )
            .await
            .map_err(|error| ProviderError::transport(error.to_string()))?;
        let result = transport.output(&body).map_err(ProviderError::transport)?;
        // Set before parsing answers: a request with malformed answers was
        // still billed.
        if let Some(usage) = result.get("usage") {
            output.usage = parse_usage(usage, model);
        }
        output.answers = parse_answers(
            label,
            result.get("answers").unwrap_or(&Value::Null),
            context,
        )
        .map_err(ProviderError::transport)?
        .into_iter()
        .collect();
        Ok(())
    }
    .await;

    if let Err(error) = run {
        output.stop_reason = if options
            .signal
            .as_ref()
            .is_some_and(|token| token.is_cancelled())
        {
            ClassifierStopReason::Aborted
        } else {
            ClassifierStopReason::Error
        };
        // Upstream `formatProviderError(normalizeProviderError(error),
        // `${label} error`)`: the HTTP path composes status+body at the throw
        // site (see the module docs); everything else surfaces its message.
        output.error_message = Some(error.message);
    }
    output
}
