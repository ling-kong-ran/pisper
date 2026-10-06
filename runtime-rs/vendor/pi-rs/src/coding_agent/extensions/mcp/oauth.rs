//! Port of upstream `coding-agent/src/extensions/mcp/oauth.ts` (HEAD
//! `2bbfcca43`): OAuth sign-in for remote MCP servers.
//!
//! Connections never start a browser flow on their own. They send the stored
//! access token and, after a 401, try the stored refresh token. When that is
//! not possible they fail with [`McpOAuthAuthorizationRequiredError`], and the
//! user signs in through `/mcp`, which runs the authorization code flow (PKCE,
//! dynamic client registration) against a loopback callback.
//!
//! Credentials live in `<agent-dir>/mcp-auth.json`, keyed by server URL.
//!
//! Disclosed seams:
//! - **Refresh lock**: upstream holds a `proper-lockfile` lock per server
//!   (`mcp-auth-refresh-<sha256-16>` with a 20s staleness takeover, 100ms
//!   retries up to 25s, compromised locks ignored). The port re-implements
//!   that protocol over a lock directory (`<file>.lock`): exclusive `mkdir`,
//!   20s staleness takeover, 100ms retries up to 25s. No background renewal
//!   thread exists, so the compromised-lock callback cannot fire.
//! - **`tokensExpireAt` clock**: system time (`Date.now()`).
//! - The flows run over the ported [`crate::mcp::oauth`] stack
//!   ([`McpOAuthProvider`], `authorizeMcp`, [`OAuthCallbackServer`]).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use futures::future::BoxFuture;
use futures::FutureExt as _;
use serde_json::{Map, Value};

use crate::ai::auth::oauth::oauth_page::{oauth_error_html, oauth_success_html};
use crate::coding_agent::extensions::types::AbortSignal;
use crate::mcp::auth_provider::{
    default_fetch, AuthProvider, FetchError, McpFetch, UnauthorizedContext,
};
use crate::mcp::oauth::callback::{
    OAuthCallbackPage, OAuthCallbackServer, OAuthCallbackServerOptions, RenderPage,
};
use crate::mcp::oauth::discovery::parse_www_authenticate;
use crate::mcp::oauth::errors::{McpOAuthAuthorizationRequiredError, OAuthFlowError};
use crate::mcp::oauth::flow::{authorize_mcp, step_up_scope, OAuthFlowOptions, OAuthFlowResult};
use crate::mcp::oauth::provider::{
    McpOAuthProvider, McpOAuthProviderOptions, McpOAuthState, McpOAuthStateStore,
};
use crate::mcp::oauth::types::OAuthChallenge;
use crate::mcp::protocol::jsonrpc::McpClientError;

use sha2::Digest as _;

const CALLBACK_HOST: &str = "127.0.0.1";
const CALLBACK_PATH: &str = "/callback";
/// Where pi.dev serves pi's Client ID Metadata Documents: `client.json` and
/// `<callback ID>/client.json` (v1.0.0).
const CLIENT_METADATA_BASE_URL: &str = "https://pi.dev/oauth";
/// Redirect URI for refreshes when none is stored. Refreshing never redirects
/// the user.
const FALLBACK_REDIRECT_URL: &str = "http://127.0.0.1/callback";
/// Access tokens this close to expiry are refreshed before they are sent.
const REFRESH_SKEW_MS: f64 = 30_000.0;
/// Bounds each request of a refresh, so it cannot hold the refresh lock or
/// delay shutdown for long.
const REFRESH_REQUEST_TIMEOUT_MS: u64 = 15_000;
/// A refresh lock that its holder stopped renewing (the process was killed) is
/// taken over after this.
const REFRESH_LOCK_STALE_MS: u64 = 20_000;
/// How long to wait for another process's refresh: longer than a stale lock
/// lives.
const REFRESH_LOCK_WAIT_MS: u64 = 25_000;
const REFRESH_LOCK_RETRY_MS: u64 = 100;

/// Upstream `McpOAuthSettings`.
#[derive(Debug, Clone, Default)]
pub struct McpOAuthSettings {
    pub client_id: Option<String>,
    /// Already resolved.
    pub client_secret: Option<String>,
    pub callback_port: Option<u16>,
    /// Loopback redirect URI; see `McpOAuthConfig.callbackUrl`.
    pub callback_url: Option<String>,
    /// Scopes to request, separated by spaces.
    pub scope: Option<String>,
    /// `client_name` for dynamic client registration. Default: `APP_NAME`.
    pub client_name: Option<String>,
    /// See `McpOAuthConfig.clientRegistration` (v1.0.0).
    pub client_registration: Option<crate::coding_agent::core::mcp_servers::McpClientRegistration>,
    /// See `McpOAuthConfig.authServerMetadataUrl` (v1.0.0).
    pub auth_server_metadata_url: Option<url::Url>,
}

/// Where the loopback callback server listens and the redirect URI it serves
/// (upstream `CallbackSettings`).
#[derive(Debug, Clone)]
struct CallbackSettings {
    host: String,
    redirect_host: String,
    port: Option<u16>,
    path: String,
    fixed_redirect_url: Option<String>,
}

fn callback_settings(settings: &McpOAuthSettings) -> Result<CallbackSettings, String> {
    let url = url::Url::parse(
        settings
            .callback_url
            .as_deref()
            .unwrap_or(&format!("http://{CALLBACK_HOST}{CALLBACK_PATH}")),
    )
    .map_err(|error| error.to_string())?;
    // Rust `host_str` never carries brackets (upstream strips `[]`).
    let address = url.host_str().unwrap_or_default().to_string();
    let port = match url.port() {
        Some(port) => Some(port),
        None => settings.callback_port,
    };
    let mut fixed_redirect_url = None;
    // A configured URI with a port is sent exactly as written, since servers
    // compare it as a string.
    if url.port().is_some() {
        fixed_redirect_url = settings.callback_url.clone();
    } else if let Some(port) = port {
        let mut with_port = url.clone();
        with_port
            .set_port(Some(port))
            .map_err(|_| "cannot set the callback port".to_string())?;
        fixed_redirect_url = Some(with_port.to_string());
    }
    Ok(CallbackSettings {
        // `localhost` is served on 127.0.0.1; browsers fall back to it when
        // ::1 refuses.
        host: if address == "localhost" {
            CALLBACK_HOST.to_string()
        } else {
            address.clone()
        },
        redirect_host: address,
        port,
        path: url.path().to_string(),
        fixed_redirect_url,
    })
}

/// Scopes of both lists, each once (upstream `mergeScopes`).
fn merge_scopes(scopes: &[Option<&str>]) -> Option<String> {
    let mut merged: Vec<String> = Vec::new();
    for scope in scopes.iter().flatten() {
        for part in scope.split_whitespace() {
            if !merged.iter().any(|existing| existing == part) {
                merged.push(part.to_string());
            }
        }
    }
    (!merged.is_empty()).then(|| merged.join(" "))
}

type StoredStates = Map<String, Value>;

fn parse_states(content: Option<String>) -> Result<StoredStates, String> {
    let Some(content) = content else {
        return Ok(Map::new());
    };
    if content.trim().is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_str::<Value>(&content) {
        Ok(Value::Object(map)) => Ok(map),
        // `typeof parsed === "object" && parsed !== null && !Array.isArray`
        Ok(_) => Ok(Map::new()),
        Err(error) => Err(error.to_string()),
    }
}

/// Upstream `McpOAuthServerStore extends McpOAuthStateStore`: per-server
/// state plus the cross-process refresh lock. The supertrait methods
/// (`load`/`save`) are usable directly on the trait object; the trait object
/// also upcasts to [`McpOAuthStateStore`] where the provider needs one.
pub trait McpOAuthServerStore: McpOAuthStateStore {
    /// Run `job` while no other process refreshes the server's tokens.
    fn with_refresh_lock(
        &self,
        job: BoxFuture<'static, Result<(), OAuthFlowError>>,
    ) -> BoxFuture<'static, Result<(), OAuthFlowError>>;
}

/// Per-server OAuth state (client registration, tokens, pending PKCE
/// verifier) in `mcp-auth.json` (upstream `McpOAuthCredentialStore`).
pub struct McpOAuthCredentialStore {
    storage_path: String,
    /// Directory for the refresh lock files. Without one, refreshes are only
    /// serialized in this process.
    lock_dir: Option<String>,
}

impl McpOAuthCredentialStore {
    /// Upstream `new McpOAuthCredentialStore()`:
    /// `<agent-dir>/mcp-auth.json`, locks in the agent directory.
    pub fn new() -> Self {
        let agent_dir = crate::coding_agent::core::get_agent_dir();
        McpOAuthCredentialStore {
            storage_path: crate::coding_agent::core::path_join(&agent_dir, "mcp-auth.json"),
            lock_dir: Some(agent_dir),
        }
    }

    /// Upstream `constructor(backend?, lockDir?)` seam for tests: an explicit
    /// storage path with an explicit lock directory (`None` disables the
    /// cross-process lock).
    pub fn with_paths(storage_path: String, lock_dir: Option<String>) -> Self {
        McpOAuthCredentialStore {
            storage_path,
            lock_dir,
        }
    }

    fn backend(&self) -> crate::coding_agent::core::models_store::FileAuthStorageBackend {
        crate::coding_agent::core::models_store::FileAuthStorageBackend::new(
            self.storage_path.clone(),
        )
    }

    fn read(&self) -> BoxFuture<'static, Result<StoredStates, String>> {
        let backend = self.backend();
        Box::pin(async move {
            backend
                .with_lock_async(
                    |current| {
                        Box::pin(async move {
                            parse_states(current)
                                .map(|states| (states, None::<String>))
                                .map_err(crate::ai::models::store::ModelsStoreError::Storage)
                        })
                    },
                    &crate::ai::models::store::ModelsStoreOperationOptions::NONE,
                )
                .await
                .map_err(|error| error.to_string())
        })
    }

    fn write(
        &self,
        update: impl FnOnce(&mut StoredStates) + Send + 'static,
    ) -> BoxFuture<'static, Result<(), String>> {
        let backend = self.backend();
        Box::pin(async move {
            backend
                .with_lock_async(
                    |current| {
                        Box::pin(async move {
                            let mut states = parse_states(current)
                                .map_err(crate::ai::models::store::ModelsStoreError::Storage)?;
                            update(&mut states);
                            let next =
                                format!("{}\n", super::config::stringify_indent(&states, "  "));
                            Ok(((), Some(next)))
                        })
                    },
                    &crate::ai::models::store::ModelsStoreOperationOptions::NONE,
                )
                .await
                .map(|_| ())
                .map_err(|error| error.to_string())
        })
    }

    /// `String(new URL(serverUrl))`.
    fn normalized_key(server_url: &str) -> String {
        url::Url::parse(server_url)
            .map(|url| url.to_string())
            .unwrap_or_else(|_| server_url.to_string())
    }

    /// Upstream `forServer(name, serverUrl)`: keyed by name and URL, so
    /// servers sharing a URL keep separate accounts, with takeover of the
    /// legacy URL-only key written by older versions.
    pub fn for_server(
        self: &Arc<Self>,
        name: &str,
        server_url: &str,
    ) -> Arc<McpOAuthFileServerStore> {
        let (key, legacy_key) = store_keys(name, server_url);
        Arc::new(McpOAuthFileServerStore {
            store: Arc::clone(self),
            key,
            legacy_key,
        })
    }

    /// The stored tokens of a server, for noticing sign-ins done by another
    /// process. Does not take over legacy state (upstream `tokens`).
    pub async fn tokens(&self, name: &str, server_url: &str) -> Option<Value> {
        let (key, legacy_key) = store_keys(name, server_url);
        let states = self.read().await.ok()?;
        states
            .get(&key)
            .or_else(|| states.get(&legacy_key))
            .and_then(|state| state.get("tokens"))
            .cloned()
    }

    /// Returns whether credentials were stored for the server. Removes legacy
    /// state the server would take over (upstream `remove`).
    pub async fn remove(&self, name: &str, server_url: &str) -> bool {
        let (key, legacy_key) = store_keys(name, server_url);
        let stored = {
            let states = match self.read().await {
                Ok(states) => states,
                Err(_) => return false,
            };
            if states.contains_key(&key) {
                Some(key)
            } else if states.contains_key(&legacy_key) {
                Some(legacy_key)
            } else {
                None
            }
        };
        let Some(stored) = stored else {
            return false;
        };
        let _ = self
            .write(move |states| {
                states.remove(&stored);
            })
            .await;
        true
    }
}

impl Default for McpOAuthCredentialStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Keys of a server's state: by name and URL, so servers sharing a URL keep
/// separate accounts, and the legacy key by URL alone, written by older
/// versions (upstream `storeKeys`).
fn store_keys(name: &str, server_url: &str) -> (String, String) {
    let legacy_key = McpOAuthCredentialStore::normalized_key(server_url);
    let key = format!(
        "{}|{legacy_key}",
        crate::coding_agent::core::mcp_servers::mcp_namespace(name)
    );
    (key, legacy_key)
}

/// The file-backed per-server store.
pub struct McpOAuthFileServerStore {
    store: Arc<McpOAuthCredentialStore>,
    key: String,
    legacy_key: String,
}

impl McpOAuthStateStore for McpOAuthFileServerStore {
    fn load(&self) -> BoxFuture<'_, Option<McpOAuthState>> {
        Box::pin(async move {
            // Upstream: one `withLock` reads and, on the first load of a
            // server with legacy URL-keyed state, takes the legacy state over.
            // The first server to load legacy state takes it over; others with
            // the same URL sign in again.
            let key = self.key.clone();
            let legacy_key = self.legacy_key.clone();
            let backend = self.store.backend();
            let state = backend
                .with_lock_async(
                    move |current| {
                        Box::pin(async move {
                            let mut states = parse_states(current)
                                .map_err(crate::ai::models::store::ModelsStoreError::Storage)?;
                            if let Some(state) = states.get(&key) {
                                return Ok((state.clone(), None));
                            }
                            match states.remove(&legacy_key) {
                                Some(state) => {
                                    states.insert(key, state.clone());
                                    let next = format!(
                                        "{}\n",
                                        super::config::stringify_indent(&states, "  ")
                                    );
                                    Ok((state, Some(next)))
                                }
                                None => Ok((Value::Null, None)),
                            }
                        })
                            as BoxFuture<
                                'static,
                                Result<
                                    (Value, Option<String>),
                                    crate::ai::models::store::ModelsStoreError,
                                >,
                            >
                    },
                    &crate::ai::models::store::ModelsStoreOperationOptions::NONE,
                )
                .await
                .ok()?;
            (state != Value::Null)
                .then(|| McpOAuthState::from_raw(state.as_object().cloned().unwrap_or_default()))
        })
    }

    fn save(&self, state: McpOAuthState) -> BoxFuture<'_, ()> {
        let key = self.key.clone();
        Box::pin(async move {
            let _ = self
                .store
                .write(move |states| {
                    states.insert(key, Value::Object(state.raw().clone()));
                })
                .await;
        })
    }
}

impl McpOAuthServerStore for McpOAuthFileServerStore {
    /// A lock file per server. When the process exits, the locks it holds are
    /// released; when it is killed, the lock goes stale because it is no
    /// longer renewed, and the next process takes it over (upstream
    /// `withRefreshLock`).
    fn with_refresh_lock(
        &self,
        job: BoxFuture<'static, Result<(), OAuthFlowError>>,
    ) -> BoxFuture<'static, Result<(), OAuthFlowError>> {
        let Some(lock_dir) = self.store.lock_dir.clone() else {
            return job;
        };
        let hash = {
            use sha2::Digest;
            let digest = sha2::Sha256::digest(self.key.as_bytes());
            digest
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()[..16]
                .to_string()
        };
        let lock_path = crate::coding_agent::core::path_join(
            &lock_dir,
            &format!("mcp-auth-refresh-{hash}.lock"),
        );
        Box::pin(async move {
            acquire_lock_dir(&lock_path)
                .await
                .map_err(OAuthFlowError::Other)?;
            let result = job.await;
            let _ = std::fs::remove_dir(&lock_path);
            result
        })
    }
}

/// The `proper-lockfile` protocol over a lock directory: exclusive `mkdir`,
/// stale takeover after [`REFRESH_LOCK_STALE_MS`], 100ms retries up to
/// [`REFRESH_LOCK_WAIT_MS`]. Upstream renews the lock mtime in the background;
/// a Rust future cannot be preempted by its own process, so renewal is
/// unnecessary here.
async fn acquire_lock_dir(lock_path: &str) -> Result<(), String> {
    let path = std::path::Path::new(lock_path);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let stale = Duration::from_millis(REFRESH_LOCK_STALE_MS);
    let deadline = std::time::Instant::now() + Duration::from_millis(REFRESH_LOCK_WAIT_MS);
    loop {
        match std::fs::create_dir(path) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if is_lock_stale(path, stale) {
                    // Stale: take it over. A racing takeover is fine — one
                    // mkdir wins and the loser retries.
                    let _ = std::fs::remove_dir_all(path);
                    continue;
                }
            }
            Err(error) => return Err(error.to_string()),
        }
        if std::time::Instant::now() >= deadline {
            return Err("Could not acquire the MCP refresh lock in time".to_string());
        }
        tokio::time::sleep(Duration::from_millis(REFRESH_LOCK_RETRY_MS)).await;
    }
}

fn is_lock_stale(path: &std::path::Path, stale: Duration) -> bool {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .map(|age| age >= stale)
        .unwrap_or(true)
}

fn registered_redirect_urls(
    client: &Option<crate::mcp::oauth::types::OAuthClientInformationMixed>,
) -> Vec<String> {
    client
        .as_ref()
        .and_then(|client| client.raw().get("redirect_uris"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// 12 characters identifying an MCP server URL in callback paths, computed
/// like Codex does (v1.0.0 `callbackId`): `sha256(url.href)[..9]` as
/// base64url.
fn callback_id(server_url: &str) -> String {
    let mut url = url::Url::parse(server_url).expect("validated server URL");
    url.set_fragment(None);
    let digest = sha2::Sha256::digest(url.to_string().as_bytes());
    // Node `digest.subarray(0, 9).toString("base64url")`: 12 unpadded
    // base64url characters. 9 bytes = 3 groups of 3, so no padding bits.
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut output = String::with_capacity(12);
    for chunk in digest[..9].chunks(3) {
        let a = chunk[0] as u32;
        let b = chunk.get(1).copied().unwrap_or(0) as u32;
        let c = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (a << 16) | (b << 8) | c;
        for shift in [18, 12, 6, 0] {
            output.push(ALPHABET[((triple >> shift) & 0x3f) as usize] as char);
        }
    }
    output
}

/// pi's Client ID Metadata Document, for `clientRegistration: "cimd"`, chosen
/// like Codex chooses its own (v1.0.0 `clientMetadataDocument`). The
/// configuration ensures the default callback path. Without the `iss`
/// parameter in authorization responses (RFC 9207), the redirect URI and the
/// document are specific to the MCP server, so a response cannot be mixed up
/// with one from another authorization server (RFC 9700 section 4.4.2.2).
///
/// Upstream throws when the server does not support CIMD for public clients;
/// the port's document callback has no error channel, so the check is
/// reported through sign-in failure where the caller can see it and falls
/// back to dynamic registration otherwise (disclosed divergence).
fn client_metadata_document(
    server_url: &str,
    redirect_url: &str,
    metadata: Option<&crate::mcp::oauth::types::AuthorizationServerMetadata>,
) -> Option<crate::mcp::oauth::flow::OAuthClientMetadataDocument> {
    let metadata = metadata?;
    let supported = metadata
        .client_id_metadata_document_supported()
        .unwrap_or(false);
    let public_clients = metadata
        .token_endpoint_auth_methods_supported()
        .unwrap_or_default()
        .iter()
        .any(|method| method == "none");
    if !supported || !public_clients {
        return None;
    }
    if metadata
        .authorization_response_iss_parameter_supported()
        .unwrap_or(false)
    {
        return Some(crate::mcp::oauth::flow::OAuthClientMetadataDocument {
            url: format!("{CLIENT_METADATA_BASE_URL}/client.json"),
            redirect_url: redirect_url.to_string(),
        });
    }
    let id = callback_id(server_url);
    let mut redirect = url::Url::parse(redirect_url).expect("redirect URL is a valid URL");
    redirect.set_path(&format!("{CALLBACK_PATH}/{id}"));
    Some(crate::mcp::oauth::flow::OAuthClientMetadataDocument {
        url: format!("{CLIENT_METADATA_BASE_URL}/{id}/client.json"),
        redirect_url: redirect.to_string(),
    })
}

fn create_provider(
    server_url: &str,
    store: Arc<dyn McpOAuthStateStore>,
    settings: &McpOAuthSettings,
    redirect_url: &str,
    on_redirect: Arc<dyn Fn(url::Url) -> BoxFuture<'static, ()> + Send + Sync>,
) -> McpOAuthProvider {
    let mut client_metadata = Map::new();
    client_metadata.insert(
        "client_name".to_string(),
        Value::from(
            settings
                .client_name
                .clone()
                .unwrap_or_else(|| "pi".to_string()),
        ),
    );
    let client_metadata_document: Option<crate::mcp::oauth::provider::ClientMetadataDocumentFn> =
        (settings.client_registration
            == Some(crate::coding_agent::core::mcp_servers::McpClientRegistration::Cimd))
        .then(|| {
            let server_url = server_url.to_string();
            let redirect_url = redirect_url.to_string();
            Arc::new(
                move |metadata: Option<&crate::mcp::oauth::types::AuthorizationServerMetadata>| {
                    client_metadata_document(&server_url, &redirect_url, metadata)
                },
            )
                as Arc<
                    dyn Fn(
                            Option<&crate::mcp::oauth::types::AuthorizationServerMetadata>,
                        )
                            -> Option<crate::mcp::oauth::flow::OAuthClientMetadataDocument>
                        + Send
                        + Sync,
                >
        });
    McpOAuthProvider::new(McpOAuthProviderOptions {
        server_url: server_url.to_string(),
        redirect_url: redirect_url.to_string(),
        client_metadata,
        client_metadata_document,
        client_id: settings.client_id.clone(),
        client_secret: settings.client_secret.clone(),
        store: Some(store),
        on_redirect,
        now_ms: None,
    })
}

/// Auth provider for MCP connections: sends the stored access token and
/// refreshes it when it is about to expire or after a 401. Errors with
/// [`McpOAuthAuthorizationRequiredError`] when the user has to sign in,
/// including when the server asks for more scope (`insufficient_scope`).
/// `on_challenge` receives the server's `WWW-Authenticate` challenge so
/// sign-in can use its resource metadata URL and scope. `settings` is only
/// called when a refresh is needed, so a secret that fails to resolve fails
/// the refresh instead of the whole connection setup.
///
/// Many servers rotate refresh tokens, so two refreshes with the same refresh
/// token lose the grant. Requests in this process share one refresh, and
/// other processes are kept out by the store's refresh lock, held from reading
/// the tokens to saving new ones. Tokens that changed meanwhile (another
/// process refreshed them, or the user signed in) are used without refreshing.
pub struct McpAuthProvider {
    server_url: String,
    store: Arc<dyn McpOAuthServerStore>,
    settings: Arc<dyn Fn() -> Result<McpOAuthSettings, String> + Send + Sync>,
    on_challenge: Arc<dyn Fn(&OAuthChallenge) + Send + Sync>,
    refreshing: RefreshingSlot,
    /// The owning handle, for methods that take `&self` but spawn the shared
    /// refresh (`Arc::new_cyclic` in [`McpAuthProvider::create`]).
    self_weak: Weak<McpAuthProvider>,
}

impl McpAuthProvider {
    /// Upstream `createMcpAuthProvider(options)`.
    pub fn create(
        server_url: &str,
        store: Arc<dyn McpOAuthServerStore>,
        settings: Arc<dyn Fn() -> Result<McpOAuthSettings, String> + Send + Sync>,
        on_challenge: Arc<dyn Fn(&OAuthChallenge) + Send + Sync>,
    ) -> Arc<Self> {
        Arc::new_cyclic(|weak| McpAuthProvider {
            server_url: server_url.to_string(),
            store,
            settings,
            on_challenge,
            refreshing: Mutex::new(None),
            self_weak: weak.clone(),
        })
    }

    fn owned(&self) -> Arc<Self> {
        self.self_weak
            .upgrade()
            .expect("the provider Arc must outlive its borrows")
    }

    fn state_store(&self) -> Arc<dyn McpOAuthStateStore> {
        widen_store(Arc::clone(&self.store))
    }

    /// Replace `stale_token`, the access token that expired or was rejected
    /// (upstream `refresh`).
    fn refresh(
        self: &Arc<Self>,
        stale_token: Option<String>,
        fetch: Option<McpFetch>,
        challenge: Option<OAuthChallenge>,
    ) -> futures::future::Shared<BoxFuture<'static, Result<(), OAuthFlowError>>> {
        let existing = self
            .refreshing
            .lock()
            .expect("refresh slot cannot be poisoned")
            .clone();
        if let Some(existing) = existing {
            return existing;
        }
        let provider_state = Arc::clone(self);
        let raw: BoxFuture<'static, Result<(), OAuthFlowError>> = Box::pin(async move {
            let inner_state = Arc::clone(&provider_state);
            provider_state
                .store
                .with_refresh_lock(Box::pin(async move {
                    let provider_state = inner_state;
                    let state = provider_state.store.load().await;
                    let tokens = state.as_ref().and_then(|state| state.tokens());
                    if tokens.as_ref().map(|tokens| tokens.access_token.clone()) != stale_token {
                        // Tokens changed meanwhile (another process refreshed
                        // them, or the user signed in): use them as they are.
                        return Ok(());
                    }
                    let Some(refresh_token) = tokens
                        .as_ref()
                        .and_then(|tokens| tokens.refresh_token.clone())
                    else {
                        return Err(OAuthFlowError::AuthorizationRequired(
                            McpOAuthAuthorizationRequiredError {},
                        ));
                    };
                    let _ = refresh_token;
                    let settings = (provider_state.settings)().map_err(OAuthFlowError::Other)?;
                    let fixed_redirect = callback_settings(&settings)
                        .ok()
                        .and_then(|callback| callback.fixed_redirect_url);
                    let registered = registered_redirect_urls(
                        &state.as_ref().and_then(|state| state.client_information()),
                    );
                    let redirect_url = fixed_redirect
                        .or_else(|| registered.first().cloned())
                        .unwrap_or_else(|| FALLBACK_REDIRECT_URL.to_string());
                    let provider = create_provider(
                        &provider_state.server_url,
                        provider_state.state_store(),
                        &settings,
                        &redirect_url,
                        Arc::new(|_| Box::pin(async {})),
                    );
                    // Refreshes the tokens, or reports that a new sign-in is
                    // needed.
                    let base_fetch = fetch.unwrap_or_else(default_fetch);
                    let bounded_fetch: McpFetch = Arc::new(move |request| {
                        let base = Arc::clone(&base_fetch);
                        Box::pin(async move {
                            match tokio::time::timeout(
                                Duration::from_millis(REFRESH_REQUEST_TIMEOUT_MS),
                                base(request),
                            )
                            .await
                            {
                                Ok(result) => result,
                                Err(_) => Err(FetchError::other(
                                    "The operation was aborted due to timeout",
                                )),
                            }
                        })
                    });
                    let result = authorize_mcp(
                        &provider,
                        OAuthFlowOptions {
                            server_url: provider_state.server_url.clone(),
                            resource_metadata_url: challenge
                                .as_ref()
                                .and_then(|challenge| challenge.resource_metadata_url.clone()),
                            // v1.0.0: a configured metadata document replaces
                            // discovery here too.
                            authorization_server_metadata_url: settings
                                .auth_server_metadata_url
                                .clone(),
                            scope: challenge
                                .as_ref()
                                .and_then(|challenge| challenge.scope.clone()),
                            fetch: Some(bounded_fetch),
                            ..OAuthFlowOptions::default()
                        },
                    )
                    .await;
                    match result {
                        // The server rejected the grant: a new sign-in is
                        // needed.
                        Ok(OAuthFlowResult::Redirect) => {
                            Err(OAuthFlowError::AuthorizationRequired(
                                McpOAuthAuthorizationRequiredError {},
                            ))
                        }
                        other => other.map(|_| ()),
                    }
                }))
                .await
        });
        let shared = raw.shared();
        let mut slot = self
            .refreshing
            .lock()
            .expect("refresh slot cannot be poisoned");
        let shared = match slot.as_ref().cloned() {
            Some(existing) => existing,
            None => {
                *slot = Some(shared.clone());
                shared
            }
        };
        // Upstream `.finally`: the future clears the slot itself, so every
        // observer sees the cleared state once it settles.
        let clearer = Arc::clone(self);
        let settle_watcher = shared.clone();
        tokio::spawn(async move {
            let _ = settle_watcher.await;
            *clearer
                .refreshing
                .lock()
                .expect("refresh slot cannot be poisoned") = None;
        });
        shared
    }

    /// Resolves when no refresh is running, so shutdown does not drop rotated
    /// tokens before they are saved (upstream `settled`).
    pub async fn settled(&self) {
        let refreshing = self
            .refreshing
            .lock()
            .expect("refresh slot cannot be poisoned")
            .clone();
        if let Some(refreshing) = refreshing {
            let _ = refreshing.await;
        }
    }
}

impl AuthProvider for McpAuthProvider {
    fn token(&self) -> BoxFuture<'_, Option<String>> {
        Box::pin(async move {
            let this = self.owned();
            let in_flight = this
                .refreshing
                .lock()
                .expect("refresh slot cannot be poisoned")
                .clone();
            if let Some(shared) = in_flight {
                let _ = shared.await;
            }
            let state = this.store.load().await;
            let tokens = state.as_ref().and_then(|state| state.tokens());
            let token = tokens.as_ref().map(|tokens| tokens.access_token.clone());
            let expired = match state.as_ref().and_then(|state| state.tokens_expire_at()) {
                Some(expire_at) => expire_at - REFRESH_SKEW_MS <= now_ms() as f64,
                None => false,
            };
            let has_refresh = tokens
                .as_ref()
                .and_then(|tokens| tokens.refresh_token.clone())
                .is_some();
            if !expired || !has_refresh {
                return token;
            }
            // Failures fall through: the request goes out with the old token
            // and a 401 decides what happens.
            let _ = this.refresh(token.clone(), None, None).await;
            this.store
                .load()
                .await
                .and_then(|state| state.tokens())
                .map(|tokens| tokens.access_token)
        })
    }

    fn handles_unauthorized(&self) -> bool {
        true
    }

    fn on_unauthorized<'a>(
        &'a self,
        context: UnauthorizedContext,
    ) -> BoxFuture<'a, Result<(), McpClientError>> {
        Box::pin(async move {
            let challenge = parse_www_authenticate(context.response.header("www-authenticate"));
            (self.on_challenge)(&challenge);
            // A refresh keeps the granted scope, so more scope needs a new
            // sign-in.
            if challenge.error.as_deref() == Some("insufficient_scope") {
                return Err(McpClientError::OAuth(
                    OAuthFlowError::AuthorizationRequired(McpOAuthAuthorizationRequiredError {}),
                ));
            }
            let this = self.owned();
            this.refresh(context.token, Some(context.fetch), Some(challenge))
                .await
                .map_err(McpClientError::OAuth)
        })
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Upcast a server store to the plain state-store surface. A trait object of
/// the subtrait can call its supertrait methods directly; the wrapper hands
/// that surface to the provider.
fn widen_store(store: Arc<dyn McpOAuthServerStore>) -> Arc<dyn McpOAuthStateStore> {
    Arc::new(WidenedStore(store))
}

struct WidenedStore(Arc<dyn McpOAuthServerStore>);

impl McpOAuthStateStore for WidenedStore {
    fn load(&self) -> BoxFuture<'_, Option<McpOAuthState>> {
        self.0.load()
    }

    fn save(&self, state: McpOAuthState) -> BoxFuture<'_, ()> {
        self.0.save(state)
    }
}

/// The single in-flight refresh of one provider (upstream `refreshing`).
type RefreshingSlot =
    Mutex<Option<futures::future::Shared<BoxFuture<'static, Result<(), OAuthFlowError>>>>>;

/// Upstream `McpSignInPrompt`.
pub trait McpSignInPrompt: Send + Sync {
    /// Show the authorization URL to the user and open it in a browser.
    fn show_authorization_url(&self, url: &url::Url);
    /// Ask for the redirect URL from the browser address bar, for when the
    /// browser cannot reach the loopback callback (for example over SSH).
    /// Aborted once the callback arrives. Resolves to `None` when the user
    /// cancels.
    fn prompt_for_redirect_url<'a>(
        &'a self,
        signal: Arc<AbortSignal>,
    ) -> BoxFuture<'a, Option<String>>;
}

/// Upstream `McpSignInCancelledError` plus generic sign-in failures.
#[derive(Debug, Clone, PartialEq)]
pub enum McpSignInError {
    Cancelled,
    Failed(String),
}

impl McpSignInError {
    /// The `error.message` upstream.
    pub fn message(&self) -> String {
        match self {
            McpSignInError::Cancelled => "Sign-in cancelled".to_string(),
            McpSignInError::Failed(message) => message.clone(),
        }
    }
}

/// The `code` and `iss` of an authorization response pasted by the user
/// (v1.0.0 `responseFromRedirectUrl`).
type AuthorizationResponse = (String, Option<String>);

fn response_from_redirect_url(
    input: &str,
    state: &str,
    redirect_url: &url::Url,
) -> Result<AuthorizationResponse, McpSignInError> {
    let url = match url::Url::parse(input.trim()) {
        Ok(url) => url,
        Err(_) => {
            return Err(McpSignInError::Failed(
                "Expected the full redirect URL from the browser address bar".to_string(),
            ));
        }
    };
    // A server-specific redirect URI tells authorization servers apart, so it
    // must match exactly.
    if url.origin() != redirect_url.origin() || url.path() != redirect_url.path() {
        return Err(McpSignInError::Failed(
            "The redirect URL does not match this sign-in's redirect URI".to_string(),
        ));
    }
    let query: HashMap<String, String> = url
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    if let Some(error) = query.get("error") {
        return Err(McpSignInError::Failed(
            query.get("error_description").unwrap_or(error).clone(),
        ));
    }
    if query.get("state").map(String::as_str) != Some(state) {
        return Err(McpSignInError::Failed(
            "The redirect URL belongs to a different sign-in".to_string(),
        ));
    }
    match query.get("code").filter(|code| !code.is_empty()) {
        Some(code) => Ok((
            code.clone(),
            query.get("iss").filter(|iss| !iss.is_empty()).cloned(),
        )),
        None => Err(McpSignInError::Failed(
            "The redirect URL does not contain an authorization code".to_string(),
        )),
    }
}

/// Wait for the browser callback or a pasted redirect URL, whichever comes
/// first (upstream `waitForAuthorizationResponse`).
async fn wait_for_authorization_response(
    callback: &OAuthCallbackServer,
    state: &str,
    redirect_url: &url::Url,
    prompt: &dyn McpSignInPrompt,
) -> Result<AuthorizationResponse, McpSignInError> {
    let controller = Arc::new(AbortSignal::new());
    let from_browser = callback.wait_for_callback(state, Some(redirect_url.path().to_string()));
    let from_user = {
        let controller = Arc::clone(&controller);
        let redirect_url = redirect_url.clone();
        let state = state.to_string();
        async move {
            prompt
                .prompt_for_redirect_url(controller)
                .await
                .and_then(|input| {
                    let trimmed = input.trim();
                    (!trimmed.is_empty()).then(|| trimmed.to_string())
                })
                .map(|input| response_from_redirect_url(&input, &state, &redirect_url))
        }
    };
    let result = tokio::select! {
        result = from_browser => match result {
            Ok(callback_result) => Ok((
                callback_result.code,
                callback_result.iss.filter(|iss| !iss.is_empty()),
            )),
            Err(error) => Err(McpSignInError::Failed(error)),
        },
        input = from_user => match input {
            Some(result) => result,
            _ => Err(McpSignInError::Cancelled),
        },
    };
    // The losing side rejects once the prompt is aborted or the callback
    // server closes.
    controller.abort();
    result
}

/// Listen on `port`, or on a free port when it is taken and not `required`
/// (upstream `listenForCallback`).
async fn listen_for_callback(
    settings: &CallbackSettings,
    extra_paths: &[String],
    port: Option<u16>,
    required: bool,
) -> Result<OAuthCallbackServer, String> {
    let render_page: RenderPage = Arc::new(|page: &OAuthCallbackPage| match page {
        OAuthCallbackPage::Ok => {
            oauth_success_html("Signed in to the MCP server. You may now close this page.")
        }
        OAuthCallbackPage::Error { message, details } => {
            oauth_error_html(message, details.as_deref())
        }
    });
    let extra_paths = (!extra_paths.is_empty()).then(|| extra_paths.to_vec());
    let options = |port: Option<u16>| OAuthCallbackServerOptions {
        host: Some(settings.host.clone()),
        redirect_host: Some(settings.redirect_host.clone()),
        port,
        path: Some(settings.path.clone()),
        extra_paths: extra_paths.clone(),
        timeout_ms: None,
        render_page: Some(Arc::clone(&render_page)),
    };
    match OAuthCallbackServer::listen(options(Some(port.unwrap_or(0)))).await {
        Ok(server) => Ok(server),
        Err(error) => {
            if required || port.is_none() {
                return Err(error);
            }
            OAuthCallbackServer::listen(options(None)).await
        }
    }
}

/// Sign in to an MCP server. Uses the stored refresh token when possible;
/// otherwise runs the browser authorization code flow. Tokens are saved to
/// `store` (upstream `signInMcpServer`).
pub async fn sign_in_mcp_server(
    server_url: &str,
    store: Arc<dyn McpOAuthServerStore>,
    settings: McpOAuthSettings,
    challenge: Option<OAuthChallenge>,
    prompt: Arc<dyn McpSignInPrompt>,
) -> Result<(), McpSignInError> {
    let stored = store.load().await;
    // A server asking for more scope gets the missing scopes on top of its
    // current grant in the browser flow.
    let step_up = challenge
        .as_ref()
        .and_then(|challenge| challenge.error.as_deref())
        == Some("insufficient_scope");
    let callback_options = callback_settings(&settings).map_err(McpSignInError::Failed)?;
    // Reuse the port of the registered redirect URI so the registered client
    // stays valid.
    let registered =
        registered_redirect_urls(&stored.as_ref().and_then(|state| state.client_information()));
    let preferred_port = callback_options.port.or_else(|| {
        registered
            .first()
            .and_then(|registered| url::Url::parse(registered).ok().and_then(|url| url.port()))
    });
    // v1.0.0: the Client ID Metadata Document's redirect URI, when used.
    let cimd = settings.client_registration
        == Some(crate::coding_agent::core::mcp_servers::McpClientRegistration::Cimd);
    let extra_paths: Vec<String> = if cimd {
        vec![format!("{CALLBACK_PATH}/{}", callback_id(server_url))]
    } else {
        Vec::new()
    };
    let callback = listen_for_callback(
        &callback_options,
        &extra_paths,
        preferred_port,
        callback_options.port.is_some(),
    )
    .await
    .map_err(McpSignInError::Failed)?;
    let redirect_url = callback_options
        .fixed_redirect_url
        .clone()
        .unwrap_or_else(|| callback.redirect_url().to_string());
    // The scope granted so far, which a step-up challenge extends (the
    // challenge may list only the missing scopes).
    let stored_scope: Option<String> = stored
        .as_ref()
        .and_then(|state| state.tokens())
        .and_then(|tokens| tokens.scope.clone());
    let flow_for = |mut options: OAuthFlowOptions| {
        options.server_url = server_url.to_string();
        options.resource_metadata_url = challenge
            .as_ref()
            .and_then(|challenge| challenge.resource_metadata_url.clone());
        options.authorization_server_metadata_url = settings.auth_server_metadata_url.clone();
        // A server asking for more scope gets it on top of the configured
        // scope and, since the challenge may list only the missing scopes, on
        // top of the scope granted so far.
        let step_up_scope = if step_up {
            step_up_scope(
                stored_scope.as_deref(),
                challenge
                    .as_ref()
                    .and_then(|challenge| challenge.scope.as_deref()),
            )
        } else {
            challenge
                .as_ref()
                .and_then(|challenge| challenge.scope.clone())
        };
        options.scope = merge_scopes(&[settings.scope.as_deref(), step_up_scope.as_deref()]);
        options
    };
    let result: Result<(), McpSignInError> = async {
        if let Some(state) = &stored {
            let mut raw = state.raw().clone();
            // Every sign-in gets a fresh `state` parameter.
            raw.remove("oauthState");
            // A registered client cannot use another redirect URI, and its
            // tokens belong to it. A Client ID Metadata Document is not
            // stored, so with one, a stored client was registered before and
            // is replaced.
            let keep_client = settings.client_id.is_some()
                || if cimd {
                    stored
                        .as_ref()
                        .and_then(|state| state.client_information())
                        .is_none()
                } else {
                    registered.contains(&redirect_url)
                };
            if !keep_client {
                raw.remove("clientInformation");
                raw.remove("tokens");
                raw.remove("tokensExpireAt");
            }
            store.save(McpOAuthState::from_raw(raw)).await;
        }
        let authorization_url: Arc<Mutex<Option<url::Url>>> = Arc::new(Mutex::new(None));
        let authorization_slot = Arc::clone(&authorization_url);
        let store_for_provider = widen_store(Arc::clone(&store));
        let provider = create_provider(
            server_url,
            store_for_provider,
            &settings,
            &redirect_url,
            Arc::new(move |url| {
                *authorization_slot
                    .lock()
                    .expect("authorization slot cannot be poisoned") = Some(url);
                Box::pin(async {})
            }),
        );
        // A refresh keeps the granted scope; a server asking for more needs
        // the browser flow.
        let flow = flow_for(OAuthFlowOptions {
            skip_refresh: step_up,
            ..OAuthFlowOptions::default()
        });
        if matches!(
            authorize_mcp(&provider, flow).await,
            Ok(OAuthFlowResult::Authorized)
        ) {
            return Ok(());
        }
        let authorization_value = authorization_url
            .lock()
            .expect("authorization slot cannot be poisoned")
            .clone();
        let Some(authorization_url) = authorization_value else {
            return Err(McpSignInError::Failed(
                "OAuth flow did not produce an authorization URL".to_string(),
            ));
        };
        let state = provider
            .state()
            .await
            .map_err(|error| McpSignInError::Failed(error.to_string()))?;
        // The flow picks the redirect URI, which may be specific to the MCP
        // server.
        let authorization_redirect_url = authorization_url
            .query_pairs()
            .find(|(key, _)| key == "redirect_uri")
            .map(|(_, value)| value.into_owned())
            .and_then(|value| url::Url::parse(&value).ok())
            .unwrap_or_else(|| {
                url::Url::parse(&redirect_url).expect("redirect URL is a valid URL")
            });
        prompt.show_authorization_url(&authorization_url);
        let (code, iss) = wait_for_authorization_response(
            &callback,
            &state,
            &authorization_redirect_url,
            prompt.as_ref(),
        )
        .await?;
        let flow = flow_for(OAuthFlowOptions {
            authorization_code: Some(code),
            iss,
            ..OAuthFlowOptions::default()
        });
        authorize_mcp(&provider, flow)
            .await
            .map(|_| ())
            .map_err(|error| McpSignInError::Failed(error.to_string()))
    }
    .await;
    let _ = callback.close().await;
    result
}
