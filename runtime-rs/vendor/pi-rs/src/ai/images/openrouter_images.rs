//! The openrouter-images API implementation and the built-in openrouter
//! image provider, ported from upstream `packages/ai/src/api/openrouter-images.ts`
//! and `packages/ai/src/providers/openrouter-images.ts`.
//!
//! Upstream drives the OpenAI SDK client against
//! `{baseUrl}/chat/completions` with `stream: false` and an image/text
//! `modalities` array; the port issues the same request with `reqwest`
//! (same M2b substitution as every chat API impl) and parses the same
//! response shape: message text plus `message.images[].image_url` data URLs.
//!
//! Port deviations, mirroring the chat API modules:
//! - `options.fetch` has no equivalent; `reqwest` is the transport.
//! - `options.onPayload`/`onResponse` (the M2a deferral) are not ported.
//! - `sanitizeSurrogates` (utils/sanitize-unicode.ts) is a no-op here: Rust
//!   `String` cannot hold the lone surrogates the upstream sanitizer strips.
//! - Error formatting reuses the shared openai-SDK-shaped
//!   `format_http_error` composer (`"{status}: {body}"`, body truncated)
//!   instead of `formatProviderError(normalizeProviderError(error))` on the
//!   SDK's error object — the same message shape for HTTP failures.
//! - The abort `signal` is the port's non-serialized
//!   [`CancellationToken`](tokio_util::sync::CancellationToken) on
//!   [`ImagesOptions`]; a pre-cancelled or mid-flight cancellation produces
//!   the upstream aborted result (`"Request aborted"`).

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::ai::api::openai_completions::stream::format_http_error;
use crate::ai::retry::{retry_provider_request, ProviderError};
use crate::ai::types::content::{ImageContent, TextContent};
use crate::ai::types::images::{AssistantImages, ImagesContext, ImagesOptions, ImagesStopReason};
use crate::ai::types::message::TextOrImageBlock;
use crate::ai::types::model::ImageModel;
use crate::ai::types::primitives::{Usage, UsageCost};

use super::registry::ImagesApiFn;

/// Upstream `ImageModel.api` for this implementation
/// (`KnownImageApi`, types.ts).
pub const OPENROUTER_IMAGES_API: &str = "openrouter-images";

/// Upstream `generateImages` (api/openrouter-images.ts:40-115).
pub async fn generate_images(
    model: &ImageModel,
    context: &ImagesContext,
    options: Option<&ImagesOptions>,
) -> AssistantImages {
    let mut output = AssistantImages {
        api: model.api.clone(),
        provider: model.provider.clone(),
        model: model.id.clone(),
        output: Vec::new(),
        response_id: None,
        usage: None,
        stop_reason: ImagesStopReason::Stop,
        error_message: None,
        timestamp: crate::ai::now_ms(),
    };

    let run = async {
        let Some(api_key) = options.and_then(|options| options.api_key.clone()) else {
            return Err(format!("No API key for provider: {}", model.provider));
        };
        let signal = options.and_then(|options| options.signal.clone());

        let params = build_params(model, context);
        let url = format!("{}/chat/completions", model.base_url.trim_end_matches('/'));
        let default_headers = default_headers(model, options);

        let max_retries = options.and_then(|options| options.max_retries).unwrap_or(0);
        let max_retry_delay_ms = options.and_then(|options| options.max_retry_delay_ms);
        let timeout = options
            .and_then(|options| options.timeout_ms)
            .map(Duration::from_millis);

        // Upstream wraps the SDK call in retryProviderRequest with
        // maxRetries: 0 on the client and options.maxRetries on the retry
        // helper (api/openrouter-images.ts:65-80).
        let response =
            retry_provider_request(max_retries, max_retry_delay_ms, signal.as_ref(), || {
                send_request(
                    url.as_str(),
                    &api_key,
                    default_headers.as_ref(),
                    &params,
                    timeout,
                    signal.as_ref(),
                )
            })
            .await
            .map_err(|error| error.to_string())?;

        let image_response: Value =
            serde_json::from_str(&response).map_err(|error| error.to_string())?;

        output.response_id = image_response
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some(raw_usage) = image_response.get("usage").filter(|usage| !usage.is_null()) {
            output.usage = Some(parse_usage(raw_usage, model));
        }

        if let Some(choice) = image_response
            .pointer("/choices/0")
            .filter(|choice| !choice.is_null())
        {
            // Non-empty string message content becomes a text block
            // (api/openrouter-images.ts:91-94).
            if let Some(text) = choice.pointer("/message/content").and_then(Value::as_str) {
                if !text.is_empty() {
                    output.output.push(TextOrImageBlock::Text(TextContent {
                        text: text.to_string(),
                        text_signature: None,
                    }));
                }
            }

            // message.images[].image_url (string or {url}) — only data URLs
            // become image blocks (api/openrouter-images.ts:96-106).
            let images = choice
                .pointer("/message/images")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for image in images {
                let image_url = match image.get("image_url") {
                    Some(Value::String(url)) => Some(url.clone()),
                    Some(other) => other.get("url").and_then(Value::as_str).map(str::to_string),
                    None => None,
                };
                let Some(image_url) = image_url.filter(|url| url.starts_with("data:")) else {
                    continue;
                };
                let Some((mime_type, data)) = parse_data_url(&image_url) else {
                    continue;
                };
                output
                    .output
                    .push(TextOrImageBlock::Image(ImageContent { data, mime_type }));
            }
        }

        Ok(())
    };

    match run.await {
        Ok(()) => output,
        Err(message) => {
            let aborted = options
                .and_then(|options| options.signal.as_ref())
                .is_some_and(CancellationToken::is_cancelled);
            output.stop_reason = if aborted {
                ImagesStopReason::Aborted
            } else {
                ImagesStopReason::Error
            };
            output.error_message = Some(message);
            output
        }
    }
}

/// Upstream `Options.signal?.aborted ? "aborted" : "error"` outcome text:
/// cancellation maps to the SDK's `"Request aborted"` error message.
const REQUEST_ABORTED: &str = "Request aborted";

/// One retryable request attempt: POST the completion, returning the body
/// text. Cancellation during the request surfaces as a transport error
/// carrying [`REQUEST_ABORTED`].
async fn send_request(
    url: &str,
    api_key: &str,
    default_headers: Option<&crate::ai::types::options::ProviderHeaders>,
    params: &Value,
    timeout: Option<Duration>,
    signal: Option<&CancellationToken>,
) -> Result<String, ProviderError> {
    if signal.is_some_and(CancellationToken::is_cancelled) {
        return Err(ProviderError::transport(REQUEST_ABORTED));
    }
    let mut request = crate::ai::http_client().post(url).bearer_auth(api_key);
    if let Some(default_headers) = default_headers {
        for (name, value) in default_headers {
            if let Some(value) = value {
                request = request.header(name.as_str(), value.as_str());
            }
        }
    }
    if let Some(timeout) = timeout {
        request = request.timeout(timeout);
    }
    let future = request.json(params).send();
    let response = match signal {
        Some(signal) => tokio::select! {
            response = future => response.map_err(|error| ProviderError::transport(error.to_string()))?,
            _ = signal.cancelled() => return Err(ProviderError::transport(REQUEST_ABORTED)),
        },
        None => future
            .await
            .map_err(|error| ProviderError::transport(error.to_string()))?,
    };
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| ProviderError::transport(error.to_string()))?;
    if !status.is_success() {
        // Upstream formatProviderError over the SDK's APIError:
        // `"{status}: {body}"` with the truncated body.
        return Err(ProviderError::http(
            status.as_u16(),
            reqwest::header::HeaderMap::new(),
            format_http_error(status.as_u16(), &body),
        ));
    }
    Ok(body)
}

/// Upstream `createClient`'s `defaultHeaders: providerHeadersToRecord({
/// ...model.headers, ...options.headers })` (api/openrouter-images.ts:117-130):
/// the model's static headers under the explicit request headers. `None`
/// header values (upstream `null`) suppress the model default.
fn default_headers(
    model: &ImageModel,
    options: Option<&ImagesOptions>,
) -> Option<crate::ai::types::options::ProviderHeaders> {
    let model_headers = model
        .headers
        .as_ref()
        .filter(|headers| !headers.is_empty())?;
    let mut merged: crate::ai::types::options::ProviderHeaders = model_headers.clone();
    if let Some(options_headers) = options.and_then(|options| options.headers.as_ref()) {
        for (name, value) in options_headers {
            // Case-insensitive replacement of the model default, then insert
            // (providerHeadersToRecord folds case-insensitively upstream).
            let lowercase = name.to_lowercase();
            let replaced: Vec<String> = merged
                .keys()
                .filter(|existing| existing.to_lowercase() == lowercase)
                .cloned()
                .collect();
            for existing in replaced {
                merged.remove(&existing);
            }
            merged.insert(name.clone(), value.clone());
        }
    }
    Some(merged)
}

/// Upstream `OpenRouterImagesCreateParams` + `buildParams`
/// (api/openrouter-images.ts:132-163).
fn build_params(model: &ImageModel, context: &ImagesContext) -> Value {
    let content: Vec<Value> = context
        .input
        .iter()
        .map(|item| match item {
            TextOrImageBlock::Text(text) => json!({"type": "text", "text": text.text}),
            TextOrImageBlock::Image(image) => json!({
                "type": "image_url",
                "image_url": {"url": format!("data:{};base64,{}", image.mime_type, image.data)},
            }),
        })
        .collect();

    json!({
        "model": model.id,
        "messages": [{"role": "user", "content": content}],
        "stream": false,
        "modalities": if model.output.contains(&crate::ai::types::model::ModelInput::Text) {
            vec!["image", "text"]
        } else {
            vec!["image"]
        },
    })
}

/// Upstream `image.image_url.match(/^data:([^;]+);base64,(.+)$/)`:
/// `(mimeType, data)` from a data URL, `None` when the shape differs. The
/// regex's `(.+)` requires at least one data byte, so an empty base64 payload
/// (`data:image/png;base64,`) does not match upstream and is rejected here too.
fn parse_data_url(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("data:")?;
    let (mime_type, data) = rest.split_once(";base64,")?;
    if mime_type.is_empty() || mime_type.contains(';') || data.is_empty() {
        return None;
    }
    Some((mime_type.to_string(), data.to_string()))
}

/// Upstream `parseUsage` (api/openrouter-images.ts:165-196): split the
/// prompt tokens across cache read/write and price the request.
fn parse_usage(raw_usage: &Value, model: &ImageModel) -> Usage {
    let prompt_tokens = raw_usage
        .get("prompt_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let reported_cached_tokens = raw_usage
        .pointer("/prompt_tokens_details/cached_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_write_tokens = raw_usage
        .pointer("/prompt_tokens_details/cache_write_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_read_tokens = if cache_write_tokens > 0 {
        reported_cached_tokens.saturating_sub(cache_write_tokens)
    } else {
        reported_cached_tokens
    };
    let input = prompt_tokens
        .saturating_sub(cache_read_tokens)
        .saturating_sub(cache_write_tokens);
    let output_tokens = raw_usage
        .get("completion_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let mut usage = Usage {
        input,
        output: output_tokens,
        cache_read: cache_read_tokens,
        cache_write: cache_write_tokens,
        cache_write_1h: None,
        reasoning: None,
        total_tokens: input + output_tokens + cache_read_tokens + cache_write_tokens,
        cost: UsageCost {
            input: model.cost.input / 1_000_000.0 * input as f64,
            output: model.cost.output / 1_000_000.0 * output_tokens as f64,
            cache_read: model.cost.cache_read / 1_000_000.0 * cache_read_tokens as f64,
            cache_write: model.cost.cache_write / 1_000_000.0 * cache_write_tokens as f64,
            total: 0.0,
        },
    };
    usage.cost.total =
        usage.cost.input + usage.cost.output + usage.cost.cache_read + usage.cost.cache_write;
    usage
}

/// Upstream `openrouterImagesApi` (api/openrouter-images.lazy.ts) folded
/// with `generateImages`: the registry handler for `openrouter-images`
/// (there is no lazy module load to port — the implementation is linked).
pub fn images_api_fn() -> ImagesApiFn {
    Arc::new(
        |model: ImageModel, context: ImagesContext, options: Option<ImagesOptions>| {
            Box::pin(async move { Ok(generate_images(&model, &context, options.as_ref()).await) })
        },
    )
}

/// Upstream `images: { "openrouter-images": openrouterImagesApi() }` on the
/// OpenRouter chat provider (providers/openrouter-images.ts was deleted by
/// #9948; its capability moved onto the chat provider's `images` map): the
/// [`ImagesApiImpl`](crate::ai::models::provider::ImagesApiImpl) adapter the
/// `Models.generateImages` routing dispatches to. Resolved auth rides in the
/// routed [`ProviderConfig`](crate::ai::ProviderConfig) (base-URL override) and
/// the options (api key), exactly what upstream passes as `requestModel` +
/// `requestOptions`.
pub struct OpenRouterImagesApi;

impl crate::ai::models::provider::ImagesApiImpl for OpenRouterImagesApi {
    fn generate_images(
        &self,
        _config: &crate::ai::ProviderConfig,
        model: &ImageModel,
        context: &ImagesContext,
        options: &ImagesOptions,
    ) -> futures::future::BoxFuture<
        'static,
        Result<AssistantImages, crate::ai::auth::resolve::ModelsError>,
    > {
        let model = model.clone();
        let context = context.clone();
        let options = options.clone();
        Box::pin(async move { Ok(generate_images(&model, &context, Some(&options)).await) })
    }
}

#[cfg(test)]
mod tests {
    use super::OPENROUTER_IMAGES_API;
    use super::*;
    use crate::ai::types::model::ModelInput;
    use std::collections::BTreeMap;

    fn model(output: &[ModelInput], headers: Option<&[(&str, &str)]>) -> ImageModel {
        ImageModel {
            id: "google/gemini-3.1-flash-image-preview".to_string(),
            name: "Gemini 3.1 Flash Image Preview".to_string(),
            api: OPENROUTER_IMAGES_API.to_string(),
            provider: "openrouter".to_string(),
            base_url: "https://example.test/api/v1".to_string(),
            input: vec![ModelInput::Text, ModelInput::Image],
            input_limits: None,
            cost: crate::ai::types::primitives::ModelCost {
                input: 0.015,
                output: 0.03,
                cache_read: 0.0,
                cache_write: 0.0,
                tiers: None,
            },
            headers: headers.map(|headers| {
                headers
                    .iter()
                    .map(|(k, v)| (k.to_string(), Some(v.to_string())))
                    .collect::<BTreeMap<_, _>>()
            }),
            r#type: crate::ai::types::model::ModelType::Image,
            output: output.to_vec(),
        }
    }

    fn text_context(text: &str) -> ImagesContext {
        ImagesContext {
            input: vec![TextOrImageBlock::Text(TextContent {
                text: text.to_string(),
                text_signature: None,
            })],
        }
    }

    const SUCCESS_BODY: &str = r#"{
        "id": "img-1",
        "usage": {"prompt_tokens": 12, "completion_tokens": 34, "prompt_tokens_details": {"cached_tokens": 0}},
        "choices": [{
            "message": {
                "content": "Here is your image.",
                "images": [{"image_url": "data:image/png;base64,ZmFrZS1wbmc="}]
            }
        }]
    }"#;

    /// Oracle openrouter-images.test.ts: "returns text plus images in final
    /// output" — output shape, responseId, usage parse, and the request
    /// params (stream false, modalities, message content).
    #[tokio::test]
    async fn returns_text_plus_images_in_final_output() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/api/v1/chat/completions"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(SUCCESS_BODY))
            .expect(1)
            .mount(&server)
            .await;

        let mut model = model(
            &[ModelInput::Text, ModelInput::Image],
            Some(&[("HTTP-Referer", "https://example.com")]),
        );
        model.base_url = format!("{}/api/v1", server.uri());
        let output = generate_images(
            &model,
            &text_context("Generate a dog"),
            Some(&ImagesOptions {
                api_key: Some("test".to_string()),
                ..ImagesOptions::default()
            }),
        )
        .await;

        assert_eq!(output.stop_reason, ImagesStopReason::Stop, "{output:?}");
        assert_eq!(output.response_id.as_deref(), Some("img-1"));
        assert_eq!(output.output.len(), 2);
        assert_eq!(
            output.output[0],
            TextOrImageBlock::Text(TextContent {
                text: "Here is your image.".to_string(),
                text_signature: None,
            })
        );
        assert_eq!(
            output.output[1],
            TextOrImageBlock::Image(ImageContent {
                data: "ZmFrZS1wbmc=".to_string(),
                mime_type: "image/png".to_string(),
            })
        );
        // usage: 12 prompt tokens uncached, 34 completion tokens.
        let usage = output.usage.unwrap();
        assert_eq!(usage.input, 12);
        assert_eq!(usage.output, 34);
        assert_eq!(usage.cache_read, 0);
        assert_eq!(usage.total_tokens, 46);
        assert_eq!(usage.cost.input, 0.015 / 1_000_000.0 * 12.0);
        assert_eq!(usage.cost.output, 0.03 / 1_000_000.0 * 34.0);
        assert_eq!(usage.cost.total, usage.cost.input + usage.cost.output);

        // Request shape: stream false, modalities, user text, and the merged
        // model header plus bearer key.
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0]
                .headers
                .get("authorization")
                .and_then(|value| value.to_str().ok()),
            Some("Bearer test")
        );
        assert_eq!(
            requests[0]
                .headers
                .get("http-referer")
                .and_then(|value| value.to_str().ok()),
            Some("https://example.com")
        );
        let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(body.get("stream"), Some(&json!(false)));
        assert_eq!(body.get("modalities"), Some(&json!(["image", "text"])));
        assert_eq!(
            body.get("model"),
            Some(&json!("google/gemini-3.1-flash-image-preview"))
        );
        assert_eq!(
            body.pointer("/messages/0/content/0"),
            Some(&json!({"type": "text", "text": "Generate a dog"}))
        );
    }

    /// Oracle: "passes through abort signal and returns aborted result" — a
    /// pre-cancelled token yields the aborted result without a request.
    #[tokio::test]
    async fn aborted_signal_returns_aborted_result() {
        let server = wiremock::MockServer::start().await;
        let model = model(&[ModelInput::Image], None);
        let token = CancellationToken::new();
        token.cancel();

        let output = generate_images(
            &model,
            &text_context("Generate a dog"),
            Some(&ImagesOptions {
                api_key: Some("test".to_string()),
                signal: Some(token),
                ..ImagesOptions::default()
            }),
        )
        .await;

        assert_eq!(output.stop_reason, ImagesStopReason::Aborted);
        assert_eq!(output.error_message.as_deref(), Some("Request aborted"));
        assert!(server
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty());
    }

    /// Oracle: "generateImages resolves the final assistant images result".
    #[tokio::test]
    async fn resolves_the_final_result_with_an_image() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(SUCCESS_BODY))
            .mount(&server)
            .await;
        let mut model = model(&[ModelInput::Image], None);
        model.base_url = server.uri();

        let output = generate_images(
            &model,
            &text_context("Generate a dog"),
            Some(&ImagesOptions {
                api_key: Some("test".to_string()),
                ..ImagesOptions::default()
            }),
        )
        .await;
        assert_eq!(output.stop_reason, ImagesStopReason::Stop);
        assert!(
            output
                .output
                .iter()
                .any(|item| matches!(item, TextOrImageBlock::Image(_))),
            "{output:?}"
        );
        // Image-only output requests modalities: ["image"].
        let requests = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(body.get("modalities"), Some(&json!(["image"])));
    }

    /// Missing api key: the upstream pre-flight error result.
    #[tokio::test]
    async fn missing_api_key_yields_error_result() {
        let model = model(&[ModelInput::Image], None);
        let output = generate_images(&model, &text_context("go"), None).await;
        assert_eq!(output.stop_reason, ImagesStopReason::Error);
        assert_eq!(
            output.error_message.as_deref(),
            Some("No API key for provider: openrouter")
        );
    }

    /// Image input round-trips into the request as a data URL part.
    #[tokio::test]
    async fn image_input_becomes_a_data_url_part() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(SUCCESS_BODY))
            .mount(&server)
            .await;
        let mut model = model(&[ModelInput::Image], None);
        model.base_url = server.uri();

        let context = ImagesContext {
            input: vec![
                TextOrImageBlock::Text(TextContent {
                    text: "Create a variation".to_string(),
                    text_signature: None,
                }),
                TextOrImageBlock::Image(ImageContent {
                    data: "aGk=".to_string(),
                    mime_type: "image/png".to_string(),
                }),
            ],
        };
        generate_images(
            &model,
            &context,
            Some(&ImagesOptions {
                api_key: Some("test".to_string()),
                ..ImagesOptions::default()
            }),
        )
        .await;

        let requests = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(
            body.pointer("/messages/0/content/1"),
            Some(&json!({
                "type": "image_url",
                "image_url": {"url": "data:image/png;base64,aGk="},
            }))
        );
    }

    /// HTTP failures carry the formatted status+body message.
    #[tokio::test]
    async fn http_error_yields_formatted_error_result() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(wiremock::ResponseTemplate::new(401).set_body_string("bad key"))
            .mount(&server)
            .await;
        let mut model = model(&[ModelInput::Image], None);
        model.base_url = server.uri();

        let output = generate_images(
            &model,
            &text_context("go"),
            Some(&ImagesOptions {
                api_key: Some("test".to_string()),
                ..ImagesOptions::default()
            }),
        )
        .await;
        assert_eq!(output.stop_reason, ImagesStopReason::Error);
        assert_eq!(output.error_message.as_deref(), Some("401: bad key"));
    }

    /// Non-data image URLs are skipped (upstream `startsWith("data:")`); an
    /// empty base64 payload fails upstream's `(.+)` and is rejected too.
    #[test]
    fn parse_data_url_matches_upstream_regex() {
        assert_eq!(
            parse_data_url("data:image/png;base64,QUJD"),
            Some(("image/png".to_string(), "QUJD".to_string()))
        );
        assert_eq!(parse_data_url("https://example.com/x.png"), None);
        assert_eq!(parse_data_url("data:image/png,raw"), None);
        assert_eq!(parse_data_url("data:image/png;base64,"), None);
    }

    /// The images capability rides the OpenRouter chat provider's `images`
    /// map (upstream: the deleted providers/openrouter-images.ts factory
    /// became `images: { "openrouter-images": openrouterImagesApi() }` on
    /// the chat provider); the registry handler self-registers from the
    /// free entry point.
    #[test]
    fn openrouter_images_capability_is_an_api_impl_and_registered_handler() {
        // The adapter exists and the registry handler is registered through
        // the import-side-effect port.
        super::super::registry::ensure_builtin_images_apis_registered();
        assert!(super::super::registry::get_images_api_provider(OPENROUTER_IMAGES_API).is_some());

        // The chat provider exposes the capability on its images map.
        let provider = crate::ai::models::providers::openrouter_provider();
        assert!(provider.images_for(OPENROUTER_IMAGES_API).is_some());
    }
}
