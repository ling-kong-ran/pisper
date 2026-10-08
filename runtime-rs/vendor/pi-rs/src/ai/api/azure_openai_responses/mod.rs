//! Azure OpenAI Responses API endpoint — full port of the stream/streamSimple
//! implementations from upstream
//! `packages/ai/src/api/azure-openai-responses.ts` (350 lines): the request
//! body (deployment name as `model`, converted `input` items, tools,
//! `stream: true`, `store: false`, the clamped `prompt_cache_key`, max output
//! tokens floored at 16, temperature, tool choice, the reasoning
//! effort/summary mapping), the client headers (pi User-Agent, model headers,
//! caller overrides), the retry-wrapped HTTP send (upstream
//! `retryProviderRequest` around `client.responses.create(...).withResponse()`),
//! the SSE loop feeding the T7 [`ResponsesStreamProcessor`] (whose terminal
//! encrypted-content backfill is the Azure pi-issue-#6409 behavior), and the
//! terminal/done/error framing of the catch block (lines 148-159).
//!
//! Azure wire shape (upstream delegates to the pinned `openai` SDK's
//! `AzureOpenAI` client, openai-node 6.40.0 `src/azure.ts`, which the port
//! reproduces directly):
//! - auth is the `api-key: <key>` header (the SDK's `authHeaders` override),
//!   never `Authorization: Bearer`;
//! - every request carries the `api-version=<version>` query parameter
//!   (the SDK's `defaultQuery`);
//! - `/responses` is not one of the SDK's deployment-prefixed endpoints, so
//!   the path is `{normalized base URL}/responses` with the deployment name
//!   riding in the body's `model` field.
//!
//! Deviations from upstream, all structural (mirroring the openai-responses
//! port):
//! - The `AzureOpenAIResponsesOptions` extension fields have no port option
//!   surface on `StreamOptions`: `azureApiVersion`, `azureBaseUrl`,
//!   `azureResourceName`, and `azureDeploymentName` stay unset (env and
//!   `model.baseUrl` still resolve them), `reasoningSummary` is always the
//!   `"auto"` default (making the effortless-`"medium"` branch unreachable),
//!   and the direct-`stream` `toolChoice` union is unreachable — only the
//!   provider-neutral `SimpleStreamOptions.toolChoice` (`"auto"`/`"none"`)
//!   forwards.
//! - The port resolves the endpoint from the upstream chain minus the option
//!   fields — `AZURE_OPENAI_BASE_URL` / `AZURE_OPENAI_RESOURCE_NAME` env
//!   (scoped overrides, then process env), then `model.baseUrl` — and errors
//!   when none is set. `ProviderConfig.base_url` is not consulted: upstream
//!   has no equivalent input, and reading `model.baseUrl` keeps the oracle
//!   resolution order exact.
//! - Upstream events carry the live `partial`; the port emits events without
//!   it and consumers reconstruct via `PartialAssistant` (the M2a contract).
//! - Upstream `options.signal` aborts have no equivalent here: the two abort
//!   checks (lines 135-137) and the catch block's `"aborted"` branch have no
//!   input to act on.
//! - The `options.onPayload` / `options.onResponse` hooks (lines 113, 130)
//!   ride the port's process-local [`RequestCallbacks`]
//!   (crate::ai::types::request_callbacks::RequestCallbacks): onPayload is
//!   awaited before the retry-wrapped send (a replacement applies to every
//!   attempt; JS `undefined` keeps the built body, `null` replaces it), and
//!   onResponse fires once the send resolves — a failed send never reaches
//!   it — before `start`. Hook rejections surface through the same catch
//!   block as any other error.
//! - HTTP error bodies are surfaced as
//!   `"Azure OpenAI API error (<status>): <body>"` — the upstream
//!   `formatProviderError(normalizeProviderError(...), "Azure OpenAI API
//!   error")` composition for the openai SDK error shape. Non-HTTP errors
//!   surface their plain message (the prefix only applies when a status was
//!   extracted upstream). JSON bodies render with `serde_json` key order
//!   (sorted), not JS insertion order.
//! - The streamSimple base-option merge folds `model.samplingParams` into
//!   `options.samplingParams` upstream; the port merges both in the body
//!   builder, so direct `stream` calls merge `model.samplingParams` too.
//! - JSON object key order follows `serde_json` (sorted), not JS insertion
//!   order — same documented deviation as the other request builders.

#[cfg(test)]
mod callback_tests;

use std::collections::HashSet;
use std::time::Duration;

use eventsource_stream::Eventsource;
use futures::StreamExt;
use serde_json::{json, Map, Value};

use crate::ai::api::openai_completions::request::{
    clamp_max_tokens_to_context, clamp_openai_prompt_cache_key, clamp_thinking_level,
    create_grammar_tool_input_properties, level_key, map_level, remove_header,
    resolve_sampling_params, set_header, MappedLevel,
};
use crate::ai::api::openai_completions::stream::{
    truncate_error_text, MAX_PROVIDER_ERROR_BODY_CHARS,
};
use crate::ai::api::openai_responses_shared::{
    convert_responses_messages, convert_responses_tools, ConvertResponsesMessagesOptions,
    ConvertResponsesToolsOptions, ResponsesStreamEvent, ResponsesStreamOptions,
    ResponsesStreamProcessor,
};
use crate::ai::api::{http_client, pi_user_agent, request_signal, ApiImpl, REQUEST_WAS_ABORTED};
use crate::ai::retry::{retry_provider_request, ProviderError};
use crate::ai::transcript::{get_declared_tools, resolve_transcript, TranscriptContext};
use crate::ai::types::events::{AssistantMessageEvent, ErrorReason, SuccessReason};
use crate::ai::types::options::{ProviderEnv, SimpleStreamOptions, StreamOptions};
use crate::ai::types::primitives::StopReason;
use crate::ai::types::Model;
use crate::ai::ProviderConfig;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Upstream `DEFAULT_AZURE_API_VERSION` (azure-openai-responses.ts:25).
const DEFAULT_AZURE_API_VERSION: &str = "v1";

/// Upstream `AZURE_TOOL_CALL_PROVIDERS` (azure-openai-responses.ts:26):
/// providers whose tool calls keep their `call_id|item_id` wire ids on replay
/// — the OpenAI set plus azure itself.
const AZURE_TOOL_CALL_PROVIDERS: [&str; 4] = [
    "openai",
    "openai-codex",
    "opencode",
    "azure-openai-responses",
];

/// Upstream `OPENAI_RESPONSES_MIN_OUTPUT_TOKENS`
/// (azure-openai-responses.ts:28): the Responses endpoint rejects
/// `max_output_tokens` below 16 (pi issue #6265).
const OPENAI_RESPONSES_MIN_OUTPUT_TOKENS: u64 = 16;

pub struct AzureOpenAiResponses;

impl ApiImpl for AzureOpenAiResponses {
    // Upstream wires both `options?.onPayload?.(params, model)`
    // (azure-openai-responses.ts:113) and `options?.onResponse?.(...)`
    // (line 130) inside the stream body.
    fn supports_request_callbacks(&self) -> bool {
        true
    }

    fn stream(
        &self,
        cfg: &ProviderConfig,
        model: &Model,
        ctx: &TranscriptContext,
        options: &StreamOptions,
    ) -> mpsc::Receiver<AssistantMessageEvent> {
        // Upstream `stream` takes the API-specific option extension
        // (`reasoningEffort`/`reasoningSummary`/`toolChoice`/`azure*`) on top
        // of the base options; the port's `StreamOptions` is the base set, so
        // the extension fields stay unset for direct `stream` calls.
        let simple = SimpleStreamOptions {
            stream: options.clone(),
            ..SimpleStreamOptions::default()
        };
        run_stream(cfg.clone(), model.clone(), ctx.clone(), simple)
    }

    fn stream_simple(
        &self,
        cfg: &ProviderConfig,
        model: &Model,
        ctx: &TranscriptContext,
        options: &SimpleStreamOptions,
    ) -> mpsc::Receiver<AssistantMessageEvent> {
        // Upstream `streamSimple` (lines 165-186): buildBaseOptions shaping
        // (context-clamped maxTokens default) plus the reasoning clamp — a
        // clamp to "off" drops the effort entirely so `buildParams` falls to
        // its `thinkingLevelMap.off` branch — then delegates to `stream`.
        let mut shaped = options.clone();
        shaped.stream.max_tokens = Some(clamp_max_tokens_to_context(
            model,
            ctx,
            options.stream.max_tokens.unwrap_or(model.max_tokens),
        ));
        shaped.reasoning = options
            .reasoning
            .and_then(|level| clamp_thinking_level(model, Some(level)));
        run_stream(cfg.clone(), model.clone(), ctx.clone(), shaped)
    }
}

fn run_stream(
    cfg: ProviderConfig,
    model: Model,
    ctx: TranscriptContext,
    options: SimpleStreamOptions,
) -> mpsc::Receiver<AssistantMessageEvent> {
    let (tx, rx) = mpsc::channel(64);
    tokio::spawn(async move {
        run_stream_task(cfg, model, ctx, options, tx).await;
    });
    rx
}

async fn run_stream_task(
    cfg: ProviderConfig,
    model: Model,
    ctx: TranscriptContext,
    options: SimpleStreamOptions,
    tx: mpsc::Sender<AssistantMessageEvent>,
) {
    let compat = get_compat(&model);
    // Upstream resolves the transcript synchronously before anything else
    // (azure-openai-responses.ts line 77), then computes the grammar input
    // properties from the normalized messages (lines 108-111). Both steps are
    // infallible, so running them up front keeps the upstream error
    // precedence — apiKey line 103 -> client/URL line 107 -> grammar ->
    // params line 112 — in the check order inside the block below (the
    // processor needs the grammar map at construction).
    let normalized = resolve_transcript(ctx, Some(compat.supports_mid_convo_system_messages));
    let grammar_result = create_grammar_tool_input_properties(
        &get_declared_tools(normalized.messages()),
        compat.supports_openai_grammar_tools,
    );
    let processor_options = ResponsesStreamOptions {
        grammar_tool_input_properties: grammar_result.clone().unwrap_or_default(),
        ..Default::default()
    };
    let mut processor = ResponsesStreamProcessor::new(&model, processor_options);
    let signal = request_signal(&options.stream.signal);
    let outcome = async {
        // Upstream line 103: the API key, from the options or the provider
        // credential (the port's wiring, per the M2c ruling), checked first.
        let api_key = resolve_api_key(&model, &cfg, &options)?;
        // Upstream line 107: createClient resolves + normalizes the endpoint;
        // an invalid URL fails here, before the grammar properties.
        let azure_config = resolve_azure_config(&model, options.stream.env.as_ref())?;
        let grammar_tool_input_properties = grammar_result?;
        let deployment_name = resolve_deployment_name(&model, options.stream.env.as_ref());
        let body = build_params(
            &model,
            &normalized,
            &options,
            &compat,
            &deployment_name,
            &grammar_tool_input_properties,
        )?;
        // Upstream lines 113-116: onPayload is awaited before the SDK request
        // (and the retry seam — a replacement applies to every attempt); JS
        // `undefined` preserves the built params while any other value —
        // including `null` — replaces them. A hook rejection throws into the
        // catch block before any HTTP traffic.
        let body = options
            .stream
            .callbacks
            .payload(body, &model)
            .await
            .map_err(|error| error.to_string())?;
        let headers = build_headers(&model, &options);

        let response = send_stream_request(
            &azure_config.base_url,
            &azure_config.api_version,
            &api_key,
            &body,
            &headers,
            &options,
            &signal,
        )
        .await?;

        // Upstream line 130: onResponse after the request resolves (the retry
        // seam settles first, so a failed send never reaches the hook) and
        // before `start`; a hook rejection takes the same catch path.
        options
            .stream
            .callbacks
            .http_response(&response, &model)
            .await
            .map_err(|error| error.to_string())?;

        // Upstream line 131: `start` after the response arrives, before any
        // event.
        let _ = tx
            .send(AssistantMessageEvent::Start {
                message: processor.output().clone(),
            })
            .await;

        let mut events = response.bytes_stream().eventsource();
        loop {
            let item = tokio::select! {
                biased;
                _ = signal.cancelled() => return Err(REQUEST_WAS_ABORTED.to_string()),
                next = events.next() => match next {
                    Some(item) => item,
                    None => break,
                },
            };
            let event = item.map_err(|error| error.to_string())?;
            let payload: Value = serde_json::from_str(&event.data)
                .map_err(|error| format!("Could not parse Responses SSE event: {error}"))?;
            processor.observe_raw_event(payload.clone()).await?;
            processor
                .process_event(&ResponsesStreamEvent::from_value(payload), &tx)
                .await?;
        }

        // Upstream post-loop guard inside processResponsesStream.
        processor.finish()?;
        let output = processor.output();
        // Upstream lines 135-137: the post-loop abort check precedes the
        // pending / error guards.
        if signal.is_cancelled() {
            return Err(REQUEST_WAS_ABORTED.to_string());
        }
        if output.stop_reason == StopReason::Pending {
            return Err("Azure OpenAI Responses stream ended without a stop reason".to_string());
        }
        if output.stop_reason == StopReason::Aborted || output.stop_reason == StopReason::Error {
            return Err(output
                .error_message
                .clone()
                .unwrap_or_else(|| "An unknown error occurred".to_string()));
        }
        let reason = match output.stop_reason {
            StopReason::Length => SuccessReason::Length,
            StopReason::ToolUse => SuccessReason::ToolUse,
            _ => SuccessReason::Stop,
        };
        let _ = tx
            .send(AssistantMessageEvent::Done {
                reason,
                message: processor.output().clone(),
            })
            .await;
        Ok(())
    };
    match outcome.await {
        Ok(()) => {}
        Err(message) => {
            // Upstream catch block (lines 148-159): the partial message keeps
            // its content; stopReason settles to "aborted" when the request
            // signal fired, else "error", and the thrown value becomes the
            // errorMessage.
            let mut output = processor.into_output();
            output.stop_reason = if signal.is_cancelled() {
                StopReason::Aborted
            } else {
                StopReason::Error
            };
            output.error_message = Some(message);
            let _ = tx
                .send(AssistantMessageEvent::Error {
                    reason: if signal.is_cancelled() {
                        ErrorReason::Aborted
                    } else {
                        ErrorReason::Error
                    },
                    error: output,
                })
                .await;
        }
    }
}

/// Upstream compat reads: every field azure-openai-responses.ts consumes,
/// with its documented default. Note `supportsStrictMode` defaults to `true`
/// here (unlike the openai-responses port) — azure-openai-responses.ts uses
/// `model.compat?.supportsStrictMode ?? true`.
#[derive(Debug, Clone, PartialEq)]
pub struct AzureResponsesCompat {
    supports_mid_convo_system_messages: bool,
    supports_strict_mode: bool,
    supports_openai_grammar_tools: bool,
    supports_additional_tools: bool,
    supports_tool_search: bool,
}

pub fn get_compat(model: &Model) -> AzureResponsesCompat {
    let raw = model.openai_responses_compat().unwrap_or_default();
    AzureResponsesCompat {
        supports_mid_convo_system_messages: raw.supports_mid_convo_system_messages.unwrap_or(false),
        supports_strict_mode: raw.supports_strict_mode.unwrap_or(true),
        supports_openai_grammar_tools: raw.supports_openai_grammar_tools.unwrap_or(false),
        supports_additional_tools: raw.supports_additional_tools.unwrap_or(false),
        supports_tool_search: raw.supports_tool_search.unwrap_or(false),
    }
}

/// The request credential (upstream lines 103-106 read `options?.apiKey`):
/// the options key, then the provider credential `cfg.api_key` (the port's
/// wiring, per the M2c ruling). Upstream throws when both are missing.
fn resolve_api_key(
    model: &Model,
    cfg: &ProviderConfig,
    options: &SimpleStreamOptions,
) -> Result<String, String> {
    if let Some(key) = options
        .stream
        .api_key
        .as_deref()
        .filter(|key| !key.is_empty())
    {
        return Ok(key.to_string());
    }
    if !cfg.api_key.is_empty() {
        return Ok(cfg.api_key.clone());
    }
    Err(format!("No API key for provider: {}", model.provider))
}

/// Upstream `getProviderEnvValue` (`utils/provider-env.ts`): the scoped env
/// map, then the process environment; empty values fall through like the JS
/// `||` chain.
pub(crate) fn get_provider_env_value(name: &str, env: Option<&ProviderEnv>) -> Option<String> {
    env.and_then(|env| env.get(name))
        .filter(|value| !value.is_empty())
        .cloned()
        .or_else(|| std::env::var(name).ok().filter(|value| !value.is_empty()))
}

/// Upstream `parseDeploymentNameMap` (azure-openai-responses.ts:30-41):
/// `modelId=deploymentName` pairs separated by commas; entries missing a pair
/// are skipped. JS `split("=", 2)` takes the first two segments.
fn parse_deployment_name_map(value: Option<&str>) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let Some(value) = value else {
        return map;
    };
    for entry in value.split(',') {
        let trimmed = entry.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut segments = trimmed.split('=');
        let (Some(model_id), Some(deployment_name)) = (segments.next(), segments.next()) else {
            continue;
        };
        if model_id.trim().is_empty() || deployment_name.trim().is_empty() {
            continue;
        }
        map.insert(
            model_id.trim().to_string(),
            deployment_name.trim().to_string(),
        );
    }
    map
}

/// Upstream `resolveDeploymentName` (azure-openai-responses.ts:43-51): the
/// option field first (no port surface), then the
/// `AZURE_OPENAI_DEPLOYMENT_NAME_MAP` env entry for the model id, then the
/// model id itself.
pub(crate) fn resolve_deployment_name(model: &Model, env: Option<&ProviderEnv>) -> String {
    let map_value = get_provider_env_value("AZURE_OPENAI_DEPLOYMENT_NAME_MAP", env);
    parse_deployment_name_map(map_value.as_deref())
        .get(&model.id)
        .cloned()
        .unwrap_or_else(|| model.id.clone())
}

/// Upstream `normalizeAzureBaseUrl` (azure-openai-responses.ts:188-217):
/// Azure-host roots, `/openai`, and `/openai/v1/responses` normalize to
/// `/openai/v1` (query stripped); non-Azure proxy URLs are preserved as-is.
pub(crate) fn normalize_azure_base_url(base_url: &str) -> Result<String, String> {
    let trimmed = base_url.trim().trim_end_matches('/');
    let mut url: reqwest::Url = reqwest::Url::parse(trimmed)
        .map_err(|_| format!("Invalid Azure OpenAI base URL: {base_url}"))?;

    let host = url.host_str().unwrap_or_default();
    let is_azure_host = host.ends_with(".openai.azure.com")
        || host.ends_with(".cognitiveservices.azure.com")
        || host.ends_with(".ai.azure.com");
    let normalized_path = url.path().trim_end_matches('/');

    // Ensure Azure hosts have /openai/v1 as base path so the endpoint appends
    // /responses and ?api-version=<version> correctly.
    if is_azure_host
        && (normalized_path.is_empty()
            || normalized_path == "/openai"
            || normalized_path == "/openai/v1/responses")
    {
        url.set_path("/openai/v1");
        url.set_query(None);
    }

    let mut result = url.to_string();
    while result.ends_with('/') {
        result.pop();
    }
    Ok(result)
}

/// Upstream `buildDefaultBaseUrl` (azure-openai-responses.ts:219-221).
fn build_default_base_url(resource_name: &str) -> String {
    format!("https://{resource_name}.openai.azure.com/openai/v1")
}

/// The resolved endpoint inputs (upstream `resolveAzureConfig` returns
/// `{ baseUrl, apiVersion }`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AzureConfig {
    pub base_url: String,
    pub api_version: String,
}

/// Upstream `resolveAzureConfig` (azure-openai-responses.ts:223-256): the
/// API version from the option (no port surface), then
/// `AZURE_OPENAI_API_VERSION`, then `"v1"`; the base URL from the option (no
/// port surface), then `AZURE_OPENAI_BASE_URL`, then the
/// `AZURE_OPENAI_RESOURCE_NAME` default, then `model.baseUrl`, else the
/// upstream error.
pub(crate) fn resolve_azure_config(
    model: &Model,
    env: Option<&ProviderEnv>,
) -> Result<AzureConfig, String> {
    let api_version = get_provider_env_value("AZURE_OPENAI_API_VERSION", env)
        .unwrap_or_else(|| DEFAULT_AZURE_API_VERSION.to_string());

    let base_url = get_provider_env_value("AZURE_OPENAI_BASE_URL", env)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            get_provider_env_value("AZURE_OPENAI_RESOURCE_NAME", env)
                .map(|name| name.trim().to_string())
                .filter(|name| !name.is_empty())
                .map(|name| build_default_base_url(&name))
        })
        .or_else(|| {
            let model_url = model.base_url.trim();
            (!model_url.is_empty()).then(|| model_url.to_string())
        });

    let Some(base_url) = base_url else {
        return Err(
            "Azure OpenAI base URL is required. Set AZURE_OPENAI_BASE_URL or \
             AZURE_OPENAI_RESOURCE_NAME, or pass azureBaseUrl, azureResourceName, or model.baseUrl."
                .to_string(),
        );
    };

    Ok(AzureConfig {
        base_url: normalize_azure_base_url(&base_url)?,
        api_version,
    })
}

/// Upstream `createClient` header assembly (azure-openai-responses.ts:
/// 258-264) without the HTTP client: the pi User-Agent, model headers, and
/// the caller's headers merged last (a `None` value — upstream `null` —
/// suppresses a default header).
fn build_headers(model: &Model, options: &SimpleStreamOptions) -> Vec<(String, String)> {
    let mut headers: Vec<(String, String)> = vec![("User-Agent".to_string(), pi_user_agent())];
    for (name, value) in model.headers.iter().flatten() {
        match value {
            Some(value) => set_header(&mut headers, name, value),
            None => remove_header(&mut headers, name),
        }
    }
    if let Some(option_headers) = &options.stream.headers {
        for (name, value) in option_headers {
            match value {
                Some(value) => set_header(&mut headers, name, value),
                None => remove_header(&mut headers, name),
            }
        }
    }
    headers
}

/// Upstream `buildParams` (azure-openai-responses.ts:277-350).
fn build_params(
    model: &Model,
    ctx: &TranscriptContext,
    options: &SimpleStreamOptions,
    compat: &AzureResponsesCompat,
    deployment_name: &str,
    grammar_tool_input_properties: &std::collections::HashMap<String, String>,
) -> Result<Value, String> {
    let transcript_tools = crate::ai::transcript::resolve_transcript_tools(
        ctx.messages(),
        compat.supports_additional_tools || compat.supports_tool_search,
    );
    let tool_options = ConvertResponsesToolsOptions {
        supports_strict_mode: compat.supports_strict_mode,
        supports_openai_grammar_tools: compat.supports_openai_grammar_tools,
        ..Default::default()
    };
    let allowed_providers: HashSet<String> = AZURE_TOOL_CALL_PROVIDERS
        .iter()
        .map(|provider| (*provider).to_string())
        .collect();
    let messages = convert_responses_messages(
        model,
        ctx,
        &allowed_providers,
        &ConvertResponsesMessagesOptions {
            include_system_prompt: None,
            grammar_tool_input_properties: grammar_tool_input_properties.clone(),
            supports_mid_convo_system_messages: compat.supports_mid_convo_system_messages,
            supports_additional_tools: compat.supports_additional_tools,
            supports_tool_search: compat.supports_tool_search,
            tool_options: Some(tool_options.clone()),
        },
    )?;

    let mut params = Map::new();
    // The deployment name is the body `model` (azure-openai-responses.ts:
    // 302); Azure routes by deployment.
    params.insert("model".into(), json!(deployment_name));
    params.insert("input".into(), Value::Array(messages));
    params.insert("stream".into(), json!(true));
    if let Some(key) = clamp_openai_prompt_cache_key(options.stream.session_id.as_deref()) {
        params.insert("prompt_cache_key".into(), json!(key));
    }
    params.insert("store".into(), json!(false));

    // Upstream lines 309-311: the truthy `options?.maxTokens &&` check omits
    // the field for 0, and `Math.max(options.maxTokens, 16)` floors the rest.
    if let Some(max_tokens) = options
        .stream
        .max_tokens
        .filter(|max_tokens| *max_tokens > 0)
    {
        params.insert(
            "max_output_tokens".into(),
            json!(max_tokens.max(OPENAI_RESPONSES_MIN_OUTPUT_TOKENS)),
        );
    }

    if let Some(temperature) = options.stream.temperature {
        params.insert("temperature".into(), json!(temperature));
    }

    if !transcript_tools.request_tools.is_empty() {
        params.insert(
            "tools".into(),
            Value::Array(convert_responses_tools(
                &transcript_tools.request_tools,
                &tool_options,
            )?),
        );
    }
    if let Some(tool_choice) = options.tool_choice {
        params.insert("tool_choice".into(), json!(tool_choice));
    }

    // Upstream lines 327-342: the reasoning mapping. With an explicit effort
    // (streamSimple's clamped level) the summary is always the `"auto"`
    // default (`reasoningSummary` has no port surface) and the encrypted
    // reasoning content is included; otherwise `thinkingLevelMap.off` picks
    // the value (`null` drops reasoning entirely, an absent entry means
    // `"none"`).
    if model.reasoning {
        if let Some(level) = options.reasoning {
            let key = level_key(Some(level));
            let effort = match map_level(model, key) {
                MappedLevel::Value(value) => value,
                // JS `thinkingLevelMap?.[effort] ?? effort`: a null (or
                // absent) mapping falls back to the level string.
                MappedLevel::Null | MappedLevel::Absent => key.to_string(),
            };
            params.insert(
                "reasoning".into(),
                json!({"effort": effort, "summary": "auto"}),
            );
            params.insert("include".into(), json!(["reasoning.encrypted_content"]));
        } else if map_level(model, "off") != MappedLevel::Null {
            let effort = match map_level(model, "off") {
                MappedLevel::Value(value) => value,
                MappedLevel::Null | MappedLevel::Absent => "none".to_string(),
            };
            params.insert("reasoning".into(), json!({"effort": effort}));
        }
    }

    // Last so model and request sampling parameters override the named
    // request fields (upstream lines 348-351): `resolveSamplingParams(model,
    // reasoningEffort ?? "off", options?.samplingParams)`. The port's
    // `options.reasoning` is the clamped `reasoningEffort` surface (the
    // summary-only `"medium"` default has no port surface; the resolver
    // clamps again, so raw or clamped look up identically).
    if let Some(sampling) = resolve_sampling_params(
        model,
        options.reasoning,
        options.stream.sampling_params.as_ref(),
    ) {
        for (key, value) in sampling {
            params.insert(key, value);
        }
    }

    Ok(Value::Object(params))
}

/// Send the assembled request (upstream
/// `client.responses.create(params, requestOptions).withResponse()`),
/// wrapped in the provider retry policy with `options.maxRetries`/
/// `maxRetryDelayMs`. The wire shape mirrors the pinned `openai` SDK's
/// `AzureOpenAI` client (openai-node 6.40.0 `src/azure.ts`): `api-key` auth
/// header, `api-version` query parameter, `{base}/responses` path. Retries
/// cover transport failures and retryable statuses only — once stream bytes
/// flow an error is never retried.
async fn send_stream_request(
    base_url: &str,
    api_version: &str,
    api_key: &str,
    body: &Value,
    headers: &[(String, String)],
    options: &SimpleStreamOptions,
    signal: &CancellationToken,
) -> Result<reqwest::Response, String> {
    let url = format!(
        "{}/responses?api-version={api_version}",
        base_url.trim_end_matches('/')
    );
    let mut header_map = reqwest::header::HeaderMap::new();
    let auth = reqwest::header::HeaderValue::from_str(api_key)
        .map_err(|error| format!("Invalid api-key header: {error}"))?;
    header_map.insert("api-key", auth);
    for (name, value) in headers {
        let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
            .map_err(|error| format!("Invalid header name \"{name}\": {error}"))?;
        let value = reqwest::header::HeaderValue::from_str(value)
            .map_err(|error| format!("Invalid header value for \"{name}\": {error}"))?;
        header_map.insert(name, value);
    }
    let mut request = http_client().post(&url).headers(header_map).json(body);
    if let Some(ms) = options.stream.timeout_ms {
        request = request.timeout(Duration::from_millis(ms));
    }
    let max_retries = options.stream.max_retries.unwrap_or(0);
    let max_retry_delay_ms = options.stream.max_retry_delay_ms;
    retry_provider_request(max_retries, max_retry_delay_ms, Some(signal), || async {
        let response = request
            .try_clone()
            .expect("JSON request body is buffered and clonable")
            .send()
            .await
            .map_err(|error| ProviderError::transport(error.to_string()))?;
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let status_code = status.as_u16();
        let response_headers = response.headers().clone();
        let response_body = response.text().await.unwrap_or_default();
        Err(ProviderError::http(
            status_code,
            response_headers,
            format_azure_http_error(status_code, &response_body),
        ))
    })
    .await
    .map_err(|error| error.message)
}

/// Upstream catch-block error composition (`formatAzureOpenAIError` =
/// `formatProviderError(normalizeProviderError(error), "Azure OpenAI API
/// error")`) for the openai SDK error shape: the SDK parses the body into a
/// plain object, so the display string is
/// `"Azure OpenAI API error (<status>): <body>"` (raw text for non-JSON
/// bodies, the SDK's empty-body message otherwise). Non-HTTP errors surface
/// their plain message — the prefix only applies when a status was
/// extracted.
fn format_azure_http_error(status: u16, body_text: &str) -> String {
    let trimmed = body_text.trim();
    let body = if trimmed.is_empty() {
        format!("{status} status code with empty body")
    } else {
        match serde_json::from_str::<Value>(trimmed) {
            Ok(value) => truncate_error_text(&value.to_string(), MAX_PROVIDER_ERROR_BODY_CHARS),
            Err(_) => truncate_error_text(trimmed, MAX_PROVIDER_ERROR_BODY_CHARS),
        }
    };
    format!("Azure OpenAI API error ({status}): {body}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::api::{abort_test_support::stalled_sse_server, REQUEST_ABORTED};
    use crate::ai::transcript::{normalize_context, Context};
    use crate::ai::types::content::ThinkingContent;
    use crate::ai::types::events::PartialAssistant;
    use crate::ai::types::message::{
        AssistantBlock, AssistantMessage, Message, StringOrBlocks, UserMessage,
    };
    use crate::ai::types::options::ProviderHeaders;
    use crate::ai::types::primitives::{ModelCost, ToolChoice};
    use crate::ai::types::tool::{ConstrainedSampling, JsonSchemaSampling, Strict, Tool};
    use crate::ai::types::{Model, ModelInput, ThinkingLevel};
    use serde_json::json;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// The API id stamped on every emitted message.
    const API: &str = "azure-openai-responses";

    const TS: i64 = 1758240000000;

    // ---- fixtures ----

    fn model(base_url: &str) -> Model {
        Model {
            r#type: None,
            prompt_cache: None,
            input_limits: None,
            id: "gpt-4o-mini".to_string(),
            name: "GPT-4o mini".to_string(),
            api: API.to_string(),
            provider: API.to_string(),
            base_url: base_url.to_string(),
            reasoning: false,
            thinking_level_map: None,
            input: vec![ModelInput::Text],
            cost: ModelCost::default(),
            context_window: 400000,
            max_tokens: 128000,
            sampling_params: None,
            sampling_params_by_thinking_level: None,
            headers: None,
            compat: None,
        }
    }

    fn reasoning_model(base_url: &str) -> Model {
        let mut model = model(base_url);
        model.id = "gpt-5-mini".to_string();
        model.name = "GPT-5 Mini".to_string();
        model.reasoning = true;
        model
    }

    fn model_with_map(base_url: &str, map: serde_json::Value) -> Model {
        let mut model = reasoning_model(base_url);
        model.thinking_level_map = Some(serde_json::from_value(map).unwrap());
        model
    }

    fn model_with_compat(base_url: &str, compat: serde_json::Value) -> Model {
        let mut model = model(base_url);
        model.compat = Some(compat);
        model
    }

    fn ctx_with(messages: Vec<Message>, tools: Option<Vec<Tool>>) -> TranscriptContext {
        normalize_context(&Context {
            system_prompt: None,
            messages,
            tools,
        })
    }

    fn user_msg(text: &str) -> Message {
        Message::User(UserMessage {
            content: StringOrBlocks::Text(text.to_string()),
            timestamp: TS,
        })
    }

    fn tool(name: &str) -> Tool {
        Tool {
            name: name.into(),
            description: "Read a file".into(),
            parameters: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
            constrained_sampling: None,
        }
    }

    fn strict_schema_tool(name: &str, strict: Strict) -> Tool {
        Tool {
            name: name.into(),
            description: "Preferred constrained tool".into(),
            parameters: json!({"type": "object", "properties": {"value": {"type": "string"}}}),
            constrained_sampling: Some(ConstrainedSampling::JsonSchema(JsonSchemaSampling {
                strict,
            })),
        }
    }

    fn cfg(_server: &wiremock::MockServer) -> ProviderConfig {
        ProviderConfig {
            // The port resolves the endpoint from env/model.baseUrl, so the
            // ProviderConfig base_url deliberately does not point at the
            // server (it must not be consulted).
            base_url: "https://unused.example.com".to_string(),
            api_key: "test-api-key".to_string(),
            max_tokens: 8192,
        }
    }

    fn env_map(entries: &[(&str, &str)]) -> ProviderEnv {
        entries
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    /// Removes the AZURE_OPENAI_* process variables for the duration of a
    /// test and restores the prior values on drop — the port analog of the
    /// upstream oracle's `beforeEach`/`afterEach` `process.env`
    /// sanitization. A scoped env map cannot express absence: missing keys
    /// fall through to the process environment (upstream
    /// `env?.[name] || process.env[name]`), so the env-absence branches need
    /// this guard to stay hermetic.
    struct EnvGuard {
        saved: Vec<(String, Option<String>)>,
    }

    impl EnvGuard {
        fn sanitize() -> EnvGuard {
            let mut saved = Vec::new();
            for name in [
                "AZURE_OPENAI_BASE_URL",
                "AZURE_OPENAI_RESOURCE_NAME",
                "AZURE_OPENAI_API_VERSION",
                "AZURE_OPENAI_DEPLOYMENT_NAME_MAP",
            ] {
                saved.push((name.to_string(), std::env::var(name).ok()));
                std::env::remove_var(name);
            }
            EnvGuard { saved }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (name, value) in &self.saved {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }

    /// The scoped env pinned into wire tests that leave it unset. An absent
    /// or keyless scoped map falls through to the process environment
    /// (upstream `getProviderEnvValue`), so a machine exporting
    /// AZURE_OPENAI_* variables would otherwise redirect the request. The
    /// pinned values reproduce the suite's expected defaults (the wiremock
    /// endpoint, api-version `v1`, and deployment = model id via a map entry
    /// that parses to nothing).
    fn pinned_env(server: &wiremock::MockServer) -> ProviderEnv {
        env_map(&[
            ("AZURE_OPENAI_BASE_URL", &format!("{}/v1", server.uri())),
            ("AZURE_OPENAI_API_VERSION", "v1"),
            ("AZURE_OPENAI_DEPLOYMENT_NAME_MAP", " "),
        ])
    }

    fn hermetic_options(
        mut options: SimpleStreamOptions,
        server: &wiremock::MockServer,
    ) -> SimpleStreamOptions {
        if options.stream.env.is_none() {
            options.stream.env = Some(pinned_env(server));
        }
        options
    }

    fn sse(body: &str) -> wiremock::ResponseTemplate {
        wiremock::ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_string(body.to_string())
    }

    fn completed_sse() -> String {
        format!(
            "data: {}\n\n",
            json!({
                "type": "response.completed",
                "response": {
                    "id": "resp_ok",
                    "status": "completed",
                    "usage": {
                        "input_tokens": 20,
                        "output_tokens": 7,
                        "total_tokens": 27,
                        "input_tokens_details": {"cached_tokens": 2, "cache_write_tokens": 3},
                    },
                },
            })
        )
    }

    async fn mount(server: &wiremock::MockServer, body: &str) {
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/responses"))
            .respond_with(sse(body))
            .mount(server)
            .await;
    }

    async fn collect_simple(
        server: &wiremock::MockServer,
        model: &Model,
        ctx: &TranscriptContext,
        options: &SimpleStreamOptions,
    ) -> Vec<AssistantMessageEvent> {
        let api = AzureOpenAiResponses;
        let options = hermetic_options(options.clone(), server);
        let mut rx = api.stream_simple(&cfg(server), model, ctx, &options);
        let mut out = Vec::new();
        while let Some(event) = rx.recv().await {
            out.push(event);
        }
        out
    }

    async fn collect_stream(
        server: &wiremock::MockServer,
        model: &Model,
        ctx: &TranscriptContext,
        options: &StreamOptions,
    ) -> Vec<AssistantMessageEvent> {
        let api = AzureOpenAiResponses;
        let mut stream_options = options.clone();
        if stream_options.env.is_none() {
            stream_options.env = Some(pinned_env(server));
        }
        let mut rx = api.stream(&cfg(server), model, ctx, &stream_options);
        let mut out = Vec::new();
        while let Some(event) = rx.recv().await {
            out.push(event);
        }
        out
    }

    /// Runs one stream and returns the captured last request (URL, headers,
    /// JSON body) and the event sequence. Tests that need an exact request
    /// count assert `server.received_requests()` themselves.
    async fn capture_simple(
        server: &wiremock::MockServer,
        model: &Model,
        ctx: &TranscriptContext,
        options: &SimpleStreamOptions,
    ) -> (
        String,
        reqwest::header::HeaderMap,
        Value,
        Vec<AssistantMessageEvent>,
    ) {
        let events = collect_simple(server, model, ctx, options).await;
        let requests = server.received_requests().await.unwrap();
        assert!(!requests.is_empty(), "at least one request expected");
        let last = requests.last().unwrap();
        let body: Value = serde_json::from_slice(&last.body).unwrap();
        let url = last.url.clone();
        (format!("{url:?}"), last.headers.clone(), body, events)
    }

    fn apply_all(events: &[AssistantMessageEvent]) -> PartialAssistant {
        let mut partial = PartialAssistant::new();
        for event in events {
            partial
                .apply(event)
                .unwrap_or_else(|error| panic!("reducer rejected {}: {error}", event.event_type()));
        }
        partial
    }

    fn event_types(events: &[AssistantMessageEvent]) -> Vec<&'static str> {
        events
            .iter()
            .map(AssistantMessageEvent::event_type)
            .collect()
    }

    fn error_of(events: &[AssistantMessageEvent]) -> (ErrorReason, AssistantMessage) {
        match events.last() {
            Some(AssistantMessageEvent::Error { reason, error }) => (*reason, error.clone()),
            other => panic!("expected terminal error, got {other:?}"),
        }
    }

    fn body_of<'a>(header_map: &'a reqwest::header::HeaderMap, name: &str) -> Option<&'a str> {
        header_map.get(name).and_then(|value| value.to_str().ok())
    }

    // ---- 1. base URL resolution + normalization (azure-openai-base-url
    //         oracle; the pure URL values, since wiremock hosts cannot carry
    //         *.azure.com names) ----

    #[test]
    fn normalizes_azure_hosts_to_openai_v1() {
        let cases = [
            (
                "https://marc-quicktests-resource.cognitiveservices.azure.com",
                "https://marc-quicktests-resource.cognitiveservices.azure.com/openai/v1",
            ),
            (
                "https://marc-quicktests-resource.ai.azure.com",
                "https://marc-quicktests-resource.ai.azure.com/openai/v1",
            ),
            (
                "https://my-resource.openai.azure.com",
                "https://my-resource.openai.azure.com/openai/v1",
            ),
            (
                "https://my-resource.cognitiveservices.azure.com/openai",
                "https://my-resource.cognitiveservices.azure.com/openai/v1",
            ),
            (
                "https://my-resource.cognitiveservices.azure.com/openai/v1",
                "https://my-resource.cognitiveservices.azure.com/openai/v1",
            ),
            (
                "https://my-resource.services.ai.azure.com/openai/v1/responses",
                "https://my-resource.services.ai.azure.com/openai/v1",
            ),
            (
                "https://my-proxy.example.com/v1",
                "https://my-proxy.example.com/v1",
            ),
            (
                "https://my-resource.openai.azure.com/openai?api-version=2024-12-01",
                "https://my-resource.openai.azure.com/openai/v1",
            ),
            (
                "https://my-proxy.example.com/v1?custom=true",
                "https://my-proxy.example.com/v1?custom=true",
            ),
        ];
        for (input, expected) in cases {
            // The upstream oracle sets AZURE_OPENAI_BASE_URL per case; the
            // scoped map is the port's injection point (and wins over
            // model.baseUrl, which carries the same value here).
            let env = env_map(&[("AZURE_OPENAI_BASE_URL", input)]);
            let resolved = resolve_azure_config(&model(input), Some(&env)).unwrap();
            assert_eq!(resolved.base_url, expected, "input: {input}");
        }
    }

    #[test]
    fn invalid_base_url_is_rejected() {
        let env = env_map(&[("AZURE_OPENAI_BASE_URL", "not-a-url")]);
        let error = resolve_azure_config(&model("not-a-url"), Some(&env)).unwrap_err();
        assert!(error.contains("Invalid Azure OpenAI base URL"), "{error}");
        // The same failure surfaces on the wire as the lone error event.
    }

    #[test]
    fn env_base_url_beats_model_base_url_and_resource_name() {
        let env = env_map(&[(
            "AZURE_OPENAI_BASE_URL",
            "https://from-env.openai.azure.com/",
        )]);
        let resolved =
            resolve_azure_config(&model("https://from-model.openai.azure.com"), Some(&env))
                .unwrap();
        assert_eq!(
            resolved.base_url,
            "https://from-env.openai.azure.com/openai/v1"
        );

        // The resource name only wins when no base URL resolves: the
        // whitespace BASE_URL entry falls through (empty after trim, like
        // upstream's `||` chain), keeping the case hermetic against a
        // machine-exported AZURE_OPENAI_BASE_URL.
        let env = env_map(&[
            ("AZURE_OPENAI_BASE_URL", " "),
            ("AZURE_OPENAI_RESOURCE_NAME", "from-resource"),
        ]);
        let resolved =
            resolve_azure_config(&model("https://from-model.openai.azure.com"), Some(&env))
                .unwrap();
        assert_eq!(
            resolved.base_url,
            "https://from-resource.openai.azure.com/openai/v1"
        );
    }

    #[test]
    fn missing_base_url_errors_with_upstream_guidance() {
        // The env-absence branch: sanitize the machine environment (an empty
        // scoped map would still fall through to it).
        let _guard = EnvGuard::sanitize();
        let error = resolve_azure_config(&model(""), None).unwrap_err();
        assert_eq!(
            error,
            "Azure OpenAI base URL is required. Set AZURE_OPENAI_BASE_URL or \
             AZURE_OPENAI_RESOURCE_NAME, or pass azureBaseUrl, azureResourceName, or model.baseUrl."
        );
    }

    #[test]
    fn api_version_defaults_to_v1_and_env_overrides() {
        let resolved = {
            let _guard = EnvGuard::sanitize();
            resolve_azure_config(&model("https://r.openai.azure.com"), None).unwrap()
        };
        assert_eq!(resolved.api_version, "v1");
        let env = env_map(&[("AZURE_OPENAI_API_VERSION", "2024-12-01")]);
        let resolved =
            resolve_azure_config(&model("https://r.openai.azure.com"), Some(&env)).unwrap();
        assert_eq!(resolved.api_version, "2024-12-01");
    }

    #[test]
    fn deployment_name_defaults_to_model_id_and_map_overrides() {
        let resolved = {
            let _guard = EnvGuard::sanitize();
            resolve_deployment_name(&model("https://r.openai.azure.com"), None)
        };
        assert_eq!(resolved, "gpt-4o-mini");

        let env = env_map(&[(
            "AZURE_OPENAI_DEPLOYMENT_NAME_MAP",
            "gpt-4o-mini=gpt4o-deploy, other=extra",
        )]);
        let resolved = resolve_deployment_name(&model("https://r.openai.azure.com"), Some(&env));
        assert_eq!(resolved, "gpt4o-deploy");

        // Entries without a "=" pair are skipped; the model falls back.
        let env = env_map(&[(
            "AZURE_OPENAI_DEPLOYMENT_NAME_MAP",
            "broken, gpt-4o-mini=good",
        )]);
        let resolved = resolve_deployment_name(&model("https://r.openai.azure.com"), Some(&env));
        assert_eq!(resolved, "good");
    }

    // ---- 2. wire shape: path, query, auth, headers ----

    #[tokio::test]
    async fn wire_request_carries_api_key_header_version_query_and_deployment_model() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &completed_sse()).await;
        let model = model(&format!("{}/v1", server.uri()));
        let ctx = ctx_with(vec![user_msg("hello")], None);
        let (url, headers, body, events) =
            capture_simple(&server, &model, &ctx, &SimpleStreamOptions::default()).await;

        assert!(url.contains("/v1/responses"), "{url}");
        assert!(url.contains("api-version=v1"), "{url}");
        // Azure key auth: the `api-key` header, never a bearer token.
        assert_eq!(body_of(&headers, "api-key"), Some("test-api-key"));
        assert_eq!(body_of(&headers, "authorization"), None);
        // The pi User-Agent rides on the client by default.
        assert_eq!(
            body_of(&headers, "user-agent"),
            Some(pi_user_agent().as_str())
        );
        // Deployment name (== model id without a map) is the body model.
        assert_eq!(body["model"], json!("gpt-4o-mini"));
        assert_eq!(body["stream"], json!(true));
        assert_eq!(body["store"], json!(false));
        assert_eq!(
            body["input"][0],
            json!({"role": "user", "content": [{"type": "input_text", "text": "hello"}]})
        );
        assert_eq!(event_types(&events), ["start", "done"], "{events:?}");
    }

    #[tokio::test]
    async fn env_api_version_flows_into_the_query() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &completed_sse()).await;
        let model = model(&format!("{}/v1", server.uri()));
        let ctx = ctx_with(vec![user_msg("hello")], None);
        // Fully-specified scoped map: keys left out would fall through to the
        // machine environment (upstream `getProviderEnvValue`).
        let options = SimpleStreamOptions {
            stream: StreamOptions {
                env: Some(env_map(&[
                    ("AZURE_OPENAI_BASE_URL", &format!("{}/v1", server.uri())),
                    ("AZURE_OPENAI_API_VERSION", "2024-12-01"),
                    ("AZURE_OPENAI_DEPLOYMENT_NAME_MAP", " "),
                ])),
                ..Default::default()
            },
            ..SimpleStreamOptions::default()
        };
        let (url, _, _, _) = capture_simple(&server, &model, &ctx, &options).await;
        assert!(url.contains("api-version=2024-12-01"), "{url}");
    }

    #[tokio::test]
    async fn env_deployment_map_flows_into_the_body_model() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &completed_sse()).await;
        let model = model(&format!("{}/v1", server.uri()));
        let ctx = ctx_with(vec![user_msg("hello")], None);
        // Fully-specified scoped map: keys left out would fall through to the
        // machine environment (upstream `getProviderEnvValue`).
        let options = SimpleStreamOptions {
            stream: StreamOptions {
                env: Some(env_map(&[
                    ("AZURE_OPENAI_BASE_URL", &format!("{}/v1", server.uri())),
                    ("AZURE_OPENAI_API_VERSION", "v1"),
                    (
                        "AZURE_OPENAI_DEPLOYMENT_NAME_MAP",
                        "gpt-4o-mini=gpt4o-deploy",
                    ),
                ])),
                ..Default::default()
            },
            ..SimpleStreamOptions::default()
        };
        let (_, _, body, _) = capture_simple(&server, &model, &ctx, &options).await;
        assert_eq!(body["model"], json!("gpt4o-deploy"));
    }

    #[tokio::test]
    async fn explicit_headers_override_the_user_agent() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &completed_sse()).await;
        let model = model(&format!("{}/v1", server.uri()));
        let ctx = ctx_with(vec![user_msg("hello")], None);
        let mut option_headers = ProviderHeaders::new();
        option_headers.insert("User-Agent".to_string(), Some("custom-agent".to_string()));
        let options = SimpleStreamOptions {
            stream: StreamOptions {
                headers: Some(option_headers),
                ..Default::default()
            },
            ..SimpleStreamOptions::default()
        };
        let (_, headers, _, _) = capture_simple(&server, &model, &ctx, &options).await;
        assert_eq!(body_of(&headers, "user-agent"), Some("custom-agent"));
    }

    // ---- 3. request body fields (base-url oracle payloads) ----

    #[tokio::test]
    async fn clamps_prompt_cache_key_to_64_characters() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &completed_sse()).await;
        let model = model(&format!("{}/v1", server.uri()));
        let ctx = ctx_with(vec![user_msg("hello")], None);
        let options = SimpleStreamOptions {
            stream: StreamOptions {
                session_id: Some("x".repeat(67)),
                ..Default::default()
            },
            ..SimpleStreamOptions::default()
        };
        let (_, _, body, _) = capture_simple(&server, &model, &ctx, &options).await;
        assert_eq!(body["prompt_cache_key"], json!("x".repeat(64)));
    }

    #[tokio::test]
    async fn omits_prompt_cache_key_without_a_session() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &completed_sse()).await;
        let model = model(&format!("{}/v1", server.uri()));
        let ctx = ctx_with(vec![user_msg("hello")], None);
        let (_, _, body, _) =
            capture_simple(&server, &model, &ctx, &SimpleStreamOptions::default()).await;
        assert!(body.get("prompt_cache_key").is_none(), "{body}");
    }

    #[tokio::test]
    async fn floors_max_output_tokens_at_sixteen() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &completed_sse()).await;
        let model = model(&format!("{}/v1", server.uri()));
        let ctx = ctx_with(vec![user_msg("hello")], None);
        let options = SimpleStreamOptions {
            stream: StreamOptions {
                max_tokens: Some(8),
                ..Default::default()
            },
            ..SimpleStreamOptions::default()
        };
        let (_, _, body, _) = capture_simple(&server, &model, &ctx, &options).await;
        assert_eq!(body["max_output_tokens"], json!(16));
    }

    #[tokio::test]
    async fn stream_simple_fills_context_clamped_default_max_tokens() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &completed_sse()).await;
        let model = model(&format!("{}/v1", server.uri()));
        let ctx = ctx_with(vec![user_msg("hello")], None);
        let (_, _, body, _) =
            capture_simple(&server, &model, &ctx, &SimpleStreamOptions::default()).await;
        // buildBaseOptions: clamp(maxTokens ?? model.maxTokens) to the context
        // window; 128000 fits comfortably in 400000.
        assert_eq!(body["max_output_tokens"], json!(128000));
    }

    #[tokio::test]
    async fn honors_supports_strict_mode_false() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &completed_sse()).await;
        let ctx = ctx_with(
            vec![user_msg("Use a tool.")],
            Some(vec![strict_schema_tool("preferred", Strict::Prefer)]),
        );
        // Default: supportsStrictMode ?? true, so the strict tri-state ships.
        let default_model = model(&format!("{}/v1", server.uri()));
        let (_, _, body, _) = capture_simple(
            &server,
            &default_model,
            &ctx,
            &SimpleStreamOptions::default(),
        )
        .await;
        assert_eq!(body["tools"][0]["strict"], json!(true));

        // supportsStrictMode: false drops the strict field entirely.
        let model = model_with_compat(
            &format!("{}/v1", server.uri()),
            json!({"supportsStrictMode": false}),
        );
        let (_, _, body, _) =
            capture_simple(&server, &model, &ctx, &SimpleStreamOptions::default()).await;
        assert!(body["tools"][0].get("strict").is_none(), "{body}");
        // No retries: exactly one request per stream call.
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn forwards_temperature() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &completed_sse()).await;
        let model = model(&format!("{}/v1", server.uri()));
        let ctx = ctx_with(vec![user_msg("hello")], None);
        let options = SimpleStreamOptions {
            stream: StreamOptions {
                temperature: Some(0.5),
                ..Default::default()
            },
            ..SimpleStreamOptions::default()
        };
        let (_, _, body, _) = capture_simple(&server, &model, &ctx, &options).await;
        assert_eq!(body["temperature"], json!(0.5));
    }

    // ---- 4. tool choice (azure-openai-tool-choice oracle) ----

    #[tokio::test]
    async fn forwards_provider_neutral_tool_choice_from_simple_options() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &completed_sse()).await;
        let model = model(&format!("{}/v1", server.uri()));
        let ctx = ctx_with(vec![user_msg("Summarize this")], Some(vec![tool("read")]));
        let options = SimpleStreamOptions {
            tool_choice: Some(ToolChoice::None),
            ..SimpleStreamOptions::default()
        };
        let (_, _, body, _) = capture_simple(&server, &model, &ctx, &options).await;
        assert_eq!(body["tool_choice"], json!("none"));
        assert_eq!(body["tools"].as_array().map(Vec::len), Some(1));
    }

    #[tokio::test]
    async fn direct_stream_calls_have_no_tool_choice_surface() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &completed_sse()).await;
        let model = model(&format!("{}/v1", server.uri()));
        let ctx = ctx_with(vec![user_msg("Summarize this")], Some(vec![tool("read")]));
        // The direct `stream` path (not streamSimple): tools still forward,
        // but the upstream API-specific `toolChoice` extension (e.g.
        // "required") has no port option surface, so no tool_choice ships.
        let events = collect_stream(&server, &model, &ctx, &StreamOptions::default()).await;
        assert!(matches!(
            events.last(),
            Some(AssistantMessageEvent::Done { .. })
        ));
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert!(body.get("tool_choice").is_none(), "{body}");
        assert_eq!(body["tools"].as_array().map(Vec::len), Some(1));
    }

    // ---- 5. reasoning effort mapping ----

    #[tokio::test]
    async fn default_off_branch_sends_none_effort() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &completed_sse()).await;
        // thinkingLevelMap absent: `?.off !== null` holds, effort "none".
        let model = reasoning_model(&format!("{}/v1", server.uri()));
        let ctx = ctx_with(vec![user_msg("hello")], None);
        let (_, _, body, _) =
            capture_simple(&server, &model, &ctx, &SimpleStreamOptions::default()).await;
        assert_eq!(body["reasoning"], json!({"effort": "none"}));
        assert!(body.get("include").is_none(), "{body}");
    }

    #[tokio::test]
    async fn off_mapped_null_omits_reasoning() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &completed_sse()).await;
        let model = model_with_map(&format!("{}/v1", server.uri()), json!({"off": null}));
        let ctx = ctx_with(vec![user_msg("hello")], None);
        let (_, _, body, _) =
            capture_simple(&server, &model, &ctx, &SimpleStreamOptions::default()).await;
        assert!(body.get("reasoning").is_none(), "{body}");
    }

    #[tokio::test]
    async fn mapped_off_value_is_sent_verbatim() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &completed_sse()).await;
        let model = model_with_map(&format!("{}/v1", server.uri()), json!({"off": "low"}));
        let ctx = ctx_with(vec![user_msg("hello")], None);
        let (_, _, body, _) =
            capture_simple(&server, &model, &ctx, &SimpleStreamOptions::default()).await;
        assert_eq!(body["reasoning"], json!({"effort": "low"}));
    }

    #[tokio::test]
    async fn requested_effort_maps_through_thinking_level_map() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &completed_sse()).await;
        let model = model_with_map(&format!("{}/v1", server.uri()), json!({"minimal": "low"}));
        let ctx = ctx_with(vec![user_msg("hello")], None);
        let options = SimpleStreamOptions {
            reasoning: Some(ThinkingLevel::Minimal),
            ..SimpleStreamOptions::default()
        };
        let (_, _, body, _) = capture_simple(&server, &model, &ctx, &options).await;
        assert_eq!(
            body["reasoning"],
            json!({"effort": "low", "summary": "auto"})
        );
        assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
    }

    #[tokio::test]
    async fn unmapped_effort_is_sent_verbatim_with_auto_summary() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &completed_sse()).await;
        let model = reasoning_model(&format!("{}/v1", server.uri()));
        let ctx = ctx_with(vec![user_msg("hello")], None);
        let options = SimpleStreamOptions {
            reasoning: Some(ThinkingLevel::High),
            ..SimpleStreamOptions::default()
        };
        let (_, _, body, _) = capture_simple(&server, &model, &ctx, &options).await;
        assert_eq!(
            body["reasoning"],
            json!({"effort": "high", "summary": "auto"})
        );
        assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
    }

    #[tokio::test]
    async fn non_reasoning_model_omits_reasoning_entirely() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &completed_sse()).await;
        let model = model(&format!("{}/v1", server.uri()));
        let ctx = ctx_with(vec![user_msg("hello")], None);
        let options = SimpleStreamOptions {
            reasoning: Some(ThinkingLevel::High),
            ..SimpleStreamOptions::default()
        };
        let (_, _, body, _) = capture_simple(&server, &model, &ctx, &options).await;
        assert!(body.get("reasoning").is_none(), "{body}");
        assert!(body.get("include").is_none(), "{body}");
    }

    // ---- 6. setup errors ----

    #[tokio::test]
    async fn missing_api_key_is_a_lone_error_event() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &completed_sse()).await;
        let model = model(&format!("{}/v1", server.uri()));
        let ctx = ctx_with(vec![user_msg("hello")], None);
        let api = AzureOpenAiResponses;
        let mut request_cfg = cfg(&server);
        request_cfg.api_key = String::new();
        let mut rx = api.stream_simple(&request_cfg, &model, &ctx, &SimpleStreamOptions::default());
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        let (reason, error) = error_of(&events);
        assert_eq!(reason, ErrorReason::Error);
        assert_eq!(
            error.error_message.as_deref(),
            Some("No API key for provider: azure-openai-responses")
        );
        assert_eq!(events.len(), 1);
        assert_eq!(error.api, API);
        assert!(apply_all(&events).is_terminal());
    }

    #[tokio::test]
    async fn options_api_key_beats_provider_config() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &completed_sse()).await;
        let model = model(&format!("{}/v1", server.uri()));
        let ctx = ctx_with(vec![user_msg("hello")], None);
        let options = SimpleStreamOptions {
            stream: StreamOptions {
                api_key: Some("options-key".to_string()),
                ..Default::default()
            },
            ..SimpleStreamOptions::default()
        };
        let (_, headers, _, events) = capture_simple(&server, &model, &ctx, &options).await;
        assert_eq!(body_of(&headers, "api-key"), Some("options-key"));
        assert!(matches!(
            events.last(),
            Some(AssistantMessageEvent::Done { .. })
        ));
    }

    #[tokio::test]
    async fn invalid_base_url_surfaces_as_the_error_event() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &completed_sse()).await;
        let model = model("not-a-url");
        let ctx = ctx_with(vec![user_msg("hello")], None);
        // Pin the invalid URL through the scoped env so the helper's pinned
        // endpoint does not override it (env beats model.baseUrl).
        let options = SimpleStreamOptions {
            stream: StreamOptions {
                env: Some(env_map(&[("AZURE_OPENAI_BASE_URL", "not-a-url")])),
                ..Default::default()
            },
            ..SimpleStreamOptions::default()
        };
        let events = collect_simple(&server, &model, &ctx, &options).await;
        let (reason, error) = error_of(&events);
        assert_eq!(reason, ErrorReason::Error);
        assert!(error
            .error_message
            .as_deref()
            .unwrap_or_default()
            .contains("Invalid Azure OpenAI base URL"));
        assert_eq!(events.len(), 1, "lone error event: {events:?}");
        assert_eq!(error.api, API);
    }

    #[tokio::test]
    async fn http_error_surfaces_with_the_azure_prefix() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/responses"))
            .respond_with(wiremock::ResponseTemplate::new(401).set_body_string("bad key"))
            .mount(&server)
            .await;
        let model = model(&format!("{}/v1", server.uri()));
        let ctx = ctx_with(vec![user_msg("hello")], None);
        let events = collect_simple(&server, &model, &ctx, &SimpleStreamOptions::default()).await;
        let (reason, error) = error_of(&events);
        assert_eq!(reason, ErrorReason::Error);
        // Non-JSON bodies render their raw (truncated) text.
        assert_eq!(
            error.error_message.as_deref(),
            Some("Azure OpenAI API error (401): bad key")
        );
        assert_eq!(events.len(), 1, "lone error event before start: {events:?}");
    }

    #[tokio::test]
    async fn retries_a_429_before_the_first_stream_byte() {
        let server = wiremock::MockServer::start().await;

        /// First response is an HTTP error, second (and later) succeed.
        struct Flaky {
            attempts: AtomicU32,
        }
        impl wiremock::Respond for Flaky {
            fn respond(&self, _request: &wiremock::Request) -> wiremock::ResponseTemplate {
                if self.attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                    wiremock::ResponseTemplate::new(429)
                        .insert_header("retry-after-ms", "15")
                        .set_body_string("rate limited")
                } else {
                    sse(&completed_sse())
                }
            }
        }

        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/responses"))
            .respond_with(Flaky {
                attempts: AtomicU32::new(0),
            })
            .mount(&server)
            .await;
        let model = model(&format!("{}/v1", server.uri()));
        let ctx = ctx_with(vec![user_msg("hello")], None);
        let options = SimpleStreamOptions {
            stream: StreamOptions {
                max_retries: Some(1),
                ..Default::default()
            },
            ..SimpleStreamOptions::default()
        };
        let events = collect_simple(&server, &model, &ctx, &options).await;
        assert_eq!(event_types(&events), ["start", "done"]);
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    // ---- 7. reasoning replay (azure-openai-responses-reasoning-replay
    //         oracle, over the wire) ----

    fn replay_ctx(assistant: &AssistantMessage) -> TranscriptContext {
        ctx_with(
            vec![
                user_msg("first"),
                Message::Assistant(assistant.clone()),
                user_msg("follow-up"),
            ],
            None,
        )
    }

    fn replayed_reasoning_item(body: &Value) -> &Value {
        body["input"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item.get("type").and_then(Value::as_str) == Some("reasoning"))
            .expect("reasoning item in replay input")
    }

    #[tokio::test]
    async fn backfills_encrypted_content_for_replay_when_done_omitted_it() {
        let server = wiremock::MockServer::start().await;

        // Stream 1: done item without encrypted_content; the terminal
        // response carries it (the Azure pi-issue-#6409 shape).
        let body = [
            format!(
                "data: {}\n\n",
                json!({
                    "type": "response.output_item.added",
                    "output_index": 0,
                    "item": {"type": "reasoning", "id": "rs_missing", "summary": []},
                })
            ),
            format!(
                "data: {}\n\n",
                json!({
                    "type": "response.output_item.done",
                    "output_index": 0,
                    "item": {"type": "reasoning", "id": "rs_missing", "summary": []},
                })
            ),
            format!(
                "data: {}\n\n",
                json!({
                    "type": "response.completed",
                    "response": {
                        "id": "resp_test",
                        "status": "completed",
                        "output": [
                            {"type": "reasoning", "id": "rs_missing", "summary": [],
                             "encrypted_content": "from-response-completed"}
                        ],
                    },
                })
            ),
        ]
        .concat();
        mount(&server, &body).await;

        let model = reasoning_model(&format!("{}/v1", server.uri()));
        let ctx = ctx_with(vec![user_msg("first")], None);
        let events = collect_simple(&server, &model, &ctx, &SimpleStreamOptions::default()).await;
        let assistant = match events.last() {
            Some(AssistantMessageEvent::Done { message, .. }) => message.clone(),
            other => panic!("expected done, got {other:?}"),
        };

        // Stream 2: replay the assistant message; the stored reasoning item
        // must carry the backfilled encrypted content.
        server.reset().await;
        mount(&server, &completed_sse()).await;
        let replay = replay_ctx(&assistant);
        let (_, _, body, _) =
            capture_simple(&server, &model, &replay, &SimpleStreamOptions::default()).await;
        let item = replayed_reasoning_item(&body);
        assert_eq!(item["id"], json!("rs_missing"));
        assert_eq!(item["encrypted_content"], json!("from-response-completed"));
    }

    #[tokio::test]
    async fn preserves_existing_encrypted_content_from_output_item_done() {
        let server = wiremock::MockServer::start().await;
        let body = [
            format!(
                "data: {}\n\n",
                json!({
                    "type": "response.output_item.added",
                    "output_index": 0,
                    "item": {"type": "reasoning", "id": "rs_done", "summary": []},
                })
            ),
            format!(
                "data: {}\n\n",
                json!({
                    "type": "response.output_item.done",
                    "output_index": 0,
                    "item": {"type": "reasoning", "id": "rs_done", "summary": [],
                             "encrypted_content": "from-output-item-done"},
                })
            ),
            format!(
                "data: {}\n\n",
                json!({
                    "type": "response.completed",
                    "response": {
                        "id": "resp_test",
                        "status": "completed",
                        "output": [
                            {"type": "reasoning", "id": "rs_done", "summary": [],
                             "encrypted_content": "from-response-completed"}
                        ],
                    },
                })
            ),
        ]
        .concat();
        mount(&server, &body).await;

        let model = reasoning_model(&format!("{}/v1", server.uri()));
        let ctx = ctx_with(vec![user_msg("first")], None);
        let events = collect_simple(&server, &model, &ctx, &SimpleStreamOptions::default()).await;
        let assistant = match events.last() {
            Some(AssistantMessageEvent::Done { message, .. }) => message.clone(),
            other => panic!("expected done, got {other:?}"),
        };

        server.reset().await;
        mount(&server, &completed_sse()).await;
        let replay = replay_ctx(&assistant);
        let (_, _, body, _) =
            capture_simple(&server, &model, &replay, &SimpleStreamOptions::default()).await;
        let item = replayed_reasoning_item(&body);
        assert_eq!(item["id"], json!("rs_done"));
        assert_eq!(item["encrypted_content"], json!("from-output-item-done"));
    }

    /// The oracle replays through `convertResponsesMessages` directly; pin
    /// the unsigned-thinking drop rule at the azure conversion level too (a
    /// thinking block without a signature must not become a reasoning item).
    #[tokio::test]
    async fn unsigned_thinking_is_dropped_on_replay() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &completed_sse()).await;

        let assistant = AssistantMessage {
            content: vec![AssistantBlock::Thinking(ThinkingContent {
                thinking: "unsigned".to_string(),
                thinking_signature: None,
                redacted: None,
            })],
            api: API.to_string(),
            provider: API.to_string(),
            model: "gpt-5-mini".to_string(),
            response_model: None,
            response_id: None,
            provider_thinking_level: None,
            diagnostics: None,
            usage: crate::ai::types::primitives::Usage::default(),
            stop_reason: StopReason::Stop,
            deferred: None,
            error_message: None,
            raw_stop_reason: None,
            end_turn: None,
            timestamp: TS,
        };
        let model = reasoning_model(&format!("{}/v1", server.uri()));
        let replay = replay_ctx(&assistant);
        let (_, _, body, _) =
            capture_simple(&server, &model, &replay, &SimpleStreamOptions::default()).await;
        assert!(replayed_reasoning_item_or_none(&body).is_none(), "{body}");
    }

    fn replayed_reasoning_item_or_none(body: &Value) -> Option<&Value> {
        body["input"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item.get("type").and_then(Value::as_str) == Some("reasoning"))
    }

    // ---- abort surface ----

    /// A pre-cancelled signal fails the request setup (the retry seam's
    /// `"Request aborted"` abort error) before `Start`, and the catch block
    /// settles `stopReason: "aborted"`.
    #[tokio::test]
    async fn pre_aborted_request_settles_aborted_before_start() {
        let server = wiremock::MockServer::start().await;
        let token = CancellationToken::new();
        token.cancel();
        let options = SimpleStreamOptions {
            stream: StreamOptions {
                signal: Some(token),
                ..StreamOptions::default()
            },
            ..SimpleStreamOptions::default()
        };
        let api = AzureOpenAiResponses;
        let mut rx = api.stream_simple(
            &cfg(&server),
            &model("https://unused.example.com"),
            &ctx_with(vec![user_msg("hi")], None),
            &options,
        );
        let first = rx.recv().await.expect("terminal event");
        match first {
            AssistantMessageEvent::Error { reason, error } => {
                assert_eq!(reason, ErrorReason::Aborted);
                assert_eq!(error.stop_reason, StopReason::Aborted);
                assert_eq!(error.error_message.as_deref(), Some(REQUEST_ABORTED));
            }
            other => panic!("expected terminal error event, got {other:?}"),
        }
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    /// A cancellation after `start` breaks the body wait and settles the
    /// stream aborted with `"Request was aborted"` (upstream line 135).
    #[tokio::test]
    async fn mid_stream_cancellation_settles_the_stream_aborted() {
        let base_url = stalled_sse_server().await;
        let token = CancellationToken::new();
        let options = SimpleStreamOptions {
            stream: StreamOptions {
                signal: Some(token.clone()),
                ..StreamOptions::default()
            },
            ..SimpleStreamOptions::default()
        };
        let api = AzureOpenAiResponses;
        let mut rx = api.stream_simple(
            &cfg(&wiremock::MockServer::start().await),
            &model(&base_url),
            &ctx_with(vec![user_msg("hi")], None),
            &options,
        );
        let first = rx.recv().await.expect("start event");
        assert!(matches!(first, AssistantMessageEvent::Start { .. }));
        token.cancel();
        match rx.recv().await.expect("terminal event") {
            AssistantMessageEvent::Error { reason, error } => {
                assert_eq!(reason, ErrorReason::Aborted);
                assert_eq!(error.stop_reason, StopReason::Aborted);
                assert_eq!(error.error_message.as_deref(), Some(REQUEST_WAS_ABORTED));
            }
            other => panic!("expected terminal error event, got {other:?}"),
        }
    }
}
