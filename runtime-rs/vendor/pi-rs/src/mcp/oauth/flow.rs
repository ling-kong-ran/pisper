//! The OAuth authorization-code + PKCE flow over discovered metadata, ported
//! from upstream `packages/mcp/src/oauth/flow.ts` (itself adapted from
//! modelcontextprotocol/typescript-sdk v1.29.0 `src/client/auth.ts`):
//! `authorizeMcp` with its invalid-client/invalid-grant credential
//! invalidation retries, dynamic client registration, token exchange and
//! refresh, client-authentication method selection, and the
//! `adaptOAuthProvider` bridge to the HTTP transport's [`AuthProvider`].
//!
//! Port notes (disclosed divergences):
//! - PKCE randomness maps to a 32-byte OS draw; the oracle tests inject the
//!   capture's deterministic byte stream through [`crate::mcp::test_rng`].
//!   The S256 challenge is a real SHA-256 over the verifier, matching the
//!   capture's WebCrypto.
//! - `URLSearchParams` maps to [`FormParams`], an ordered pair list with the
//!   same serialization (space `+`, WHATWG percent-encoding) and `set`
//!   replace-in-position semantics.
//! - Upstream's in-flight refresh promise in `adaptOAuthProvider` maps to a
//!   shared-future slot: concurrent 401 handlers await one run, and the slot
//!   clears when it settles.

use std::sync::Arc;

use futures::future::BoxFuture;
use futures::FutureExt;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use url::Url;

use crate::mcp::auth_provider::{
    default_fetch, AuthProvider, FetchRequest, McpFetch, UnauthorizedContext,
};
use crate::mcp::oauth::discovery::{
    discover_authorization_server_metadata, discover_oauth_server_info, parse_www_authenticate,
    select_resource, DiscoveryOptions,
};
use crate::mcp::oauth::errors::{
    McpOAuthAuthorizationRequiredError, OAuthError, OAuthFlowError, OAuthInsecureEndpointError,
    OAuthIssuerMismatchError, OAuthRegistrationError,
};
use crate::mcp::oauth::types::{
    parse_client_information, parse_oauth_tokens, AuthorizationServerMetadata,
    OAuthClientInformationMixed, OAuthClientMetadata, OAuthDiscoveryState, OAuthTokens,
};

/// Upstream `AddClientAuthentication`: full control over the token request's
/// headers and form params (both are mutated in place, then sent).
pub type AddClientAuthentication = Arc<
    dyn for<'a> Fn(
            &'a mut Vec<(String, String)>,
            &'a mut FormParams,
            &'a Url,
            Option<&'a AuthorizationServerMetadata>,
        ) -> BoxFuture<'a, Result<(), OAuthFlowError>>
        + Send
        + Sync,
>;

/// Upstream `OAuthClientMetadataDocument` (v1.0.0): a Client ID Metadata
/// Document — an https URL used as `client_id`, and a redirect URI it lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthClientMetadataDocument {
    pub url: String,
    pub redirect_url: String,
}

/// Ordered `application/x-www-form-urlencoded` pairs with `URLSearchParams`
/// semantics: `set` replaces the first match in position or appends, and the
/// serialization is the WHATWG urlencoded form (space becomes `+`).
#[derive(Debug, Clone, Default)]
pub struct FormParams {
    pairs: Vec<(String, String)>,
}

impl FormParams {
    pub fn new() -> Self {
        FormParams { pairs: Vec::new() }
    }

    /// `URLSearchParams.set`.
    pub fn set(&mut self, name: impl Into<String>, value: impl Into<String>) {
        let name = name.into();
        let value = value.into();
        if let Some(slot) = self.pairs.iter_mut().find(|(key, _)| *key == name) {
            slot.1 = value;
            return;
        }
        self.pairs.push((name, value));
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.pairs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// The upstream `URLSearchParams.toString()`.
    pub fn to_body(&self) -> String {
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        for (name, value) in &self.pairs {
            serializer.append_pair(name, value);
        }
        serializer.finish()
    }
}

/// Upstream `OAuthClientProvider` (all methods may consult storage).
pub trait OAuthClientProvider: Send + Sync {
    /// Upstream `redirectUrl`.
    fn redirect_url(&self) -> String;

    /// Upstream `clientMetadata`.
    fn client_metadata(&self) -> OAuthClientMetadata;

    /// Upstream optional `clientMetadataDocument` (v1.0.0): a Client ID
    /// Metadata Document to identify as instead of registering dynamically,
    /// or `None` to register. Called when no client information is stored;
    /// the document is not stored. `metadata` is `None` when the
    /// authorization server has none; check
    /// `client_id_metadata_document_supported`.
    fn client_metadata_document(
        &self,
        _metadata: Option<&AuthorizationServerMetadata>,
    ) -> Option<OAuthClientMetadataDocument> {
        None
    }

    /// Upstream optional `state()`.
    fn state(&self) -> BoxFuture<'_, Result<String, OAuthFlowError>> {
        Box::pin(async { Err(OAuthFlowError::Other("state not supported".to_string())) })
    }

    /// Upstream `clientInformation()`.
    fn client_information(&self) -> BoxFuture<'_, Option<OAuthClientInformationMixed>>;

    /// Whether upstream's optional `saveClientInformation` hook exists.
    fn saves_client_information(&self) -> bool {
        false
    }

    /// Upstream optional `saveClientInformation`.
    fn save_client_information(
        &self,
        _information: OAuthClientInformationMixed,
    ) -> BoxFuture<'_, Result<(), OAuthFlowError>> {
        Box::pin(async { Ok(()) })
    }

    /// Upstream `tokens()`.
    fn tokens(&self) -> BoxFuture<'_, Option<OAuthTokens>>;

    /// Upstream `saveTokens`.
    fn save_tokens(&self, tokens: OAuthTokens) -> BoxFuture<'_, Result<(), OAuthFlowError>>;

    /// Upstream `redirectToAuthorization`.
    fn redirect_to_authorization(&self, url: Url) -> BoxFuture<'_, Result<(), OAuthFlowError>>;

    /// Upstream `saveCodeVerifier`.
    fn save_code_verifier(&self, verifier: String) -> BoxFuture<'_, Result<(), OAuthFlowError>>;

    /// Upstream `codeVerifier()`.
    fn code_verifier(&self) -> BoxFuture<'_, Result<String, OAuthFlowError>>;

    /// Upstream optional `addClientAuthentication`.
    fn add_client_authentication(&self) -> Option<AddClientAuthentication> {
        None
    }

    /// Upstream optional `invalidateCredentials`.
    fn invalidate_credentials(
        &self,
        _kind: CredentialKind,
    ) -> BoxFuture<'_, Result<(), OAuthFlowError>> {
        Box::pin(async { Ok(()) })
    }

    /// Upstream optional `saveDiscoveryState`.
    fn save_discovery_state(
        &self,
        _state: OAuthDiscoveryState,
    ) -> BoxFuture<'_, Result<(), OAuthFlowError>> {
        Box::pin(async { Ok(()) })
    }

    /// Upstream optional `discoveryState()`.
    fn discovery_state(&self) -> BoxFuture<'_, Option<OAuthDiscoveryState>> {
        Box::pin(async { None })
    }
}

/// Upstream `invalidateCredentials` kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialKind {
    All,
    Client,
    Tokens,
    Verifier,
    Discovery,
}

impl CredentialKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            CredentialKind::All => "all",
            CredentialKind::Client => "client",
            CredentialKind::Tokens => "tokens",
            CredentialKind::Verifier => "verifier",
            CredentialKind::Discovery => "discovery",
        }
    }
}

/// Upstream `OAuthFlowOptions`.
#[derive(Clone, Default)]
pub struct OAuthFlowOptions {
    pub server_url: String,
    pub authorization_code: Option<String>,
    /// `iss` parameter of the authorization response that delivered
    /// `authorization_code` (RFC 9207) — v1.0.0.
    pub iss: Option<String>,
    pub scope: Option<String>,
    pub resource_metadata_url: Option<Url>,
    /// Authorization server metadata document to use instead of discovery,
    /// for servers that advertise a wrong authorization server or none. It
    /// is trusted as configured. Must use https, except on loopback —
    /// v1.0.0.
    pub authorization_server_metadata_url: Option<Url>,
    pub fetch: Option<McpFetch>,
    pub skip_issuer_validation: bool,
    /// Go straight to the authorization redirect instead of refreshing stored
    /// tokens, for example when the server asks for scopes the current grant
    /// lacks (a refresh keeps the old scope).
    pub skip_refresh: bool,
}

/// Upstream `OAuthFlowResult`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OAuthFlowResult {
    Authorized,
    Redirect,
}

impl OAuthFlowResult {
    pub fn as_str(&self) -> &'static str {
        match self {
            OAuthFlowResult::Authorized => "AUTHORIZED",
            OAuthFlowResult::Redirect => "REDIRECT",
        }
    }
}

/// Upstream `TokenRequestOptions`.
#[derive(Clone, Default)]
pub struct TokenRequestOptions {
    pub metadata: Option<AuthorizationServerMetadata>,
    pub client_information: Option<OAuthClientInformationMixed>,
    pub resource: Option<String>,
    pub add_client_authentication: Option<AddClientAuthentication>,
    pub fetch: Option<McpFetch>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClientAuthMethod {
    ClientSecretBasic,
    ClientSecretPost,
    None,
}

fn loopback(hostname: &str) -> bool {
    hostname == "localhost" || hostname == "127.0.0.1" || hostname == "[::1]" || hostname == "::1"
}

/// Upstream `secureEndpoint`: HTTPS everywhere except loopback hosts.
fn secure_endpoint(value: &str) -> Result<Url, OAuthFlowError> {
    let url = Url::parse(value).map_err(|error| OAuthFlowError::Other(error.to_string()))?;
    let hostname = url.host_str().unwrap_or_default();
    if url.scheme() != "https" && !loopback(hostname) {
        return Err(OAuthFlowError::InsecureEndpoint(
            OAuthInsecureEndpointError {
                endpoint: url.to_string(),
            },
        ));
    }
    Ok(url)
}

/// Upstream `selectClientAuthMethod`.
fn select_client_auth_method(
    information: &OAuthClientInformationMixed,
    supported: &[String],
) -> ClientAuthMethod {
    if let Some(hinted) = information.token_endpoint_auth_method() {
        let hinted_method = match hinted.as_str() {
            "client_secret_basic" => Some(ClientAuthMethod::ClientSecretBasic),
            "client_secret_post" => Some(ClientAuthMethod::ClientSecretPost),
            "none" => Some(ClientAuthMethod::None),
            _ => None,
        };
        if let Some(method) = hinted_method {
            if supported.is_empty() || supported.iter().any(|item| item == &hinted) {
                return method;
            }
        }
    }
    let has_secret = information.client_secret().is_some();
    let supports = |method: &str| supported.iter().any(|item| item == method);
    if supported.is_empty() {
        return if has_secret {
            ClientAuthMethod::ClientSecretBasic
        } else {
            ClientAuthMethod::None
        };
    }
    if has_secret && supports("client_secret_basic") {
        return ClientAuthMethod::ClientSecretBasic;
    }
    if has_secret && supports("client_secret_post") {
        return ClientAuthMethod::ClientSecretPost;
    }
    if supports("none") {
        return ClientAuthMethod::None;
    }
    if has_secret {
        ClientAuthMethod::ClientSecretPost
    } else {
        ClientAuthMethod::None
    }
}

/// Replace-or-append with case-insensitive match (upstream `Headers.set`).
/// Public for provider implementations that build custom authentication.
pub fn set_header(headers: &mut Vec<(String, String)>, name: &str, value: &str) {
    if let Some(slot) = headers
        .iter_mut()
        .find(|(existing, _)| existing.eq_ignore_ascii_case(name))
    {
        slot.1 = value.to_string();
        return;
    }
    headers.push((name.to_string(), value.to_string()));
}

/// Upstream `applyClientAuthentication`.
fn apply_client_authentication(
    method: ClientAuthMethod,
    information: &OAuthClientInformationMixed,
    headers: &mut Vec<(String, String)>,
    params: &mut FormParams,
) -> Result<(), OAuthFlowError> {
    if method == ClientAuthMethod::ClientSecretBasic {
        let Some(client_secret) = information.client_secret() else {
            return Err(OAuthFlowError::Other(
                "client_secret_basic requires a client secret".to_string(),
            ));
        };
        let credentials =
            base64_encode(format!("{}:{}", information.client_id(), client_secret).as_bytes());
        // Upstream builds a `Headers` instance here, whose serialization
        // lowercases names; the port spells them lowercase directly.
        set_header(headers, "authorization", &format!("Basic {credentials}"));
    } else {
        params.set("client_id", information.client_id());
        if method == ClientAuthMethod::ClientSecretPost {
            if let Some(client_secret) = information.client_secret() {
                params.set("client_secret", client_secret);
            }
        }
    }
    Ok(())
}

/// Upstream `pkce()`: a 32-byte base64url verifier and its real S256
/// challenge.
async fn pkce() -> (String, String) {
    let bytes = random_bytes_32();
    let verifier = crate::ai::auth::oauth::pkce::base64url_encode(&bytes);
    let digest = Sha256::digest(verifier.as_bytes());
    let challenge = crate::ai::auth::oauth::pkce::base64url_encode(&digest);
    (verifier, challenge)
}

/// 32 bytes of randomness: the deterministic test stream under `cfg(test)`
/// (the capture stubs `crypto.getRandomValues`), OS entropy otherwise.
fn random_bytes_32() -> Vec<u8> {
    #[cfg(test)]
    return crate::mcp::test_rng::draw(32);
    #[cfg(not(test))]
    {
        let mut bytes = vec![0u8; 32];
        rand::fill(&mut bytes);
        bytes
    }
}

/// Standard base64 with padding (upstream
/// `Buffer.from(...).toString("base64")`); used for `client_secret_basic`.
fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let group = (b0 << 16) | (b1 << 8) | b2;
        encoded.push(ALPHABET[(group >> 18) as usize & 0x3f] as char);
        encoded.push(ALPHABET[(group >> 12) as usize & 0x3f] as char);
        encoded.push(if chunk.len() > 1 {
            ALPHABET[(group >> 6) as usize & 0x3f] as char
        } else {
            '='
        });
        encoded.push(if chunk.len() > 2 {
            ALPHABET[group as usize & 0x3f] as char
        } else {
            '='
        });
    }
    encoded
}

/// Upstream `startAuthorization`: build the authorization redirect URL with
/// the exact parameter order the oracle pins. Returns the URL and PKCE
/// verifier.
#[allow(clippy::too_many_arguments)]
pub async fn start_authorization(
    authorization_server_url: &str,
    metadata: Option<&AuthorizationServerMetadata>,
    client_information: &OAuthClientInformationMixed,
    redirect_url: &str,
    scope: Option<&str>,
    state: Option<&str>,
    resource: Option<&str>,
) -> Result<(Url, String), OAuthFlowError> {
    if let Some(metadata) = metadata {
        if !metadata
            .response_types_supported()
            .iter()
            .any(|response_type| response_type == "code")
        {
            return Err(OAuthFlowError::Other(
                "Authorization server does not support authorization codes".to_string(),
            ));
        }
        if let Some(challenge_methods) = metadata.code_challenge_methods_supported() {
            if !challenge_methods.iter().any(|method| method == "S256") {
                return Err(OAuthFlowError::Other(
                    "Authorization server does not support PKCE S256".to_string(),
                ));
            }
        }
    }
    let endpoint = match metadata {
        Some(metadata) => metadata.authorization_endpoint(),
        None => default_endpoint(authorization_server_url, "/authorize")?,
    };
    let mut url =
        Url::parse(&endpoint).map_err(|error| OAuthFlowError::Other(error.to_string()))?;
    let (verifier, challenge) = pkce().await;
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query.append_pair("response_type", "code");
    query.append_pair("client_id", &client_information.client_id());
    query.append_pair("code_challenge", &challenge);
    query.append_pair("code_challenge_method", "S256");
    query.append_pair("redirect_uri", redirect_url);
    if let Some(state) = state {
        query.append_pair("state", state);
    }
    if let Some(scope) = scope {
        query.append_pair("scope", scope);
    }
    if scope.is_some_and(|scope| {
        scope
            .split_whitespace()
            .any(|part| part == "offline_access")
    }) {
        query.append_pair("prompt", "consent");
    }
    if let Some(resource) = resource {
        query.append_pair("resource", resource);
    }
    url.set_query(Some(&query.finish()));
    Ok((url, verifier))
}

/// Upstream `new URL(path, authorizationServerUrl)` for the metadata-less
/// fallback endpoints.
fn default_endpoint(authorization_server_url: &str, path: &str) -> Result<String, OAuthFlowError> {
    let base = Url::parse(authorization_server_url)
        .map_err(|error| OAuthFlowError::Other(error.to_string()))?;
    base.join(path)
        .map(|url| url.to_string())
        .map_err(|error| OAuthFlowError::Other(error.to_string()))
}

/// The shared token-request core (upstream `tokenRequest`): endpoint
/// hardening, client authentication, OAuth error body over HTTP status.
async fn token_request(
    authorization_server_url: &str,
    options: &TokenRequestOptions,
    mut params: FormParams,
) -> Result<OAuthTokens, OAuthFlowError> {
    let endpoint = match &options.metadata {
        Some(metadata) => metadata.token_endpoint(),
        None => default_endpoint(authorization_server_url, "/token")?,
    };
    let url = secure_endpoint(&endpoint)?;
    let client_information = options.client_information.as_ref().ok_or_else(|| {
        OAuthFlowError::Other("token request is missing client information".to_string())
    })?;
    let mut headers = vec![
        ("accept".to_string(), "application/json".to_string()),
        (
            "content-type".to_string(),
            "application/x-www-form-urlencoded".to_string(),
        ),
    ];
    if let Some(resource) = &options.resource {
        params.set("resource", resource.clone());
    }
    if let Some(add) = &options.add_client_authentication {
        add(&mut headers, &mut params, &url, options.metadata.as_ref()).await?;
    } else {
        let supported = options
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.token_endpoint_auth_methods_supported())
            .unwrap_or_default();
        apply_client_authentication(
            select_client_auth_method(client_information, &supported),
            client_information,
            &mut headers,
            &mut params,
        )?;
    }
    let fetch = options.fetch.clone().unwrap_or_else(default_fetch);
    let request = FetchRequest {
        url,
        method: "POST".to_string(),
        headers,
        body: Some(params.to_body()),
        cancel: tokio_util::sync::CancellationToken::new(),
    };
    let response = fetch(request)
        .await
        .map_err(|error| OAuthFlowError::Other(error.message))?;
    let status = response.status;
    let text = response.into_text().await.map_err(OAuthFlowError::Other)?;
    // Servers may report OAuth errors with any status, so check the body
    // before the status (upstream JSON.parse in a try/catch).
    let value: Option<Value> = serde_json::from_str(&text).ok();
    if let Some(Value::Object(object)) = &value {
        if let Some(error) = object.get("error").and_then(Value::as_str) {
            let description = match object.get("error_description") {
                Some(Value::String(description)) => description.clone(),
                _ => error.to_string(),
            };
            let error_uri = match object.get("error_uri") {
                Some(Value::String(uri)) => Some(uri.clone()),
                _ => None,
            };
            return Err(OAuthFlowError::OAuth(OAuthError::new(
                error,
                description,
                error_uri,
            )));
        }
    }
    if !(200..300).contains(&status) {
        return Err(OAuthFlowError::OAuth(OAuthError::new(
            "server_error",
            format!("HTTP {status}: {text}"),
            None,
        )));
    }
    let Some(value) = value else {
        return Err(OAuthFlowError::Other(
            "Invalid OAuth token response".to_string(),
        ));
    };
    parse_oauth_tokens(value).map_err(OAuthFlowError::Other)
}

/// Upstream `registerClient` (dynamic client registration).
pub async fn register_client(
    authorization_server_url: &str,
    metadata: Option<&AuthorizationServerMetadata>,
    client_metadata: &OAuthClientMetadata,
    scope: Option<&str>,
    fetch: Option<McpFetch>,
) -> Result<OAuthClientInformationMixed, OAuthFlowError> {
    let endpoint = match metadata.map(|metadata| metadata.registration_endpoint()) {
        Some(Some(endpoint)) => endpoint,
        Some(None) => {
            return Err(OAuthFlowError::Other(
                "Authorization server does not support dynamic client registration".to_string(),
            ));
        }
        None => default_endpoint(authorization_server_url, "/register")?,
    };
    let url = Url::parse(&endpoint).map_err(|error| OAuthFlowError::Other(error.to_string()))?;
    // `JSON.stringify({ ...clientMetadata, ...(scope ? { scope } : {}) })`:
    // scope replaces an existing key in position or appends last.
    let mut body = client_metadata.raw.clone();
    if let Some(scope) = scope {
        body.insert("scope".to_string(), Value::from(scope));
    }
    let fetch = fetch.unwrap_or_else(default_fetch);
    let request = FetchRequest {
        url,
        method: "POST".to_string(),
        headers: vec![
            ("Accept".to_string(), "application/json".to_string()),
            ("content-type".to_string(), "application/json".to_string()),
        ],
        body: Some(
            serde_json::to_string(&Value::Object(body))
                .map_err(|error| OAuthFlowError::Other(error.to_string()))?,
        ),
        cancel: tokio_util::sync::CancellationToken::new(),
    };
    let response = fetch(request)
        .await
        .map_err(|error| OAuthFlowError::Other(error.message))?;
    let status = response.status;
    let text = response.into_text().await.map_err(OAuthFlowError::Other)?;
    if !(200..300).contains(&status) {
        return Err(OAuthFlowError::Registration(OAuthRegistrationError {
            status,
            body: text,
        }));
    }
    let value: Value =
        serde_json::from_str(&text).map_err(|error| OAuthFlowError::Other(error.to_string()))?;
    parse_client_information(value).map_err(OAuthFlowError::Other)
}

/// Upstream `exchangeAuthorizationCode`.
pub async fn exchange_authorization_code(
    authorization_server_url: &str,
    options: &TokenRequestOptions,
    code: &str,
    code_verifier: &str,
    redirect_url: &str,
) -> Result<OAuthTokens, OAuthFlowError> {
    let mut params = FormParams::new();
    params.set("grant_type", "authorization_code");
    params.set("code", code);
    params.set("code_verifier", code_verifier);
    params.set("redirect_uri", redirect_url);
    token_request(authorization_server_url, options, params).await
}

/// Upstream `refreshAuthorization`: an absent rotated refresh token keeps the
/// old one (`{ refresh_token: options.refreshToken, ...tokens }`).
pub async fn refresh_authorization(
    authorization_server_url: &str,
    options: &TokenRequestOptions,
    refresh_token: &str,
) -> Result<OAuthTokens, OAuthFlowError> {
    let mut params = FormParams::new();
    params.set("grant_type", "refresh_token");
    params.set("refresh_token", refresh_token);
    let mut tokens = token_request(authorization_server_url, options, params).await?;
    if tokens.refresh_token.is_none() {
        tokens.refresh_token = Some(refresh_token.to_string());
    }
    Ok(tokens)
}

/// The flow body (upstream `runFlow`): discovery (reusing the provider's
/// persisted state when present), resource selection, client registration,
/// then either the code exchange, the silent refresh, or the redirect.
async fn run_flow(
    provider: &dyn OAuthClientProvider,
    options: &OAuthFlowOptions,
) -> Result<OAuthFlowResult, OAuthFlowError> {
    let discovery_options = DiscoveryOptions {
        fetch: options.fetch.clone(),
        protocol_version: None,
    };
    // v1.0.0: with a configured metadata URL, discovery is not cached, so
    // changing the URL applies at once.
    let metadata_url = match &options.authorization_server_metadata_url {
        Some(url) => Some(secure_endpoint(url.as_ref())?),
        None => None,
    };
    let cached = if metadata_url.is_some() {
        None
    } else {
        provider.discovery_state().await
    };
    let discovered = match cached.filter(|cached| !cached.authorization_server_url.is_empty()) {
        Some(cached) => {
            let metadata = match cached.authorization_server_metadata {
                Some(metadata) => Some(metadata),
                None => {
                    discover_authorization_server_metadata(
                        &cached.authorization_server_url,
                        discovery_options,
                        options.skip_issuer_validation,
                    )
                    .await?
                }
            };
            crate::mcp::oauth::types::OAuthServerInfo {
                authorization_server_url: cached.authorization_server_url,
                authorization_server_metadata: metadata,
                resource_metadata: cached.resource_metadata,
            }
        }
        None => {
            discover_oauth_server_info(
                &options.server_url,
                discovery_options,
                options
                    .resource_metadata_url
                    .clone()
                    .map(|url| url.to_string()),
                metadata_url.as_ref().map(|url| url.to_string()),
                options.skip_issuer_validation,
            )
            .await?
        }
    };
    if metadata_url.is_none() {
        provider
            .save_discovery_state(OAuthDiscoveryState {
                authorization_server_url: discovered.authorization_server_url.clone(),
                authorization_server_metadata: discovered.authorization_server_metadata.clone(),
                resource_metadata: discovered.resource_metadata.clone(),
                resource_metadata_url: options
                    .resource_metadata_url
                    .as_ref()
                    .map(|url| url.to_string()),
            })
            .await?;
    }
    let metadata = discovered.authorization_server_metadata.clone();
    let resource = select_resource(&options.server_url, discovered.resource_metadata.as_ref())?;
    // `||`, not `??` (v1.0.0): an empty scope (for example from
    // `scopes_supported: []`) falls through to the next source.
    let scope = {
        let from_options = options.scope.clone().filter(|scope| !scope.is_empty());
        from_options.or_else(|| {
            discovered
                .resource_metadata
                .as_ref()
                .map(|metadata| metadata.scopes_supported().join(" "))
                .filter(|scope| !scope.is_empty())
        })
    }
    .or_else(|| provider.client_metadata().scope());
    let stored = provider.client_information().await;
    let client_document = if stored.is_none() {
        provider.client_metadata_document(metadata.as_ref())
    } else {
        None
    };
    if let Some(document) = &client_document {
        let parsed =
            Url::parse(&document.url).map_err(|error| OAuthFlowError::Other(error.to_string()))?;
        if parsed.scheme() != "https" || parsed.path() == "/" {
            return Err(OAuthFlowError::Other(
                "Invalid OAuth client metadata URL".to_string(),
            ));
        }
    }
    let client = match stored {
        Some(stored) => stored,
        None => {
            if let Some(document) = &client_document {
                let mut raw = Map::new();
                raw.insert("client_id".to_string(), Value::from(document.url.clone()));
                OAuthClientInformationMixed::from_raw(raw)
            } else {
                if options.authorization_code.is_some() {
                    return Err(OAuthFlowError::Other(
                        "OAuth client information is missing during code exchange".to_string(),
                    ));
                }
                if !provider.saves_client_information() {
                    return Err(OAuthFlowError::Other(
                        "OAuth client information cannot be persisted".to_string(),
                    ));
                }
                let registered = register_client(
                    &discovered.authorization_server_url,
                    metadata.as_ref(),
                    &provider.client_metadata(),
                    scope.as_deref(),
                    options.fetch.clone(),
                )
                .await?;
                provider.save_client_information(registered.clone()).await?;
                registered
            }
        }
    };
    // The document's redirect URI may differ from the provider's, for
    // example by a server-specific path (v1.0.0).
    let redirect_url = client_document
        .as_ref()
        .map(|document| document.redirect_url.clone())
        .unwrap_or_else(|| provider.redirect_url());
    let token_options = TokenRequestOptions {
        metadata: metadata.clone(),
        client_information: Some(client.clone()),
        resource: resource.clone(),
        add_client_authentication: provider.add_client_authentication(),
        fetch: options.fetch.clone(),
    };
    if let Some(code) = options.authorization_code.clone() {
        // RFC 9207 (v1.0.0): never send a code from another authorization
        // server to this one.
        if let Some(issuer_metadata) = &metadata {
            let iss_check_required = options.iss.is_some()
                || issuer_metadata.authorization_response_iss_parameter_supported() == Some(true);
            if iss_check_required
                && options.iss.as_deref() != Some(issuer_metadata.issuer().as_str())
            {
                return Err(OAuthFlowError::IssuerMismatch(
                    OAuthIssuerMismatchError::new(issuer_metadata.issuer(), options.iss.clone()),
                ));
            }
        }
        let code_verifier = provider.code_verifier().await?;
        let tokens = exchange_authorization_code(
            &discovered.authorization_server_url,
            &token_options,
            &code,
            &code_verifier,
            &redirect_url,
        )
        .await?;
        // A response without `scope` grants the requested scope (RFC 6749
        // §5.1, v1.0.0). Callers pass the options of the authorization
        // request, so `scope` is what was requested.
        provider
            .save_tokens(with_scope(tokens, scope.as_deref()))
            .await?;
        return Ok(OAuthFlowResult::Authorized);
    }
    let existing = if options.skip_refresh {
        None
    } else {
        provider.tokens().await
    };
    if let Some(existing) = existing {
        if let Some(refresh_token) = existing.refresh_token.clone() {
            match refresh_authorization(
                &discovered.authorization_server_url,
                &token_options,
                &refresh_token,
            )
            .await
            {
                Ok(tokens) => {
                    // A refresh without `scope` keeps the scope of the grant
                    // (RFC 6749 §6, v1.0.0).
                    provider
                        .save_tokens(with_scope(tokens, existing.scope.as_deref()))
                        .await?;
                    return Ok(OAuthFlowResult::Authorized);
                }
                Err(error) => {
                    if error.is_insecure_endpoint() {
                        return Err(error);
                    }
                    if let OAuthFlowError::OAuth(oauth_error) = &error {
                        if oauth_error.code != "server_error" {
                            return Err(error);
                        }
                    }
                }
            }
        }
    }
    let state = provider.state().await.ok();
    let (authorization_url, code_verifier) = start_authorization(
        &discovered.authorization_server_url,
        metadata.as_ref(),
        &client,
        &redirect_url,
        scope.as_deref(),
        state.as_deref(),
        resource.as_deref(),
    )
    .await?;
    provider.save_code_verifier(code_verifier).await?;
    provider
        .redirect_to_authorization(authorization_url)
        .await?;
    Ok(OAuthFlowResult::Redirect)
}

/// Upstream `withScope` (v1.0.0): a response without `scope` keeps the
/// granted/requested one.
fn with_scope(tokens: OAuthTokens, scope: Option<&str>) -> OAuthTokens {
    if tokens.scope.is_none() {
        if let Some(scope) = scope {
            if !scope.is_empty() {
                return OAuthTokens {
                    scope: Some(scope.to_string()),
                    ..tokens
                };
            }
        }
    }
    tokens
}

/// Upstream `stepUpScope` (v1.0.0): scopes for a step-up authorization — the
/// challenged scopes plus the ones granted so far, since a challenge may
/// list only the missing scopes and a token with just those would lose
/// access the old one had (SEP-2350). Without challenged scopes, `None`
/// lets the flow pick its default.
pub fn step_up_scope(granted: Option<&str>, challenged: Option<&str>) -> Option<String> {
    let challenged = challenged.filter(|scope| !scope.is_empty())?;
    let mut scopes: Vec<String> = Vec::new();
    for part in [granted, Some(challenged)] {
        for scope in part.unwrap_or_default().split_whitespace() {
            if !scope.is_empty() && !scopes.iter().any(|seen| seen == scope) {
                scopes.push(scope.to_string());
            }
        }
    }
    Some(scopes.join(" "))
}

/// Upstream `authorizeMcp`: one automatic retry with invalidated credentials
/// for the two "stored grant is stale" OAuth error codes.
pub async fn authorize_mcp(
    provider: &dyn OAuthClientProvider,
    options: OAuthFlowOptions,
) -> Result<OAuthFlowResult, OAuthFlowError> {
    match run_flow(provider, &options).await {
        Err(OAuthFlowError::OAuth(error))
            if error.code == "invalid_client" || error.code == "unauthorized_client" =>
        {
            provider.invalidate_credentials(CredentialKind::All).await?;
            run_flow(provider, &options).await
        }
        Err(OAuthFlowError::OAuth(error)) if error.code == "invalid_grant" => {
            provider
                .invalidate_credentials(CredentialKind::Tokens)
                .await?;
            run_flow(provider, &options).await
        }
        other => other,
    }
}

/// One shared in-flight authorization future (upstream's `inFlight` promise).
type InFlightFuture = futures::future::Shared<BoxFuture<'static, Result<(), OAuthFlowError>>>;

/// Shared state of the adapted provider: the wrapped flow provider and the
/// single in-flight authorization future.
struct AdaptedShared {
    provider: Arc<dyn OAuthClientProvider>,
    in_flight: std::sync::Mutex<Option<InFlightFuture>>,
}

/// The `adaptOAuthProvider` product: an [`AuthProvider`] for
/// [`crate::mcp::transports::StreamableHttpTransport`]. After a 401 it
/// refreshes the tokens, or errors with
/// [`McpOAuthAuthorizationRequiredError`] when the user has to authorize
/// (again). Concurrent 401s share one refresh, and a request whose token was
/// already replaced is just retried: with rotating refresh tokens, a second
/// refresh with the old refresh token would fail and discard the new grant.
pub struct AdaptedOAuthProvider {
    shared: Arc<AdaptedShared>,
}

impl AuthProvider for AdaptedOAuthProvider {
    fn token(&self) -> BoxFuture<'_, Option<String>> {
        Box::pin(async move {
            self.shared
                .provider
                .tokens()
                .await
                .map(|tokens| tokens.access_token)
        })
    }

    fn handles_unauthorized(&self) -> bool {
        true
    }

    fn on_unauthorized<'a>(
        &'a self,
        context: UnauthorizedContext,
    ) -> BoxFuture<'a, Result<(), crate::mcp::protocol::jsonrpc::McpClientError>> {
        Box::pin(async move {
            let challenge = parse_www_authenticate(context.response.header("www-authenticate"));
            let insufficient_scope = challenge.error.as_deref() == Some("insufficient_scope");
            if !insufficient_scope {
                let in_flight = self
                    .shared
                    .in_flight
                    .lock()
                    .expect("in-flight slot cannot be poisoned")
                    .is_some();
                if !in_flight {
                    if let Some(token) = &context.token {
                        // A different current token means another request
                        // already refreshed it.
                        let current = self.shared.provider.tokens().await;
                        if current.is_some_and(|tokens| tokens.access_token != *token) {
                            return Ok(());
                        }
                    }
                }
            }
            let existing = self
                .shared
                .in_flight
                .lock()
                .expect("in-flight slot cannot be poisoned")
                .clone();
            let future = match existing {
                Some(future) => future,
                None => {
                    let provider = Arc::clone(&self.shared.provider);
                    // v1.0.0 (SEP-2350): a step-up authorization asks for the
                    // challenged scopes plus the ones granted so far.
                    let scope = if insufficient_scope {
                        let granted: Option<OAuthTokens> = provider.tokens().await;
                        step_up_scope(
                            granted.as_ref().and_then(|tokens| tokens.scope.as_deref()),
                            challenge.scope.as_deref(),
                        )
                    } else {
                        challenge.scope.clone()
                    };
                    let options = OAuthFlowOptions {
                        server_url: context.server_url.to_string(),
                        resource_metadata_url: challenge.resource_metadata_url.clone(),
                        scope,
                        fetch: Some(Arc::clone(&context.fetch)),
                        skip_refresh: insufficient_scope,
                        ..OAuthFlowOptions::default()
                    };
                    let shared = Arc::clone(&self.shared);
                    // The future clears the slot itself (upstream's
                    // `.finally`), so every Shared observer sees the cleared
                    // state once it settles.
                    let raw: BoxFuture<'static, Result<(), OAuthFlowError>> =
                        Box::pin(async move {
                            let result = match authorize_mcp(provider.as_ref(), options).await {
                                Ok(OAuthFlowResult::Redirect) => {
                                    Err(OAuthFlowError::AuthorizationRequired(
                                        McpOAuthAuthorizationRequiredError,
                                    ))
                                }
                                other => other.map(|_| ()),
                            };
                            *shared
                                .in_flight
                                .lock()
                                .expect("in-flight slot cannot be poisoned") = None;
                            result
                        });
                    let future = raw.shared();
                    // Claim the slot unless another caller inserted first.
                    let mut slot = self
                        .shared
                        .in_flight
                        .lock()
                        .expect("in-flight slot cannot be poisoned");
                    let future = match slot.as_ref().cloned() {
                        Some(existing) => existing,
                        None => {
                            *slot = Some(future.clone());
                            future
                        }
                    };
                    future
                }
            };
            future
                .await
                .map_err(crate::mcp::protocol::jsonrpc::McpClientError::OAuth)
        })
    }
}

/// Upstream `adaptOAuthProvider`.
pub fn adapt_oauth_provider(provider: Arc<dyn OAuthClientProvider>) -> Arc<dyn AuthProvider> {
    Arc::new(AdaptedOAuthProvider {
        shared: Arc::new(AdaptedShared {
            provider,
            in_flight: std::sync::Mutex::new(None),
        }),
    })
}
