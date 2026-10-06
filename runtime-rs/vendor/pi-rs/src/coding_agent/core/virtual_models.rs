//! Port of upstream `coding-agent/src/core/virtual-models.ts` (HEAD
//! 2bbfcca43, v0.99.1): catalog entries that route each request to a
//! physical model.
//!
//! The selection (`model_change`, `agent.state.model`, `ctx.model`) may name
//! a virtual model. Everything below the routing step only sees physical
//! models: providers stream them and assistant messages record them. A
//! virtual model never reaches a provider.
//!
//! Byte-level behavior of the pure surface (catalog entry construction,
//! branch-selection/state lookups over session JSONL entries, the
//! `withVirtualModels` catalog wrapper) is pinned against the verbatim
//! upstream sources in `tests/fixtures/core_delta_oracle/virtual-models/`.
//!
//! # Seams for the later wiring slice (agent-session / ModelRuntime)
//!
//! - [`ModelRouteRequest::route`] handlers are registered on the
//!   [`ModelRuntime`](super::model_runtime::ModelRuntime) (upstream keeps a
//!   `virtualModels` map keyed by `provider/id` and wraps the provider's
//!   catalog with [`with_virtual_models`]). The runtime wiring itself is a
//!   later slice; here only the catalog wrapper and helpers live.
//! - Upstream `unroutedStream` produces a stream whose setup throws
//!   ``Virtual model {provider}/{id} must be routed before streaming``. The
//!   port's providers route streams through
//!   [`crate::ai::models::Provider::api_for`], which has no stream surface —
//!   the wrapper reports virtual models as unroutable
//!   ([`Provider::api_for`] → `None`), and [`unrouted_stream_error_message`]
//!   renders the exact upstream error text for the dispatch path that turns
//!   a missing API implementation into a stream error. [`unrouted_stream`]
//!   provides the port's settled-error-channel equivalent of the upstream
//!   `lazyStream` throw.
//! - Upstream `ModelRouteRequest.signal` is an `AbortSignal`; the port uses
//!   [`CancellationToken`] like the rest of the port.

use std::collections::HashSet;
use std::sync::Arc;

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::agent_core::types::AgentMessage;
use crate::ai::auth::types::{ApiKeyAuth, ApiKeyAuthInput, AuthError, AuthResult, ModelAuth};
use crate::ai::models::{Provider, RefreshModelsContext, RefreshModelsError};
use crate::ai::types::events::{AssistantMessageEvent, ErrorReason};
use crate::ai::types::message::AssistantMessage;
use crate::ai::types::primitives::{ModelCost, ModelThinkingLevel, ThinkingLevelMap};
use crate::ai::types::Model;
use crate::coding_agent::session_manager::SessionEntry;

/// API id of virtual catalog entries. Requests for it fail unless routed first.
pub const VIRTUAL_MODEL_API: &str = "pi-virtual";

/// Custom entry type that stores router state on the session branch.
pub const VIRTUAL_MODEL_STATE_ENTRY: &str = "pi.virtual-model-state";

/// Upstream module constant: every model thinking level, in union order.
pub const THINKING_LEVELS: [ModelThinkingLevel; 7] = [
    ModelThinkingLevel::Off,
    ModelThinkingLevel::Minimal,
    ModelThinkingLevel::Low,
    ModelThinkingLevel::Medium,
    ModelThinkingLevel::High,
    ModelThinkingLevel::Xhigh,
    ModelThinkingLevel::Max,
];

/// Data of a `pi.virtual-model-state` custom entry. `state` is the router's
/// own JSON value (upstream generic `TState = unknown`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VirtualModelStateData {
    pub provider: String,
    pub model_id: String,
    pub state: serde_json::Value,
}

/// Why a request is being routed.
/// - `user`: first request after a message the user wrote (prompt, steering,
///   or follow-up)
/// - `continuation`: any other request in the agent loop, e.g. after tool
///   results or extension messages
/// - `retry`: automatic retry after a failed request, including after
///   compaction for a context overflow
/// - `direct`: a request outside the agent loop, e.g. a compaction summary
///   or an extension call
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelRouteReason {
    User,
    Continuation,
    Retry,
    Direct,
}

impl ModelRouteReason {
    /// The upstream literal.
    pub fn as_str(&self) -> &'static str {
        match self {
            ModelRouteReason::User => "user",
            ModelRouteReason::Continuation => "continuation",
            ModelRouteReason::Retry => "retry",
            ModelRouteReason::Direct => "direct",
        }
    }
}

/// Upstream `ModelRouteRequest["previous"]`: physical model and thinking
/// level of the latest successful response in `messages`.
#[derive(Debug, Clone, PartialEq)]
pub struct PreviousRoute {
    pub model: Model,
    pub thinking_level: Option<ModelThinkingLevel>,
}

/// Upstream `ModelRouteRequest["failed"]`: the failed request, which
/// `messages` no longer contains. `message` carries its `stopReason` and
/// `errorMessage`. Absent when the router itself failed.
#[derive(Debug, Clone, PartialEq)]
pub struct FailedRoute {
    pub model: Model,
    pub thinking_level: Option<ModelThinkingLevel>,
    pub message: AssistantMessage,
}

/// Upstream `ModelRouteRequest`. The selected model is a virtual catalog
/// entry; the returned [`ModelRoute`] names the physical model to stream.
#[derive(Clone)]
pub struct ModelRouteRequest {
    /// The selected virtual model.
    pub model: Model,
    /// The selected thinking level. Its meaning is up to the router.
    pub thinking_level: ModelThinkingLevel,
    pub reason: ModelRouteReason,
    pub previous: Option<PreviousRoute>,
    pub failed: Option<FailedRoute>,
    /// Router state last returned on this session branch. `None` before the
    /// first state and for `direct` requests.
    pub state: Option<serde_json::Value>,
    /// Conversation for this request, including system messages.
    pub messages: Vec<crate::ai::types::Message>,
    pub signal: Option<CancellationToken>,
}

impl std::fmt::Debug for ModelRouteRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelRouteRequest")
            .field("model", &self.model)
            .field("thinking_level", &self.thinking_level)
            .field("reason", &self.reason.as_str())
            .field("previous", &self.previous)
            .field("failed", &self.failed)
            .field("state", &self.state)
            .field("messages", &self.messages.len())
            .field("signal", &self.signal.is_some())
            .finish()
    }
}

/// Physical model and thinking level for one request.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelRoute {
    pub model: Model,
    pub thinking_level: ModelThinkingLevel,
    /// New router state, stored on the session branch unless it is
    /// `request.state` itself. Return `request.state` or `None` to keep the
    /// current state. Must be JSON-serializable. Ignored for `direct`
    /// requests.
    pub state: Option<serde_json::Value>,
}

/// The result of one route call. Upstream routers may throw; the error is
/// carried as `Err` so the dispatch layer can surface it exactly like the
/// upstream rejected promise (failed routing leaves the virtual model on its
/// message).
pub type ModelRouteResult = Result<ModelRoute, String>;

/// Upstream `VirtualModelDefinition["route"]`: pick the physical model,
/// which must have credentials, and thinking level for one request.
pub type RouteFn =
    Arc<dyn Fn(ModelRouteRequest) -> BoxFuture<'static, ModelRouteResult> + Send + Sync>;

/// Upstream `VirtualModelDefinition`. The `route` seam is required; use
/// [`CreateVirtualModelOptions`] for the catalog-entry-only shape.
#[derive(Clone)]
pub struct VirtualModelDefinition {
    /// Provider the virtual model is listed under. May be a provider with
    /// physical models.
    pub provider: String,
    /// Model id. Must not be the id of a physical model of `provider`.
    pub id: String,
    pub name: String,
    /// Thinking levels offered for selection. Defaults to `["off"]`.
    pub thinking_levels: Option<Vec<ModelThinkingLevel>>,
    /// Limits shown before the first response. Afterwards, Pi uses the
    /// limits of the physical model that answered. Unset limits are
    /// unknown (0).
    pub context_window: Option<u64>,
    pub max_tokens: Option<u64>,
    /// Input types accepted for selection. Defaults to text and images;
    /// routed models without image support get placeholders.
    pub input: Option<Vec<crate::ai::types::ModelInput>>,
    /// Pick the physical model, which must have credentials, and thinking
    /// level for one request.
    pub route: RouteFn,
}

impl std::fmt::Debug for VirtualModelDefinition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VirtualModelDefinition")
            .field("provider", &self.provider)
            .field("id", &self.id)
            .field("name", &self.name)
            .field("thinking_levels", &self.thinking_levels)
            .field("context_window", &self.context_window)
            .field("max_tokens", &self.max_tokens)
            .field("input", &self.input)
            .field("route", &"<fn>")
            .finish()
    }
}

/// Upstream `Omit<VirtualModelDefinition, "route">` — the catalog-entry
/// shape [`create_virtual_model`] builds a [`Model`] from.
#[derive(Debug, Clone, PartialEq)]
pub struct CreateVirtualModelOptions {
    pub provider: String,
    pub id: String,
    pub name: String,
    pub thinking_levels: Option<Vec<ModelThinkingLevel>>,
    pub context_window: Option<u64>,
    pub max_tokens: Option<u64>,
    pub input: Option<Vec<crate::ai::types::ModelInput>>,
}

/// Whether a model or message names a virtual model. Failed routing leaves
/// the virtual model on its message. Upstream takes any `{ api }` shape;
/// the port carries the structural parameter through [`VirtualModelRef`].
pub fn is_virtual_model<V: VirtualModelRef + ?Sized>(model: &V) -> bool {
    model.virtual_model_api() == VIRTUAL_MODEL_API
}

/// The `{ api: string }` structural parameter of upstream `isVirtualModel`.
pub trait VirtualModelRef {
    fn virtual_model_api(&self) -> &str;
}

impl VirtualModelRef for Model {
    fn virtual_model_api(&self) -> &str {
        &self.api
    }
}

impl VirtualModelRef for AssistantMessage {
    fn virtual_model_api(&self) -> &str {
        &self.api
    }
}

impl VirtualModelRef for crate::ai::types::AnyModel {
    fn virtual_model_api(&self) -> &str {
        self.api()
    }
}

/// Latest successful response. Its model is physical: failed or aborted
/// requests, including failed routing, are skipped.
pub fn find_latest_response(messages: &[AgentMessage]) -> Option<&AssistantMessage> {
    for message in messages.iter().rev() {
        if let AgentMessage::Assistant(assistant) = message {
            if assistant.stop_reason != crate::ai::types::primitives::StopReason::Error
                && assistant.stop_reason != crate::ai::types::primitives::StopReason::Aborted
            {
                return Some(assistant);
            }
        }
    }
    None
}

/// Upstream `getBranchSelection`'s return shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchSelection {
    pub provider: String,
    pub model_id: String,
}

/// The model selection a session branch records. A virtual `model_change`
/// holds until the next `model_change`, because responses name the physical
/// models it routed to. Otherwise the latest physical response wins, as in
/// sessions without virtual models. A virtual model that is no longer
/// registered does not hold, so the selection falls back to the physical
/// model that answered last.
///
/// Only the last `model_change` can hold, so this looks up at most one model
/// in the catalog.
pub fn get_branch_selection(
    branch: &[SessionEntry],
    get_model: &dyn Fn(&str, &str) -> Option<Model>,
) -> Option<BranchSelection> {
    for (index, entry) in branch.iter().enumerate().rev() {
        if let SessionEntry::ModelChange(change) = entry {
            return Some(BranchSelection {
                provider: change.provider.clone(),
                model_id: change.model_id.clone(),
            });
        }
        if let SessionEntry::Message(message_entry) = entry {
            let AgentMessage::Assistant(assistant) = &message_entry.message else {
                continue;
            };
            if is_virtual_model(assistant) {
                continue;
            }
            let response = BranchSelection {
                provider: assistant.provider.clone(),
                model_id: assistant.model.clone(),
            };
            let change = find_last_model_change(branch, index);
            let model = change.and_then(|(provider, model_id)| get_model(provider, model_id));
            return match (change, model) {
                (Some((provider, model_id)), Some(model)) if is_virtual_model(&model) => {
                    Some(BranchSelection {
                        provider: provider.to_string(),
                        model_id: model_id.to_string(),
                    })
                }
                _ => Some(response),
            };
        }
    }
    None
}

fn find_last_model_change(branch: &[SessionEntry], before: usize) -> Option<(&str, &str)> {
    branch[..before].iter().rev().find_map(|entry| match entry {
        SessionEntry::ModelChange(change) => {
            Some((change.provider.as_str(), change.model_id.as_str()))
        }
        _ => None,
    })
}

/// Latest router state a session branch stores for a virtual model.
pub fn get_virtual_model_state(
    branch: &[SessionEntry],
    provider: &str,
    model_id: &str,
) -> Option<serde_json::Value> {
    for entry in branch.iter().rev() {
        let SessionEntry::Custom(custom) = entry else {
            continue;
        };
        if custom.custom_type != VIRTUAL_MODEL_STATE_ENTRY {
            continue;
        }
        let Some(data) = &custom.data else {
            continue;
        };
        let Ok(data) = serde_json::from_value::<VirtualModelStateData>(data.clone()) else {
            // Upstream reads `entry.data as VirtualModelStateData | undefined`
            // structurally; a differently shaped payload never matches.
            continue;
        };
        if data.provider == provider && data.model_id == model_id {
            return Some(data.state);
        }
    }
    None
}

/// Build the catalog entry of a virtual model.
pub fn create_virtual_model(definition: &CreateVirtualModelOptions) -> Model {
    let levels = definition
        .thinking_levels
        .clone()
        .unwrap_or_else(|| vec![ModelThinkingLevel::Off]);
    let mut thinking_level_map: ThinkingLevelMap = ThinkingLevelMap::new();
    for level in THINKING_LEVELS {
        let wire = serde_json::to_string(&level)
            .unwrap_or_default()
            .trim_matches('"')
            .to_string();
        let supported = levels.contains(&level);
        thinking_level_map.insert(wire.clone(), supported.then_some(wire));
    }
    Model {
        id: definition.id.clone(),
        name: definition.name.clone(),
        api: VIRTUAL_MODEL_API.to_string(),
        provider: definition.provider.clone(),
        base_url: String::new(),
        r#type: None,
        reasoning: levels.iter().any(|level| *level != ModelThinkingLevel::Off),
        thinking_level_map: Some(thinking_level_map),
        prompt_cache: None,
        input: definition.input.clone().unwrap_or_else(|| {
            vec![
                crate::ai::types::ModelInput::Text,
                crate::ai::types::ModelInput::Image,
            ]
        }),
        input_limits: None,
        cost: ModelCost::default(),
        context_window: definition.context_window.unwrap_or(0),
        max_tokens: definition.max_tokens.unwrap_or(0),
        sampling_params: None,
        sampling_params_by_thinking_level: None,
        headers: None,
        compat: None,
    }
}

/// Upstream `unroutedStream`'s setup error:
/// ``Virtual model {provider}/{id} must be routed before streaming``.
pub fn unrouted_stream_error_message(model: &Model) -> String {
    format!(
        "Virtual model {}/{} must be routed before streaming",
        model.provider, model.id
    )
}

/// Stream for a virtual model that was not routed, e.g. `stream()` with
/// API-specific options. Upstream builds a `lazyStream` whose setup throws;
/// the port settles the channel with the single setup-error event (the
/// port's `lazyStream` convention, see `model_runtime`).
pub fn unrouted_stream(
    model: &Model,
    _context: &crate::ai::transcript::Context,
) -> mpsc::Receiver<AssistantMessageEvent> {
    let message = AssistantMessage {
        content: Vec::new(),
        api: model.api.clone(),
        provider: model.provider.clone(),
        model: model.id.clone(),
        response_model: None,
        response_id: None,
        provider_thinking_level: None,
        diagnostics: None,
        usage: crate::ai::types::primitives::Usage::default(),
        stop_reason: crate::ai::types::primitives::StopReason::Error,
        deferred: None,
        error_message: Some(unrouted_stream_error_message(model)),
        raw_stop_reason: None,
        end_turn: None,
        timestamp: crate::ai::now_ms(),
    };
    let (tx, rx) = mpsc::channel(4);
    tokio::spawn(async move {
        let _ = tx
            .send(AssistantMessageEvent::Error {
                reason: ErrorReason::Error,
                error: message,
            })
            .await;
    });
    rx
}

/// The keyless provider's api-key auth: `resolve` always reports configured
/// with `source: "virtual"` (upstream `auth.apiKey` literal).
struct VirtualModelApiKeyAuth;

impl ApiKeyAuth for VirtualModelApiKeyAuth {
    fn name(&self) -> &str {
        "Virtual model"
    }

    fn resolve<'a>(
        &'a self,
        _input: ApiKeyAuthInput<'a>,
    ) -> BoxFuture<'a, Result<Option<AuthResult>, AuthError>> {
        Box::pin(async {
            Ok(Some(AuthResult {
                auth: ModelAuth::default(),
                env: None,
                source: Some("virtual".to_string()),
            }))
        })
    }
}

/// Upstream `withVirtualModels`: add virtual models to a provider's catalog.
/// Without a provider, the result is a keyless provider that only lists the
/// virtual models. A virtual model hides a physical chat model with the same
/// id, which a catalog refresh can add after registration. Availability
/// follows the provider's auth.
pub fn with_virtual_models(
    provider_id: &str,
    provider: Option<Arc<dyn Provider>>,
    virtual_models: Vec<Model>,
) -> Arc<dyn Provider> {
    Arc::new(VirtualModelsProvider::new(
        provider_id,
        provider,
        virtual_models,
    ))
}

/// The catalog wrapper implementing upstream's object literal.
pub struct VirtualModelsProvider {
    provider_id: String,
    inner: Option<Arc<dyn Provider>>,
    auth: crate::ai::auth::types::ProviderAuth,
    virtual_models: Vec<Model>,
    ids: HashSet<String>,
}

impl VirtualModelsProvider {
    fn new(
        provider_id: &str,
        inner: Option<Arc<dyn Provider>>,
        virtual_models: Vec<Model>,
    ) -> Self {
        let auth = match &inner {
            Some(provider) => provider.auth().clone(),
            None => crate::ai::auth::types::ProviderAuth {
                api_key: Some(Arc::new(VirtualModelApiKeyAuth)),
                oauth: None,
            },
        };
        let ids = virtual_models
            .iter()
            .map(|model| model.id.clone())
            .collect();
        Self {
            provider_id: provider_id.to_string(),
            inner,
            auth,
            virtual_models,
            ids,
        }
    }

    /// Upstream `physical(models)` over the chat list: drop virtual entries
    /// and physical chat models whose id a virtual model hides.
    fn physical_chat(&self, models: Vec<Model>) -> Vec<Model> {
        models
            .into_iter()
            .filter(|model| model.api != VIRTUAL_MODEL_API && !self.ids.contains(&model.id))
            .collect()
    }

    /// Upstream `physical(models)` over any-type lists.
    fn physical_any(
        &self,
        models: Vec<crate::ai::types::AnyModel>,
    ) -> Vec<crate::ai::types::AnyModel> {
        use crate::ai::ModelType;
        models
            .into_iter()
            .filter(|model| {
                model.api() != VIRTUAL_MODEL_API
                    && !(model.model_type() == ModelType::Chat && self.ids.contains(model.id()))
            })
            .collect()
    }

    /// Upstream `virtual(models)`.
    fn virtual_any(
        &self,
        models: Vec<crate::ai::types::AnyModel>,
    ) -> Vec<crate::ai::types::AnyModel> {
        models
            .into_iter()
            .filter(|model| model.api() == VIRTUAL_MODEL_API)
            .collect()
    }

    fn virtual_as_any(&self) -> Vec<crate::ai::types::AnyModel> {
        self.virtual_models
            .iter()
            .cloned()
            .map(crate::ai::types::AnyModel::Chat)
            .collect()
    }
}

impl Provider for VirtualModelsProvider {
    fn id(&self) -> &str {
        &self.provider_id
    }

    fn name(&self) -> &str {
        match &self.inner {
            Some(provider) => provider.name(),
            None => &self.provider_id,
        }
    }

    fn base_url(&self) -> Option<&str> {
        self.inner.as_ref().and_then(|provider| provider.base_url())
    }

    fn headers(&self) -> Option<&crate::ai::types::options::ProviderHeaders> {
        self.inner.as_ref().and_then(|provider| provider.headers())
    }

    fn auth(&self) -> &crate::ai::auth::types::ProviderAuth {
        &self.auth
    }

    fn get_models(&self) -> Result<Vec<Model>, crate::ai::auth::resolve::ModelsError> {
        let physical = match &self.inner {
            Some(provider) => self.physical_chat(provider.get_models()?),
            None => Vec::new(),
        };
        Ok([physical, self.virtual_models.clone()].concat())
    }

    fn get_all_models(
        &self,
    ) -> Result<Vec<crate::ai::types::AnyModel>, crate::ai::auth::resolve::ModelsError> {
        let physical = match &self.inner {
            // Upstream `provider.getAllModels?.() ?? provider.getModels()` —
            // the trait default already falls back to `get_models`.
            Some(provider) => self.physical_any(provider.get_all_models()?),
            None => Vec::new(),
        };
        Ok([physical, self.virtual_as_any()].concat())
    }

    /// Upstream keyless wrapper defines no `filterModels`; the wrapped
    /// wrapper always does (`filterModels?.(real, credential) ?? real`).
    fn filter_models(
        &self,
        models: &[Model],
        credential: Option<&crate::ai::auth::types::Credential>,
    ) -> Option<Vec<Model>> {
        let inner = self.inner.as_ref()?;
        let real: Vec<Model> = models
            .iter()
            .filter(|model| model.api != VIRTUAL_MODEL_API && !self.ids.contains(&model.id))
            .cloned()
            .collect();
        let virtual_models: Vec<Model> = models
            .iter()
            .filter(|model| model.api == VIRTUAL_MODEL_API)
            .cloned()
            .collect();
        let kept = inner.filter_models(&real, credential).unwrap_or(real);
        Some([kept, virtual_models].concat())
    }

    fn has_filter_models(&self) -> bool {
        self.inner.is_some()
    }

    /// Upstream `filterAllModels` is only defined when the wrapped provider
    /// has one (`filterAllModels && (...)`).
    fn filter_all_models(
        &self,
        models: &[crate::ai::types::AnyModel],
        credential: Option<&crate::ai::auth::types::Credential>,
    ) -> Option<Vec<crate::ai::types::AnyModel>> {
        let inner = self.inner.as_ref()?;
        if !inner.has_filter_all_models() {
            return None;
        }
        let physical = self.physical_any(models.to_vec());
        let virtual_models = self.virtual_any(models.to_vec());
        let filtered = inner.filter_all_models(&physical, credential)?;
        Some([filtered, virtual_models].concat())
    }

    fn has_filter_all_models(&self) -> bool {
        self.inner
            .as_ref()
            .map(|provider| provider.has_filter_all_models())
            .unwrap_or(false)
    }

    /// Upstream `stream`/`streamSimple` route virtual models to
    /// `unroutedStream`; the port's dispatch goes through `api_for`, so a
    /// virtual model reports no implementation (the wiring layer renders
    /// [`unrouted_stream_error_message`] for that case).
    fn api_for(&self, model: &Model) -> Option<Arc<dyn crate::ai::ApiImpl>> {
        if is_virtual_model(model) {
            return None;
        }
        self.inner
            .as_ref()
            .and_then(|provider| provider.api_for(model))
    }

    fn images_for(&self, api: &str) -> Option<Arc<dyn crate::ai::models::provider::ImagesApiImpl>> {
        self.inner
            .as_ref()
            .and_then(|provider| provider.images_for(api))
    }

    fn classifiers_for(
        &self,
        api: &str,
    ) -> Option<Arc<dyn crate::ai::models::provider::ClassifierApiImpl>> {
        self.inner
            .as_ref()
            .and_then(|provider| provider.classifiers_for(api))
    }

    fn is_dynamic(&self) -> bool {
        self.inner
            .as_ref()
            .map(|provider| provider.is_dynamic())
            .unwrap_or(false)
    }

    fn refresh_models(
        &self,
        context: RefreshModelsContext,
    ) -> Option<BoxFuture<'static, Result<(), RefreshModelsError>>> {
        self.inner
            .as_ref()
            .and_then(|provider| provider.refresh_models(context))
    }
}

#[cfg(test)]
#[path = "virtual_models_tests.rs"]
mod tests;
