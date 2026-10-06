//! Port of upstream `experimental/client-runtime.ts`
//! (sha256 9f467a682b00c75bab5fe8d15119a08cb6d79442bd26258615d82767db9881c8).
//!
//! Ported: the `ClientCommand` option shape (upstream
//! `cli/experimental/commands/client.ts` inputs), `openClientRuntime`'s
//! option validation ladder and routing decisions with exact error strings,
//! `routeFromExplicitPath`, the connect-failure classification that decides
//! automatic reactivation (`DisconnectedError` / version `ServerError` /
//! transient errno causes), the dispose error aggregation, and
//! `activateBuiltinClientServices`' conditional session-management wrapper
//! decision (remove awaits `whenDetached` only when the removed session was
//! the current attachment; upstream's attach→`whenAttached` and
//! detach→`whenDetached` are unconditional awaits with no decision to port).
//!
//! D11 seam (disclosed in this module's docs): the live `Client` transport (`pi-client`
//! connect, unix discovery, server auto-activation), the replicated service
//! namespaces and the Radius reconnect wiring are embedder-owned behind the
//! [`ClientRuntimeSeam`] trait; the port owns every branch decision and
//! error string. Upstream `isServerId` is reused from the protocol port.

use serde_json::Value;

use crate::coding_agent::experimental::radius_auth::AuthInput;

/// Upstream `AuthInput` (`cli/experimental/command-options.ts`) — union of
/// token and path inputs; reuses the radius-auth port's shape.
pub type CommandAuthInput = AuthInput;

/// Upstream `command-options.ts` `ConnectTarget`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectTarget {
    /// `{ transport: "unix", path }`.
    Unix { path: String },
    /// `{ transport: "radius", serverId }`.
    Radius { server_id: String },
}

impl ConnectTarget {
    /// Upstream `transport` discriminator.
    pub fn transport(&self) -> &'static str {
        match self {
            ConnectTarget::Unix { .. } => "unix",
            ConnectTarget::Radius { .. } => "radius",
        }
    }
}

/// Upstream `ClientCommand` (the non-interactive client + TUI inputs).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClientCommand {
    pub auth: Option<CommandAuthInput>,
    pub connect: Option<ConnectTarget>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub plugin_packages: Option<Vec<String>>,
    pub session_id: Option<String>,
    /// Upstream `--continue`.
    pub continue_session: bool,
    /// Upstream `--resume`.
    pub resume: bool,
    pub prompt: Option<String>,
}

/// Upstream `UnixServerRoute` (`pi-client/unix`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnixServerRoute {
    pub server_id: String,
    pub path: String,
}

/// Upstream `ClientRuntimeRoute`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientRuntimeRoute {
    Unix(UnixServerRoute),
    Radius { server_id: String },
}

impl ClientRuntimeRoute {
    pub fn server_id(&self) -> &str {
        match self {
            ClientRuntimeRoute::Unix(route) => &route.server_id,
            ClientRuntimeRoute::Radius { server_id } => server_id,
        }
    }

    pub fn transport(&self) -> &'static str {
        match self {
            ClientRuntimeRoute::Unix(_) => "unix",
            ClientRuntimeRoute::Radius { .. } => "radius",
        }
    }
}

/// Upstream `openClientRuntime`'s validation prelude. Exact upstream error
/// strings, evaluated in declaration order.
pub fn validate_open_options(command: &ClientCommand) -> Result<(), String> {
    if command.auth.is_some() && command.connect.as_ref().map(|c| c.transport()) != Some("radius") {
        return Err(
            "Authentication is only supported for experimental Radius connections".to_string(),
        );
    }
    if command.provider.is_some() && command.model.is_none() {
        return Err("Server model provider requires a model".to_string());
    }
    if command.connect.is_some() && command.model.is_some() {
        return Err(
            "Model selection is only valid when automatically activating a new server".to_string(),
        );
    }
    if command.connect.as_ref().map(|c| c.transport()) == Some("radius")
        && command.plugin_packages.is_some()
    {
        return Err(
            "Plugin package paths can only be configured on a local Unix server".to_string(),
        );
    }
    Ok(())
}

/// Upstream `routeFromExplicitPath`: derive `<uuidv4-server-id>.sock` route
/// from an explicit socket path.
pub fn route_from_explicit_path(path: &str) -> Result<UnixServerRoute, String> {
    let name = std::path::Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    let server_id = name.strip_suffix(".sock").unwrap_or("");
    if !crate::protocol::protocol::is_server_id(server_id) {
        return Err("--connect path must end with <uuidv4-server-id>.sock".to_string());
    }
    Ok(UnixServerRoute {
        server_id: server_id.to_string(),
        path: path.to_string(),
    })
}

/// One dispose result handed back by the embedder (D11): the upstream
/// `Promise.allSettled` group inputs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisposeFailure(pub String);

/// Upstream `dispose`: single failure propagates bare, several aggregate
/// with the exact upstream message.
pub fn aggregate_dispose_errors(errors: Vec<DisposeFailure>) -> Result<(), String> {
    match errors.len() {
        0 => Ok(()),
        1 => Err(errors.into_iter().next().unwrap().0),
        _ => {
            let joined: Vec<String> = errors.into_iter().map(|failure| failure.0).collect();
            Err(format!(
                "Failed to dispose experimental client runtime: [{}]",
                joined.join(", ")
            ))
        }
    }
}

/// Upstream `AggregateError([error, cleanupError], "Experimental client
/// startup and cleanup failed")` message face.
pub fn startup_cleanup_error(original: &str, cleanup: &str) -> String {
    format!("Experimental client startup and cleanup failed: [{original}, {cleanup}]")
}

/// The live-runtime collaborators `open_client_runtime` drives (D11 seam).
pub trait ClientRuntimeSeam {
    /// Upstream `discoverUnixServers({ directory })`.
    fn discover_unix_servers(&mut self, directory: &str) -> Result<Vec<UnixServerRoute>, String>;
    /// Upstream `activateServer(...)` (spawn + wait). Returns the route and
    /// an already-connected client token.
    fn activate_server(
        &mut self,
        directory: &str,
        requested_server_id: Option<&str>,
        session_dir: &str,
        provider: Option<&str>,
        model: Option<&str>,
    ) -> Result<(UnixServerRoute, String), String>;
    /// Upstream `Client.connect(...)`: errors classify through
    /// [`ConnectFailure`].
    fn connect(&mut self, route: &ClientRuntimeRoute) -> Result<String, ConnectFailure>;
    /// Upstream `resolveServerDirectory(options.directory)` env face.
    fn server_directory_env(&self) -> Option<String>;
    /// Upstream `process.env[ENV_SERVER_ID]`.
    fn server_id_env(&self) -> Option<String>;
    /// Upstream `resolveSessionDirectory()`.
    fn session_directory(&self) -> String;
}

/// Upstream `connect(route)` failure classification (D11): the port decides
/// reactivation from these classes exactly as upstream's catch block does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectFailure {
    /// `DisconnectedError` — no live server on the socket.
    Disconnected,
    /// `ServerError` with `code === "version"`.
    Version,
    /// A cause-chain errno in upstream's transient list.
    TransportErrno(String),
    /// Any other failure: propagate.
    Fatal(String),
}

impl ConnectFailure {
    /// Upstream: reactivation happens for unix routes whose failure is
    /// disconnected or a version mismatch; transient errnos report as
    /// "no server" (undefined) and the rest propagate.
    pub fn allows_reactivation(&self) -> bool {
        matches!(self, ConnectFailure::Disconnected | ConnectFailure::Version)
    }

    /// Upstream `connect`'s errno walk over the cause chain.
    pub fn is_transient_transport_failure(&self) -> bool {
        match self {
            ConnectFailure::TransportErrno(code) => {
                matches!(
                    code.as_str(),
                    "ENOENT" | "ECONNREFUSED" | "ECONNRESET" | "EPIPE" | "ETIMEDOUT"
                )
            }
            _ => false,
        }
    }
}

/// Upstream `openClientRuntime` route planning (the deterministic half of
/// the function; the live client construction is the seam). Returns the
/// routes and, when automatic activation ran, the activated client token.
pub fn plan_client_routes(
    command: &ClientCommand,
    seam: &mut dyn ClientRuntimeSeam,
    directory: Option<&str>,
) -> Result<(Vec<ClientRuntimeRoute>, Option<String>), String> {
    validate_open_options(command)?;
    let directory = directory
        .map(str::to_string)
        .or_else(|| seam.server_directory_env())
        .unwrap_or_else(|| super::server::resolve_server_directory(None, None));

    let mut activated_client: Option<String> = None;
    let routes: Vec<ClientRuntimeRoute> = match &command.connect {
        Some(ConnectTarget::Radius { server_id }) => vec![ClientRuntimeRoute::Radius {
            server_id: server_id.clone(),
        }],
        Some(ConnectTarget::Unix { path }) => {
            vec![ClientRuntimeRoute::Unix(route_from_explicit_path(path)?)]
        }
        None => {
            let discovered = seam.discover_unix_servers(&directory)?;
            let mut routes: Vec<ClientRuntimeRoute> = discovered
                .into_iter()
                .map(ClientRuntimeRoute::Unix)
                .collect();
            if !routes.is_empty() && command.model.is_some() {
                return Err(
                    "Model selection is only valid when automatically activating a new server"
                        .to_string(),
                );
            }
            if routes.is_empty() {
                let (route, client) = seam.activate_server(
                    &directory,
                    seam.server_id_env().as_deref(),
                    &seam.session_directory(),
                    command.provider.as_deref(),
                    command.model.as_deref(),
                )?;
                activated_client = Some(client);
                routes = vec![ClientRuntimeRoute::Unix(route)];
            }
            routes
        }
    };
    if command.plugin_packages.is_some() && routes.len() != 1 {
        return Err("Plugin selection requires exactly one local server".to_string());
    }
    Ok((routes, activated_client))
}

/// Upstream `openClientRuntime`'s per-route client acquisition decision: an
/// already-activated client wins, then a direct connect; on a unix route
/// whose failure allows reactivation the server is activated and used.
pub fn acquire_route_client(
    route: &ClientRuntimeRoute,
    activated_client: Option<String>,
    command: &ClientCommand,
    seam: &mut dyn ClientRuntimeSeam,
    directory: &str,
) -> Result<String, String> {
    if let Some(client) = activated_client {
        return Ok(client);
    }
    match seam.connect(route) {
        Ok(client) => Ok(client),
        Err(failure) => {
            if command.connect.is_some()
                || route.transport() != "unix"
                || !failure.allows_reactivation()
            {
                return Err(match failure {
                    ConnectFailure::Fatal(message) => message,
                    ConnectFailure::TransportErrno(code)
                        if failure.is_transient_transport_failure() =>
                    {
                        // Upstream returns `undefined` from `connect` for
                        // transient errnos; the runtime treats a silent
                        // socket as "no server" only in the discovery path,
                        // so an explicit route surfaces the raw failure.
                        format!("transport failure ({code})")
                    }
                    other => format!("{other:?}"),
                });
            }
            let (_, client) = seam.activate_server(
                directory,
                Some(route.server_id()),
                &seam.session_directory(),
                None,
                None,
            )?;
            Ok(client)
        }
    }
}

/// Upstream `activateBuiltinClientServices`' session-management wrapper: the
/// remote management calls are sequenced with the session-source attach/
/// detach waits (remove awaits `whenDetached` only when the removed session
/// was the current attachment).
pub fn builtin_management_remove_waits_for_detach(
    current_attachment: Option<&str>,
    removed_session: &str,
) -> bool {
    current_attachment == Some(removed_session)
}

/// Upstream `AggregateError(errors, "Failed to dispose experimental client
/// runtime")` carries the grouped failures; the JSON face mirrors it for
/// embedder reporting.
pub fn dispose_errors_to_json(errors: &[DisposeFailure]) -> Value {
    Value::Array(
        errors
            .iter()
            .map(|failure| Value::String(failure.0.clone()))
            .collect(),
    )
}

#[cfg(test)]
mod tests;
