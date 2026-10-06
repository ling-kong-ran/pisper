//! Port of upstream `coding-agent/src/extensions/mcp/runtime.ts` (HEAD
//! `2bbfcca43`): the part of the MCP integration that talks to servers —
//! connections, transports, and OAuth sign-in. It pulls in the MCP client, so
//! the extension only constructs connections when a server is configured
//! (upstream loads it through `runtime.lazy.ts`).
//!
//! Disclosed seams:
//! - **`transport instanceof StdioTransport`**: the port's transport factory
//!   returns a [`CreatedTransport`] carrying the stdio handle alongside the
//!   erased transport, so the stderr tails in `failed` / `disconnected`
//!   states keep working without downcasting.
//! - **`VERSION`**: the client identifies as `pi` with the port's package
//!   version (upstream embeds the npm package version).
//! - Client/transport errors travel as their `Display` strings
//!   (`error.message` upstream).
//! - Upstream compares client objects by identity; the port tags the current
//!   client with a generation number and compares that.

use std::sync::{Arc, Mutex, Weak};

use futures::future::BoxFuture;
use futures::FutureExt as _;

use crate::coding_agent::core::path_join;
use crate::coding_agent::core::resolve_config_value::{
    resolve_config_value_or_throw, resolve_headers_or_throw,
};
use crate::coding_agent::extensions::mcp::config::McpServerEntry;
use crate::coding_agent::extensions::mcp::oauth::{
    McpAuthProvider, McpOAuthCredentialStore, McpOAuthSettings,
};
use crate::coding_agent::extensions::mcp::resources::McpResourceServer;
use crate::coding_agent::extensions::mcp::tools::McpToolCaller;
use crate::mcp::auth_provider::AuthProvider;
use crate::mcp::client::{
    ClientState, McpClient, McpClientOptions, McpRequestOptions, RootsProvider,
};
use crate::mcp::oauth::errors::OAuthFlowError;
use crate::mcp::oauth::types::OAuthChallenge;
use crate::mcp::protocol::content::CallToolResult;
use crate::mcp::protocol::jsonrpc::{McpClientError, JSON_RPC_ERROR_CODES_METHOD_NOT_FOUND};
use crate::mcp::protocol::types::{
    ListResourceTemplatesResult, ListResourcesResult, ReadResourceResult, Resource,
    ResourceTemplate, Root, Tool as McpTool,
};
use crate::mcp::transports::stdio::{StdioTransport, StdioTransportOptions};
use crate::mcp::transports::streamable_http::{
    StreamableHttpTransport, StreamableHttpTransportOptions,
};
use crate::mcp::transports::McpTransport;

// Upstream `export { McpServerLog }`, `export { McpOAuthCredentialStore,
// McpSignInCancelledError, signInMcpServer } from "./oauth.ts"`.
pub use crate::coding_agent::extensions::mcp::log::McpServerLog;
pub use crate::coding_agent::extensions::mcp::oauth::{sign_in_mcp_server, McpSignInError};

const DEFAULT_TIMEOUT_SECONDS: f64 = 60.0;
const STDERR_TAIL_CHARS: usize = 2_000;
/// Delays between attempts to connect to an HTTP server that failed with a
/// transient error.
const CONNECT_RETRY_DELAYS_MS: [u64; 2] = [250, 1_000];

/// Upstream `ServerState`.
///
/// - `disconnected`: the connection dropped (for example the stdio server
///   exited); the next call reconnects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerState {
    Connecting,
    Connected,
    Disconnected,
    NeedsAuth,
    Failed,
    Closed,
}

impl ServerState {
    pub fn as_str(&self) -> &'static str {
        match self {
            ServerState::Connecting => "connecting",
            ServerState::Connected => "connected",
            ServerState::Disconnected => "disconnected",
            ServerState::NeedsAuth => "needs-auth",
            ServerState::Failed => "failed",
            ServerState::Closed => "closed",
        }
    }
}

/// The transport a factory built, with its stdio half when there is one (see
/// the module docs for the `instanceof` seam).
pub struct CreatedTransport {
    pub transport: Arc<dyn McpTransport>,
    pub stdio: Option<Arc<StdioTransport>>,
}

/// Upstream `McpTransportFactory`:
/// `(entry, cwd, authProvider) => McpTransport`.
pub type TransportFactoryFn = dyn Fn(&McpServerEntry, &str, Option<Arc<dyn AuthProvider>>) -> Result<CreatedTransport, String>
    + Send
    + Sync;
pub type McpTransportFactory = Arc<TransportFactoryFn>;

/// The connection's `onTools` callback (`(connection) => void`).
pub type OnToolsHandler = Arc<dyn Fn(&McpServerConnection) + Send + Sync>;
/// The connection's `onChange` callback.
pub type OnChangeHandler = Arc<dyn Fn(&McpServerConnection) + Send + Sync>;

/// Network failures and overloaded or restarting servers, which are worth
/// another attempt (upstream `isTransientError`).
fn is_transient_error(error: &McpClientError) -> bool {
    match error {
        McpClientError::Http(http) => {
            http.status == 408 || http.status == 429 || (http.status >= 500 && http.status != 501)
        }
        // Upstream `TypeError`.
        McpClientError::Network(_) => true,
        _ => false,
    }
}

/// `MCP server "x" requires sign-in. Run /mcp to sign in.` — with
/// `auth.provider`, sign-in goes through that provider instead (v1.0.0).
fn sign_in_required_message(entry: &McpServerEntry) -> String {
    let provider = entry.config.auth_provider();
    format!(
        "MCP server \"{}\" requires sign-in. Run {} to sign in.",
        entry.name,
        match provider {
            Some(provider) => format!("/login {provider}"),
            None => "/mcp".to_string(),
        }
    )
}

/// HTTP servers authenticate with OAuth unless the config supplies an
/// `Authorization` header or `auth` (upstream `usesOAuth`).
pub fn uses_oauth(entry: &McpServerEntry) -> bool {
    if entry.config.url().is_none() || entry.config.auth_provider().is_some() {
        return false;
    }
    !entry
        .config
        .string_record("headers")
        .unwrap_or_default()
        .iter()
        .any(|(header, _)| header.to_lowercase() == "authorization")
}

/// `~` and `~/…` (also `~\…` on Windows) name the home directory, like in a
/// shell (upstream `expandHome`).
fn expand_home(value: &str) -> String {
    let home = home_dir();
    if value == "~" {
        return home;
    }
    if let Some(rest) = value.strip_prefix("~/").or_else(|| {
        if cfg!(windows) {
            value.strip_prefix("~\\")
        } else {
            None
        }
    }) {
        return path_join(&home, rest);
    }
    value.to_string()
}

fn home_dir() -> String {
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_default()
}

/// Node `path.resolve(cwd, value)`: absolute values pass through, others join.
fn resolve_path(cwd: &str, value: &str) -> String {
    if std::path::Path::new(value).is_absolute() {
        value.to_string()
    } else {
        path_join(cwd, value)
    }
}

/// Upstream `createDefaultTransport`: stdio and streamable HTTP transports
/// built from the server config.
pub fn create_default_transport(
    entry: &McpServerEntry,
    cwd: &str,
    auth_provider: Option<Arc<dyn AuthProvider>>,
) -> Result<CreatedTransport, String> {
    let name = &entry.name;
    if let Some(url) = entry.config.url() {
        let parsed = url::Url::parse(url).map_err(|error| error.to_string())?;
        let headers = resolve_headers_or_throw(
            entry.config.string_record("headers").as_deref(),
            &format!("MCP server \"{name}\""),
            None,
        )
        .map_err(|error| error.to_string())?
        .unwrap_or_default();
        let mut options = StreamableHttpTransportOptions::new(parsed);
        options.headers = headers;
        options.auth_provider = auth_provider;
        return Ok(CreatedTransport {
            transport: Arc::new(StreamableHttpTransport::new(options)),
            stdio: None,
        });
    }
    let mut env: Vec<(String, String)> = Vec::new();
    for (key, value) in entry.config.string_record("env").unwrap_or_default() {
        let resolved = resolve_config_value_or_throw(
            &value,
            &format!("MCP server \"{name}\" env \"{key}\""),
            None,
        )
        .map_err(|error| error.to_string())?;
        env.push((key, resolved));
    }
    let command = expand_home(entry.config.command().unwrap_or_default());
    let args: Vec<String> = entry
        .config
        .args()
        .unwrap_or_default()
        .into_iter()
        .map(expand_home)
        .collect();
    let cwd_resolved = resolve_path(cwd, &expand_home(entry.config.cwd().unwrap_or(".")));
    let mut options = StdioTransportOptions::new(command);
    options.args = args;
    options.cwd = Some(cwd_resolved);
    options.env = Some(env);
    // Upstream `stderr: "pipe"` (the collector tails the last 2KB).
    options.inherit_stderr = false;
    let stdio = Arc::new(StdioTransport::new(options));
    Ok(CreatedTransport {
        transport: stdio.clone(),
        stdio: Some(stdio),
    })
}

/// Servers that do not implement `resources/templates/list` have no templates
/// (upstream `withoutTemplates`).
async fn without_templates<T>(
    list: impl std::future::Future<Output = Result<T, McpClientError>>,
    empty: T,
) -> Result<T, McpClientError> {
    match list.await {
        Ok(value) => Ok(value),
        Err(error) => match &error {
            McpClientError::Mcp(mcp_error)
                if mcp_error.code == JSON_RPC_ERROR_CODES_METHOD_NOT_FOUND =>
            {
                Ok(empty)
            }
            _ => Err(error),
        },
    }
}

/// Resources and templates at connect time, for the counts in `/mcp` and
/// `pi mcp list`. A server whose lists fail still connects: the resource tools
/// list and read its resources on demand (upstream `fetchResources`).
async fn fetch_resources(client: &McpClient) -> (Vec<Resource>, Vec<ResourceTemplate>) {
    let resources = client
        .list_resources(McpRequestOptions::default())
        .await
        .unwrap_or_default();
    let resource_templates = without_templates(
        client.list_resource_templates(McpRequestOptions::default()),
        Vec::new(),
    )
    .await
    .unwrap_or_default();
    let is_app = |value: &serde_json::Value| super::resources::is_mcp_app_resource(value);
    let resources = resources
        .into_iter()
        .filter(|resource| {
            !is_app(&serde_json::to_value(resource).unwrap_or(serde_json::Value::Null))
        })
        .collect();
    let resource_templates = resource_templates
        .into_iter()
        .filter(|template| {
            !is_app(&serde_json::to_value(template).unwrap_or(serde_json::Value::Null))
        })
        .collect();
    (resources, resource_templates)
}

type OpeningFuture = futures::future::Shared<BoxFuture<'static, Result<McpClient, String>>>;

struct ConnectionShared {
    state: ServerState,
    error: Option<String>,
    tools: Vec<McpTool>,
    /// Whether the server offers resources. The lists below are what it listed
    /// at the last connect or change, without MCP App resources.
    has_resources: bool,
    resources: Vec<Resource>,
    resource_templates: Vec<ResourceTemplate>,
    /// Server instructions from `initialize`, describing its tools as a group.
    instructions: Option<String>,
    client: Option<McpClient>,
    /// Keep setup resources reachable by close(), even before initialize and
    /// tools/list finish (Pisper native MCP cancellation patch).
    connecting: Option<(u64, McpClient, Arc<dyn McpTransport>)>,
    /// Generation tag of `client` (identity stand-in).
    client_generation: u64,
    next_client_generation: u64,
    opening: Option<OpeningFuture>,
    closed: bool,
    /// Stderr of the last stdio server that failed to connect.
    stderr_tail: Option<String>,
}

/// One connection's auth source (upstream `this.authProvider`: either a full
/// [`McpAuthProvider`] or, since v1.0.0, the read-only token resolver of
/// `auth.provider`).
#[derive(Clone)]
enum ConnectionAuth {
    /// OAuth with stored tokens and refreshes.
    OAuth(Arc<McpAuthProvider>),
    /// `auth.provider` (v1.0.0): read the pi provider's token on every
    /// request, so the provider's refreshes apply; MCP stores no copy.
    /// The provider name is already bound.
    ProviderToken(Arc<dyn Fn() -> BoxFuture<'static, Option<String>> + Send + Sync>),
}

impl ConnectionAuth {
    /// The provider surface the transport uses.
    fn as_auth_provider(self: &Arc<Self>) -> Option<Arc<dyn AuthProvider>> {
        match self.as_ref() {
            ConnectionAuth::OAuth(provider) => Some(Arc::clone(provider) as Arc<dyn AuthProvider>),
            ConnectionAuth::ProviderToken(resolver) => {
                Some(Arc::new(ProviderTokenAuthProvider(Arc::clone(resolver))))
            }
        }
    }

    /// Resolves when no refresh is in flight, so shutdown does not drop
    /// rotated tokens before they are saved (upstream `settled`).
    fn settled(&self) -> BoxFuture<'_, ()> {
        match self {
            ConnectionAuth::OAuth(provider) => Box::pin(provider.settled()),
            ConnectionAuth::ProviderToken(_) => Box::pin(async {}),
        }
    }
}

/// `{ token: async () => options.providerToken?.(provider) }`: no
/// `onUnauthorized`, so step-up responses surface as ordinary errors.
struct ProviderTokenAuthProvider(Arc<dyn Fn() -> BoxFuture<'static, Option<String>> + Send + Sync>);

impl AuthProvider for ProviderTokenAuthProvider {
    fn token(&self) -> BoxFuture<'_, Option<String>> {
        Box::pin(async move { (self.0)().await })
    }
}

/// Upstream `McpServerConnectionOptions.providerToken` (v1.0.0): the current
/// token of a pi provider, for servers with `auth.provider`.
pub type ProviderTokenResolver =
    Arc<dyn Fn(&str) -> BoxFuture<'static, Option<String>> + Send + Sync>;

/// One configured server. Reconnects lazily when a call finds the connection
/// gone (upstream `McpServerConnection`, implementing `McpToolCaller` and
/// `McpResourceServer`).
pub struct McpServerConnection {
    pub entry: McpServerEntry,
    shared: Mutex<ConnectionShared>,
    /// Last OAuth challenge from the server; sign-in uses its resource
    /// metadata URL and scope (shared with the auth provider's `onChallenge`).
    challenge_slot: Mutex<Option<OAuthChallenge>>,
    cwd: String,
    create_transport: McpTransportFactory,
    auth_provider: Option<Arc<ConnectionAuth>>,
    on_tools: OnToolsHandler,
    /// Called when `state`, `error`, or `tools` change.
    on_change: Option<OnChangeHandler>,
    /// Receives the server's log messages (`notifications/message`).
    log: Option<Arc<McpServerLog>>,
    /// The owning handle (`Arc::new_cyclic`), so callbacks and the
    /// `&self`-taking trait methods can reach an owned connection.
    self_handle: Weak<McpServerConnection>,
}

/// Upstream `new McpServerConnection(options)`.
pub struct McpServerConnectionOptions {
    pub entry: McpServerEntry,
    pub cwd: String,
    pub create_transport: McpTransportFactory,
    pub credentials: Arc<McpOAuthCredentialStore>,
    /// The current token of a pi provider, for servers with `auth.provider`
    /// (v1.0.0).
    pub provider_token: Option<ProviderTokenResolver>,
    pub on_tools: OnToolsHandler,
    pub on_change: Option<OnChangeHandler>,
    pub log: Option<Arc<McpServerLog>>,
}

impl McpServerConnection {
    pub fn new(options: McpServerConnectionOptions) -> Arc<Self> {
        let entry = options.entry;
        let oauth_url = oauth_url_of(&entry);
        let provider = entry.config.auth_provider().map(str::to_string);
        let challenge_slot: Arc<Mutex<Option<OAuthChallenge>>> = Arc::new(Mutex::new(None));
        let auth_provider: Option<Arc<ConnectionAuth>> = if let Some(server_url) = &oauth_url {
            let credentials = Arc::clone(&options.credentials);
            let settings_entry = entry.clone();
            let settings: Arc<dyn Fn() -> Result<McpOAuthSettings, String> + Send + Sync> =
                Arc::new(move || Ok(oauth_settings_of(&settings_entry)));
            let on_challenge_slot = Arc::clone(&challenge_slot);
            let on_challenge: Arc<dyn Fn(&OAuthChallenge) + Send + Sync> =
                Arc::new(move |challenge| {
                    *on_challenge_slot
                        .lock()
                        .expect("challenge slot cannot be poisoned") = Some(challenge.clone());
                });
            Some(Arc::new(ConnectionAuth::OAuth(McpAuthProvider::create(
                server_url,
                credentials.for_server(&entry.name, server_url),
                settings,
                on_challenge,
            ))))
        } else if let Some(provider_name) = provider {
            // Read on every request, so the provider's refreshes apply; MCP
            // stores no copy.
            let resolver = options.provider_token.clone();
            let token: Arc<dyn Fn() -> BoxFuture<'static, Option<String>> + Send + Sync> =
                Arc::new(move || {
                    let resolver = resolver.clone();
                    let provider_name = provider_name.clone();
                    Box::pin(async move {
                        match resolver {
                            Some(resolver) => resolver(&provider_name).await,
                            None => None,
                        }
                    })
                });
            Some(Arc::new(ConnectionAuth::ProviderToken(token)))
        } else {
            None
        };
        Arc::new_cyclic(|handle| McpServerConnection {
            entry,
            shared: Mutex::new(ConnectionShared {
                state: ServerState::Connecting,
                error: None,
                tools: Vec::new(),
                has_resources: false,
                resources: Vec::new(),
                resource_templates: Vec::new(),
                instructions: None,
                client: None,
                connecting: None,
                client_generation: 0,
                next_client_generation: 1,
                opening: None,
                closed: false,
                stderr_tail: None,
            }),
            challenge_slot: Mutex::new(None),
            cwd: options.cwd,
            create_transport: options.create_transport,
            auth_provider,
            on_tools: options.on_tools,
            on_change: options.on_change,
            log: options.log,
            self_handle: handle.clone(),
        })
    }

    pub fn name(&self) -> &str {
        &self.entry.name
    }

    pub fn timeout_ms(&self) -> u64 {
        let seconds = self
            .entry
            .config
            .timeout()
            .unwrap_or(DEFAULT_TIMEOUT_SECONDS);
        (seconds * 1000.0) as u64
    }

    /// Server URL when the server authenticates with OAuth (upstream
    /// `oauthUrl`).
    pub fn oauth_url(&self) -> Option<String> {
        oauth_url_of(&self.entry)
    }

    pub fn oauth_settings(&self) -> McpOAuthSettings {
        oauth_settings_of(&self.entry)
    }

    fn handle(&self) -> Arc<McpServerConnection> {
        self.self_handle
            .upgrade()
            .expect("connection Arc must outlive its callbacks")
    }

    /// The currently stored client, if any (used by notification listeners so
    /// they never hold a strong client handle inside the client itself).
    fn current_client(&self) -> Option<McpClient> {
        self.lock().client.clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ConnectionShared> {
        self.shared
            .lock()
            .expect("connection state cannot be poisoned")
    }

    pub fn state(&self) -> ServerState {
        self.lock().state
    }

    pub fn error(&self) -> Option<String> {
        self.lock().error.clone()
    }

    pub fn tools(&self) -> Vec<McpTool> {
        self.lock().tools.clone()
    }

    pub fn has_resources(&self) -> bool {
        self.lock().has_resources
    }

    pub fn resources(&self) -> Vec<Resource> {
        self.lock().resources.clone()
    }

    pub fn resource_templates(&self) -> Vec<ResourceTemplate> {
        self.lock().resource_templates.clone()
    }

    /// Server instructions from `initialize`, describing its tools as a group.
    pub fn instructions(&self) -> Option<String> {
        self.lock().instructions.clone()
    }

    /// Last OAuth challenge from the server.
    pub fn challenge(&self) -> Option<OAuthChallenge> {
        self.challenge_slot
            .lock()
            .expect("challenge slot cannot be poisoned")
            .clone()
    }

    /// Upstream `connection.challenge = undefined`.
    pub fn clear_challenge(&self) {
        *self
            .challenge_slot
            .lock()
            .expect("challenge slot cannot be poisoned") = None;
    }

    pub fn is_closed(&self) -> bool {
        self.lock().closed
    }

    fn changed(&self) {
        if let Some(on_change) = &self.on_change {
            on_change(&self.handle());
        }
    }

    fn mark_needs_auth(&self) {
        {
            let mut shared = self.lock();
            if shared.closed {
                return;
            }
            shared.state = ServerState::NeedsAuth;
            shared.error = None;
        }
        self.changed();
    }

    /// Upstream `getClient()`.
    pub async fn get_client(self: &Arc<Self>) -> Result<McpClient, String> {
        let opening = {
            let mut shared = self.lock();
            if shared.closed {
                return Err(format!("MCP server \"{}\" is shut down", self.entry.name));
            }
            if let Some(client) = &shared.client {
                if client.connection_state() == ClientState::Connected {
                    return Ok(client.clone());
                }
            }
            shared
                .opening
                .get_or_insert_with(|| {
                    let connection = Arc::downgrade(self);
                    let raw: BoxFuture<'static, Result<McpClient, String>> =
                        Box::pin(async move {
                            let connection = connection.upgrade().ok_or_else(|| {
                                "MCP connection was dropped before setup".to_string()
                            })?;
                            connection.open().await
                        });
                    raw.shared()
                })
                .clone()
        };
        let result = opening.clone().await;
        // Upstream `.finally(() => { this.opening = undefined; })`.
        {
            let mut shared = self.lock();
            if shared
                .opening
                .as_ref()
                .is_some_and(|cached| cached.ptr_eq(&opening))
            {
                shared.opening = None;
            }
        }
        result
    }

    /// Upstream `withClient(run, readOnly?)`: run a request, reconnecting when
    /// needed. `read_only` requests are retried once after a transient HTTP
    /// error; tool calls are not, since they may have run.
    async fn with_client<T>(
        self: &Arc<Self>,
        run: impl Fn(McpClient) -> BoxFuture<'static, Result<T, McpClientError>> + Send,
        read_only: bool,
    ) -> Result<T, String> {
        let mut attempt = 1u32;
        loop {
            let client = self.get_client().await?;
            match run(client.clone()).await {
                Ok(value) => return Ok(value),
                Err(error) => {
                    if read_only && attempt == 1 && is_transient_error(&error) {
                        tokio::time::sleep(std::time::Duration::from_millis(
                            CONNECT_RETRY_DELAYS_MS[0],
                        ))
                        .await;
                        attempt += 1;
                        continue;
                    }
                    if matches!(error, McpClientError::SessionExpired(_)) && attempt == 1 {
                        // The server no longer knows the session (restart,
                        // deploy), so it did not run the request. Retry once on
                        // a new session. The old client is detached but not
                        // closed: closing would fail its other in-flight calls,
                        // which instead get the same 404 and retry the same
                        // way.
                        {
                            let mut shared = self.lock();
                            if shared.client_generation != 0 {
                                shared.client = None;
                            }
                        }
                        attempt += 1;
                        continue;
                    }
                    if !self.needs_sign_in(&error) {
                        return Err(error.to_string());
                    }
                    self.drop_client().await;
                    self.mark_needs_auth();
                    return Err(sign_in_required_message(&self.entry));
                }
            }
        }
    }

    fn needs_sign_in(&self, error: &McpClientError) -> bool {
        let authorization_required = matches!(
            error,
            McpClientError::OAuth(OAuthFlowError::AuthorizationRequired(_))
        );
        authorization_required
            || (self.auth_provider.is_some() && matches!(error, McpClientError::AuthRequired(_)))
    }

    async fn drop_client(&self) {
        let client = {
            let mut shared = self.lock();
            shared.client.take()
        };
        if let Some(client) = client {
            let _ = client.close().await;
        }
    }

    /// Connect again with fresh credentials, for example after signing in
    /// (upstream `reconnect`).
    pub async fn reconnect(self: &Arc<Self>) -> Result<(), String> {
        let opening = self.lock().opening.clone();
        if let Some(opening) = opening {
            let _ = opening.await;
        }
        self.drop_client().await;
        self.get_client().await.map(|_| ())
    }

    /// Disconnect after the stored credentials were removed (upstream
    /// `signOut`).
    pub async fn sign_out(self: &Arc<Self>) {
        let opening = self.lock().opening.clone();
        if let Some(opening) = opening {
            let _ = opening.await;
        }
        self.drop_client().await;
        let closed = self.lock().closed;
        if !closed {
            self.mark_needs_auth();
        }
    }

    async fn open(self: Arc<Self>) -> Result<McpClient, String> {
        {
            let mut shared = self.lock();
            if shared.closed {
                return Err(format!("MCP server \"{}\" is shut down", self.entry.name));
            }
            shared.state = ServerState::Connecting;
        }
        self.changed();
        let retries: &[u64] = if self.entry.config.url().is_some() {
            &CONNECT_RETRY_DELAYS_MS
        } else {
            &[]
        };
        let mut attempt = 0usize;
        loop {
            {
                self.lock().stderr_tail = None;
            }
            match Arc::clone(&self).connect_once().await {
                Ok(client) => return Ok(client),
                Err(error) => {
                    let delay = retries.get(attempt).copied();
                    let closed = self.lock().closed;
                    if closed || delay.is_none() || !is_transient_error(&error) {
                        return Err(self.connect_failed(error));
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(delay.expect("checked")))
                        .await;
                    if self.lock().closed {
                        return Err(self.connect_failed(error));
                    }
                    attempt += 1;
                }
            }
        }
    }

    async fn connect_once(self: Arc<Self>) -> Result<McpClient, McpClientError> {
        let cwd = self.cwd.clone();
        let mut client_options =
            McpClientOptions::new("pi", crate::coding_agent::extensions::mcp::VERSION);
        client_options.request_timeout_ms = Some(self.timeout_ms());
        client_options.roots = Some(RootsProvider::List(Arc::new(vec![Root {
            uri: file_url(&cwd),
            name: Some(basename(&cwd)),
            extra: Default::default(),
        }])));
        let client = McpClient::new(client_options);
        let generation = {
            let mut shared = self.lock();
            let generation = shared.next_client_generation;
            shared.next_client_generation += 1;
            generation
        };
        if let Some(log) = &self.log {
            let log = Arc::clone(log);
            let server_name = self.entry.name.clone();
            client.on_notification(
                "notifications/message",
                Arc::new(move |params: &serde_json::Value| {
                    log.write(&server_name, params);
                }),
            );
        }
        let transport = {
            let provider: Option<Arc<dyn AuthProvider>> = self
                .auth_provider
                .as_ref()
                .and_then(|provider| provider.as_auth_provider());
            (self.create_transport)(&self.entry, &self.cwd, provider)
                .map_err(McpClientError::Other)?
        };
        let tracked = {
            let mut shared = self.lock();
            if shared.closed {
                false
            } else {
                shared.connecting = Some((generation, client.clone(), transport.transport.clone()));
                true
            }
        };
        if !tracked {
            self.close_setup(generation, &client, &transport.transport).await;
            return Err(McpClientError::Other("shut down while connecting".to_string()));
        }
        if let Err(error) = client.connect(transport.transport.clone()).await {
            self.close_setup(generation, &client, &transport.transport).await;
            if let Some(stdio) = &transport.stdio {
                let tail = tail_chars(&stdio.stderr(), STDERR_TAIL_CHARS);
                self.lock().stderr_tail = (!tail.is_empty()).then_some(tail);
            }
            return Err(error);
        }
        // notifications/tools/list_changed → refresh tools.
        {
            // The listeners hold weak handles: the connection owns the client,
            // and the client owns its listeners (no reference cycles).
            let connection = Arc::downgrade(&self);
            client.on_notification(
                "notifications/tools/list_changed",
                Arc::new(move |_params: &serde_json::Value| {
                    if let Some(connection) = connection.upgrade() {
                        if let Some(client) = connection.current_client() {
                            tokio::spawn(async move {
                                connection.refresh_tools(&client, generation).await;
                            });
                        }
                    }
                }),
            );
            let connection = Arc::downgrade(&self);
            client.on_notification(
                "notifications/resources/list_changed",
                Arc::new(move |_params: &serde_json::Value| {
                    if let Some(connection) = connection.upgrade() {
                        if let Some(client) = connection.current_client() {
                            tokio::spawn(async move {
                                connection.refresh_resources(&client, generation).await;
                            });
                        }
                    }
                }),
            );
            let connection = Arc::downgrade(&self);
            let stdio = transport.stdio.clone();
            client.on_close(Arc::new(move || {
                if let Some(connection) = connection.upgrade() {
                    connection.handle_client_close(generation, stdio.as_ref());
                }
            }));
        }
        // Servers without the tools capability (prompts or resources only) do
        // not answer tools/list.
        let capabilities = client.server_capabilities();
        let has_resources = capabilities
            .as_ref()
            .is_some_and(|caps| caps.contains_key("resources"));
        let has_tools = capabilities
            .as_ref()
            .is_some_and(|caps| caps.contains_key("tools"));
        let tools = if has_tools {
            match client.list_tools(McpRequestOptions::default()).await {
                Ok(tools) => tools,
                Err(error) => {
                    self.close_setup(generation, &client, &transport.transport).await;
                    return Err(error);
                }
            }
        } else {
            Vec::new()
        };
        let resources = if has_resources {
            fetch_resources(&client).await
        } else {
            (Vec::new(), Vec::new())
        };
        if client.connection_state() != ClientState::Connected {
            self.close_setup(generation, &client, &transport.transport).await;
            return Err(McpClientError::Other(
                "connection closed during setup".to_string(),
            ));
        }
        let promoted = {
            let mut shared = self.lock();
            if shared.closed
                || shared.connecting.as_ref().map(|(active, _, _)| *active) != Some(generation)
            {
                false
            } else {
                shared.connecting = None;
                shared.client = Some(client.clone());
                shared.client_generation = generation;
                shared.tools = tools;
                shared.has_resources = has_resources;
                shared.resources = resources.0;
                shared.resource_templates = resources.1;
                shared.instructions = client.instructions().and_then(|instructions| {
                    let trimmed = instructions.trim();
                    (!trimmed.is_empty()).then(|| trimmed.to_string())
                });
                shared.state = ServerState::Connected;
                shared.error = None;
                true
            }
        };
        if !promoted {
            self.close_setup(generation, &client, &transport.transport).await;
            return Err(McpClientError::Other("shut down while connecting".to_string()));
        }
        let connection = Arc::clone(&self);
        (connection.on_tools)(&connection);
        self.changed();
        Ok(client)
    }

    async fn close_setup(
        &self,
        generation: u64,
        client: &McpClient,
        transport: &Arc<dyn McpTransport>,
    ) {
        let _ = client.close().await;
        // close can race the client's first transport attachment. Close the
        // separately tracked transport too; transport close is idempotent.
        let _ = transport.close().await;
        let mut shared = self.lock();
        if shared.connecting.as_ref().map(|(active, _, _)| *active) == Some(generation) {
            shared.connecting = None;
        }
    }

    fn connect_failed(&self, error: McpClientError) -> String {
        if self.needs_sign_in(&error) && !self.lock().closed {
            self.mark_needs_auth();
            return sign_in_required_message(&self.entry);
        }
        let message = error.to_string();
        let combined = {
            let mut shared = self.lock();
            shared.state = if shared.closed {
                ServerState::Closed
            } else {
                ServerState::Failed
            };
            let error_text = match shared.stderr_tail.clone() {
                Some(tail) => format!("{message}\n{tail}"),
                None => message.clone(),
            };
            shared.error = Some(error_text.clone());
            error_text
        };
        self.changed();
        format!(
            "MCP server \"{}\" failed to connect: {combined}",
            self.entry.name
        )
    }

    /// The transport dropped. The next call reconnects; until then the status
    /// shows why (upstream `handleClientClose`).
    fn handle_client_close(&self, generation: u64, stdio: Option<&Arc<StdioTransport>>) {
        let mut shared = self.lock();
        if shared.client_generation != generation || shared.closed {
            return;
        }
        shared.client = None;
        shared.state = ServerState::Disconnected;
        let stderr = stdio.map(|stdio| tail_chars(&stdio.stderr(), STDERR_TAIL_CHARS));
        shared.error = Some(match stderr {
            Some(stderr) if !stderr.is_empty() => format!("Connection closed\n{stderr}"),
            _ => "Connection closed".to_string(),
        });
        drop(shared);
        self.changed();
    }

    async fn refresh_tools(self: &Arc<Self>, client: &McpClient, generation: u64) {
        let result = client.list_tools(McpRequestOptions::default()).await;
        match result {
            Ok(tools) => {
                {
                    let mut shared = self.lock();
                    if shared.client_generation != generation || shared.closed {
                        return;
                    }
                    shared.tools = tools;
                }
                let connection = Arc::clone(self);
                (connection.on_tools)(&connection);
            }
            Err(error) => {
                self.lock().error = Some(format!("Failed to refresh tools: {error}"));
            }
        }
        self.changed();
    }

    async fn refresh_resources(self: &Arc<Self>, client: &McpClient, generation: u64) {
        let (resources, resource_templates) = fetch_resources(client).await;
        {
            let mut shared = self.lock();
            if shared.client_generation != generation || shared.closed {
                return;
            }
            shared.resources = resources;
            shared.resource_templates = resource_templates;
        }
        let connection = Arc::clone(self);
        (connection.on_tools)(&connection);
        self.changed();
    }

    /// Upstream `close()`.
    pub async fn close(self: &Arc<Self>) -> Result<(), String> {
        let (client, connecting, opening) = {
            let mut shared = self.lock();
            shared.closed = true;
            shared.state = ServerState::Closed;
            shared.tools.clear();
            shared.has_resources = false;
            shared.resources.clear();
            shared.resource_templates.clear();
            shared.instructions = None;
            (shared.client.take(), shared.connecting.take(), shared.opening.take())
        };
        // Drop the cached future outside the state mutex. A cancelled caller
        // may otherwise leave this future holding the connection alive.
        drop(opening);
        self.changed();
        if let Some(client) = client {
            let _ = client.close().await;
        }
        if let Some((generation, client, transport)) = connecting {
            self.close_setup(generation, &client, &transport).await;
        }
        // A refresh the server already answered may have rotated the refresh
        // token; exiting before the new tokens are saved would lose the grant.
        if let Some(provider) = &self.auth_provider {
            provider.settled().await;
        }
        Ok(())
    }
}

/// Upstream `oauthUrl` (free function over the entry).
fn oauth_url_of(entry: &McpServerEntry) -> Option<String> {
    if uses_oauth(entry) {
        entry.config.url().map(str::to_string)
    } else {
        None
    }
}

fn oauth_settings_of(entry: &McpServerEntry) -> McpOAuthSettings {
    let Some(oauth) = entry.config.oauth() else {
        return McpOAuthSettings::default();
    };
    let client_secret = oauth.client_secret.as_deref().map(|secret| {
        resolve_config_value_or_throw(
            secret,
            &format!("MCP server \"{}\" oauth.clientSecret", entry.name),
            None,
        )
        .unwrap_or_default()
    });
    McpOAuthSettings {
        client_id: oauth.client_id,
        client_secret,
        callback_port: oauth.callback_port,
        callback_url: oauth.callback_url,
        scope: oauth.scope,
        client_name: oauth.client_name,
        client_registration: oauth.client_registration,
        // Upstream wraps the configured string with `new URL(...)`; parsing
        // here keeps the same validate-first guarantee.
        auth_server_metadata_url: oauth
            .auth_server_metadata_url
            .as_deref()
            .and_then(|value| url::Url::parse(value).ok()),
    }
}

/// `pathToFileURL(cwd).href`.
fn file_url(path: &str) -> String {
    url::Url::from_file_path(path)
        .map(|url| url.to_string())
        .unwrap_or_else(|_| path.to_string())
}

/// Node `path.basename`.
fn basename(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

/// Upstream `.slice(-STDERR_TAIL_CHARS)`, char-wise.
fn tail_chars(text: &str, max_chars: usize) -> String {
    let total = text.chars().count();
    if total <= max_chars {
        return text.to_string();
    }
    text.chars().skip(total - max_chars).collect()
}

impl McpToolCaller for McpServerConnection {
    fn call_tool(
        &self,
        name: &str,
        args: serde_json::Map<String, serde_json::Value>,
        options: McpRequestOptions,
    ) -> BoxFuture<'static, Result<CallToolResult, String>> {
        let connection = self.handle();
        let name = name.to_string();
        Box::pin(async move {
            connection
                .with_client(
                    move |client| {
                        let name = name.clone();
                        let args = args.clone();
                        let options = options.clone();
                        Box::pin(async move { client.call_tool(&name, Some(args), options).await })
                    },
                    false,
                )
                .await
        })
    }
}

impl McpResourceServer for McpServerConnection {
    fn name(&self) -> &str {
        &self.entry.name
    }

    fn timeout_ms(&self) -> u64 {
        McpServerConnection::timeout_ms(self)
    }

    fn resources_page(
        &self,
        cursor: Option<String>,
        options: McpRequestOptions,
    ) -> BoxFuture<'static, Result<ListResourcesResult, String>> {
        let connection = self.handle();
        Box::pin(async move {
            connection
                .with_client(
                    move |client| {
                        let cursor = cursor.clone();
                        let options = options.clone();
                        Box::pin(async move { client.list_resources_page(cursor, options).await })
                    },
                    true,
                )
                .await
        })
    }

    fn resource_templates_page(
        &self,
        cursor: Option<String>,
        options: McpRequestOptions,
    ) -> BoxFuture<'static, Result<ListResourceTemplatesResult, String>> {
        let connection = self.handle();
        Box::pin(async move {
            connection
                .with_client(
                    move |client| {
                        let cursor = cursor.clone();
                        let options = options.clone();
                        Box::pin(async move {
                            without_templates(
                                client.list_resource_templates_page(cursor, options),
                                ListResourceTemplatesResult {
                                    resource_templates: Vec::new(),
                                    next_cursor: None,
                                    meta: None,
                                },
                            )
                            .await
                        })
                    },
                    true,
                )
                .await
        })
    }

    fn all_resources(
        &self,
        options: McpRequestOptions,
    ) -> BoxFuture<'static, Result<Vec<Resource>, String>> {
        let connection = self.handle();
        Box::pin(async move {
            connection
                .with_client(
                    move |client| {
                        let options = options.clone();
                        Box::pin(async move { client.list_resources(options).await })
                    },
                    true,
                )
                .await
        })
    }

    fn all_resource_templates(
        &self,
        options: McpRequestOptions,
    ) -> BoxFuture<'static, Result<Vec<ResourceTemplate>, String>> {
        let connection = self.handle();
        Box::pin(async move {
            connection
                .with_client(
                    move |client| {
                        let options = options.clone();
                        Box::pin(async move {
                            without_templates(client.list_resource_templates(options), Vec::new())
                                .await
                        })
                    },
                    true,
                )
                .await
        })
    }

    fn read_resource(
        &self,
        uri: &str,
        options: McpRequestOptions,
    ) -> BoxFuture<'static, Result<ReadResourceResult, String>> {
        let connection = self.handle();
        let uri = uri.to_string();
        Box::pin(async move {
            connection
                .with_client(
                    move |client| {
                        let uri = uri.clone();
                        let options = options.clone();
                        Box::pin(async move { client.read_resource(&uri, options).await })
                    },
                    true,
                )
                .await
        })
    }
}
