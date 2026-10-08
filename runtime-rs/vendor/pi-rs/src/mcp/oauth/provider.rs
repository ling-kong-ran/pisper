//! The default stateful OAuth provider for one exact MCP server URL, ported
//! from upstream `packages/mcp/src/oauth/provider.ts`.
//!
//! `McpOAuthState` is carried as the raw JSON object the upstream spreads
//! build: incremental updates (`{ ...state, key: value }`) replace existing
//! keys in position or append, and `invalidateCredentials` deletes — so the
//! serialized state shape (the oracle's `rawState`) matches the capture.
//! Stored state for another server URL is ignored so credentials never leak
//! across servers (upstream `own`). Updates serialize through one async mutex,
//! mirroring the upstream `writes` promise chain.
//!
//! Port notes (disclosed divergences):
//! - `Date.now()` maps to an injectable `now_ms` clock (system time by
//!   default); the oracle pins `FIXED_NOW`.
//! - `crypto.getRandomValues` for `state()` maps to 32 OS-random bytes, hex
//!   encoded; the oracle injects the capture's deterministic stream.

use std::sync::Arc;

use futures::future::BoxFuture;
use serde_json::{Map, Value};
use tokio::sync::Mutex;

use crate::mcp::oauth::errors::OAuthFlowError;
use crate::mcp::oauth::flow::{CredentialKind, OAuthClientMetadataDocument, OAuthClientProvider};
use crate::mcp::oauth::types::{
    AuthorizationServerMetadata, OAuthClientInformationMixed, OAuthClientMetadata,
    OAuthDiscoveryState, OAuthTokens,
};

/// Upstream `McpOAuthState`: the persisted state for one server URL. The raw
/// object carrier preserves the upstream update order; the accessors read the
/// typed fields.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct McpOAuthState {
    raw: Map<String, Value>,
}

impl McpOAuthState {
    /// A fresh state for `server_url` (upstream `{ serverUrl }`).
    pub fn new(server_url: impl Into<String>) -> Self {
        let mut raw = Map::new();
        raw.insert("serverUrl".to_string(), Value::from(server_url.into()));
        McpOAuthState { raw }
    }

    pub fn server_url(&self) -> String {
        self.string("serverUrl")
    }

    pub fn client_information(&self) -> Option<OAuthClientInformationMixed> {
        self.object("clientInformation")
            .map(OAuthClientInformationMixed::from_raw)
    }

    pub fn tokens(&self) -> Option<OAuthTokens> {
        let raw = self.object("tokens")?;
        Some(OAuthTokens {
            access_token: raw.get("access_token")?.as_str()?.to_string(),
            token_type: raw.get("token_type")?.as_str()?.to_string(),
            expires_in: raw.get("expires_in").and_then(Value::as_f64),
            scope: raw.get("scope").and_then(Value::as_str).map(str::to_string),
            refresh_token: raw
                .get("refresh_token")
                .and_then(Value::as_str)
                .map(str::to_string),
            id_token: raw
                .get("id_token")
                .and_then(Value::as_str)
                .map(str::to_string),
        })
    }

    /// When the access token expires, in milliseconds since the epoch, from
    /// `expires_in` at the time it was saved.
    pub fn tokens_expire_at(&self) -> Option<f64> {
        self.raw.get("tokensExpireAt").and_then(Value::as_f64)
    }

    pub fn code_verifier(&self) -> Option<String> {
        self.string_opt("codeVerifier")
    }

    pub fn oauth_state(&self) -> Option<String> {
        self.string_opt("oauthState")
    }

    pub fn discovery(&self) -> Option<OAuthDiscoveryState> {
        OAuthDiscoveryState::from_value(&Value::Object(self.object("discovery")?))
    }

    /// The full stored object (the oracle's `rawState`).
    pub fn raw(&self) -> &Map<String, Value> {
        &self.raw
    }

    /// Replace the stored object wholesale (used by stores).
    pub fn from_raw(raw: Map<String, Value>) -> Self {
        McpOAuthState { raw }
    }

    fn string(&self, key: &str) -> String {
        self.string_opt(key).unwrap_or_default()
    }

    fn string_opt(&self, key: &str) -> Option<String> {
        self.raw
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    fn object(&self, key: &str) -> Option<Map<String, Value>> {
        self.raw.get(key).and_then(Value::as_object).cloned()
    }

    // -- update helpers (upstream `{ ...state, key: value }` spreads) -------

    fn set(&mut self, key: &str, value: Value) {
        self.raw.insert(key.to_string(), value);
    }

    fn set_object(&mut self, key: &str, value: Map<String, Value>) {
        self.raw.insert(key.to_string(), Value::Object(value));
    }

    fn remove(&mut self, key: &str) {
        self.raw.remove(key);
    }
}

/// Upstream `McpOAuthStateStore`: durable state storage for the provider.
pub trait McpOAuthStateStore: Send + Sync {
    /// Upstream `load()`.
    fn load(&self) -> BoxFuture<'_, Option<McpOAuthState>>;

    /// Upstream `save(state)`.
    fn save(&self, state: McpOAuthState) -> BoxFuture<'_, ()>;
}

/// Upstream `MemoryOAuthStateStore`: process-lifetime storage; every read and
/// write deep-clones (upstream `structuredClone`).
#[derive(Default)]
pub struct MemoryOAuthStateStore {
    value: std::sync::Mutex<Option<McpOAuthState>>,
}

impl MemoryOAuthStateStore {
    pub fn new() -> Self {
        MemoryOAuthStateStore::default()
    }
}

impl McpOAuthStateStore for MemoryOAuthStateStore {
    fn load(&self) -> BoxFuture<'_, Option<McpOAuthState>> {
        Box::pin(async move { self.value.lock().expect("store cannot be poisoned").clone() })
    }

    fn save(&self, state: McpOAuthState) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            *self.value.lock().expect("store cannot be poisoned") = Some(state);
        })
    }
}

/// The `clientMetadataDocument` callback (upstream the optional method):
/// resolves the Client ID Metadata Document for the server's metadata.
pub type ClientMetadataDocumentFn = Arc<
    dyn Fn(Option<&AuthorizationServerMetadata>) -> Option<OAuthClientMetadataDocument>
        + Send
        + Sync,
>;

/// Upstream `McpOAuthProviderOptions`.
#[derive(Clone)]
pub struct McpOAuthProviderOptions {
    pub server_url: String,
    pub redirect_url: String,
    /// The client metadata; `redirect_uris`, `grant_types`,
    /// `response_types` and `token_endpoint_auth_method` receive the upstream
    /// constructor defaults when absent.
    pub client_metadata: Map<String, Value>,
    /// See `OAuthClientProvider::client_metadata_document` (v1.0.0).
    pub client_metadata_document: Option<ClientMetadataDocumentFn>,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub store: Option<Arc<dyn McpOAuthStateStore>>,
    /// Upstream `onRedirect(url)`.
    pub on_redirect: Arc<dyn Fn(url::Url) -> BoxFuture<'static, ()> + Send + Sync>,
    /// Injectable wall clock for `tokensExpireAt` (milliseconds since the
    /// epoch); system time when absent. Test seam for the oracle's
    /// `FIXED_NOW`.
    pub now_ms: Option<Arc<dyn Fn() -> u64 + Send + Sync>>,
}

/// Upstream `McpOAuthProvider`: the default stateful provider for one exact
/// MCP server URL. Applications inject durable storage if needed.
pub struct McpOAuthProvider {
    redirect_url: String,
    client_metadata: OAuthClientMetadata,
    /// See `OAuthClientProvider::client_metadata_document` (v1.0.0).
    client_metadata_document: Option<ClientMetadataDocumentFn>,
    server_url: String,
    configured_client: Option<OAuthClientInformationMixed>,
    store: Arc<dyn McpOAuthStateStore>,
    on_redirect: Arc<dyn Fn(url::Url) -> BoxFuture<'static, ()> + Send + Sync>,
    /// Serializes read-modify-write updates (upstream `this.writes`).
    writes: Mutex<()>,
    now_ms: Option<Arc<dyn Fn() -> u64 + Send + Sync>>,
}

impl McpOAuthProvider {
    pub fn new(options: McpOAuthProviderOptions) -> Self {
        let server_url = normalized_url(&options.server_url);
        let redirect_url = normalized_url(&options.redirect_url);
        // The upstream constructor spread: explicit keys keep their position,
        // defaults append in the written order.
        let mut client_metadata = options.client_metadata.clone();
        let mut default_for = |key: &str, default: Value| {
            if !client_metadata.contains_key(key) {
                client_metadata.insert(key.to_string(), default);
            }
        };
        default_for("redirect_uris", serde_json::json!([redirect_url.clone()]));
        default_for(
            "grant_types",
            serde_json::json!(["authorization_code", "refresh_token"]),
        );
        default_for("response_types", serde_json::json!(["code"]));
        default_for(
            "token_endpoint_auth_method",
            Value::from(if options.client_secret.is_some() {
                "client_secret_post"
            } else {
                "none"
            }),
        );
        let configured_client = options.client_id.as_ref().map(|client_id| {
            let mut raw = Map::new();
            raw.insert("client_id".to_string(), Value::from(client_id.clone()));
            if let Some(client_secret) = &options.client_secret {
                raw.insert(
                    "client_secret".to_string(),
                    Value::from(client_secret.clone()),
                );
            }
            OAuthClientInformationMixed::from_raw(raw)
        });
        McpOAuthProvider {
            redirect_url,
            client_metadata: OAuthClientMetadata::from_raw(client_metadata),
            client_metadata_document: options.client_metadata_document.clone(),
            server_url,
            configured_client,
            store: options
                .store
                .unwrap_or_else(|| Arc::new(MemoryOAuthStateStore::new())),
            on_redirect: options.on_redirect,
            writes: Mutex::new(()),
            now_ms: options.now_ms,
        }
    }

    /// The metadata defaults the constructor applied (the oracle's
    /// `clientMetadataDefaults`).
    pub fn client_metadata(&self) -> OAuthClientMetadata {
        self.client_metadata.clone()
    }

    /// Upstream `redirectUrl`.
    pub fn redirect_url(&self) -> String {
        self.redirect_url.clone()
    }

    /// Upstream `state()`: reuse the stored OAuth state or generate one
    /// (64 hex characters).
    pub async fn state(&self) -> Result<String, OAuthFlowError> {
        if let Some(existing) = self.load().await.oauth_state() {
            return Ok(existing);
        }
        let bytes = random_bytes_32().await;
        let state: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        let for_storage = state.clone();
        self.update(Box::new(move |mut value| {
            value.set("oauthState", Value::from(for_storage));
            value
        }))
        .await?;
        Ok(state)
    }

    /// Upstream `clientInformation()`.
    pub async fn client_information(&self) -> Option<OAuthClientInformationMixed> {
        if self.configured_client.is_some() {
            return self.configured_client.clone();
        }
        self.load().await.client_information()
    }

    /// Upstream `saveClientInformation` (a configured client is never
    /// overwritten).
    pub async fn save_client_information(
        &self,
        information: OAuthClientInformationMixed,
    ) -> Result<(), OAuthFlowError> {
        if self.configured_client.is_some() {
            return Ok(());
        }
        self.update(Box::new(move |mut value| {
            value.set_object("clientInformation", information.raw().clone());
            value
        }))
        .await
    }

    /// Upstream `tokens()`.
    pub async fn tokens(&self) -> Option<OAuthTokens> {
        self.load().await.tokens()
    }

    /// Upstream `saveTokens`: derives `tokensExpireAt` from `expires_in`.
    pub async fn save_tokens(&self, tokens: OAuthTokens) -> Result<(), OAuthFlowError> {
        let expires_at = tokens
            .expires_in
            .map(|expires_in| self.now_ms() + expires_in * 1000.0);
        self.update(Box::new(move |mut value| {
            value.set_object(
                "tokens",
                tokens.to_value().as_object().expect("object").clone(),
            );
            match expires_at {
                Some(expires_at) => value.set("tokensExpireAt", number_value(expires_at)),
                None => value.remove("tokensExpireAt"),
            }
            value
        }))
        .await
    }

    /// Upstream `redirectToAuthorization`.
    pub async fn redirect_to_authorization(&self, url: url::Url) -> Result<(), OAuthFlowError> {
        (self.on_redirect)(url).await;
        Ok(())
    }

    /// Upstream `saveCodeVerifier`.
    pub async fn save_code_verifier(&self, verifier: String) -> Result<(), OAuthFlowError> {
        self.update(Box::new(move |mut value| {
            value.set("codeVerifier", Value::from(verifier.clone()));
            value
        }))
        .await
    }

    /// Upstream `codeVerifier()`.
    pub async fn code_verifier(&self) -> Result<String, OAuthFlowError> {
        self.load().await.code_verifier().ok_or_else(|| {
            OAuthFlowError::Other("No OAuth PKCE code verifier is stored".to_string())
        })
    }

    /// Upstream `invalidateCredentials`.
    pub async fn invalidate_credentials(&self, kind: CredentialKind) -> Result<(), OAuthFlowError> {
        self.update(Box::new(move |mut value| {
            if kind == CredentialKind::All || kind == CredentialKind::Client {
                value.remove("clientInformation");
            }
            if kind == CredentialKind::All || kind == CredentialKind::Tokens {
                value.remove("tokens");
                value.remove("tokensExpireAt");
            }
            if kind == CredentialKind::All || kind == CredentialKind::Verifier {
                value.remove("codeVerifier");
            }
            if kind == CredentialKind::All || kind == CredentialKind::Discovery {
                value.remove("discovery");
            }
            if kind == CredentialKind::All {
                value.remove("oauthState");
            }
            value
        }))
        .await
    }

    /// Upstream `saveDiscoveryState`.
    pub async fn save_discovery_state(
        &self,
        discovery: OAuthDiscoveryState,
    ) -> Result<(), OAuthFlowError> {
        self.update(Box::new(move |mut value| {
            value.set_object(
                "discovery",
                discovery.to_value().as_object().expect("object").clone(),
            );
            value
        }))
        .await
    }

    /// Upstream `discoveryState()`.
    pub async fn discovery_state(&self) -> Option<OAuthDiscoveryState> {
        self.load().await.discovery()
    }

    // -- internals ----------------------------------------------------------

    /// Upstream `load()`: the stored state scoped to this server URL.
    async fn load(&self) -> McpOAuthState {
        let state = self.store.load().await;
        self.own(state)
    }

    /// Upstream `update()`: serialized read-modify-write through the store.
    async fn update(
        &self,
        update: Box<dyn FnOnce(McpOAuthState) -> McpOAuthState + Send>,
    ) -> Result<(), OAuthFlowError> {
        let _guard = self.writes.lock().await;
        let state = self.own(self.store.load().await);
        self.store.save(update(state)).await;
        Ok(())
    }

    /// Stored state for another server URL is ignored so credentials never
    /// leak across servers (upstream `own`).
    fn own(&self, state: Option<McpOAuthState>) -> McpOAuthState {
        match state {
            Some(state) if state.server_url() == self.server_url => state,
            _ => McpOAuthState::new(self.server_url.clone()),
        }
    }

    fn now_ms(&self) -> f64 {
        match &self.now_ms {
            Some(now_ms) => now_ms() as f64,
            None => std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_millis() as f64)
                .unwrap_or(0.0),
        }
    }
}

/// Upstream `String(new URL(value))` normalization.
fn normalized_url(value: &str) -> String {
    url::Url::parse(value)
        .map(|url| url.to_string())
        .unwrap_or_else(|_| value.to_string())
}

/// JS numbers: integral values serialize without a fractional part
/// (`JSON.stringify(1758240060000.0) === "1758240060000"`).
fn number_value(value: f64) -> Value {
    if value.fract() == 0.0 && value.abs() <= 9.007_199_254_740_992e15 {
        return Value::from(value as i64);
    }
    serde_json::Number::from_f64(value)
        .map(Value::Number)
        .unwrap_or(Value::Null)
}

/// 32 random bytes for `state()`: the deterministic test stream under
/// `cfg(test)`, OS entropy otherwise.
async fn random_bytes_32() -> Vec<u8> {
    #[cfg(test)]
    return crate::mcp::test_rng::draw(32);
    #[cfg(not(test))]
    {
        let mut bytes = vec![0u8; 32];
        rand::fill(&mut bytes);
        bytes
    }
}

// -- OAuthClientProvider bridge ---------------------------------------------

impl OAuthClientProvider for McpOAuthProvider {
    fn redirect_url(&self) -> String {
        self.redirect_url.clone()
    }

    fn client_metadata(&self) -> OAuthClientMetadata {
        self.client_metadata.clone()
    }

    fn client_metadata_document(
        &self,
        metadata: Option<&AuthorizationServerMetadata>,
    ) -> Option<OAuthClientMetadataDocument> {
        self.client_metadata_document
            .as_ref()
            .and_then(|build| build(metadata))
    }

    fn state(&self) -> BoxFuture<'_, Result<String, OAuthFlowError>> {
        Box::pin(async move { McpOAuthProvider::state(self).await })
    }

    fn client_information(&self) -> BoxFuture<'_, Option<OAuthClientInformationMixed>> {
        Box::pin(async move { McpOAuthProvider::client_information(self).await })
    }

    fn saves_client_information(&self) -> bool {
        true
    }

    fn save_client_information(
        &self,
        information: OAuthClientInformationMixed,
    ) -> BoxFuture<'_, Result<(), OAuthFlowError>> {
        Box::pin(async move { McpOAuthProvider::save_client_information(self, information).await })
    }

    fn tokens(&self) -> BoxFuture<'_, Option<OAuthTokens>> {
        Box::pin(async move { McpOAuthProvider::tokens(self).await })
    }

    fn save_tokens(&self, tokens: OAuthTokens) -> BoxFuture<'_, Result<(), OAuthFlowError>> {
        Box::pin(async move { McpOAuthProvider::save_tokens(self, tokens).await })
    }

    fn redirect_to_authorization(
        &self,
        url: url::Url,
    ) -> BoxFuture<'_, Result<(), OAuthFlowError>> {
        Box::pin(async move { McpOAuthProvider::redirect_to_authorization(self, url).await })
    }

    fn save_code_verifier(&self, verifier: String) -> BoxFuture<'_, Result<(), OAuthFlowError>> {
        Box::pin(async move { McpOAuthProvider::save_code_verifier(self, verifier).await })
    }

    fn code_verifier(&self) -> BoxFuture<'_, Result<String, OAuthFlowError>> {
        Box::pin(async move { McpOAuthProvider::code_verifier(self).await })
    }

    fn invalidate_credentials(
        &self,
        kind: CredentialKind,
    ) -> BoxFuture<'_, Result<(), OAuthFlowError>> {
        Box::pin(async move { McpOAuthProvider::invalidate_credentials(self, kind).await })
    }

    fn save_discovery_state(
        &self,
        state: OAuthDiscoveryState,
    ) -> BoxFuture<'_, Result<(), OAuthFlowError>> {
        Box::pin(async move { McpOAuthProvider::save_discovery_state(self, state).await })
    }

    fn discovery_state(&self) -> BoxFuture<'_, Option<OAuthDiscoveryState>> {
        Box::pin(async move { McpOAuthProvider::discovery_state(self).await })
    }
}
